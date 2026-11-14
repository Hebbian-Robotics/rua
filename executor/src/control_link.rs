//! The command and feedback boundary shared by station and controller.
//!
//! Clients enter through a [`LinkHandle`]. The control loop never blocks on it: commands go
//! to the control loop through a channel (`handoff`), which answers them at the
//! start of its next cycle, and
//! everything the executor says leaves through a queue to one writer thread,
//! which owns the sequenced event log and every client connection.
//!
//! Two kinds of output. A sample (the cycle's state) is dropped when the writer
//! falls behind. A log event (a command's record changed, a trip, ...) is never
//! dropped: it gets the next sequence number, is kept in a bounded retained
//! window and is sent to every client, so a client that reconnects asks for what
//! it missed. A client that stops reading is disconnected rather than allowed to
//! stall the others; it reconnects and resumes from its last number.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::arm::{PoseGoalSettings, ReachGoal};
use crate::config::ExecutorConfig;
use crate::handoff::{ArmHandle, ArmInbox, arm_channel};
use crate::lease::{
    AcquireError, AdmissionOptions, ArmGate, Lease, LeaseRequest, SharedInterlocks,
    is_allowed_while_interlocked, is_always_allowed,
};
use crate::planning::Planner;
use crate::preview::{Preview, PreviewRequest};
use crate::sync::LockUnpoisoned;
use hebbian_contracts::{
    ArmMode, ArmMove, CommandRecord, Envelope, Event, Frame, IkResult, Interlock, LeaseAnswer,
    LeaseStatus, OrientationGoal, PlannedMotion, PoseDescription, PoseOrientation, PoseProjection,
    RejectionReason, Request, Sample, Waypoint,
};

/// Samples waiting for the writer; more than this are dropped.
const MAXIMUM_PENDING_SAMPLES: usize = 64;
/// Log events kept for clients that reconnect; older ones are lost to them.
const RETAINED_LOG_LENGTH: usize = 4096;
/// Request ids remembered, so a resent request is answered, not acted on twice.
const REMEMBERED_REQUESTS: usize = 1024;
/// Records a snapshot carries, as the log has told them.
const SNAPSHOT_RECORDS: usize = 200;
/// Records kept by command id for lookups, newest; a few megabytes at most.
const REMEMBERED_RECORDS: usize = 10_000;
const WRITER_POLL: Duration = Duration::from_millis(20);

/// What the readers ask the writer thread to do, in order.
enum Outgoing {
    Sample(Sample),
    Log(Event),
    EnsureRecord(CommandRecord),
    ControllerUnavailable(String),
    Connected(u64, ClientSink),
    Disconnected(u64),
    Reply(u64, Event),
    Subscribe(u64, Option<u64>),
    Snapshot(u64),
    FindRecord(String, tokio::sync::oneshot::Sender<Option<CommandRecord>>),
    LatestSample(tokio::sync::oneshot::Sender<Option<Sample>>),
    View(tokio::sync::oneshot::Sender<ArmView>),
}

/// Where the writer sends one client's lines: a queue its connection
/// drains. A queue that is full is a client that stopped reading, and is
/// disconnected.
type ClientSink = tokio::sync::mpsc::Sender<String>;

/// How a request was answered.
#[derive(Clone)]
struct Answer {
    record: Option<CommandRecord>,
    records: Vec<CommandRecord>,
    plan: Option<Box<PlannedMotion>>,
    projection: Option<Box<PoseProjection>>,
    description: Option<Box<PoseDescription>>,
    ik: Option<Box<IkResult>>,
    lease: Option<Box<LeaseAnswer>>,
    refusal: Option<(RejectionReason, String)>,
}

impl Answer {
    /// Accepted, with nothing to report.
    fn nothing() -> Self {
        Self {
            record: None,
            records: Vec::new(),
            plan: None,
            projection: None,
            description: None,
            ik: None,
            lease: None,
            refusal: None,
        }
    }

    fn planned(plan: PlannedMotion) -> Self {
        Self {
            plan: Some(Box::new(plan)),
            ..Self::nothing()
        }
    }

    fn projected(projection: PoseProjection) -> Self {
        Self {
            projection: Some(Box::new(projection)),
            ..Self::nothing()
        }
    }

    fn described(description: PoseDescription) -> Self {
        Self {
            description: Some(Box::new(description)),
            ..Self::nothing()
        }
    }

    fn solved(result: IkResult) -> Self {
        Self {
            ik: Some(Box::new(result)),
            ..Self::nothing()
        }
    }

    fn accepted(record: Option<CommandRecord>) -> Self {
        Self {
            record,
            ..Self::nothing()
        }
    }

    fn accepted_group(records: Vec<CommandRecord>) -> Self {
        Self {
            records,
            ..Self::nothing()
        }
    }

    fn leased(lease: LeaseAnswer) -> Self {
        Self {
            lease: Some(Box::new(lease)),
            ..Self::nothing()
        }
    }

    fn refused(reason: RejectionReason, detail: String) -> Self {
        Self {
            refusal: Some((reason, detail)),
            ..Self::nothing()
        }
    }

    /// An acquire that someone else's lease turned away.
    fn held_by(status: LeaseStatus) -> Self {
        let detail = format!("The lease is held by '{}'", status.label);
        Self {
            lease: Some(Box::new(LeaseAnswer::Holder {
                status: Some(status),
            })),
            ..Self::refused(RejectionReason::LeaseHeldElsewhere, detail)
        }
    }
}

/// The answers already given to request ids.
#[derive(Default)]
struct RememberedAnswers {
    by_id: HashMap<String, Answer>,
    order: VecDeque<String>,
}

impl RememberedAnswers {
    fn remember(&mut self, request_id: &str, answer: Answer) {
        if self.order.len() >= REMEMBERED_REQUESTS
            && let Some(oldest) = self.order.pop_front()
        {
            self.by_id.remove(&oldest);
        }
        self.order.push_back(request_id.to_owned());
        self.by_id.insert(request_id.to_owned(), answer);
    }
}

/// Samples the writer has sent, returned to the loop so it refills their
/// buffers instead of allocating new ones.
pub(crate) struct SampleRecycler {
    returned: Receiver<Sample>,
}

impl SampleRecycler {
    /// A sample to fill: one the writer has finished with, or a new one while
    /// the first samples are still queued.
    pub(crate) fn checkout(&self) -> Sample {
        self.returned.try_recv().unwrap_or_else(|_| Sample::blank())
    }
}

/// The loop's two ends of a link: what the readers ask of it, and the samples
/// it can reuse.
pub(crate) struct LoopEnd {
    pub(crate) inbox: ArmInbox,
    pub(crate) samples: SampleRecycler,
}

/// What every reader thread shares.
#[derive(Clone)]
struct Shared {
    arm: ArmHandle,
    generation: Arc<AtomicU64>,
    /// Who may command the arms; shared by every arm of the executor.
    lease: Arc<Lease>,
    /// This arm's own interlocks and revision.
    interlocks: SharedInterlocks,
    outgoing: Sender<Outgoing>,
    remembered: Arc<Mutex<RememberedAnswers>>,
    /// The ghost's rehearsal of a reach; present when the arm has a policy.
    preview: Option<Arc<Mutex<Preview>>>,
    /// Planning and inverse kinematics for the arm's tool; present when the
    /// config names one.
    planner: Option<Arc<Planner>>,
    /// When a rehearsed reach counts as arrived; present with the preview.
    reach_settings: Option<PoseGoalSettings>,
    /// The jaw, for turning openings into angles; none without a gripper.
    gripper: Option<crate::config::GripperLimits>,
    /// Whether a learned policy turns pose commands into motion.
    has_policy: bool,
    /// The arm's config, for what clients are told about it.
    config: Arc<ExecutorConfig>,
    /// Client numbers, one per connected client.
    next_client: Arc<AtomicU64>,
    /// Ticks when the arm's mode or active command changes or a record is logged.
    changes: Arc<tokio::sync::watch::Sender<()>>,
}

/// What one arm last reported, as its writer knows it.
#[derive(Debug, Clone)]
pub struct ArmView {
    pub sample: Option<Sample>,
    /// How long ago that sample arrived.
    pub sample_age: Option<Duration>,
    pub active_record: Option<CommandRecord>,
    /// The last finished command whose resting sample has been handled.
    pub last_record: Option<CommandRecord>,
}

/// A way into one arm's link from elsewhere in the process, such as the
/// HTTP server: send requests through the link and read its updates.
#[derive(Clone)]
pub struct LinkHandle {
    shared: Shared,
}

impl LinkHandle {
    /// Publish a controller loss and interrupt outstanding command records.
    pub fn controller_unavailable(&self, reason: String) {
        let _ = self
            .shared
            .outgoing
            .send(Outgoing::ControllerUnavailable(reason));
    }

    /// Deliver already-admitted controller requests to this link's local loop.
    #[must_use]
    pub fn controller_loop_arm(&self) -> crate::controller_bridge::LoopArm {
        crate::controller_bridge::LoopArm::new(self.shared.arm.clone())
    }

    /// Queue an arm stop only while the specified lease is current.
    #[must_use]
    pub fn stop_if_leased(&self, token: Option<&str>) -> hebbian_contracts::LeasedStopOutcome {
        let mut submitted = false;
        let owned = self.shared.lease.with_captured_lease(token, &mut || {
            self.shared.generation.fetch_add(1, Ordering::AcqRel);
            submitted = self
                .shared
                .arm
                .submit_unawaited(hebbian_contracts::Command::Stop, None)
                .is_ok();
        });
        match (owned, submitted) {
            (false, _) => hebbian_contracts::LeasedStopOutcome::LeaseLost,
            (true, true) => hebbian_contracts::LeasedStopOutcome::Submitted,
            (true, false) => hebbian_contracts::LeasedStopOutcome::Unavailable,
        }
    }

    #[must_use]
    pub fn with_lease(&self, token: &str, action: &mut dyn FnMut()) -> bool {
        self.shared.lease.with_lease(token, action)
    }

    #[must_use]
    pub fn stop_leased_motion(&self, token: &str) -> bool {
        self.shared.lease.stop_leased_motion(token)
    }

    pub(crate) fn stop_leased_motion_for_submission(
        &self,
        token: &str,
        generations: &mut [(Arc<AtomicU64>, u64)],
        cancel_sequence: impl FnOnce() -> bool,
    ) -> Result<(), crate::arm::Rejection> {
        self.shared
            .lease
            .stop_leased_motion_for_submission(token, generations, cancel_sequence)
    }

    /// The lease holder's token and the cameras it requires.
    #[must_use]
    pub fn camera_requirements(&self) -> Option<crate::lease::CameraRequirements> {
        self.shared.lease.camera_requirements()
    }

    /// The lease holder reports whether a camera it requires gives frames.
    ///
    /// # Errors
    /// The token does not hold the lease, or the lease does not require the camera.
    pub fn report_camera(&self, token: &str, camera: &str, available: bool) -> Result<(), String> {
        self.shared.lease.report_camera(token, camera, available)
    }

    /// This arm's active interlocks and its revision.
    #[must_use]
    pub fn interlocks(&self) -> (Vec<Interlock>, u64) {
        self.shared.lease.interlocks(&self.shared.interlocks)
    }

    pub(crate) fn raise_interlock(&self, interlock: Interlock) {
        self.shared.lease.raise(&self.shared.interlocks, interlock);
    }

    pub(crate) fn clear_interlocks(&self, matches: impl Fn(&Interlock) -> bool) {
        self.shared.lease.clear(&self.shared.interlocks, matches);
    }

    pub(crate) fn gate(&self, expected_revision: Option<u64>) -> ArmGate {
        ArmGate {
            interlocks: Arc::clone(&self.shared.interlocks),
            expected_revision,
        }
    }

    /// Connect a client whose lines arrive on the returned queue, which holds
    /// up to `capacity` lines before the client counts as not reading.
    #[must_use]
    pub fn connect(&self, capacity: usize) -> (u64, tokio::sync::mpsc::Receiver<String>) {
        let client = self.shared.next_client.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        let _ = self
            .shared
            .outgoing
            .send(Outgoing::Connected(client, sender));
        (client, receiver)
    }

    /// Answer one request line from `client`, from an in-process client.
    /// Blocks until the arm answers; call it off any async runtime thread.
    pub fn request(&self, client: u64, line: &str) {
        answer_line(line, client, &self.shared);
    }

    /// Send `client` the log after `after`, or a snapshot when that is gone.
    pub fn subscribe(&self, client: u64, after: Option<u64>) {
        let _ = self
            .shared
            .outgoing
            .send(Outgoing::Subscribe(client, after));
    }

    /// The record of a command, among the last few thousand this arm logged.
    pub async fn find_record(&self, command_id: String) -> Option<CommandRecord> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.shared
            .outgoing
            .send(Outgoing::FindRecord(command_id, answer))
            .ok()?;
        answered.await.ok().flatten()
    }

    /// Send `client` a snapshot of the arm.
    pub fn snapshot(&self, client: u64) {
        let _ = self.shared.outgoing.send(Outgoing::Snapshot(client));
    }

    pub fn disconnect(&self, client: u64) {
        let _ = self.shared.outgoing.send(Outgoing::Disconnected(client));
    }

    /// Wakes when the arm's mode or active command changes or an event is
    /// logged, such as a command's record; read the state after waking.
    #[must_use]
    pub fn changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.shared.changes.subscribe()
    }

    /// The newest state sample, waiting for the writer; call it off the async
    /// runtime.
    #[must_use]
    pub fn latest_sample_blocking(&self) -> Option<Sample> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.shared
            .outgoing
            .send(Outgoing::LatestSample(answer))
            .ok()?;
        answered.blocking_recv().ok().flatten()
    }

    /// What the arm last reported, with its active and newest records.
    pub async fn view(&self) -> Option<ArmView> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.shared.outgoing.send(Outgoing::View(answer)).ok()?;
        answered.await.ok()
    }

    #[must_use]
    pub fn config(&self) -> &ExecutorConfig {
        &self.shared.config
    }

    /// Who holds the lease on the executor's arms, if anyone.
    #[must_use]
    pub fn lease_status(&self) -> Option<LeaseStatus> {
        self.shared.lease.status(Instant::now())
    }

    #[must_use]
    pub fn planner(&self) -> Option<&Planner> {
        self.shared.planner.as_deref()
    }

    #[must_use]
    pub fn gripper(&self) -> Option<&crate::config::GripperLimits> {
        self.shared.gripper.as_ref()
    }

    #[must_use]
    pub fn has_policy(&self) -> bool {
        self.shared.has_policy
    }

    pub(crate) fn submission_generation(&self) -> (Arc<AtomicU64>, u64) {
        let generation = Arc::clone(&self.shared.generation);
        let current = generation.load(Ordering::Acquire);
        (generation, current)
    }

    /// Invalidate earlier pending submissions before reserving the arm for an action sequence.
    pub(crate) fn fence_prior_submissions(&self) -> (Arc<AtomicU64>, u64) {
        let generation = Arc::clone(&self.shared.generation);
        let current = generation.fetch_add(1, Ordering::AcqRel) + 1;
        (generation, current)
    }

    /// Submit a command as the holder of `token` (stop and shutdown as anyone),
    /// waiting for the loop to admit or refuse it; refused if `generation` has moved.
    ///
    /// # Errors
    /// The arm or the lease refused it.
    pub fn submit_command_fenced(
        &self,
        command: hebbian_contracts::Command,
        token: Option<&str>,
        trace_context: Option<hebbian_contracts::TraceContext>,
        expected_revision: Option<u64>,
        generation: Vec<(Arc<AtomicU64>, u64)>,
    ) -> Result<CommandRecord, (RejectionReason, String)> {
        let answer = submit_arm_command(
            &self.shared,
            command,
            token,
            trace_context,
            expected_revision,
            &generation,
        );
        command_record(answer)
    }

    /// Admit a planned command and return an asynchronously awaitable reply.
    pub(crate) fn submit_command_async(
        &self,
        command: hebbian_contracts::Command,
        token: Option<&str>,
        trace_context: Option<hebbian_contracts::TraceContext>,
        expected_revision: Option<u64>,
        generation: &[(Arc<AtomicU64>, u64)],
    ) -> Result<
        tokio::sync::oneshot::Receiver<Result<CommandRecord, crate::arm::Rejection>>,
        crate::arm::Rejection,
    > {
        if matches!(command, hebbian_contracts::Command::ApproveRecovery) {
            return self.shared.lease.admit_recovery_approval(generation, || {
                self.shared.arm.submit_async(command, trace_context)
            })?;
        }
        let always_allowed = is_always_allowed(&command);
        let options = AdmissionOptions {
            allow_fault_recovery: matches!(command, hebbian_contracts::Command::ClearFault),
            allow_while_interlocked: is_allowed_while_interlocked(&command),
        };
        let gate = ArmGate {
            interlocks: Arc::clone(&self.shared.interlocks),
            expected_revision,
        };
        enqueue_admitted(
            &self.shared,
            token,
            always_allowed,
            options,
            std::slice::from_ref(&gate),
            generation,
            || self.shared.arm.submit_async(command, trace_context),
        )
    }

    /// Admit a coordinated move and return an asynchronously awaitable reply.
    pub(crate) fn move_arms_async(
        &self,
        moves: Vec<ArmMove>,
        token: Option<&str>,
        trace_context: Option<hebbian_contracts::TraceContext>,
        gates: &[ArmGate],
        generation: &[(Arc<AtomicU64>, u64)],
    ) -> Result<
        tokio::sync::oneshot::Receiver<Result<Vec<CommandRecord>, crate::arm::Rejection>>,
        crate::arm::Rejection,
    > {
        enqueue_admitted(
            &self.shared,
            token,
            false,
            AdmissionOptions::default(),
            gates,
            generation,
            || {
                self.shared
                    .arm
                    .move_arms_async("move_arms".to_owned(), moves, trace_context)
            },
        )
    }

    pub(crate) fn interrupt_async(
        &self,
        trace_context: Option<hebbian_contracts::TraceContext>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<Result<CommandRecord, crate::arm::Rejection>>,
        crate::arm::Rejection,
    > {
        self.shared.lease.interrupt(&self.shared.generation, || {
            self.shared
                .arm
                .submit_async(hebbian_contracts::Command::Stop, trace_context)
        })
    }

    pub(crate) fn interrupt(
        &self,
        trace_context: Option<hebbian_contracts::TraceContext>,
    ) -> Result<Receiver<Result<CommandRecord, crate::arm::Rejection>>, crate::arm::Rejection> {
        self.shared.lease.interrupt(&self.shared.generation, || {
            self.shared
                .arm
                .submit_unawaited(hebbian_contracts::Command::Stop, trace_context)
        })
    }

    /// Where the arm would take the tool toward a pose from these joints.
    ///
    /// # Errors
    /// The arm has no tool link to plan for.
    pub fn project(
        &self,
        position: [f64; 3],
        orientation: Option<PoseOrientation>,
        joints: &[f64],
        gripper_angle: Option<f64>,
    ) -> Result<PoseProjection, (RejectionReason, String)> {
        let answer = projection_answer(&self.shared, position, orientation, joints, gripper_angle);
        match (answer.refusal, answer.projection) {
            (Some(refusal), _) => Err(refusal),
            (None, Some(projection)) => Ok(*projection),
            (None, None) => Err((RejectionReason::InvalidTarget, "No projection".to_owned())),
        }
    }

    /// Stop the arm, which then holds; anyone may.
    pub fn stop(&self) {
        if let Ok(answer) = self.interrupt(None) {
            let _ = answer.recv();
        }
    }

    /// End a disconnected holder and queue stops before another holder can command.
    pub fn disconnect_lease(&self, token: &str, links: &[Self]) {
        self.shared.lease.return_after(token, Instant::now(), || {
            for link in links {
                link.shared.generation.fetch_add(1, Ordering::AcqRel);
                let _ = link
                    .shared
                    .arm
                    .submit_unawaited(hebbian_contracts::Command::Stop, None);
            }
        });
    }

    /// Acquire (or with `take`, take) the lease on every arm of the executor.
    ///
    /// # Errors
    /// Someone else holds the lease, or the arms could not be stopped to take it.
    pub fn acquire_lease(
        &self,
        request: LeaseRequest,
        take: bool,
    ) -> Result<hebbian_contracts::LeaseGrant, AcquireError> {
        self.shared.lease.acquire_stopping_arms(request, take)
    }

    /// Keep the lease `token` holds; false when it lapsed or was taken.
    #[must_use]
    pub fn retain_lease(&self, token: &str) -> bool {
        self.shared.lease.retain(Some(token), Instant::now())
    }

    /// Release a current lease and cancel its action sequences under the lease lock.
    pub(crate) fn release_lease_with(
        &self,
        token: &str,
        cancel_sequence: impl FnOnce() -> bool,
    ) -> bool {
        self.shared.lease.release_with(token, cancel_sequence)
    }

    /// Give up the lease `token` holds; nothing when it is not the holder's.
    pub fn return_lease(&self, token: &str) {
        self.shared.lease.return_lease(Some(token), Instant::now());
    }
}

pub struct Link {
    shared: Shared,
    pending_samples: Arc<AtomicUsize>,
    closing: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
    /// What the loop takes once, as it starts.
    loop_end: Mutex<Option<LoopEnd>>,
}

impl Link {
    /// Preserve an accepted record before answering a station client.
    pub fn ensure_controller_record(&self, record: CommandRecord) {
        let _ = self.shared.outgoing.send(Outgoing::EnsureRecord(record));
    }

    /// End outstanding records and invalidate live feedback after losing the controller.
    pub fn controller_unavailable(&self, reason: String) {
        self.handle().controller_unavailable(reason);
    }

    /// Give a controller bridge the requests normally consumed by the local loop.
    /// This can be taken once, before a local loop takes it.
    #[must_use]
    pub fn take_controller_station_arm(&self) -> Option<crate::controller_bridge::StationArm> {
        self.take_loop_end()
            .map(|end| crate::controller_bridge::StationArm::new(end.inbox))
    }

    /// Deliver already-admitted controller requests to this link's local loop.
    #[must_use]
    pub fn controller_loop_arm(&self) -> crate::controller_bridge::LoopArm {
        crate::controller_bridge::LoopArm::new(self.shared.arm.clone())
    }

    /// A link for the arm that `config` describes, whose clients command it as
    /// `lease` allows.
    ///
    /// # Errors
    /// The arm's preview or planner could not be built from `config`.
    pub fn new(config: &ExecutorConfig, lease: Arc<Lease>) -> anyhow::Result<Self> {
        let (outgoing, queue) = channel::<Outgoing>();
        let pending_samples = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(AtomicBool::new(false));

        let (recycle_sample, returned_samples) = sync_channel::<Sample>(MAXIMUM_PENDING_SAMPLES);
        let changes = Arc::new(tokio::sync::watch::Sender::new(()));
        let writer = {
            let changes = Arc::clone(&changes);
            let pending_samples = Arc::clone(&pending_samples);
            let closing = Arc::clone(&closing);
            std::thread::spawn(move || {
                let mut hub = Hub {
                    recycle_sample: Some(recycle_sample),
                    changes: Some(changes),
                    ..Hub::default()
                };
                loop {
                    match queue.recv_timeout(WRITER_POLL) {
                        Ok(message) => hub.handle(message, &pending_samples),
                        Err(RecvTimeoutError::Timeout) => {
                            if closing.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
        };

        let (arm, inbox) = arm_channel();
        let shared = Shared {
            changes: Arc::clone(&changes),
            arm,
            generation: Arc::new(AtomicU64::new(0)),
            lease,
            interlocks: SharedInterlocks::default(),
            outgoing,
            remembered: Arc::new(Mutex::new(RememberedAnswers::default())),
            preview: Preview::new(config)?.map(|preview| Arc::new(Mutex::new(preview))),
            planner: Planner::for_arm(config)?.map(Arc::new),
            reach_settings: config.arm_limits().pose_goals,
            gripper: config.gripper.clone(),
            has_policy: config.policy.is_some(),
            config: Arc::new(config.clone()),
            next_client: Arc::new(AtomicU64::new(1)),
        };
        shared.lease.register_arm(
            shared.arm.clone(),
            Arc::clone(&shared.generation),
            Arc::clone(&shared.interlocks),
        );
        Ok(Self {
            shared,
            pending_samples,
            closing,
            writer: Some(writer),
            loop_end: Mutex::new(Some(LoopEnd {
                inbox,
                samples: SampleRecycler {
                    returned: returned_samples,
                },
            })),
        })
    }

    /// The loop's ends of this link, once: the loop that drives it takes them as
    /// it starts.
    /// A handle for clients inside this process.
    #[must_use]
    pub fn handle(&self) -> LinkHandle {
        LinkHandle {
            shared: self.shared.clone(),
        }
    }

    pub(crate) fn take_loop_end(&self) -> Option<LoopEnd> {
        self.loop_end.lock_unpoisoned().take()
    }

    /// Where the state machine sends the events it logs, in the order they
    /// happened.
    pub(crate) fn log_publisher(&self) -> impl Fn(Event) + Send + 'static + use<> {
        let log_sender = self.shared.outgoing.clone();
        move |event| {
            let _ = log_sender.send(Outgoing::Log(event));
        }
    }

    /// Tell the clients: a state sample is dropped when the writer is behind,
    /// any other event is logged and never dropped.
    ///
    /// # Panics
    /// With an event that is neither a sample nor a log event.
    pub fn publish(&self, event: Event) {
        match event {
            Event::State(sample) => {
                if self.pending_samples.load(Ordering::Relaxed) < MAXIMUM_PENDING_SAMPLES {
                    self.pending_samples.fetch_add(1, Ordering::Relaxed);
                    let _ = self.shared.outgoing.send(Outgoing::Sample(sample));
                }
            }
            event if event.is_logged() => {
                let _ = self.shared.outgoing.send(Outgoing::Log(event));
            }
            _ => unreachable!("only samples and log events are published by the loop"),
        }
    }

    /// Send what is queued and stop the writer.
    pub fn close(mut self) {
        self.closing.store(true, Ordering::Relaxed);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

/// The writer thread's state: every client, the retained log, the latest
/// sample, and the commands' latest records as the log has told them.
#[derive(Default)]
struct Hub {
    clients: HashMap<u64, ClientSink>,
    log: VecDeque<Frame>,
    /// The sequence number the next log event gets; the first is 1.
    next_seq: u64,
    latest_sample: Option<Sample>,
    latest_sample_at: Option<Instant>,
    records: HashMap<String, CommandRecord>,
    record_order: VecDeque<String>,
    /// The command that finished latest and still awaits the resting sample that
    /// follows its completion, with its finish time. Commands can be logged out of
    /// finish order (a release logs the motion it interrupted after its own record),
    /// so the latest finish time wins, not the last record logged.
    pending_finished: Option<(String, f64)>,
    /// The command that finished latest among those whose resting sample has been
    /// handled, with its finish time.
    settled_last: Option<(String, f64)>,
    mode: Option<(ArmMode, Option<String>)>,
    recycle_sample: Option<SyncSender<Sample>>,
    changes: Option<Arc<tokio::sync::watch::Sender<()>>>,
}

impl Hub {
    fn newest_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    fn handle(&mut self, message: Outgoing, pending_samples: &AtomicUsize) {
        match message {
            Outgoing::Sample(sample) => {
                let changed = self.latest_sample.as_ref().is_none_or(|previous| {
                    previous.mode != sample.mode
                        || previous.active_command != sample.active_command
                        || previous.recovery_issue != sample.recovery_issue
                });
                if let Some(finished) = self.pending_finished.take()
                    && self
                        .settled_last
                        .as_ref()
                        .is_none_or(|(_, settled_at)| finished.1 >= *settled_at)
                {
                    self.settled_last = Some(finished);
                }
                self.latest_sample = Some(sample.clone());
                self.latest_sample_at = Some(Instant::now());
                let frame = Frame::emitted_now(None, Event::State(sample));
                self.broadcast(&frame);
                if let (Event::State(sample), Some(recycle)) = (frame.event, &self.recycle_sample) {
                    // A full return queue only costs the loop an allocation.
                    let _ = recycle.try_send(sample);
                }
                // Released only once the sample is back with the loop.
                pending_samples.fetch_sub(1, Ordering::Release);
                if changed {
                    self.announce_change();
                }
            }
            Outgoing::Log(event) => {
                self.log_event(event);
                self.announce_change();
            }
            Outgoing::EnsureRecord(record) => {
                if !self.records.contains_key(&record.command_id) {
                    self.log_event(Event::Record(record));
                    self.announce_change();
                }
            }
            Outgoing::ControllerUnavailable(reason) => {
                self.latest_sample_at = None;
                if let Some(sample) = &mut self.latest_sample {
                    sample.active_command = None;
                }
                self.log_event(Event::ControllerUnavailable {
                    reason: reason.clone(),
                });
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0.0, |duration| duration.as_secs_f64());
                let interrupted: Vec<_> = self
                    .records
                    .values()
                    .filter(|record| record.completion.is_none())
                    .cloned()
                    .collect();
                for mut record in interrupted {
                    record.completion = Some(hebbian_contracts::CommandCompletion {
                        finished_at: now,
                        outcome: hebbian_contracts::CommandOutcome::Interrupted,
                        detail: Some(format!(
                            "Controller lost; command outcome and arm torque are unknown: {reason}"
                        )),
                    });
                    self.log_event(Event::Record(record));
                }
                self.announce_change();
            }
            Outgoing::Connected(client, sink) => {
                self.clients.insert(client, sink);
            }
            Outgoing::Disconnected(client) => {
                self.clients.remove(&client);
            }
            Outgoing::Reply(client, event) => self.send(client, &Frame::emitted_now(None, event)),
            Outgoing::Snapshot(client) => self.send_snapshot(client),
            Outgoing::View(answer) => {
                let record = |id: Option<&String>| id.and_then(|id| self.records.get(id)).cloned();
                let active_record = record(
                    self.latest_sample
                        .as_ref()
                        .and_then(|sample| sample.active_command.as_ref()),
                );
                let _ = answer.send(ArmView {
                    sample: self.latest_sample.clone(),
                    sample_age: self.latest_sample_at.map(|at| at.elapsed()),
                    active_record,
                    last_record: record(self.settled_last.as_ref().map(|(id, _)| id)),
                });
            }
            Outgoing::LatestSample(answer) => {
                let _ = answer.send(self.latest_sample.clone());
            }
            Outgoing::FindRecord(command_id, answer) => {
                let _ = answer.send(self.records.get(&command_id).cloned());
            }
            Outgoing::Subscribe(client, after) => self.replay(client, after.unwrap_or(0)),
        }
    }

    fn announce_change(&self) {
        if let Some(changes) = &self.changes {
            changes.send_replace(());
        }
    }

    fn log_event(&mut self, event: Event) {
        match &event {
            Event::Record(record) => {
                if let Some(finished_at) = record.finished_at()
                    && self
                        .pending_finished
                        .as_ref()
                        .is_none_or(|(_, pending_at)| finished_at >= *pending_at)
                {
                    self.pending_finished = Some((record.command_id.clone(), finished_at));
                }
                if self
                    .records
                    .insert(record.command_id.clone(), record.clone())
                    .is_none()
                {
                    self.record_order.push_back(record.command_id.clone());
                    while self.record_order.len() > REMEMBERED_RECORDS {
                        if let Some(oldest) = self.record_order.pop_front() {
                            self.records.remove(&oldest);
                        }
                    }
                }
            }
            Event::Mode { mode, fault } => self.mode = Some((*mode, fault.clone())),
            _ => {}
        }
        self.next_seq = self.next_seq.max(1);
        let frame = Frame::emitted_now(Some(self.next_seq), event);
        self.next_seq += 1;
        if self.log.len() >= RETAINED_LOG_LENGTH {
            self.log.pop_front();
        }
        self.log.push_back(frame.clone());
        self.broadcast(&frame);
    }

    fn send_snapshot(&mut self, client: u64) {
        let (mode, fault) = self.mode.clone().unwrap_or((ArmMode::Disabled, None));
        let records = self
            .record_order
            .iter()
            .skip(self.record_order.len().saturating_sub(SNAPSHOT_RECORDS))
            .filter_map(|id| self.records.get(id).cloned())
            .collect();
        let frame = Frame::emitted_now(
            None,
            Event::Snapshot {
                seq: self.newest_seq(),
                sample: self.latest_sample.clone(),
                mode,
                fault,
                records,
                log: self.log.iter().cloned().collect(),
            },
        );
        self.send(client, &frame);
    }

    /// Send the log after `after`; a client whose cursor the log no longer
    /// reaches (or is ahead of it, as after an executor restart) gets a snapshot.
    fn replay(&mut self, client: u64, after: u64) {
        let oldest_retained = self.log.front().map_or(self.next_seq.max(1), |frame| {
            frame.seq.expect("logged frames are sequenced")
        });
        if after > self.newest_seq() || after + 1 < oldest_retained {
            let resync = Frame::emitted_now(
                None,
                Event::Resync {
                    after,
                    oldest_retained,
                    newest: self.newest_seq(),
                },
            );
            self.send(client, &resync);
            self.send_snapshot(client);
            return;
        }
        let missed: Vec<Frame> = self
            .log
            .iter()
            .filter(|frame| frame.seq.is_some_and(|seq| seq > after))
            .cloned()
            .collect();
        for frame in &missed {
            self.send(client, frame);
        }
    }

    fn broadcast(&mut self, frame: &Frame) {
        let line = encode(frame);
        self.clients
            .retain(|_, sink| sink.try_send(line.clone()).is_ok());
    }

    fn send(&mut self, client: u64, frame: &Frame) {
        if let Some(sink) = self.clients.get(&client)
            && sink.try_send(encode(frame)).is_err()
        {
            self.clients.remove(&client);
        }
    }
}

fn encode(frame: &Frame) -> String {
    let mut line = serde_json::to_string(frame).expect("a frame is always serializable");
    line.push('\n');
    line
}

/// What a request asks of the writer once it has been answered.
enum Followup {
    Nothing,
    Snapshot,
    Subscribe(Option<u64>),
}

fn answer_line(line: &str, client: u64, shared: &Shared) {
    let value = serde_json::from_str::<serde_json::Value>(line).ok();
    let request_id = value
        .as_ref()
        .and_then(|value| value.get("request_id"))
        .and_then(|id| id.as_str())
        .map(str::to_owned);
    // A resent request is answered again, not acted on again.
    if let Some(id) = &request_id {
        let remembered = shared.remembered.lock_unpoisoned().by_id.get(id).cloned();
        if let Some(answer) = remembered {
            send_reply(shared, client, id, &answer);
            return;
        }
    }
    let (answer, followup) = match value
        .ok_or_else(|| "Unreadable request: not JSON".to_owned())
        .and_then(|value| {
            serde_json::from_value::<Envelope>(value)
                .map_err(|error| format!("Unreadable request: {error}"))
        }) {
        Ok(envelope) => apply(envelope.request, shared),
        Err(detail) => (
            Answer::refused(RejectionReason::Malformed, detail),
            Followup::Nothing,
        ),
    };
    match &request_id {
        Some(id) => {
            shared
                .remembered
                .lock_unpoisoned()
                .remember(id, answer.clone());
            send_reply(shared, client, id, &answer);
        }
        None => {
            if let Some((_, detail)) = &answer.refusal {
                let _ = shared.outgoing.send(Outgoing::Log(Event::Rejected {
                    reason: detail.clone(),
                }));
            }
        }
    }
    match followup {
        Followup::Nothing => {}
        Followup::Snapshot => {
            let _ = shared.outgoing.send(Outgoing::Snapshot(client));
        }
        Followup::Subscribe(after) => {
            let _ = shared.outgoing.send(Outgoing::Subscribe(client, after));
        }
    }
}

fn send_reply(shared: &Shared, client: u64, id: &str, answer: &Answer) {
    let _ = shared.outgoing.send(Outgoing::Reply(
        client,
        Event::Reply {
            request_id: id.to_owned(),
            accepted: answer.refusal.is_none(),
            record: answer.record.clone(),
            records: answer.records.clone(),
            plan: answer.plan.clone(),
            projection: answer.projection.clone(),
            description: answer.description.clone(),
            ik: answer.ik.clone(),
            lease: answer.lease.clone(),
            reason: answer.refusal.as_ref().map(|(reason, _)| *reason),
            detail: answer.refusal.as_ref().map(|(_, detail)| detail.clone()),
        },
    ));
}

/// The refusal for an arm without a tool link to plan for.
fn without_planner() -> Answer {
    Answer::refused(
        RejectionReason::InvalidTarget,
        "This arm has no tool link to plan for".to_owned(),
    )
}

/// Plan a move without touching the arm.
fn plan_answer(
    shared: &Shared,
    start: &[f64],
    waypoints: &[Waypoint],
    joint_speed: Option<f64>,
) -> Answer {
    let Some(planner) = shared.planner.as_deref() else {
        return without_planner();
    };
    match planner.plan(start, waypoints, joint_speed) {
        Ok(plan) => Answer::planned(plan),
        Err(refused) => Answer::refused(refused.reason, refused.detail),
    }
}

/// Where the arm could take the tool toward a pose, without touching the arm:
/// the policy's rehearsal on a policy arm, the solve and checks otherwise.
fn projection_answer(
    shared: &Shared,
    position: [f64; 3],
    orientation: Option<PoseOrientation>,
    joints: &[f64],
    gripper_angle: Option<f64>,
) -> Answer {
    let Some(planner) = shared.planner.as_deref() else {
        return without_planner();
    };
    let projection = match (&shared.preview, &shared.reach_settings) {
        (Some(preview), Some(settings)) => {
            planner.project_by_policy(joints, position, orientation, |position, quaternion| {
                let request = PreviewRequest {
                    goal: ReachGoal {
                        position,
                        quaternion,
                        position_tolerance_meters: settings.position_tolerance_meters,
                        orientation_tolerance_radians: settings.orientation_tolerance_radians,
                    },
                    joints: joints.to_vec(),
                    gripper_angle,
                };
                preview
                    .lock_unpoisoned()
                    .reach(&request)
                    .map_err(|error| format!("{error:#}"))
            })
        }
        _ => planner.project(joints, position, orientation, gripper_angle),
    };
    Answer::projected(projection)
}

/// The tool's pose at some joints.
fn description_answer(shared: &Shared, joints: &[f64]) -> Answer {
    let Some(planner) = shared.planner.as_deref() else {
        return without_planner();
    };
    match planner.describe(joints) {
        Ok(description) => Answer::described(description),
        Err(detail) => Answer::refused(RejectionReason::InvalidTarget, detail),
    }
}

/// Extend the holder's lease, or refuse a token that is not the holder's.
fn retain_answer(shared: &Shared, token: Option<&str>) -> Answer {
    if shared.lease.retain(token, Instant::now()) {
        Answer::nothing()
    } else {
        Answer::refused(
            RejectionReason::LeaseHeldElsewhere,
            "That lease has lapsed or been taken".to_owned(),
        )
    }
}

/// Solve for the tool's pose without touching the arm.
fn solve_answer(
    shared: &Shared,
    position: [f64; 3],
    orientation: OrientationGoal,
    seed: &[f64],
) -> Answer {
    let Some(planner) = shared.planner.as_deref() else {
        return without_planner();
    };
    match planner.solver().solve(position, orientation, seed) {
        Ok(result) => Answer::solved(result),
        Err(refusal) => Answer::refused(RejectionReason::InvalidTarget, refusal.to_string()),
    }
}

fn command_record(answer: Answer) -> Result<CommandRecord, (RejectionReason, String)> {
    match (answer.refusal, answer.record) {
        (Some(refusal), _) => Err(refusal),
        (None, Some(record)) => Ok(record),
        (None, None) => Err((
            RejectionReason::Malformed,
            "The arm gave no record".to_owned(),
        )),
    }
}

/// Admit the handoff under the lease and return its reply receiver.
fn enqueue_admitted<R>(
    shared: &Shared,
    token: Option<&str>,
    always_allowed: bool,
    options: AdmissionOptions,
    gates: &[ArmGate],
    generation: &[(Arc<AtomicU64>, u64)],
    enqueue: impl FnOnce() -> Result<R, crate::arm::Rejection>,
) -> Result<R, crate::arm::Rejection> {
    let answer = if always_allowed {
        shared
            .lease
            .interrupt_if_current(&shared.generation, generation, enqueue)??
    } else {
        shared.lease.admit_generation(
            token,
            Instant::now(),
            gates,
            generation,
            options,
            enqueue,
        )??
    };
    Ok(answer)
}

/// Wait for an already admitted handoff without holding the lease lock.
fn admitted<T>(
    shared: &Shared,
    token: Option<&str>,
    always_allowed: bool,
    options: AdmissionOptions,
    gates: &[ArmGate],
    generation: &[(Arc<AtomicU64>, u64)],
    enqueue: impl FnOnce() -> Result<
        std::sync::mpsc::Receiver<Result<T, crate::arm::Rejection>>,
        crate::arm::Rejection,
    >,
) -> Result<T, crate::arm::Rejection> {
    enqueue_admitted(
        shared,
        token,
        always_allowed,
        options,
        gates,
        generation,
        enqueue,
    )?
    .recv()
    .map_err(|_| crate::handoff::loop_not_running())?
}

fn submit_arm_command(
    shared: &Shared,
    command: hebbian_contracts::Command,
    token: Option<&str>,
    trace_context: Option<hebbian_contracts::TraceContext>,
    expected_revision: Option<u64>,
    generation: &[(Arc<AtomicU64>, u64)],
) -> Answer {
    if matches!(command, hebbian_contracts::Command::ApproveRecovery) {
        return match shared
            .lease
            .admit_recovery_approval(generation, || {
                shared.arm.submit_unawaited(command, trace_context)
            })
            .and_then(std::convert::identity)
            .map(|answer| answer.recv())
        {
            Ok(Ok(Ok(record))) => Answer::accepted(Some(record)),
            Ok(Ok(Err(refused))) | Err(refused) => Answer::refused(refused.reason, refused.detail),
            Ok(Err(_)) => Answer::refused(
                hebbian_contracts::RejectionReason::NotEnabled,
                "The control loop is not running".into(),
            ),
        };
    }
    let always_allowed = is_always_allowed(&command);
    let options = AdmissionOptions {
        allow_fault_recovery: matches!(command, hebbian_contracts::Command::ClearFault),
        allow_while_interlocked: is_allowed_while_interlocked(&command),
    };
    let gate = ArmGate {
        interlocks: Arc::clone(&shared.interlocks),
        expected_revision,
    };
    match admitted(
        shared,
        token,
        always_allowed,
        options,
        std::slice::from_ref(&gate),
        generation,
        || shared.arm.submit_unawaited(command, trace_context),
    ) {
        Ok(record) => Answer::accepted(Some(record)),
        Err(refused) => Answer::refused(refused.reason, refused.detail),
    }
}

fn submit_group_move(
    shared: &Shared,
    label: String,
    moves: Vec<ArmMove>,
    token: Option<&str>,
    trace_context: Option<hebbian_contracts::TraceContext>,
    gates: &[ArmGate],
    generation: &[(Arc<AtomicU64>, u64)],
) -> Answer {
    match admitted(
        shared,
        token,
        false,
        AdmissionOptions::default(),
        gates,
        generation,
        || shared.arm.move_arms_unawaited(label, moves, trace_context),
    ) {
        Ok(records) => Answer::accepted_group(records),
        Err(refused) => Answer::refused(refused.reason, refused.detail),
    }
}

/// Act on one request: the arm's answer, and what the writer owes the client.
fn apply(request: Request, shared: &Shared) -> (Answer, Followup) {
    match request {
        Request::Command {
            command,
            lease_token,
        } => {
            let answer =
                submit_arm_command(shared, command, lease_token.as_deref(), None, None, &[]);
            (answer, Followup::Nothing)
        }
        Request::MoveArms {
            label,
            moves,
            lease_token,
        } => {
            let gate = ArmGate {
                interlocks: Arc::clone(&shared.interlocks),
                expected_revision: None,
            };
            let answer = submit_group_move(
                shared,
                label,
                moves,
                lease_token.as_deref(),
                None,
                std::slice::from_ref(&gate),
                &[],
            );
            (answer, Followup::Nothing)
        }
        request @ (Request::AcquireLease { .. } | Request::TakeLease { .. }) => {
            (acquire_answer(shared, request), Followup::Nothing)
        }
        Request::RetainLease { lease_token } => (
            retain_answer(shared, lease_token.as_deref()),
            Followup::Nothing,
        ),
        Request::ReturnLease { lease_token } => {
            shared
                .lease
                .return_lease(lease_token.as_deref(), Instant::now());
            (Answer::nothing(), Followup::Nothing)
        }
        Request::RevokeLease => {
            let answer = if shared.lease.revoke() {
                Answer::nothing()
            } else {
                Answer::refused(
                    RejectionReason::ArmBusy,
                    "The lease cannot be revoked until the interrupted takeover succeeds".into(),
                )
            };
            (answer, Followup::Nothing)
        }
        Request::ReadLease => {
            let status = shared.lease.status(Instant::now());
            (
                Answer::leased(LeaseAnswer::Holder { status }),
                Followup::Nothing,
            )
        }
        Request::PlanMotion {
            start,
            waypoints,
            joint_speed,
        } => (
            plan_answer(shared, &start, &waypoints, joint_speed),
            Followup::Nothing,
        ),
        Request::ProjectPose {
            position,
            orientation,
            joints,
            gripper_angle,
        } => (
            projection_answer(shared, position, orientation, &joints, gripper_angle),
            Followup::Nothing,
        ),
        Request::DescribePose { joints } => {
            (description_answer(shared, &joints), Followup::Nothing)
        }
        Request::SolvePose {
            position,
            orientation,
            seed,
        } => (
            solve_answer(shared, position, orientation, &seed),
            Followup::Nothing,
        ),
        Request::Snapshot => (Answer::accepted(None), Followup::Snapshot),
        Request::Subscribe { after } => (Answer::accepted(None), Followup::Subscribe(after)),
    }
}

fn acquire_answer(shared: &Shared, request: Request) -> Answer {
    let (request, take) = match request {
        Request::AcquireLease {
            holder,
            label,
            required_cameras,
            on_interruption,
        } => (
            LeaseRequest {
                holder,
                label,
                required_cameras,
                on_interruption,
            },
            false,
        ),
        Request::TakeLease {
            holder,
            label,
            required_cameras,
            on_interruption,
        } => (
            LeaseRequest {
                holder,
                label,
                required_cameras,
                on_interruption,
            },
            true,
        ),
        _ => unreachable!("only lease requests are acquired"),
    };
    match shared.lease.acquire_stopping_arms(request, take) {
        Ok(grant) => Answer::leased(LeaseAnswer::Granted(grant)),
        Err(AcquireError::Held(holder)) => Answer::held_by(holder),
        Err(AcquireError::StopFailed { reason, detail }) => Answer::refused(reason, detail),
    }
}
