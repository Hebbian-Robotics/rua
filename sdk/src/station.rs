#[path = "generated/station_commands.rs"]
mod commands;
#[path = "http.rs"]
mod http;

pub use http::CameraJpeg;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use hebbian_contracts::{
    ActionSequenceRecord, ActionSequenceRequest, ActionSequenceStartRequest, ArmRecord,
    CommandAnswer, CommandCancellationRequest, CommandOutcome, CommandRecord,
    CommandSubmissionRequest, GroupCommandSubmissionRequest, LeaseCameraReport, LeaseHolder,
    LeaseStatus, LeaseToken, MoveArmsCommand, OnInterruption, PoseOrientation,
    PosePreviewBatchRequest, PosePreviewCandidate, PoseProjection, PreviewAnswer, PreviewPose,
    RejectionReason, RunCompiledProgramRequest, RunEvent, StationCommand, StationInstance,
    StationState, TraceContext, TraceSpan,
};
use tokio::sync::{broadcast, watch};

use crate::connection::{RunUpdate, Shared};
use crate::error::{Error, Result};

pub const DEFAULT_SERVER: &str = "http://127.0.0.1:8792";
/// Default address of the direct media listener, for clients opting out of forwarding.
pub const DEFAULT_MEDIA_SERVER: &str = "http://127.0.0.1:8781";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a command's outcome is awaited across a lost connection.
const DISCONNECT_PATIENCE: Duration = Duration::from_secs(30);

/// Where to connect and what to resume.
#[derive(Debug, Clone)]
pub struct ConnectOptions {
    /// The station's HTTP base URL.
    pub server: String,
    /// Direct media URL for camera snapshots. An empty string uses the station URL.
    pub media_server: String,
    /// Resume the run telemetry stream after this sequence number; `Some(0)`
    /// replays everything retained, `None` starts at the newest record.
    pub run_after: Option<u64>,
}

impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            server: DEFAULT_SERVER.to_owned(),
            media_server: String::new(),
            run_after: None,
        }
    }
}

/// What a command carries beyond its arm and body.
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommandOptions {
    /// The operation identity to use for retries, status, and cancellation.
    pub operation_id: Option<uuid::Uuid>,
    /// Optional joint acceleration cap for planned arm moves, in rad/s².
    pub max_joint_acceleration: Option<f64>,
    /// Optional joint jerk cap for planned arm moves, in rad/s³.
    pub max_joint_jerk: Option<f64>,
    /// The caller's trace context; the command's record carries it.
    pub trace_context: Option<TraceContext>,
    /// Per arm, the revision the caller last read (`ArmState::revision`); the
    /// station refuses a command as `StaleRevision` once that arm's revision has
    /// moved on, so nothing acts on an interrupted view of the world.
    pub expected_revisions: BTreeMap<String, u64>,
}

/// What a client asks for when it acquires or takes the lease on every arm.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LeaseRequest {
    pub holder: LeaseHolder,
    pub label: String,
    /// Cameras the holder cannot work without.
    pub required_cameras: Vec<String>,
    pub on_interruption: OnInterruption,
}

impl LeaseRequest {
    #[must_use]
    pub fn new(holder: LeaseHolder, label: impl Into<String>) -> Self {
        Self {
            holder,
            label: label.into(),
            required_cameras: Vec::new(),
            on_interruption: OnInterruption::default(),
        }
    }
}

struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// A connection to a station. Cheap to clone; its event stream closes when the last
/// clone (and every [`LeaseGuard`]) is dropped.
#[derive(Clone)]
pub struct Station {
    pub(crate) shared: Arc<Shared>,
    reader: Arc<AbortOnDrop>,
    http: reqwest::Client,
    server: String,
    media_server: String,
}

impl Station {
    /// Connect to a station at its HTTP base URL, such as `http://127.0.0.1:8792`.
    ///
    /// # Errors
    /// [`Error::Connection`] when the station cannot be reached.
    pub async fn connect(server: &str) -> Result<Self> {
        let options = ConnectOptions {
            server: server.to_owned(),
            ..ConnectOptions::default()
        };
        Ok(Self::connect_with(options).await?.0)
    }

    /// Connect, also returning the run telemetry stream from the first message
    /// on, resumed after `options.run_after`.
    ///
    /// # Errors
    /// [`Error::Connection`] when the URL is not http(s) or the station cannot be reached.
    pub async fn connect_with(
        options: ConnectOptions,
    ) -> Result<(Self, broadcast::Receiver<RunUpdate>)> {
        let (station, run_updates) = Self::open(options)?;
        station.wait_until_connected().await?;
        Ok((station, run_updates))
    }

    /// Start the event reader on the current Tokio runtime. Call
    /// `wait_until_connected` to await the first handshake.
    ///
    /// # Errors
    /// The server URL is invalid or the HTTP client could not be built.
    pub fn open(options: ConnectOptions) -> Result<(Self, broadcast::Receiver<RunUpdate>)> {
        let url = reqwest::Url::parse(&options.server)
            .map_err(|error| Error::Connection(error.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(Error::Connection(
                "expected an http:// or https:// URL".into(),
            ));
        }
        let mut server = options.server;
        server.truncate(server.trim_end_matches('/').len());
        let media_server = if options.media_server.is_empty() {
            server.clone()
        } else {
            options.media_server.trim_end_matches('/').to_owned()
        };
        let shared = Arc::new(Shared::new(server.clone(), options.run_after));
        let run_updates = shared.subscribe_run();
        let task = Arc::clone(&shared);
        let reader = tokio::spawn(async move { task.keep_connected().await });
        let station = Self {
            shared,
            reader: Arc::new(AbortOnDrop(reader)),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            server,
            media_server,
        };
        Ok((station, run_updates))
    }

    /// Stop the event reader for this station and its clones.
    pub fn close_event_stream(&self) {
        self.reader.0.abort();
    }

    /// Wait for the first usable stream handshake.
    ///
    /// # Errors
    /// The initial connection failed or the connection deadline elapsed.
    pub async fn wait_until_connected(&self) -> Result<()> {
        // INVARIANT: A refused connection fails at once; a slow one gets CONNECT_TIMEOUT.
        let connected = tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.shared
                .wait_for(crate::connection::Inner::handshake_result),
        )
        .await
        .map_err(|_| Error::Connection(format!("no connection to {}", self.shared.url)))?;
        connected.map_err(|error| {
            Error::Connection(format!(
                "no station answered at {}: {error}. Check that `serve` is running and the station URL and port are right",
                self.shared.url
            ))
        })?;
        Ok(())
    }

    /// The station's latest state: pushed ten times a second, so this is cached.
    ///
    /// # Errors
    /// [`Error::Timeout`] when no state arrives.
    pub async fn state(&self) -> Result<StationState> {
        tokio::time::timeout(
            CONNECT_TIMEOUT,
            self.shared.wait_for(|inner| inner.state.clone()),
        )
        .await
        .map_err(|_| Error::Timeout("the station state"))
    }

    /// Waits for a state frame that satisfies `predicate`, for example every arm
    /// reporting a sample after the station starts, and returns it. Checks the
    /// latest frame first.
    ///
    /// # Errors
    /// [`Error::Timeout`] when no frame satisfies it in time.
    pub async fn wait_for_state(
        &self,
        mut predicate: impl FnMut(&StationState) -> bool,
    ) -> Result<StationState> {
        tokio::time::timeout(
            REPLY_TIMEOUT,
            self.shared
                .wait_for(|inner| inner.state.clone().filter(&mut predicate)),
        )
        .await
        .map_err(|_| Error::Timeout("the awaited station state"))
    }

    /// Current state frame and reconnect generation, captured under one lock.
    #[must_use]
    pub fn observation_cursor(&self) -> (u64, u64) {
        self.shared.observation_cursor()
    }

    /// Wait until a state frame arrives or the connection drops.
    pub async fn wait_for_observation(&self, cursor: (u64, u64)) {
        self.shared.wait_for_observation(cursor).await;
    }

    /// Wait for the next sequence snapshot after a caller's last revision.
    pub async fn next_sequence(&self, revision: u64) -> (u64, Option<ActionSequenceRecord>) {
        self.shared
            .wait_for(|inner| {
                (inner.action_sequence_revision > revision).then(|| {
                    (
                        inner.action_sequence_revision,
                        inner.action_sequence.clone(),
                    )
                })
            })
            .await
    }

    /// Who holds the lease now, if anyone.
    ///
    /// # Errors
    /// As [`Station::state`].
    pub async fn lease_status(&self) -> Result<Option<LeaseStatus>> {
        Ok(self.state().await?.lease)
    }

    /// Flips to true when this connection's lease ended: taken, revoked,
    /// returned for an interruption, lapsed or disconnected.
    #[must_use]
    pub fn lease_lost(&self) -> watch::Receiver<bool> {
        self.shared.subscribe_lease_lost()
    }

    /// The current or most recent action sequence, as the station pushes it. A slow reader
    /// sees the newest value, not every intermediate one.
    #[must_use]
    pub fn action_sequence_updates(&self) -> watch::Receiver<Option<ActionSequenceRecord>> {
        self.shared.subscribe_action_sequence()
    }

    /// Live run telemetry from now on; use [`Station::connect_with`] to resume.
    #[must_use]
    pub fn run_updates(&self) -> broadcast::Receiver<RunUpdate> {
        self.shared.subscribe_run()
    }

    /// Acquire the lease on every arm for this connection. The station retains
    /// it while the connection is alive, so holding the guard is enough;
    /// await [`LeaseGuard::return_lease`] to return it; dropping the guard only
    /// attempts a background release. The arms keep doing what they were told.
    ///
    /// # Errors
    /// [`Error::LeaseHeldElsewhere`] when someone else holds the lease.
    pub async fn acquire_lease(&self, request: LeaseRequest) -> Result<LeaseGuard> {
        self.request_lease(request, false).await
    }

    /// Take the lease from whoever holds it, stopping every arm first.
    ///
    /// # Errors
    /// Connection failures, or the arms could not be stopped.
    pub async fn take_lease(&self, request: LeaseRequest) -> Result<LeaseGuard> {
        self.request_lease(request, true).await
    }

    async fn request_lease(&self, request: LeaseRequest, take: bool) -> Result<LeaseGuard> {
        let Some(acquisition_generation) = self.shared.reserve_lease_acquisition() else {
            return Err(Error::Connection(
                "this client already holds or is acquiring a lease, or is disconnected".into(),
            ));
        };
        let mut acquisition = LeaseAcquisition {
            pending: Some(&self.shared),
        };
        let submission = hebbian_contracts::LeaseAcquireRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            station_instance_id: self.get_station_instance().await?.id,
            request: hebbian_contracts::LeaseRequest {
                holder: request.holder,
                label: request.label,
                required_cameras: request
                    .required_cameras
                    .into_iter()
                    .map(hebbian_contracts::SnapshotName::try_from)
                    .collect::<std::result::Result<_, _>>()?,
                on_interruption: request.on_interruption,
            },
            take,
        };
        let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
        let grant = loop {
            match self.acquire_lease_http(&submission).await {
                Ok(grant) => break grant,
                Err(Error::Http { status: 409, body })
                    if body.starts_with("The lease is held by ") =>
                {
                    let status = self.get_state().await.ok().and_then(|state| state.lease);
                    return Err(Error::LeaseHeldElsewhere(status.map(Box::new)));
                }
                Err(Error::Connection(_)) if tokio::time::Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                Err(error) => return Err(error),
            }
        };
        let token = LeaseToken {
            lease_token: grant.lease_token.clone(),
        };
        self.renew_lease_http(&token).await?;
        if !self
            .shared
            .install_lease(acquisition_generation, grant.lease_token.clone())
        {
            let _ = self.release_lease_http(&token).await;
            return Err(Error::Connection(
                "the station connection was lost while acquiring the lease".into(),
            ));
        }
        acquisition.pending = None;
        let renewal = self.start_lease_renewal(grant.lease_token.clone(), grant.lease_seconds);
        Ok(LeaseGuard {
            station: self.clone(),
            lease_token: grant.lease_token,
            lease_seconds: grant.lease_seconds,
            status: grant.status,
            renewal,
        })
    }

    fn start_lease_renewal(
        &self,
        token: String,
        lease_seconds: f64,
    ) -> tokio::task::JoinHandle<()> {
        let station = self.clone();
        tokio::spawn(async move {
            let interval = Duration::from_secs_f64((lease_seconds / 3.0).max(0.1));
            loop {
                tokio::time::sleep(interval).await;
                let Some(observable) = station.shared.observes_lease(&token) else {
                    break;
                };
                let renewed = if observable {
                    matches!(
                        tokio::time::timeout(
                            interval,
                            station.renew_lease_http(&LeaseToken {
                                lease_token: token.clone()
                            })
                        )
                        .await,
                        Ok(Ok(()))
                    )
                } else {
                    false
                };
                if !renewed {
                    station.shared.lose_lease_if_held_by(&token);
                    break;
                }
            }
        })
    }

    /// The operator's override: end whoever holds the lease. Returns once the
    /// station reports that no one holds it, so an acquire right after succeeds.
    ///
    /// # Errors
    /// Connection failures, or [`Error::Timeout`] when the return is not seen.
    pub async fn revoke_lease(&self) -> Result<()> {
        let guard = self
            .take_lease(LeaseRequest::new(LeaseHolder::Agent, "operator override"))
            .await?;
        // CONSTRAINT: The cached frame can predate the take and show no holder, so only a frame
        // received after the take's reply counts.
        let (after_take, _) = self.shared.observation_cursor();
        drop(guard);
        tokio::time::timeout(
            REPLY_TIMEOUT,
            self.shared.wait_for(|inner| {
                (inner.state_count > after_take
                    && inner
                        .state
                        .as_ref()
                        .is_some_and(|state| state.lease.is_none()))
                .then_some(())
            }),
        )
        .await
        .map_err(|_| Error::Timeout("the lease return"))
    }

    async fn run_single_submission(
        &self,
        arm: &str,
        command: StationCommand,
        options: CommandOptions,
        expected_lease: Option<&str>,
    ) -> Result<Vec<ArmRecord>> {
        let lease_token = self.shared.lease_token();
        if expected_lease.is_some_and(|expected| lease_token.as_deref() != Some(expected)) {
            return Err(Error::Refused {
                reason: RejectionReason::LeaseHeldElsewhere,
                detail: "The lease is no longer held".into(),
            });
        }
        if lease_token.is_none()
            && !matches!(
                command,
                StationCommand::Enable
                    | StationCommand::Disable
                    | StationCommand::Stop
                    | StationCommand::ClearFault
                    | StationCommand::ApproveRecovery
            )
        {
            return Err(Error::Refused {
                reason: RejectionReason::LeaseHeldElsewhere,
                detail: "The lease is no longer held".into(),
            });
        }
        let submission = CommandSubmissionRequest {
            operation_id: options
                .operation_id
                .unwrap_or_else(uuid::Uuid::new_v4)
                .to_string(),
            station_instance_id: self.command_completion_station().await?.id,
            arm: arm.to_owned(),
            command,
            lease_token,
            expected_revision: options.expected_revisions.get(arm).copied(),
            trace_context: options.trace_context,
        };
        self.submit_and_finish_command(
            &submission.operation_id,
            &submission.station_instance_id,
            submission
                .lease_token
                .as_ref()
                .map(|token| OperationCancellation {
                    id: submission.operation_id.clone(),
                    authorization: CommandCancellationRequest {
                        station_instance_id: submission.station_instance_id.clone(),
                        lease_token: token.clone(),
                    },
                }),
            || self.submit_command_http(&submission),
        )
        .await
    }

    async fn command_completion_station(&self) -> Result<StationInstance> {
        let instance = self.get_station_instance().await?;
        if instance.supports_operation_completion == Some(true) {
            Ok(instance)
        } else {
            Err(Error::Connection(
                "Update the station to use command completion".to_owned(),
            ))
        }
    }

    async fn submit_and_finish_command<
        SubmissionFuture: Future<Output = Result<CommandAnswer>> + Send,
    >(
        &self,
        operation_id: &str,
        station_instance_id: &str,
        cancellation: Option<OperationCancellation>,
        submit: impl Fn() -> SubmissionFuture + Send,
    ) -> Result<Vec<ArmRecord>> {
        let mut pending = PendingHttpCommand::new(self, cancellation);
        match retry_operation_request(submit).await {
            Ok(CommandAnswer::Accepted { .. }) => {}
            Ok(CommandAnswer::Refused { reason, detail }) => {
                pending.disarm();
                return Err(Error::Refused { reason, detail });
            }
            Err(error) => {
                if matches!(error, Error::Http { .. }) {
                    pending.disarm();
                }
                return Err(error);
            }
        }

        let instance = StationInstance {
            id: station_instance_id.to_owned(),
            supports_operation_completion: None,
        };
        let completion = loop {
            if let Some(completion) =
                retry_operation_request(|| self.wait_operation_http(operation_id, &instance))
                    .await?
            {
                break completion;
            }
        };
        pending.disarm();
        let sequence = completion.state_sequence.parse().map_err(|_| {
            Error::Connection("Invalid station completion event sequence".to_owned())
        })?;
        tokio::time::timeout(
            REPLY_TIMEOUT,
            self.shared
                .wait_for_state_event(station_instance_id, sequence),
        )
        .await
        .map_err(|_| Error::Timeout("the station state after command completion"))?;
        Ok(completion.records)
    }

    fn accepted_records(&self, answer: CommandAnswer) -> Result<Vec<ArmRecord>> {
        match answer {
            CommandAnswer::Refused { reason, detail } => Err(Error::Refused { reason, detail }),
            CommandAnswer::Accepted { records } => {
                self.shared.update(|inner| {
                    for arm_record in &records {
                        inner.keep_record(arm_record.clone());
                    }
                });
                Ok(records)
            }
        }
    }

    /// [`Station::run`] with a trace context and expected revisions.
    ///
    /// # Errors
    /// As [`Station::run`].
    pub(crate) async fn run_with(
        &self,
        arm: &str,
        command: StationCommand,
        options: CommandOptions,
    ) -> Result<CommandRecord> {
        self.run_with_lease(arm, command, options, None).await
    }

    async fn run_with_lease(
        &self,
        arm: &str,
        command: StationCommand,
        options: CommandOptions,
        expected_lease: Option<&str>,
    ) -> Result<CommandRecord> {
        let [finished]: [ArmRecord; 1] = self
            .run_single_submission(arm, command, options, expected_lease)
            .await?
            .try_into()
            .map_err(|_| Error::Connection("expected one finished record".into()))?;
        completed(finished.record)
    }

    /// Run one of the station's power or recovery commands.
    ///
    /// # Errors
    /// Motion requires a lease. Other failures are reported by the station.
    pub async fn run_station_command(
        &self,
        arm: &str,
        command: StationCommand,
        options: CommandOptions,
    ) -> Result<CommandRecord> {
        match command {
            StationCommand::Enable
            | StationCommand::Disable
            | StationCommand::Stop
            | StationCommand::ClearFault
            | StationCommand::ApproveRecovery => self.run_with(arm, command, options).await,
            _ => Err(Error::Refused {
                reason: RejectionReason::LeaseHeldElsewhere,
                detail: "Motion requires a lease guard".into(),
            }),
        }
    }

    /// Wait for a command's record to finish, across reconnects.
    ///
    /// # Errors
    /// [`Error::Connection`] when the station stays unreachable for 30 s.
    pub async fn wait_for_outcome(&self, command_id: &str) -> Result<CommandRecord> {
        let finished = self.shared.wait_for(|inner| {
            inner
                .record(command_id)
                .filter(|arm_record| !arm_record.record.is_pending())
                .cloned()
        });
        let ArmRecord {
            arm: command_arm,
            record,
        } = tokio::select! {
            record = finished => record,
            () = self.unreachable_too_long() => {
                return Err(Error::Connection(format!(
                    "{} has been unreachable for too long",
                    self.shared.url
                )));
            }
        };
        // CONSTRAINT: The server shows a command as its arm's last command only once it finished and
        // its resting sample was handled, so a state read afterwards shows where the arm
        // came to rest. Commands that finish in one cycle show only the latest, so the
        // wait ends at that arm's last command that finished at or after this one. A lost
        // connection ends it too.
        let finished_unix_nano = record.finished_unix_nano();
        self.shared
            .wait_for(|inner| {
                let Some(state) = &inner.state else {
                    return (!inner.is_connected()).then_some(());
                };
                let arm = state.arms.iter().find(|arm| arm.arm == command_arm)?;
                if arm
                    .active_command
                    .as_ref()
                    .is_some_and(|active| active.command_id.as_str() == command_id)
                {
                    return None;
                }
                arm.last_command
                    .as_ref()
                    .is_some_and(|last| {
                        last.command_id.as_str() == command_id
                            || last.finished_unix_nano() >= finished_unix_nano
                    })
                    .then_some(())
            })
            .await;
        Ok(record)
    }

    async fn unreachable_too_long(&self) {
        self.shared
            .wait_for_continuous_disconnect(DISCONNECT_PATIENCE)
            .await;
    }

    /// Where an arm would take the tool toward a pose; nothing moves.
    ///
    /// # Errors
    /// [`Error::Refused`] when no joints reach the pose.
    pub async fn preview_pose(&self, arm: &str, pose: PreviewPose) -> Result<PoseProjection> {
        let results = self
            .preview_pose_candidates(arm, vec![(None, pose)])
            .await?;
        let [result] = <[_; 1]>::try_from(results)
            .map_err(|_| Error::Connection("Unexpected preview result count".into()))?;
        match result.answer {
            PreviewAnswer::Projected { projection } => Ok(*projection),
            PreviewAnswer::Refused { reason, detail } => Err(Error::Refused { reason, detail }),
        }
    }

    /// Where an arm would take the tool toward each of several independent poses, in input
    /// order; nothing moves. Each answer is a projection or a refusal, never an error for the
    /// whole batch. Candidate ids are generated here.
    ///
    /// # Errors
    /// Connection or HTTP failures; an unknown arm.
    pub async fn preview_pose_candidates(
        &self,
        arm: &str,
        candidates: Vec<(Option<String>, PreviewPose)>,
    ) -> Result<Vec<hebbian_contracts::PosePreviewCandidateResult>> {
        let request = PosePreviewBatchRequest {
            arm: arm.to_owned(),
            poses: candidates
                .into_iter()
                .map(|(label, pose)| PosePreviewCandidate {
                    id: uuid::Uuid::new_v4().to_string(),
                    label,
                    pose,
                })
                .collect(),
        };
        let response = self.preview_poses(&request).await?;
        if response.results.len() != request.poses.len() {
            return Err(Error::Connection("Unexpected preview result count".into()));
        }
        let mut results_by_id: BTreeMap<_, _> = response
            .results
            .into_iter()
            .map(|candidate| (candidate.id.clone(), candidate))
            .collect();
        request
            .poses
            .iter()
            .map(|candidate| {
                results_by_id
                    .remove(&candidate.id)
                    .ok_or_else(|| Error::Connection("missing preview candidate result".into()))
            })
            .collect()
    }

    pub(crate) async fn arm_state(&self, arm: &str) -> Result<hebbian_contracts::ArmState> {
        self.state()
            .await?
            .arms
            .into_iter()
            .find(|candidate| candidate.arm == arm)
            .ok_or_else(|| Error::Refused {
                reason: RejectionReason::InvalidTarget,
                detail: format!("The station has no arm {arm}"),
            })
    }

    async fn current_orientation(&self, arm: &str) -> Result<PoseOrientation> {
        let tool = self.arm_state(arm).await?.tool;
        tool.map(|tool| PoseOrientation::Quaternion {
            quaternion: tool.quaternion,
        })
        .ok_or_else(|| Error::Refused {
            reason: RejectionReason::NotEnabled,
            detail: format!("{arm} has no pose yet; is it enabled?"),
        })
    }

    /// Wait for the named execution to finish through the station's completion route.
    /// The event stream is independent of this wait.
    ///
    /// # Errors
    /// [`Error::Http`] when the station no longer remembers it.
    pub async fn wait_for_action_sequence(
        &self,
        action_sequence_id: &str,
    ) -> Result<ActionSequenceRecord> {
        loop {
            if let Some(record) =
                retry_operation_request(|| self.wait_action_sequence_completion(action_sequence_id))
                    .await?
            {
                return Ok(record);
            }
        }
    }

    /// Ask the station to cancel an accepted action sequence; waits for a reconnect like
    /// any request. Follow with [`Station::wait_for_action_sequence`] to see it end.
    ///
    /// # Errors
    /// [`Error::Connection`] when the station stays unreachable.
    pub async fn cancel_action_sequence(&self, action_sequence_id: &str) -> Result<()> {
        match self.cancel_action_sequence_http(action_sequence_id).await {
            Err(Error::Http { status: 404, .. }) => Ok(()),
            result => result,
        }
    }

    /// Report a model-loop event to the viewer's live run.
    ///
    /// # Errors
    /// [`Error::Connection`] when not connected; callers usually ignore it.
    pub async fn record_run_event(&self, event: RunEvent) -> Result<()> {
        self.record_run_event_http(&event).await
    }

    /// Report trace spans: running ones as they start, then finished.
    ///
    /// # Errors
    /// As [`Station::record_run_event`].
    pub async fn record_run_spans(&self, spans: Vec<TraceSpan>) -> Result<()> {
        self.record_run_spans_http(&hebbian_contracts::RunSpansRequest { spans })
            .await
    }
}

/// The lease on the arms, held by one connection. Dropping it attempts to return
/// the lease in the background; use [`Self::return_lease`] to await its release.
pub struct LeaseGuard {
    pub(crate) station: Station,
    lease_token: String,
    lease_seconds: f64,
    status: LeaseStatus,
    renewal: tokio::task::JoinHandle<()>,
}

impl LeaseGuard {
    ///
    /// # Errors
    /// The lease is stale, the station refused the command, or the command did not complete.
    pub async fn run_with_lease(
        &self,
        arm: &str,
        command: StationCommand,
        options: CommandOptions,
    ) -> Result<CommandRecord> {
        self.station
            .run_with_lease(arm, command, options, Some(&self.lease_token))
            .await
    }
    /// Move several arms together, planned before any moves; one refusal refuses
    /// all, and if one arm does not complete the others stop. Dropping the
    /// future requests cancellation of the submitted group in the background.
    ///
    /// # Errors
    /// As [`Station::run_with`]; the first arm that did not complete is reported.
    pub async fn run_arms(&self, command: MoveArmsCommand) -> Result<Vec<ArmRecord>> {
        self.run_arms_with(command, CommandOptions::default()).await
    }

    /// [`Station::run_arms`] with a trace context and expected revisions.
    ///
    /// # Errors
    /// As [`Station::run_arms`].
    pub async fn run_arms_with(
        &self,
        command: MoveArmsCommand,
        options: CommandOptions,
    ) -> Result<Vec<ArmRecord>> {
        let finished = self.run_arms_records(command, options).await?;
        for arm_record in &finished {
            completed(arm_record.record.clone())?;
        }
        Ok(finished)
    }

    ///
    /// # Errors
    /// The station refused the group or the connection failed before all outcomes arrived.
    pub async fn run_arms_records(
        &self,
        command: MoveArmsCommand,
        options: CommandOptions,
    ) -> Result<Vec<ArmRecord>> {
        let submission = GroupCommandSubmissionRequest {
            operation_id: options
                .operation_id
                .unwrap_or_else(uuid::Uuid::new_v4)
                .to_string(),
            station_instance_id: self.station.command_completion_station().await?.id,
            command,
            lease_token: Some(self.lease_token.clone()),
            trace_context: options.trace_context,
            expected_revisions: options.expected_revisions,
        };
        self.station
            .submit_and_finish_command(
                &submission.operation_id,
                &submission.station_instance_id,
                Some(OperationCancellation {
                    id: submission.operation_id.clone(),
                    authorization: CommandCancellationRequest {
                        station_instance_id: submission.station_instance_id.clone(),
                        lease_token: self.lease_token.clone(),
                    },
                }),
                || self.station.submit_group_command_http(&submission),
            )
            .await
    }

    /// As [`Station::move_to_pose`], but `None` keeps the tool's current orientation.
    /// # Errors
    /// As [`Station::run_with`].
    pub async fn move_to_pose_keeping_orientation(
        &self,
        arm: &str,
        position: [f64; 3],
        orientation: Option<PoseOrientation>,
        max_joint_speed: Option<hebbian_contracts::JointSpeed>,
    ) -> Result<CommandRecord> {
        self.move_to_pose_keeping_orientation_with(
            arm,
            position,
            orientation,
            max_joint_speed,
            CommandOptions::default(),
        )
        .await
    }

    /// As [`Self::move_to_pose_keeping_orientation`], with the command's options.
    /// # Errors
    /// As [`Station::run_with`].
    pub async fn move_to_pose_keeping_orientation_with(
        &self,
        arm: &str,
        position: [f64; 3],
        orientation: Option<PoseOrientation>,
        max_joint_speed: Option<hebbian_contracts::JointSpeed>,
        options: CommandOptions,
    ) -> Result<CommandRecord> {
        let orientation = match orientation {
            Some(orientation) => orientation,
            None => self.station.current_orientation(arm).await?,
        };
        self.move_to_pose_with(arm, position, orientation, max_joint_speed, options)
            .await
    }

    /// Start an action sequence and wait until it ends; inspect `phase` for how.
    ///
    /// # Errors
    /// As [`Station::start_action_sequence`] and [`Station::wait_for_action_sequence`].
    pub async fn run_action_sequence(
        &self,
        action_sequence: ActionSequenceRequest,
    ) -> Result<ActionSequenceRecord> {
        let id = self.start_action_sequence(action_sequence).await?;
        self.station.wait_for_action_sequence(&id).await
    }

    /// Submit an ordered action sequence under this connection's lease and
    /// return its station-assigned identifier.
    ///
    /// # Errors
    /// [`Error::Refused`] when the station refuses the request.
    pub async fn start_action_sequence(
        &self,
        action_sequence: ActionSequenceRequest,
    ) -> Result<String> {
        self.start_action_sequence_with_cameras(action_sequence, Vec::new())
            .await
    }

    /// Start an action sequence with best-effort station camera capture.
    /// # Errors
    /// The station refuses the request or the connection fails.
    pub async fn start_action_sequence_with_cameras(
        &self,
        action_sequence: ActionSequenceRequest,
        capture_cameras: Vec<hebbian_contracts::SnapshotName>,
    ) -> Result<String> {
        let submission = ActionSequenceStartRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            station_instance_id: self.station.get_station_instance().await?.id,
            lease_token: self.lease_token.clone(),
            action_sequence,
            capture_cameras,
        };
        retry_operation_request(|| self.station.start_action_sequence_http(&submission)).await
    }

    /// Submit a retained compilation with fresh authority and live admission checks.
    ///
    /// # Errors
    /// The compilation expired, became incompatible, or the lease is no longer current.
    pub async fn start_compiled_sequence(
        &self,
        compilation_id: &str,
        expected_revisions: BTreeMap<String, u64>,
    ) -> Result<String> {
        self.submit_compiled_sequence(compilation_id, expected_revisions, None)
            .await
    }

    /// Continue from an explicitly selected unattempted suffix of a finished execution.
    ///
    /// # Errors
    /// The prefix changed, an attempted step would repeat, or normal submission checks fail.
    pub async fn continue_compiled_sequence(
        &self,
        compilation_id: &str,
        expected_revisions: BTreeMap<String, u64>,
        continuation: hebbian_contracts::ProgramContinuation,
    ) -> Result<String> {
        self.submit_compiled_sequence(compilation_id, expected_revisions, Some(continuation))
            .await
    }

    ///
    /// # Errors
    /// The compilation or lease is no longer valid, or the station refused admission.
    pub async fn submit_compiled_sequence(
        &self,
        compilation_id: &str,
        expected_revisions: BTreeMap<String, u64>,
        continuation: Option<hebbian_contracts::ProgramContinuation>,
    ) -> Result<String> {
        let submission = RunCompiledProgramRequest {
            request_id: uuid::Uuid::new_v4().to_string(),
            station_instance_id: self.station.get_station_instance().await?.id,
            compilation_id: compilation_id.to_owned(),
            continuation,
            lease_token: self.lease_token.clone(),
            expected_revisions,
        };
        retry_operation_request(|| self.station.run_compiled_sequence_http(&submission)).await
    }

    /// Execute a retained compilation and wait for its actual outcome.
    ///
    /// # Errors
    /// Submission fails, or the station cannot report the execution outcome.
    pub async fn run_compiled_sequence(
        &self,
        compilation_id: &str,
        expected_revisions: BTreeMap<String, u64>,
    ) -> Result<ActionSequenceRecord> {
        let execution_id = self
            .start_compiled_sequence(compilation_id, expected_revisions)
            .await?;
        self.station.wait_for_action_sequence(&execution_id).await
    }

    /// Submit a command with a caller-selected identity and return its admission answer.
    ///
    /// # Errors
    /// Returns an error if the station cannot be reached or refuses the command.
    pub async fn send_operation(
        &self,
        arm: &str,
        operation_id: uuid::Uuid,
        command: StationCommand,
        expected_revision: Option<u64>,
    ) -> Result<Vec<ArmRecord>> {
        let submission = CommandSubmissionRequest {
            operation_id: operation_id.to_string(),
            station_instance_id: self.station.get_station_instance().await?.id,
            arm: arm.to_owned(),
            command,
            lease_token: Some(self.lease_token.clone()),
            expected_revision,
            trace_context: None,
        };
        let mut pending = PendingHttpCommand::new(
            self.station(),
            Some(OperationCancellation {
                id: submission.operation_id.clone(),
                authorization: CommandCancellationRequest {
                    station_instance_id: submission.station_instance_id.clone(),
                    lease_token: self.lease_token.clone(),
                },
            }),
        );
        let answer =
            retry_operation_request(|| self.station.submit_command_http(&submission)).await?;
        pending.disarm();
        self.station.accepted_records(answer)
    }

    /// Update a stream with a strictly increasing sequence within the operation.
    ///
    /// # Errors
    /// Returns an error if the station cannot be reached or refuses the goal.
    pub async fn update_stream_goal(
        &self,
        operation_id: uuid::Uuid,
        sequence: u32,
        joints: Vec<f64>,
        gripper_opening: Option<hebbian_contracts::GripperOpening>,
        expected_revision: Option<u64>,
    ) -> Result<Vec<ArmRecord>> {
        let answer = self
            .station
            .update_stream_goal_http(
                &operation_id.to_string(),
                &hebbian_contracts::StreamGoalUpdateRequest {
                    station_instance_id: self.station.get_station_instance().await?.id,
                    lease_token: self.lease_token.clone(),
                    sequence,
                    joints,
                    gripper_opening,
                    expected_revision,
                },
            )
            .await?;
        self.station.accepted_records(answer)
    }

    /// Cancel the exact operation, including while admission is pending.
    ///
    /// # Errors
    /// Returns an error if the lease or the operation is no longer recognized.
    pub async fn cancel_operation(&self, operation_id: uuid::Uuid) -> Result<()> {
        self.station
            .cancel_operation_http(
                &operation_id.to_string(),
                &CommandCancellationRequest {
                    station_instance_id: self.station.get_station_instance().await?.id,
                    lease_token: self.lease_token.clone(),
                },
            )
            .await
    }

    /// The station this guard's lease lives on; commands sent through it run
    /// under the lease.
    #[must_use]
    pub fn station(&self) -> &Station {
        &self.station
    }

    #[must_use]
    pub fn lease_seconds(&self) -> f64 {
        self.lease_seconds
    }

    #[must_use]
    pub fn status(&self) -> &LeaseStatus {
        &self.status
    }

    /// Resolves when the lease was taken, revoked, returned for an
    /// interruption, lapsed, or the connection dropped.
    pub async fn lost(&self) {
        self.station
            .shared
            .wait_for_lease_loss(&self.lease_token)
            .await;
    }

    /// Report whether a camera the lease requires gives frames; a fresh frame
    /// clears its interlock.
    ///
    /// # Errors
    /// [`Error::Refused`] as `lease_held_elsewhere` once this lease is not
    /// current, or [`Error::Connection`].
    pub async fn report_camera(&self, camera: &str, available: bool) -> Result<()> {
        self.station
            .report_lease_camera_http(&LeaseCameraReport {
                lease_token: self.lease_token.clone(),
                camera: hebbian_contracts::SnapshotName::try_from(camera.to_owned())?,
                available,
            })
            .await
            .map_err(lease_checked_http_error)
    }

    /// Stop an arm only while this lease is current; refused as
    /// `lease_held_elsewhere` once it is not.
    ///
    /// # Errors
    /// As [`LeaseGuard::report_camera`].
    pub async fn stop_arm(&self, arm: &str) -> Result<()> {
        self.station
            .stop_leased_arm_http(
                arm,
                &LeaseToken {
                    lease_token: self.lease_token.clone(),
                },
            )
            .await
            .map_err(lease_checked_http_error)
    }

    /// Stop the recording only while this lease is current.
    ///
    /// # Errors
    /// As [`LeaseGuard::report_camera`].
    pub async fn stop_recording(&self) -> Result<()> {
        self.station
            .stop_recording_for_lease(&LeaseToken {
                lease_token: self.lease_token.clone(),
            })
            .await
    }

    /// Give the lease up and wait for the station to acknowledge it.
    /// The arms keep doing what they were told.
    ///
    /// # Errors
    /// Returns an error if the release request fails. The local lease is cleared
    /// and a single best-effort release retry runs in the background.
    pub async fn return_lease(&self) -> Result<()> {
        self.renewal.abort();
        let result = self
            .station
            .release_lease_http(&LeaseToken {
                lease_token: self.lease_token.clone(),
            })
            .await;
        self.clear_local_lease();
        match result {
            Ok(()) | Err(Error::Http { status: 409, .. }) => Ok(()),
            Err(error) => {
                self.release_in_background();
                Err(error)
            }
        }
    }

    fn release_in_background(&self) {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            let station = self.station.clone();
            let token = LeaseToken {
                lease_token: self.lease_token.clone(),
            };
            runtime.spawn(async move {
                let _ = station.release_lease_http(&token).await;
            });
        }
    }

    fn clear_local_lease(&self) -> bool {
        self.station
            .shared
            .release_lease_if_held_by(&self.lease_token)
    }
}

impl Drop for LeaseGuard {
    fn drop(&mut self) {
        self.renewal.abort();
        if self.clear_local_lease() {
            self.release_in_background();
        }
    }
}

async fn retry_operation_request<Reply, RequestFuture: Future<Output = Result<Reply>> + Send>(
    request: impl Fn() -> RequestFuture + Send,
) -> Result<Reply> {
    let retry_delay = Duration::from_millis(100);
    let deadline = tokio::time::Instant::now() + REPLY_TIMEOUT;
    loop {
        match tokio::time::timeout_at(deadline, request()).await {
            Ok(Err(Error::Connection(_)))
                if tokio::time::Instant::now() + retry_delay < deadline =>
            {
                tokio::time::sleep(retry_delay).await;
            }
            Ok(result) => return result,
            Err(_) => {
                return Err(Error::Connection(
                    "Station operation request timed out".to_owned(),
                ));
            }
        }
    }
}

struct OperationCancellation {
    id: String,
    authorization: CommandCancellationRequest,
}

/// Cancel a leased submission if the caller stops waiting for completion.
struct PendingHttpCommand {
    station: Station,
    cancellation: Option<OperationCancellation>,
}

impl PendingHttpCommand {
    fn new(station: &Station, cancellation: Option<OperationCancellation>) -> Self {
        Self {
            station: station.clone(),
            cancellation,
        }
    }

    fn disarm(&mut self) {
        self.cancellation = None;
    }
}

impl Drop for PendingHttpCommand {
    fn drop(&mut self) {
        if let Some(cancellation) = self.cancellation.take()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let station = self.station.clone();
            runtime.spawn(async move {
                let _ = station
                    .cancel_operation_http(&cancellation.id, &cancellation.authorization)
                    .await;
            });
        }
    }
}

/// Release the local acquisition reservation when its future ends.
struct LeaseAcquisition<'a> {
    pending: Option<&'a Shared>,
}

impl Drop for LeaseAcquisition<'_> {
    fn drop(&mut self) {
        if let Some(shared) = self.pending {
            shared.finish_lease_acquisition();
        }
    }
}

fn completed(record: CommandRecord) -> Result<CommandRecord> {
    match record.outcome() {
        Some(CommandOutcome::Completed) => Ok(record),
        Some(outcome) => Err(Error::Incomplete {
            outcome,
            detail: record.detail().map(str::to_owned),
            record: Box::new(record),
        }),
        None => Err(Error::Connection("the command never finished".to_owned())),
    }
}

fn lease_checked_http_error(error: Error) -> Error {
    match error {
        Error::Http { status: 403, body } => Error::Refused {
            reason: RejectionReason::LeaseHeldElsewhere,
            detail: body,
        },
        other => other,
    }
}
