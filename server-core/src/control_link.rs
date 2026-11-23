// INVARIANT: The control loop hands output to one writer thread. Samples may be dropped under backpressure, but log events retain sequence order; slow clients disconnect and resume from the retained log.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::arm::{PoseGoalSettings, ReachGoal};
use crate::config::ControllerConfig;
use crate::handoff::{ArmHandle, ArmInbox, arm_channel};
use crate::lease::{
    AcquireError, AcquireMode, ArmGate, CommandAuthority, GenerationTicket, Lease, LeaseAdmission,
    LeaseRequest, SharedInterlocks,
};
use crate::planning::Planner;
use crate::preview::{Preview, PreviewRequest};
use crate::sync::LockUnpoisoned;
use hebbian_contracts::{
    ArmMode, ArmMove, CommandRecord, Event, Frame, Interlock, LeaseStatus, LoggedEvent,
    PoseOrientation, PoseProjection, RejectionReason, Sample, UnixNanos,
};

const MAXIMUM_PENDING_SAMPLES: usize = 64;
const RETAINED_LOG_LENGTH: usize = 4096;
const SNAPSHOT_RECORDS: usize = 200;
const REMEMBERED_RECORDS: usize = 10_000;
const WRITER_POLL: Duration = Duration::from_millis(20);

enum Outgoing {
    Sample(Sample),
    Log(LoggedEvent),
    EnsureRecord(CommandRecord),
    ControllerUnavailable(String),
    Connected(u64, ClientSink),
    Disconnected(u64),
    Snapshot(u64),
    FindRecord(String, tokio::sync::oneshot::Sender<Option<CommandRecord>>),
    AwaitCompletion(String, tokio::sync::oneshot::Sender<CommandRecord>),
    WaitForSample(SampleRequest),
    View(tokio::sync::oneshot::Sender<ArmView>),
}

struct SampleRequest {
    deadline: Instant,
    answer: SyncSender<Sample>,
}

// CONSTRAINT: Disconnect clients whose output queue is full so one slow reader cannot stall other clients.
type ClientSink = tokio::sync::mpsc::Sender<Frame>;

// WHY: Recycle sample buffers after the writer sends them to avoid allocations in the control loop.
pub(crate) struct SampleRecycler {
    returned: Receiver<Sample>,
}

impl SampleRecycler {
    pub(crate) fn checkout(&self) -> Sample {
        self.returned.try_recv().unwrap_or_else(|_| Sample::blank())
    }
}

pub(crate) struct LoopEnd {
    pub(crate) inbox: ArmInbox,
    pub(crate) samples: SampleRecycler,
}

#[derive(Clone)]
struct Shared {
    arm: ArmHandle,
    generation: Arc<AtomicU64>,
    lease: Arc<Lease>,
    interlocks: SharedInterlocks,
    outgoing: Sender<Outgoing>,
    preview: Option<Arc<Mutex<Preview>>>,
    planner: Option<Arc<Planner>>,
    reach_settings: Option<PoseGoalSettings>,
    gripper: Option<crate::config::GripperLimits>,
    has_policy: bool,
    config: Arc<ControllerConfig>,
    next_client: Arc<AtomicU64>,
    changes: Arc<tokio::sync::watch::Sender<Option<ArmMode>>>,
}

#[derive(Debug, Clone)]
pub struct ArmView {
    pub sample: Option<Sample>,
    pub sample_age: Option<Duration>,
    pub sample_received_at: Option<UnixNanos>,
    pub active_record: Option<CommandRecord>,
    pub last_record: Option<CommandRecord>,
}

#[derive(Clone)]
pub struct LinkHandle {
    shared: Shared,
}

impl LinkHandle {
    pub fn controller_unavailable(&self, reason: String) {
        let _ = self
            .shared
            .outgoing
            .send(Outgoing::ControllerUnavailable(reason));
    }

    #[must_use]
    pub fn controller_loop_arm(&self) -> crate::controller_bridge::LoopArm {
        crate::controller_bridge::LoopArm::new(self.shared.arm.clone())
    }

    #[must_use]
    pub fn stop_if_leased(&self, token: Option<&str>) -> hebbian_contracts::LeasedStopOutcome {
        self.stop_with_lease(token, None, || {
            self.shared
                .arm
                .submit_unawaited(hebbian_contracts::Command::Stop, None)
        })
        .map_or_else(
            |outcome| outcome,
            |_| hebbian_contracts::LeasedStopOutcome::Submitted,
        )
    }

    pub(crate) fn cancel_leased_command(
        &self,
        token: &str,
        command_id: String,
    ) -> hebbian_contracts::LeasedStopOutcome {
        use hebbian_contracts::LeasedStopOutcome;
        self.with_lease(token, || {
            if self.shared.arm.cancel_command(command_id).is_ok() {
                LeasedStopOutcome::Submitted
            } else {
                LeasedStopOutcome::Unavailable
            }
        })
        .unwrap_or(LeasedStopOutcome::LeaseLost)
    }

    pub(crate) fn stop_execution(
        &self,
        token: &str,
        ticket: &GenerationTicket,
    ) -> Option<tokio::sync::oneshot::Receiver<Result<CommandRecord, crate::arm::Rejection>>> {
        let expected = ticket.expected_for(&self.shared.generation)?;
        self.stop_with_lease(Some(token), Some(expected), || {
            self.shared
                .arm
                .submit_async(hebbian_contracts::Command::Stop, None)
        })
        .ok()
    }

    pub(crate) async fn wait_for_execution_stop(&self) {
        use std::sync::mpsc::TryRecvError;

        let wait = async {
            let poll_interval = Duration::from_millis(10);
            let mut changes = self.changes();
            loop {
                let Ok(reply) = self.shared.arm.control_handoff_readiness() else {
                    return;
                };
                let ready = loop {
                    match reply.try_recv() {
                        Ok(Ok(ready)) => break ready,
                        Err(TryRecvError::Empty) => tokio::time::sleep(poll_interval).await,
                        Ok(Err(_)) | Err(TryRecvError::Disconnected) => return,
                    }
                };
                if ready {
                    // INVARIANT: The core is stopped. Let the sample used to plan the next
                    // command catch up before publishing the terminal sequence state.
                    let _ = changes
                        .wait_for(|mode| *mode != Some(ArmMode::Moving))
                        .await;
                    return;
                }
                tokio::time::sleep(poll_interval).await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_secs(5), wait).await;
    }

    fn stop_with_lease<Reply>(
        &self,
        token: Option<&str>,
        generation: Option<u64>,
        enqueue: impl FnOnce() -> Result<Reply, crate::arm::Rejection>,
    ) -> Result<Reply, hebbian_contracts::LeasedStopOutcome> {
        self.shared
            .lease
            .with_captured_lease(token, || {
                if generation.is_some_and(|expected| {
                    self.shared.generation.load(Ordering::Acquire) != expected
                }) {
                    return None;
                }
                self.shared.generation.fetch_add(1, Ordering::AcqRel);
                enqueue().ok()
            })
            .ok_or(hebbian_contracts::LeasedStopOutcome::LeaseLost)?
            .ok_or(hebbian_contracts::LeasedStopOutcome::Unavailable)
    }

    #[must_use]
    pub fn with_lease<R>(&self, token: &str, action: impl FnOnce() -> R) -> Option<R> {
        self.shared.lease.with_lease(token, action)
    }

    #[must_use]
    pub fn stop_leased_motion(&self, token: &str) -> bool {
        self.shared.lease.stop_leased_motion(token)
    }

    pub(crate) fn stop_leased_motion_for_submission(
        &self,
        token: &str,
        generations: &mut [GenerationTicket],
        cancel_sequence: impl FnOnce() -> bool,
    ) -> Result<(), crate::arm::Rejection> {
        self.shared
            .lease
            .stop_leased_motion_for_submission(token, generations, cancel_sequence)
    }

    #[must_use]
    pub fn camera_requirements(&self) -> Option<crate::lease::CameraRequirements> {
        self.shared.lease.camera_requirements()
    }

    pub fn report_camera(
        &self,
        token: &str,
        camera: &str,
        available: bool,
    ) -> Result<(), crate::lease::CameraReportRefusal> {
        self.shared.lease.report_camera(token, camera, available)
    }

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

    #[must_use]
    pub fn connect(&self, capacity: usize) -> (u64, tokio::sync::mpsc::Receiver<Frame>) {
        let client = self.shared.next_client.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = tokio::sync::mpsc::channel(capacity);
        let _ = self
            .shared
            .outgoing
            .send(Outgoing::Connected(client, sender));
        (client, receiver)
    }

    // CONSTRAINT: This waits for control-loop admission; call it outside async runtime workers.
    pub async fn find_record(&self, command_id: String) -> Option<CommandRecord> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.shared
            .outgoing
            .send(Outgoing::FindRecord(command_id, answer))
            .ok()?;
        answered.await.ok().flatten()
    }

    pub(crate) async fn await_completion(&self, command_id: String) -> Option<CommandRecord> {
        let (complete, completion) = tokio::sync::oneshot::channel();
        self.shared
            .outgoing
            .send(Outgoing::AwaitCompletion(command_id, complete))
            .ok()?;
        completion.await.ok()
    }

    pub fn snapshot(&self, client: u64) {
        let _ = self.shared.outgoing.send(Outgoing::Snapshot(client));
    }

    pub fn disconnect(&self, client: u64) {
        let _ = self.shared.outgoing.send(Outgoing::Disconnected(client));
    }

    #[must_use]
    pub fn changes(&self) -> tokio::sync::watch::Receiver<Option<ArmMode>> {
        self.shared.changes.subscribe()
    }

    pub(crate) fn current_mode(&self) -> Option<ArmMode> {
        *self.shared.changes.borrow()
    }

    pub(crate) fn authority_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.shared.lease.authority_changes()
    }

    pub(crate) fn execution_authorized(&self, token: &str, gates: &[ArmGate]) -> bool {
        self.shared.lease.execution_authorized(token, gates)
    }

    // CONSTRAINT: Waiting for the writer blocks; call this outside the async runtime.
    pub(crate) fn wait_for_sample(&self, timeout: Duration) -> Option<Sample> {
        let (answer, answered) = sync_channel(1);
        let deadline = Instant::now() + timeout;
        self.shared
            .outgoing
            .send(Outgoing::WaitForSample(SampleRequest { deadline, answer }))
            .ok()?;
        answered
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .ok()
    }

    pub async fn view(&self) -> Option<ArmView> {
        let (answer, answered) = tokio::sync::oneshot::channel();
        self.shared.outgoing.send(Outgoing::View(answer)).ok()?;
        answered.await.ok()
    }

    #[must_use]
    pub fn config(&self) -> &ControllerConfig {
        &self.shared.config
    }

    pub(crate) fn tick_lease(&self) {
        self.shared.lease.tick(Instant::now());
    }

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

    pub(crate) fn submission_generation(&self) -> GenerationTicket {
        GenerationTicket::capture(&self.shared.generation)
    }

    pub(crate) fn fence_prior_submissions(&self) -> GenerationTicket {
        GenerationTicket::fence(&self.shared.generation)
    }

    pub(crate) fn submit_command_async(
        &self,
        command: hebbian_contracts::Command,
        token: Option<&str>,
        trace_context: Option<hebbian_contracts::TraceContext>,
        expected_revision: Option<u64>,
        generation: &[GenerationTicket],
    ) -> Result<
        tokio::sync::oneshot::Receiver<Result<CommandRecord, crate::arm::Rejection>>,
        crate::arm::Rejection,
    > {
        let authority = CommandAuthority::for_command(&command);
        let enqueue = || self.shared.arm.submit_async(command, trace_context);
        match authority {
            CommandAuthority::Interrupt => self.shared.lease.interrupt_if_current(
                &self.shared.generation,
                generation,
                enqueue,
            )?,
            CommandAuthority::ApproveRecovery => self
                .shared
                .lease
                .admit_recovery_approval(generation, enqueue)?,
            CommandAuthority::Lease(admission) => {
                let gate = ArmGate {
                    interlocks: Arc::clone(&self.shared.interlocks),
                    expected_revision,
                };
                enqueue_with_lease(
                    &self.shared,
                    token,
                    admission,
                    std::slice::from_ref(&gate),
                    generation,
                    enqueue,
                )
            }
        }
    }

    pub(crate) fn action_step_async(
        &self,
        actions: Vec<crate::action_step::PreparedArmAction>,
        token: Option<&str>,
        gates: &[ArmGate],
        generation: &[GenerationTicket],
        cancellation: &tokio::sync::watch::Receiver<bool>,
    ) -> Result<
        tokio::sync::oneshot::Receiver<
            Result<Vec<hebbian_contracts::ArmRecord>, crate::arm::Rejection>,
        >,
        crate::arm::Rejection,
    > {
        enqueue_with_lease(
            &self.shared,
            token,
            LeaseAdmission::Normal,
            gates,
            generation,
            || {
                // INVARIANT: Cancellation and enqueue are ordered while admission holds the lease.
                let cancelled = cancellation.borrow();
                if *cancelled {
                    return Err(crate::arm::Rejection {
                        reason: hebbian_contracts::RejectionReason::ArmBusy,
                        detail: "Program execution canceled".into(),
                    });
                }
                self.shared.arm.action_step_async(actions, None)
            },
        )
    }

    pub(crate) fn move_arms_async(
        &self,
        moves: Vec<ArmMove>,
        token: Option<&str>,
        trace_context: Option<hebbian_contracts::TraceContext>,
        gates: &[ArmGate],
        generation: &[GenerationTicket],
    ) -> Result<
        tokio::sync::oneshot::Receiver<
            Result<Vec<hebbian_contracts::ArmRecord>, crate::arm::Rejection>,
        >,
        crate::arm::Rejection,
    > {
        enqueue_with_lease(
            &self.shared,
            token,
            LeaseAdmission::Normal,
            gates,
            generation,
            || {
                let actions = moves
                    .into_iter()
                    .map(|part| {
                        crate::action_step::PreparedArmAction::from_group_move("move_arms", part)
                    })
                    .collect();
                self.shared.arm.action_step_async(actions, trace_context)
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

    pub fn project(
        &self,
        position: [f64; 3],
        orientation: Option<PoseOrientation>,
        joints: &[f64],
        gripper_angle: Option<f64>,
    ) -> Result<PoseProjection, (RejectionReason, String)> {
        let planner = self.shared.planner.as_deref().ok_or_else(|| {
            (
                RejectionReason::InvalidTarget,
                "This arm has no tool link to plan for".to_owned(),
            )
        })?;
        Ok(match (&self.shared.preview, &self.shared.reach_settings) {
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
        })
    }

    pub fn stop(&self) {
        if let Ok(answer) = self.interrupt(None) {
            let _ = answer.recv();
        }
    }

    // INVARIANT: End a disconnected holder and queue stops before another holder can command.
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

    pub fn acquire_lease(
        &self,
        request: LeaseRequest,
        mode: AcquireMode,
    ) -> Result<hebbian_contracts::LeaseGrant, AcquireError> {
        self.shared.lease.acquire_stopping_arms(request, mode)
    }

    #[must_use]
    pub fn retain_lease(&self, token: &str) -> bool {
        self.shared.lease.retain(Some(token), Instant::now())
    }

    // INVARIANT: Release a current lease and cancel its action sequences under the lease lock.
    pub(crate) fn release_lease_with(
        &self,
        token: &str,
        cancel_sequence: impl FnOnce() -> bool,
    ) -> bool {
        self.shared.lease.release_with(token, cancel_sequence)
    }

    pub fn return_lease(&self, token: &str) {
        self.shared.lease.return_lease(Some(token), Instant::now());
    }
}

pub struct Link {
    shared: Shared,
    pending_samples: Arc<AtomicUsize>,
    closing: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
    loop_end: Mutex<Option<LoopEnd>>,
}

impl Link {
    // INVARIANT: Preserve an accepted record before answering a station client.
    pub fn ensure_controller_record(&self, record: CommandRecord) {
        let _ = self.shared.outgoing.send(Outgoing::EnsureRecord(record));
    }

    pub fn controller_unavailable(&self, reason: String) {
        self.handle().controller_unavailable(reason);
    }

    #[must_use]
    pub fn take_controller_station_arm(&self) -> Option<crate::controller_bridge::StationArm> {
        self.take_loop_end()
            .map(|end| crate::controller_bridge::StationArm::new(end.inbox))
    }

    #[must_use]
    pub fn controller_loop_arm(&self) -> crate::controller_bridge::LoopArm {
        crate::controller_bridge::LoopArm::new(self.shared.arm.clone())
    }

    pub fn new(config: &ControllerConfig, lease: Arc<Lease>) -> anyhow::Result<Self> {
        let preview = Preview::new(config)?.map(|preview| Arc::new(Mutex::new(preview)));
        let planner = Planner::for_arm(config)?.map(Arc::new);
        Ok(Self::with_handoff(config, lease, planner, preview))
    }

    pub fn for_controller(config: &ControllerConfig, lease: Arc<Lease>) -> Self {
        Self::with_handoff(config, lease, None, None)
    }

    fn with_handoff(
        config: &ControllerConfig,
        lease: Arc<Lease>,
        planner: Option<Arc<Planner>>,
        preview: Option<Arc<Mutex<Preview>>>,
    ) -> Self {
        let (outgoing, queue) = channel::<Outgoing>();
        let pending_samples = Arc::new(AtomicUsize::new(0));
        let closing = Arc::new(AtomicBool::new(false));

        let (recycle_sample, returned_samples) = sync_channel::<Sample>(MAXIMUM_PENDING_SAMPLES);
        let changes = Arc::new(tokio::sync::watch::Sender::new(None));
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
            preview,
            planner,
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
        Self {
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
        }
    }

    #[must_use]
    pub fn handle(&self) -> LinkHandle {
        LinkHandle {
            shared: self.shared.clone(),
        }
    }

    pub(crate) fn take_loop_end(&self) -> Option<LoopEnd> {
        self.loop_end.lock_unpoisoned().take()
    }

    pub(crate) fn log_publisher(&self) -> impl Fn(LoggedEvent) + Send + 'static + use<> {
        let log_sender = self.shared.outgoing.clone();
        move |event| {
            let _ = log_sender.send(Outgoing::Log(event));
        }
    }

    pub fn publish_sample(&self, sample: Sample) {
        if self.pending_samples.load(Ordering::Relaxed) < MAXIMUM_PENDING_SAMPLES {
            self.pending_samples.fetch_add(1, Ordering::Relaxed);
            let _ = self.shared.outgoing.send(Outgoing::Sample(sample));
        }
    }

    pub fn publish_log(&self, event: LoggedEvent) {
        let _ = self.shared.outgoing.send(Outgoing::Log(event));
    }

    pub fn close(mut self) {
        self.closing.store(true, Ordering::Relaxed);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
    }
}

#[derive(Clone, Copy)]
struct SampleReceipt {
    monotonic: Instant,
    unix: UnixNanos,
}

#[derive(Default)]
struct Hub {
    clients: HashMap<u64, ClientSink>,
    log: VecDeque<Frame>,
    next_seq: u64,
    latest_sample: Option<Sample>,
    latest_sample_at: Option<SampleReceipt>,
    sample_waiters: Vec<SampleRequest>,
    records: HashMap<String, CommandRecord>,
    record_order: VecDeque<String>,
    completion_waiters: HashMap<String, Vec<tokio::sync::oneshot::Sender<CommandRecord>>>,
    // INVARIANT: The command that finished latest and still awaits the resting sample that
    // follows its completion, with its finish time. Commands can be logged out of
    // finish order (a release logs the motion it interrupted after its own record),
    // so the latest finish time wins, not the last record logged.
    pending_finished: Option<(String, f64)>,
    settled_last: Option<(String, f64)>,
    mode: Option<(ArmMode, Option<String>)>,
    recycle_sample: Option<SyncSender<Sample>>,
    changes: Option<Arc<tokio::sync::watch::Sender<Option<ArmMode>>>>,
}

impl Hub {
    fn newest_seq(&self) -> u64 {
        self.next_seq.saturating_sub(1)
    }

    fn handle(&mut self, message: Outgoing, pending_samples: &AtomicUsize) {
        match message {
            Outgoing::Sample(sample) => self.receive_sample(sample, pending_samples),
            Outgoing::Log(event) => {
                self.log_event(event.into());
                self.announce_change();
            }
            Outgoing::EnsureRecord(record) => {
                if !self.records.contains_key(&record.command_id) {
                    self.log_event(Event::Record(record));
                    self.announce_change();
                }
            }
            Outgoing::ControllerUnavailable(reason) => self.controller_unavailable(&reason),
            Outgoing::Connected(client, sink) => {
                self.clients.insert(client, sink);
            }
            Outgoing::Disconnected(client) => {
                self.clients.remove(&client);
            }
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
                    sample_age: self.latest_sample_at.map(|at| at.monotonic.elapsed()),
                    sample_received_at: self.latest_sample_at.map(|at| at.unix),
                    active_record,
                    last_record: record(self.settled_last.as_ref().map(|(id, _)| id)),
                });
            }
            Outgoing::WaitForSample(request) => {
                let now = Instant::now();
                if now < request.deadline {
                    if let Some(sample) = &self.latest_sample {
                        let _ = request.answer.try_send(sample.clone());
                    } else {
                        self.sample_waiters.retain(|waiting| now < waiting.deadline);
                        self.sample_waiters.push(request);
                    }
                }
            }
            Outgoing::FindRecord(command_id, answer) => {
                let _ = answer.send(self.records.get(&command_id).cloned());
            }
            Outgoing::AwaitCompletion(command_id, complete) => {
                if let Some(record) = self.records.get(&command_id) {
                    if record.is_pending() {
                        self.completion_waiters
                            .entry(command_id)
                            .or_default()
                            .push(complete);
                    } else {
                        let _ = complete.send(record.clone());
                    }
                }
            }
        }
    }

    fn receive_sample(&mut self, sample: Sample, pending_samples: &AtomicUsize) {
        let changed = self.latest_sample_at.is_none()
            || self.latest_sample.as_ref().is_none_or(|previous| {
                previous.mode != sample.mode
                    || previous.active_command != sample.active_command
                    || previous.active_trip != sample.active_trip
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
        let frame = Frame::emitted_now(None, Event::State(sample));
        self.latest_sample_at = Some(SampleReceipt {
            monotonic: Instant::now(),
            unix: frame.emitted_unix_nano,
        });
        self.broadcast(&frame);
        if let Event::State(sample) = frame.event
            && let Some(previous) = self.latest_sample.replace(sample)
            && let Some(recycle) = &self.recycle_sample
        {
            // WHY: Keep the newest sample and recycle the previous buffers without copying either.
            let _ = recycle.try_send(previous);
        }
        if let Some(sample) = &self.latest_sample
            && !self.sample_waiters.is_empty()
        {
            let now = Instant::now();
            for request in self.sample_waiters.drain(..) {
                if now < request.deadline {
                    let _ = request.answer.try_send(sample.clone());
                }
            }
        }
        // INVARIANT: Release the writer slot after publishing and recycling.
        pending_samples.fetch_sub(1, Ordering::Release);
        if changed {
            self.announce_change();
        }
    }

    fn controller_unavailable(&mut self, reason: &str) {
        self.latest_sample_at = None;
        if let Some(sample) = &mut self.latest_sample {
            sample.active_command = None;
        }
        self.log_event(Event::ControllerUnavailable {
            reason: reason.to_owned(),
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |duration| duration.as_secs_f64());
        let interrupted: Vec<_> = self
            .records
            .values()
            .filter(|record| record.is_pending())
            .cloned()
            .collect();
        for mut record in interrupted {
            record.phase = hebbian_contracts::CommandPhase::OutcomeUnknown {
                finished_at: now,
                detail: Some(format!(
                    "Controller lost; command outcome and arm torque are unknown: {reason}"
                )),
            };
            self.log_event(Event::Record(record));
        }
        self.announce_change();
    }

    fn announce_change(&self) {
        if let Some(changes) = &self.changes {
            let mode = self
                .latest_sample_at
                .and(self.latest_sample.as_ref())
                .map(|sample| sample.mode);
            changes.send_replace(mode);
        }
    }

    fn log_event(&mut self, event: Event) {
        match &event {
            Event::Record(record) => {
                if !record.is_pending()
                    && let Some(waiters) = self.completion_waiters.remove(&record.command_id)
                {
                    for complete in waiters {
                        let _ = complete.send(record.clone());
                    }
                }
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
                }
                while self.record_order.len() > REMEMBERED_RECORDS {
                    let Some(oldest_finished) = self.record_order.iter().position(|id| {
                        self.records
                            .get(id)
                            .is_some_and(|record| !record.is_pending())
                    }) else {
                        break;
                    };
                    if let Some(id) = self.record_order.remove(oldest_finished) {
                        self.records.remove(&id);
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
        if let Some(sink) = self.clients.get(&client)
            && sink.try_send(frame).is_err()
        {
            self.clients.remove(&client);
        }
    }

    fn broadcast(&mut self, frame: &Frame) {
        self.clients
            .retain(|_, sink| sink.try_send(frame.clone()).is_ok());
    }
}

fn enqueue_with_lease<R>(
    shared: &Shared,
    token: Option<&str>,
    admission: LeaseAdmission,
    gates: &[ArmGate],
    generation: &[GenerationTicket],
    enqueue: impl FnOnce() -> Result<R, crate::arm::Rejection>,
) -> Result<R, crate::arm::Rejection> {
    shared
        .lease
        .admit_generation(token, Instant::now(), gates, generation, admission, enqueue)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use hebbian_contracts::{Command, LeaseHolder, OnInterruption};

    #[test]
    fn a_feedback_request_made_before_publication_receives_the_first_sample() {
        let config = ControllerConfig::parse(crate::config::TEST_CONFIG).unwrap();
        let link = Link::new(&config, Arc::new(Lease::new(crate::lease::LEASE))).unwrap();
        let handle = link.handle();
        let (answer, received) = sync_channel(1);
        let deadline = Instant::now() + Duration::from_secs(2);
        handle
            .shared
            .outgoing
            .send(Outgoing::WaitForSample(SampleRequest { deadline, answer }))
            .unwrap();

        let mut first = Sample::blank();
        first.positions = vec![0.1, 0.2];
        link.publish_sample(first);

        let sample = received.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(sample.positions, vec![0.1, 0.2]);
        link.close();
    }

    #[tokio::test]
    async fn station_feedback_timestamp_only_changes_with_a_new_sample() {
        use crate::station_api::station_state;
        use std::collections::BTreeMap;

        let config =
            ControllerConfig::parse(include_str!("../tests/support/two_joint_config.json"))
                .unwrap();
        let link = Link::new(&config, Arc::new(Lease::new(crate::lease::LEASE))).unwrap();
        let links = BTreeMap::from([("arm".to_owned(), link.handle())]);
        link.publish_sample(Sample::blank());
        let first = station_state(&links).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        let unchanged = station_state(&links).await.unwrap();
        let sampled_at = first.arms[0].sampled_at_unix_seconds.unwrap();
        assert_eq!(unchanged.arms[0].sampled_at_unix_seconds, Some(sampled_at));

        link.publish_sample(Sample::blank());
        let next = station_state(&links).await.unwrap();
        assert!(next.arms[0].sampled_at_unix_seconds.unwrap() > sampled_at);
        link.close();
    }

    #[test]
    fn cleanup_for_an_old_execution_leaves_the_next_command_admissible() {
        let config =
            ControllerConfig::parse(include_str!("../tests/support/two_joint_config.json"))
                .unwrap();
        let link = Link::new(&config, Arc::new(Lease::new(crate::lease::LEASE))).unwrap();
        let handle = link.handle();
        let lease = handle
            .acquire_lease(
                LeaseRequest {
                    holder: LeaseHolder::Agent,
                    label: "program".into(),
                    required_cameras: Vec::new(),
                    on_interruption: OnInterruption::WaitForApproval,
                },
                AcquireMode::WaitOrFail,
            )
            .unwrap();
        let previous_execution = handle.fence_prior_submissions();
        let current_execution = handle.fence_prior_submissions();

        let _ = handle.stop_execution(&lease.lease_token, &previous_execution);

        assert!(
            handle
                .submit_command_async(
                    Command::Enable,
                    Some(&lease.lease_token),
                    None,
                    None,
                    &[current_execution],
                )
                .is_ok()
        );
        link.close();
    }

    #[test]
    fn a_pending_motor_command_does_not_block_a_lease_claim() {
        let config =
            ControllerConfig::parse(include_str!("../tests/support/two_joint_config.json"))
                .unwrap();
        let link = Link::new(&config, Arc::new(Lease::new(crate::lease::LEASE))).unwrap();
        let handle = link.handle();
        let mut reply = handle
            .submit_command_async(Command::Enable, None, None, None, &[])
            .unwrap();
        assert!(matches!(
            reply.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        assert!(
            handle
                .acquire_lease(
                    LeaseRequest {
                        holder: LeaseHolder::Manual,
                        label: "test".into(),
                        required_cameras: Vec::new(),
                        on_interruption: OnInterruption::WaitForApproval,
                    },
                    AcquireMode::WaitOrFail,
                )
                .is_ok()
        );
        link.close();
    }
}
