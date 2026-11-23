// INVARIANT: All arms share one lease, including requests that bypass the station. See docs/control-model.md for admission and interlock behavior.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::arm::Rejection;
use crate::handoff::ArmHandle;
use crate::sync::LockUnpoisoned;
use hebbian_contracts::{
    Command, Interlock, LeaseGrant, LeaseHolder, LeaseStatus, OnInterruption, RejectionReason,
};

mod transition;

use transition::{
    ExpiryPolicy, LeaseEffect, LeasePhase, Settling, SettlingDecision, TakeoverMode, TakeoverPoll,
    TakeoverResult,
};

pub const LEASE: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub(crate) struct GenerationTicket {
    generation: Arc<AtomicU64>,
    expected: u64,
}

pub(crate) struct StaleGeneration;

impl GenerationTicket {
    pub(crate) fn capture(generation: &Arc<AtomicU64>) -> Self {
        Self {
            generation: Arc::clone(generation),
            expected: generation.load(Ordering::Acquire),
        }
    }

    pub(crate) fn fence(generation: &Arc<AtomicU64>) -> Self {
        Self {
            generation: Arc::clone(generation),
            expected: generation.fetch_add(1, Ordering::AcqRel) + 1,
        }
    }

    pub(crate) fn expected_for(&self, generation: &Arc<AtomicU64>) -> Option<u64> {
        Arc::ptr_eq(&self.generation, generation).then_some(self.expected)
    }

    fn check(&self) -> Result<(), StaleGeneration> {
        if self.generation.load(Ordering::Acquire) == self.expected {
            Ok(())
        } else {
            Err(StaleGeneration)
        }
    }
}

#[derive(Clone, Copy)]
pub enum AcquireMode {
    WaitOrFail,
    Takeover,
}

pub(crate) enum CommandAuthority {
    Interrupt,
    ApproveRecovery,
    Lease(LeaseAdmission),
}

impl CommandAuthority {
    pub(crate) fn for_command(command: &Command) -> Self {
        match command {
            Command::Stop | Command::Shutdown => Self::Interrupt,
            Command::ApproveRecovery => Self::ApproveRecovery,
            Command::Disable => Self::Lease(LeaseAdmission::Disable),
            Command::ClearFault => Self::Lease(LeaseAdmission::ClearFault),
            Command::Enable
            | Command::Move { .. }
            | Command::StreamJoints { .. }
            | Command::StreamPose { .. }
            | Command::ReachPose { .. }
            | Command::SetGripper { .. } => Self::Lease(LeaseAdmission::Normal),
        }
    }
}

pub use hebbian_contracts::LeaseRequest;

struct Held {
    token: String,
    request: LeaseRequest,
    unavailable_cameras: BTreeSet<String>,
    expires_at: Instant,
    phase: LeasePhase<Vec<PendingArmStop>>,
}

struct TakeoverReservation {
    token: String,
    controller_epoch: ControllerEpoch,
}

type StopAcknowledgement =
    Result<Receiver<Result<hebbian_contracts::CommandRecord, Rejection>>, Rejection>;

struct TakeoverStopPlan {
    arms: Vec<LeasedArm>,
    acknowledgements: Vec<StopAcknowledgement>,
}

struct PendingArmStop {
    arm: ArmHandle,
    readiness: Receiver<Result<bool, Rejection>>,
}

impl PendingArmStop {
    fn poll_stopped(&mut self) -> Result<bool, Rejection> {
        match self.readiness.try_recv() {
            Ok(Ok(true)) => Ok(true),
            Ok(Ok(false)) => {
                self.readiness = self.arm.control_handoff_readiness()?;
                Ok(false)
            }
            Ok(Err(rejection)) => Err(rejection),
            Err(TryRecvError::Empty) => Ok(false),
            Err(TryRecvError::Disconnected) => Err(crate::handoff::loop_not_running()),
        }
    }
}

fn poll_stops(pending: &mut Vec<PendingArmStop>) -> Settling {
    let mut failed = false;
    pending.retain_mut(|arm| {
        if let Ok(stopped) = arm.poll_stopped() {
            !stopped
        } else {
            failed = true;
            false
        }
    });
    if failed {
        Settling::Unavailable
    } else if pending.is_empty() {
        Settling::Confirmed
    } else {
        Settling::Pending
    }
}

impl Held {
    fn status(&self, now: Instant) -> LeaseStatus {
        LeaseStatus {
            holder: self.request.holder,
            label: self.request.label.clone(),
            required_cameras: self.request.required_cameras.clone(),
            on_interruption: self.request.on_interruption,
            expires_in_seconds: self.expires_at.saturating_duration_since(now).as_secs_f64(),
        }
    }

    fn is_held_by(&self, token: Option<&str>) -> bool {
        token.is_some_and(|token| same_token(&self.token, token))
    }

    fn active_at(&self, now: Instant) -> bool {
        matches!(self.phase, LeasePhase::Active) && self.expires_at > now
    }

    fn is_active_holder(&self, token: Option<&str>) -> bool {
        matches!(self.phase, LeasePhase::Active) && self.is_held_by(token)
    }
}

fn same_token(held: &str, presented: &str) -> bool {
    held.len() == presented.len()
        && held
            .bytes()
            .zip(presented.bytes())
            .fold(0_u8, |difference, (held, presented)| {
                difference | (held ^ presented)
            })
            == 0
}

fn new_token() -> std::io::Result<String> {
    let mut bytes = [0_u8; 24];
    std::fs::File::open("/dev/urandom").and_then(|mut random| random.read_exact(&mut bytes))?;
    Ok(bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    }))
}

#[derive(Debug, Default)]
pub struct ArmInterlocks {
    active: Vec<Interlock>,
    revision: u64,
}

pub type SharedInterlocks = Arc<Mutex<ArmInterlocks>>;

#[derive(Clone)]
pub(crate) struct ArmGate {
    pub interlocks: SharedInterlocks,
    pub expected_revision: Option<u64>,
}

pub struct Lease {
    duration: Duration,
    authority: Mutex<AuthorityState>,
    arms: Mutex<Vec<LeasedArm>>,
    authority_changes: tokio::sync::watch::Sender<()>,
}

struct AuthorityState {
    held: Option<Held>,
    controller: ControllerPresence,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ControllerEpoch(u64);

#[derive(Clone, Copy, PartialEq, Eq)]
enum ControllerPresence {
    Ready(ControllerEpoch),
    Lost(ControllerEpoch),
}

impl ControllerPresence {
    fn ready_epoch(self) -> Option<ControllerEpoch> {
        match self {
            Self::Ready(epoch) => Some(epoch),
            Self::Lost(_) => None,
        }
    }
}

#[derive(Clone)]
pub struct CameraRequirements {
    pub token: String,
    pub cameras: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CameraReportRefusal {
    LeaseLost,
    CameraNotRequired,
}

#[derive(Clone, Copy)]
pub(crate) enum LeaseAdmission {
    Normal,
    Disable,
    ClearFault,
}

impl LeaseAdmission {
    fn blocks_on_interlocks(self) -> bool {
        match self {
            Self::Normal => true,
            Self::Disable | Self::ClearFault => false,
        }
    }

    fn permits_interrupted_lease(self) -> bool {
        match self {
            Self::ClearFault => true,
            Self::Normal | Self::Disable => false,
        }
    }
}

#[derive(Clone)]
struct LeasedArm {
    arm: ArmHandle,
    generation: Arc<AtomicU64>,
    interlocks: SharedInterlocks,
}

#[derive(Debug)]
pub enum AcquireError {
    Held(LeaseStatus),
    Unavailable(std::io::Error),
    StopFailed {
        reason: RejectionReason,
        detail: String,
    },
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held(status) => write!(formatter, "The lease is held by '{}'.", status.label),
            Self::Unavailable(error) => write!(
                formatter,
                "Could not generate a secure lease token: {error}"
            ),
            Self::StopFailed { detail, .. } => {
                write!(formatter, "Could not stop to take the lease: {detail}")
            }
        }
    }
}

impl std::error::Error for AcquireError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable(error) => Some(error),
            Self::Held(_) | Self::StopFailed { .. } => None,
        }
    }
}

fn controller_unavailable() -> AcquireError {
    AcquireError::StopFailed {
        reason: RejectionReason::NotEnabled,
        detail: "The motor controller is unavailable".into(),
    }
}

const TAKEOVER_TIMEOUT: Duration = Duration::from_secs(20);
const TAKEOVER_POLL_INTERVAL: Duration = Duration::from_millis(10);

impl Lease {
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            authority: Mutex::new(AuthorityState {
                held: None,
                controller: ControllerPresence::Ready(ControllerEpoch(0)),
            }),
            arms: Mutex::new(Vec::new()),
            authority_changes: tokio::sync::watch::Sender::new(()),
        }
    }

    pub fn controller_lost(&self) {
        let mut authority = self.authority.lock_unpoisoned();
        if let ControllerPresence::Ready(epoch) = authority.controller {
            authority.controller = ControllerPresence::Lost(epoch);
        }
        authority.held = None;
        for arm in self.arms.lock_unpoisoned().iter() {
            arm.generation.fetch_add(1, Ordering::AcqRel);
        }
        self.authority_changes.send_replace(());
    }

    pub(crate) fn controller_ready(&self) {
        let mut authority = self.authority.lock_unpoisoned();
        if let ControllerPresence::Lost(epoch) = authority.controller {
            authority.controller = ControllerPresence::Ready(ControllerEpoch(epoch.0 + 1));
        }
    }

    pub(crate) fn register_arm(
        &self,
        arm: ArmHandle,
        generation: Arc<AtomicU64>,
        interlocks: SharedInterlocks,
    ) {
        self.arms.lock_unpoisoned().push(LeasedArm {
            arm,
            generation,
            interlocks,
        });
    }

    pub fn stop_leased_motion(&self, token: &str) -> bool {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        if !held
            .as_mut()
            .filter(|claim| claim.active_at(Instant::now()))
            .is_some_and(|claim| claim.is_active_holder(Some(token)))
        {
            return false;
        }
        self.stop_all_arms()
    }

    // INVARIANT: Stop a lease holder's sequence without invalidating the command that
    // requested the stop. Any earlier stop still invalidates that command.
    pub(crate) fn stop_leased_motion_for_submission(
        &self,
        token: &str,
        generations: &mut [GenerationTicket],
        cancel_sequence: impl FnOnce() -> bool,
    ) -> Result<(), Rejection> {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        if !held
            .as_mut()
            .filter(|claim| claim.active_at(Instant::now()))
            .is_some_and(|claim| claim.is_active_holder(Some(token)))
        {
            return Err(Rejection {
                reason: RejectionReason::LeaseHeldElsewhere,
                detail: "The lease is no longer held".into(),
            });
        }
        Self::check_controller_generation(authority.controller, generations)?;
        if !cancel_sequence() {
            return Ok(());
        }
        if !self.stop_all_arms() {
            return Err(crate::handoff::loop_not_running());
        }
        for ticket in generations {
            *ticket = GenerationTicket::capture(&ticket.generation);
        }
        Ok(())
    }

    fn stop_all_arms(&self) -> bool {
        let arms = self.arms.lock_unpoisoned().clone();
        let mut submitted = true;
        for arm in &arms {
            arm.generation.fetch_add(1, Ordering::AcqRel);
            submitted &= arm.arm.submit_unawaited(Command::Stop, None).is_ok();
        }
        submitted
    }

    pub fn with_lease(&self, token: &str, action: &mut dyn FnMut()) -> bool {
        self.with_captured_lease(Some(token), action)
    }

    pub(crate) fn authority_changes(&self) -> tokio::sync::watch::Receiver<()> {
        self.authority_changes.subscribe()
    }

    pub(crate) fn execution_authorized(&self, token: &str, gates: &[ArmGate]) -> bool {
        let mut revisions_match = false;
        self.with_lease(token, &mut || {
            revisions_match = gates.iter().all(|gate| {
                gate.expected_revision
                    .is_none_or(|revision| gate.interlocks.lock_unpoisoned().revision == revision)
            });
        }) && revisions_match
    }

    pub fn with_captured_lease(&self, token: Option<&str>, action: &mut dyn FnMut()) -> bool {
        let authority = self.authority.lock_unpoisoned();
        if authority.controller.ready_epoch().is_none() {
            return false;
        }
        let now = Instant::now();
        let authorized = match (&authority.held, token) {
            (None, None) => true,
            (Some(claim), Some(token)) => {
                claim.expires_at > now && claim.is_active_holder(Some(token))
            }
            _ => false,
        };
        if authorized {
            action();
        }
        authorized
    }

    // CONSTRAINT: Lease transfer waits for every arm to stop before granting control and must run outside asynchronous workers.
    pub fn acquire_stopping_arms(
        &self,
        request: LeaseRequest,
        mode: AcquireMode,
    ) -> Result<LeaseGrant, AcquireError> {
        if matches!(mode, AcquireMode::WaitOrFail) {
            match self.acquire(request.clone(), Instant::now()) {
                Ok(grant) => return Ok(grant),
                Err(AcquireError::Held(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let arms = self.arms.lock_unpoisoned().clone();
        let (reservation, stop_plan) = self.begin_takeover(
            request,
            match mode {
                AcquireMode::WaitOrFail => TakeoverMode::Recover,
                AcquireMode::Takeover => TakeoverMode::Replace,
            },
            Instant::now(),
            || {
                let acknowledgements = arms
                    .iter()
                    .map(|arm| {
                        arm.generation.fetch_add(1, Ordering::AcqRel);
                        arm.arm.submit_unawaited(Command::Stop, None)
                    })
                    .collect();
                TakeoverStopPlan {
                    arms,
                    acknowledgements,
                }
            },
        )?;
        let result = self.wait_for_takeover_stop(&reservation, stop_plan);
        let completion = if result.is_ok() {
            TakeoverResult::Settled
        } else {
            TakeoverResult::Failed
        };
        let controller_epoch = reservation.controller_epoch;
        let grant = self.finish_takeover(reservation, Instant::now(), completion);
        if self.authority.lock_unpoisoned().controller.ready_epoch() != Some(controller_epoch) {
            return Err(controller_unavailable());
        }
        match result {
            Ok(()) => grant.ok_or_else(|| AcquireError::StopFailed {
                reason: RejectionReason::ArmBusy,
                detail: "The lease transfer was interrupted".into(),
            }),
            Err(refusal) => Err(AcquireError::StopFailed {
                reason: refusal.reason,
                detail: refusal.detail,
            }),
        }
    }

    fn wait_for_takeover_stop(
        &self,
        reservation: &TakeoverReservation,
        plan: TakeoverStopPlan,
    ) -> Result<(), Rejection> {
        let deadline = Instant::now() + TAKEOVER_TIMEOUT;
        for acknowledgement in plan.acknowledgements {
            let answer = acknowledgement?;
            answer
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| crate::handoff::loop_not_running())??;
        }
        loop {
            let readiness = {
                let authority = self.authority.lock_unpoisoned();
                if authority.controller.ready_epoch() != Some(reservation.controller_epoch) {
                    return Err(crate::handoff::loop_not_running());
                }
                plan.arms
                    .iter()
                    .map(|arm| arm.arm.control_handoff_readiness())
                    .collect::<Result<Vec<_>, _>>()?
            };
            let mut all_ready = true;
            for answer in readiness {
                all_ready &= answer
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| crate::handoff::loop_not_running())??;
            }
            match transition::takeover_poll(all_ready, Instant::now(), deadline) {
                TakeoverPoll::Complete => return Ok(()),
                TakeoverPoll::Wait => std::thread::sleep(TAKEOVER_POLL_INTERVAL),
                TakeoverPoll::TimedOut => {
                    return Err(Rejection {
                        reason: RejectionReason::ArmBusy,
                        detail: "Timed out waiting for the arms to stop".into(),
                    });
                }
            }
        }
    }

    // SAFETY: Keep expired manual control fenced until every arm confirms it has stopped.
    pub(crate) fn tick(&self, now: Instant) {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        let Some(claim) = held.as_mut() else {
            return;
        };
        let decision = match claim
            .phase
            .on_time(claim.request.holder, claim.expires_at, now)
        {
            LeaseEffect::None => return,
            LeaseEffect::StopExpired(policy) => {
                let stops_submitted = self.stop_all_arms();
                if matches!(policy, ExpiryPolicy::ReleaseWhenSettled)
                    && stops_submitted
                    && let Ok(pending) = self.request_stop_feedback()
                {
                    claim.phase = LeasePhase::ExpiredManual(pending);
                }
                self.authority_changes.send_replace(());
                // WHY: Even a station with no registered arms has completed its stop.
                let LeasePhase::ExpiredManual(pending) = &mut claim.phase else {
                    return;
                };
                transition::expired_manual_stop(poll_stops(pending))
            }
            LeaseEffect::CheckSettling(pending) => {
                transition::expired_manual_stop(poll_stops(pending))
            }
        };
        match decision {
            SettlingDecision::KeepWaiting => {}
            SettlingDecision::Release => {
                *held = None;
                self.authority_changes.send_replace(());
            }
            SettlingDecision::Fence => {
                claim.phase = LeasePhase::Interrupted;
                self.authority_changes.send_replace(());
            }
        }
    }

    fn request_stop_feedback(&self) -> Result<Vec<PendingArmStop>, Rejection> {
        self.arms
            .lock_unpoisoned()
            .iter()
            .map(|arm| {
                Ok(PendingArmStop {
                    arm: arm.arm.clone(),
                    readiness: arm.arm.control_handoff_readiness()?,
                })
            })
            .collect()
    }

    #[must_use]
    pub fn status(&self, now: Instant) -> Option<LeaseStatus> {
        self.authority
            .lock_unpoisoned()
            .held
            .as_ref()
            .map(|held| held.status(now))
    }

    #[must_use]
    pub fn camera_requirements(&self) -> Option<CameraRequirements> {
        let now = Instant::now();
        self.authority
            .lock_unpoisoned()
            .held
            .as_ref()
            .filter(|claim| matches!(claim.phase, LeasePhase::Active) && claim.expires_at > now)
            .map(|claim| CameraRequirements {
                token: claim.token.clone(),
                cameras: claim.request.required_cameras.clone(),
            })
    }

    pub fn report_camera(
        &self,
        token: &str,
        camera: &str,
        available: bool,
    ) -> Result<(), CameraReportRefusal> {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        let Some(claim) = held
            .as_mut()
            .filter(|claim| claim.active_at(Instant::now()))
            .filter(|claim| claim.is_active_holder(Some(token)))
        else {
            return Err(CameraReportRefusal::LeaseLost);
        };
        if !claim
            .request
            .required_cameras
            .iter()
            .any(|name| name == camera)
        {
            return Err(CameraReportRefusal::CameraNotRequired);
        }
        if available {
            if claim.unavailable_cameras.remove(camera) {
                self.authority_changes.send_replace(());
            }
        } else if claim.unavailable_cameras.insert(camera.to_owned()) {
            for arm in self.arms.lock_unpoisoned().iter() {
                let mut interlocks = arm.interlocks.lock_unpoisoned();
                interlocks.revision = interlocks.revision.saturating_add(1);
            }
            self.stop_all_arms();
            self.authority_changes.send_replace(());
        }
        Ok(())
    }

    #[must_use]
    pub fn interlocks(&self, arm: &SharedInterlocks) -> (Vec<Interlock>, u64) {
        let cameras: Vec<Interlock> = {
            self.authority
                .lock_unpoisoned()
                .held
                .as_ref()
                .map(|claim| {
                    claim
                        .unavailable_cameras
                        .iter()
                        .map(|camera| Interlock::RequiredCameraUnavailable {
                            camera: camera.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let interlocks = arm.lock_unpoisoned();
        let mut active = interlocks.active.clone();
        active.extend(cameras);
        (active, interlocks.revision)
    }

    pub fn raise(&self, arm: &SharedInterlocks, interlock: Interlock) {
        let raised = {
            let mut interlocks = arm.lock_unpoisoned();
            let existing = interlocks.active.iter().position(|active| {
                std::mem::discriminant(active) == std::mem::discriminant(&interlock)
            });
            match existing {
                Some(index) if interlocks.active[index] == interlock => false,
                Some(index) => {
                    interlocks.active[index] = interlock.clone();
                    interlocks.revision = interlocks.revision.saturating_add(1);
                    true
                }
                None => {
                    interlocks.active.push(interlock.clone());
                    interlocks.revision = interlocks.revision.saturating_add(1);
                    true
                }
            }
        };
        if raised && matches!(interlock, Interlock::RecoveryApprovalRequired { .. }) {
            let mut authority = self.authority.lock_unpoisoned();
            let held = &mut authority.held;
            if held
                .as_mut()
                .filter(|claim| claim.active_at(Instant::now()))
                .is_some_and(|claim| {
                    matches!(claim.phase, LeasePhase::Active)
                        && claim.request.on_interruption == OnInterruption::ReturnLease
                })
            {
                *held = None;
            }
        }
        if raised {
            self.authority_changes.send_replace(());
        }
    }

    pub fn clear(&self, arm: &SharedInterlocks, matches: impl Fn(&Interlock) -> bool) {
        let mut interlocks = arm.lock_unpoisoned();
        let previous_count = interlocks.active.len();
        interlocks.active.retain(|interlock| !matches(interlock));
        if interlocks.active.len() != previous_count {
            self.authority_changes.send_replace(());
        }
    }

    fn acquire(&self, request: LeaseRequest, now: Instant) -> Result<LeaseGrant, AcquireError> {
        let mut authority = self.authority.lock_unpoisoned();
        authority
            .controller
            .ready_epoch()
            .ok_or_else(controller_unavailable)?;
        let held = &mut authority.held;
        if let Some(existing) = held.as_ref() {
            return Err(AcquireError::Held(existing.status(now)));
        }
        let claim = Held {
            token: new_token().map_err(AcquireError::Unavailable)?,
            request,
            unavailable_cameras: BTreeSet::new(),
            expires_at: now + self.duration,
            phase: LeasePhase::Active,
        };
        let grant = LeaseGrant {
            lease_token: claim.token.clone(),
            lease_seconds: self.duration.as_secs_f64(),
            status: claim.status(now),
        };
        *held = Some(claim);
        Ok(grant)
    }

    // INVARIANT: Reserve the lease and queue all stops while command admission is locked.
    // The reservation's token is never given to a client until the stop ends.
    fn begin_takeover<T>(
        &self,
        request: LeaseRequest,
        mode: TakeoverMode,
        now: Instant,
        enqueue_stops: impl FnOnce() -> T,
    ) -> Result<(TakeoverReservation, T), AcquireError> {
        let mut authority = self.authority.lock_unpoisoned();
        let controller_epoch = authority
            .controller
            .ready_epoch()
            .ok_or_else(controller_unavailable)?;
        let held = &mut authority.held;
        if let Some(existing) = held.as_ref()
            && !existing.phase.can_start_takeover(mode)
            && (existing.active_at(now) || matches!(existing.phase, LeasePhase::Transferring))
        {
            return Err(AcquireError::Held(existing.status(now)));
        }
        let reservation = Held {
            token: new_token().map_err(AcquireError::Unavailable)?,
            request,
            unavailable_cameras: BTreeSet::new(),
            expires_at: now + self.duration,
            phase: LeasePhase::Transferring,
        };
        let token = TakeoverReservation {
            token: reservation.token.clone(),
            controller_epoch,
        };
        *held = Some(reservation);
        self.authority_changes.send_replace(());
        Ok((token, enqueue_stops()))
    }

    // SAFETY: Complete the reservation only after every arm has stopped. A failed
    // transfer stays reserved, preventing unleased commands until a retry.
    fn finish_takeover(
        &self,
        token: TakeoverReservation,
        now: Instant,
        result: TakeoverResult,
    ) -> Option<LeaseGrant> {
        let mut authority = self.authority.lock_unpoisoned();
        if authority.controller.ready_epoch() != Some(token.controller_epoch) {
            return None;
        }
        let held = &mut authority.held;
        let reservation = held
            .as_mut()
            .filter(|reservation| reservation.is_held_by(Some(&token.token)))?;
        let completion = reservation.phase.finish_takeover(result)?;
        self.authority_changes.send_replace(());
        match completion {
            TakeoverResult::Settled => {}
            TakeoverResult::Failed => return None,
        }
        reservation.expires_at = now + self.duration;
        Some(LeaseGrant {
            lease_token: reservation.token.clone(),
            lease_seconds: self.duration.as_secs_f64(),
            status: reservation.status(now),
        })
    }

    pub fn retain(&self, token: Option<&str>, now: Instant) -> bool {
        let mut authority = self.authority.lock_unpoisoned();
        authority
            .held
            .as_mut()
            .filter(|claim| claim.expires_at > now && claim.is_active_holder(token))
            .map(|claim| claim.expires_at = now + self.duration)
            .is_some()
    }

    pub fn return_lease(&self, token: Option<&str>, now: Instant) {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        if held
            .as_mut()
            .filter(|claim| claim.active_at(now))
            .is_some_and(|claim| claim.is_active_holder(token))
        {
            *held = None;
            self.authority_changes.send_replace(());
        }
    }

    pub(crate) fn release_with(&self, token: &str, cancel_sequence: impl FnOnce() -> bool) -> bool {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        if !held
            .as_mut()
            .filter(|claim| claim.active_at(Instant::now()))
            .is_some_and(|claim| claim.is_active_holder(Some(token)))
        {
            return false;
        }
        if cancel_sequence() {
            self.stop_all_arms();
        }
        *held = None;
        self.authority_changes.send_replace(());
        true
    }

    // INVARIANT: Queue the holder's disconnect stops before allowing another holder.
    pub(crate) fn return_after(&self, token: &str, now: Instant, stop: impl FnOnce()) {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        if held
            .as_mut()
            .filter(|claim| claim.active_at(now))
            .is_some_and(|claim| claim.is_active_holder(Some(token)))
        {
            stop();
            *held = None;
            self.authority_changes.send_replace(());
        }
    }

    // INVARIANT: An incomplete handoff cannot be revoked: its stop must finish before control can become available.
    pub fn revoke(&self) -> bool {
        let mut authority = self.authority.lock_unpoisoned();
        let held = &mut authority.held;
        if held
            .as_ref()
            .is_none_or(|claim| claim.active_at(Instant::now()))
        {
            if held.take().is_some() {
                self.authority_changes.send_replace(());
            }
            true
        } else {
            false
        }
    }

    // INVARIANT: Invalidate prior submissions for this arm and queue its stop atomically.
    pub(crate) fn interrupt<T>(&self, generation: &AtomicU64, enqueue: impl FnOnce() -> T) -> T {
        let _held = self.authority.lock_unpoisoned();
        generation.fetch_add(1, Ordering::AcqRel);
        enqueue()
    }

    pub(crate) fn interrupt_if_current<T>(
        &self,
        arm_generation: &AtomicU64,
        generations: &[GenerationTicket],
        enqueue: impl FnOnce() -> T,
    ) -> Result<T, Rejection> {
        let authority = self.authority.lock_unpoisoned();
        Self::check_controller_generation(authority.controller, generations)?;
        arm_generation.fetch_add(1, Ordering::AcqRel);
        Ok(enqueue())
    }

    fn check_controller_generation(
        controller: ControllerPresence,
        generations: &[GenerationTicket],
    ) -> Result<(), Rejection> {
        if controller.ready_epoch().is_none() {
            return Err(crate::handoff::loop_not_running());
        }
        if generations.iter().any(|ticket| ticket.check().is_err()) {
            return Err(Rejection {
                reason: RejectionReason::ArmBusy,
                detail: "The command was superseded by a stop or controller interruption".into(),
            });
        }
        Ok(())
    }

    // SAFETY: Approving recovery is the operator's, whoever holds the lease.
    pub(crate) fn admit_recovery_approval<T>(
        &self,
        generations: &[GenerationTicket],
        enqueue: impl FnOnce() -> T,
    ) -> Result<T, Rejection> {
        let authority = self.authority.lock_unpoisoned();
        Self::check_controller_generation(authority.controller, generations)?;
        Ok(enqueue())
    }

    pub(crate) fn admit_generation<T>(
        &self,
        token: Option<&str>,
        now: Instant,
        gates: &[ArmGate],
        generations: &[GenerationTicket],
        admission: LeaseAdmission,
        act: impl FnOnce() -> T,
    ) -> Result<T, Rejection> {
        let authority = self.authority.lock_unpoisoned();
        if authority.controller.ready_epoch().is_none() {
            return Err(crate::handoff::loop_not_running());
        }
        let held = &authority.held;
        let (manual_holder, cameras_unavailable) = held.as_ref().map_or((false, false), |claim| {
            (
                claim.active_at(now)
                    && claim.is_held_by(token)
                    && claim.request.holder == LeaseHolder::Manual,
                !claim.unavailable_cameras.is_empty(),
            )
        });
        for gate in gates {
            let interlocks = gate.interlocks.lock_unpoisoned();
            if gate
                .expected_revision
                .is_some_and(|expected| expected != interlocks.revision)
            {
                return Err(Rejection {
                    reason: RejectionReason::StaleRevision,
                    detail: format!(
                        "An interlock changed since revision {}; the arm is at revision {}. \
                         Read the state again and retry.",
                        gate.expected_revision.unwrap_or_default(),
                        interlocks.revision
                    ),
                });
            }
            if admission.blocks_on_interlocks()
                && let Some(interlock) = interlocks.active.first()
            {
                return Err(Rejection {
                    reason: RejectionReason::Interlocked,
                    detail: interlock.to_string(),
                });
            }
        }
        if admission.blocks_on_interlocks() && cameras_unavailable && !manual_holder {
            return Err(Rejection {
                reason: RejectionReason::Interlocked,
                detail: "A camera the lease requires is unavailable. Motion resumes when it gives a fresh frame".to_owned(),
            });
        }
        if generations.iter().any(|ticket| ticket.check().is_err()) {
            return Err(Rejection {
                reason: RejectionReason::ArmBusy,
                detail: "The command was superseded by a stop".to_owned(),
            });
        }
        if let Some(claim) = held.as_ref() {
            if matches!(
                claim.phase,
                LeasePhase::Interrupted | LeasePhase::ExpiredManual(_)
            ) && admission.permits_interrupted_lease()
            {
                return Ok(act());
            }
            if !claim.active_at(now) {
                return Err(Rejection {
                    reason: RejectionReason::ArmBusy,
                    detail: "The lease transfer has not finished; retry taking the lease"
                        .to_owned(),
                });
            }
            if !claim.is_held_by(token) {
                return Err(Rejection {
                    reason: RejectionReason::LeaseHeldElsewhere,
                    detail: format!(
                        "The lease is held by {} '{}'. Stop is always accepted; \
                         otherwise wait for it to finish, or take the lease.",
                        match claim.request.holder {
                            LeaseHolder::Agent => "agent",
                            LeaseHolder::Manual => "manual",
                        },
                        claim.request.label
                    ),
                });
            }
        } else if token.is_some() {
            return Err(Rejection {
                reason: RejectionReason::LeaseHeldElsewhere,
                detail: "The lease has ended; acquire it again".to_owned(),
            });
        }
        Ok(act())
    }
}

// CONSTRAINT: Manual control needs no interlock but a required camera's on any arm.
pub(crate) fn manual_control_ready_without_waiting(
    links: &[crate::control_link::LinkHandle],
) -> Result<(), String> {
    for link in links {
        if let Some(interlock) = link.interlocks().0.into_iter().find(|interlock| {
            !matches!(
                interlock,
                hebbian_contracts::Interlock::RequiredCameraUnavailable { .. }
            )
        }) {
            return Err(format!(
                "Manual control is inhibited by an interlock: {interlock:?}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEASE_FOR_TESTS: Duration = Duration::from_secs(10);

    fn lease() -> (Lease, Instant) {
        (Lease::new(LEASE_FOR_TESTS), Instant::now())
    }

    fn request(holder: LeaseHolder, label: &str) -> LeaseRequest {
        LeaseRequest {
            holder,
            label: label.into(),
            required_cameras: vec!["front".into()],
            on_interruption: OnInterruption::WaitForApproval,
        }
    }

    fn token_of(acquired: Result<LeaseGrant, AcquireError>) -> String {
        match acquired {
            Ok(grant) => grant.lease_token,
            Err(holder) => panic!("refused, held by {holder:?}"),
        }
    }

    fn acquire_agent(lease: &Lease, now: Instant) -> String {
        token_of(lease.acquire(request(LeaseHolder::Agent, "touch the block"), now))
    }

    fn take(lease: &Lease, holder: LeaseHolder, label: &str, now: Instant) -> String {
        let (reservation, ()) = lease
            .begin_takeover(request(holder, label), TakeoverMode::Replace, now, || ())
            .unwrap();
        lease
            .finish_takeover(reservation, now, TakeoverResult::Settled)
            .unwrap()
            .lease_token
    }

    fn command(lease: &Lease, token: Option<&str>, now: Instant) -> Result<(), Rejection> {
        lease.admit_generation(token, now, &[], &[], LeaseAdmission::Normal, || ())
    }

    fn gated(
        lease: &Lease,
        token: Option<&str>,
        arm: &SharedInterlocks,
        expected_revision: Option<u64>,
    ) -> Result<(), Rejection> {
        lease.admit_generation(
            token,
            Instant::now(),
            &[ArmGate {
                interlocks: arm.clone(),
                expected_revision,
            }],
            &[],
            LeaseAdmission::Normal,
            || (),
        )
    }

    #[test]
    fn with_no_holder_anyone_may_command() {
        let (lease, now) = lease();
        assert_eq!(lease.status(now), None);
        assert!(command(&lease, None, now).is_ok());
        assert!(command(&lease, Some("a-stale-token"), now).is_err());
    }

    #[test]
    fn a_raised_interlock_refuses_motion_and_makes_older_revisions_stale() {
        let (lease, _) = lease();
        let arm = SharedInterlocks::default();
        assert!(gated(&lease, None, &arm, Some(0)).is_ok());
        lease.raise(&arm, Interlock::ArmDisconnected);
        lease.raise(&arm, Interlock::ArmDisconnected);
        assert_eq!(
            gated(&lease, None, &arm, None).unwrap_err().reason,
            RejectionReason::Interlocked
        );
        let waiter = lease.authority_changes();
        lease.clear(&arm, |interlock| *interlock == Interlock::ArmDisconnected);
        assert!(waiter.has_changed().unwrap());
        assert_eq!(lease.interlocks(&arm), (Vec::new(), 1));
        assert_eq!(
            gated(&lease, None, &arm, Some(0)).unwrap_err().reason,
            RejectionReason::StaleRevision
        );
        assert!(gated(&lease, None, &arm, Some(1)).is_ok());
    }

    #[test]
    fn return_lease_on_interruption_gives_the_lease_up_for_recovery() {
        let (lease, now) = lease();
        let arm = SharedInterlocks::default();
        let mut returning = request(LeaseHolder::Agent, "agent");
        returning.on_interruption = OnInterruption::ReturnLease;
        token_of(lease.acquire(returning, now));
        lease.raise(&arm, Interlock::ArmDisconnected);
        assert!(lease.status(Instant::now()).is_some());
        lease.raise(
            &arm,
            Interlock::RecoveryApprovalRequired {
                motors: vec!["2".into()],
            },
        );
        assert_eq!(lease.status(Instant::now()), None);
    }

    #[test]
    fn a_captured_unleased_command_cannot_stop_a_new_holder() {
        let (lease, now) = lease();
        let mut stopped = false;
        assert!(lease.with_captured_lease(None, &mut || stopped = true));
        assert!(stopped);
        stopped = false;
        let token = acquire_agent(&lease, now);
        assert!(!lease.with_captured_lease(None, &mut || stopped = true));
        assert!(!stopped);
        assert!(lease.with_captured_lease(Some(&token), &mut || stopped = true));
        assert!(stopped);
    }

    #[test]
    fn a_manual_holder_moves_through_a_camera_interlock_but_not_an_arm_interlock() {
        let (lease, now) = lease();
        let arm = SharedInterlocks::default();
        let (handle, _inbox) = crate::handoff::arm_channel();
        lease.register_arm(handle, Arc::new(AtomicU64::new(0)), arm.clone());
        let agent = acquire_agent(&lease, now);
        lease.report_camera(&agent, "front", false).unwrap();
        assert_eq!(
            lease.interlocks(&arm),
            (
                vec![Interlock::RequiredCameraUnavailable {
                    camera: "front".into()
                }],
                1
            )
        );
        assert_eq!(
            gated(&lease, Some(&agent), &arm, None).unwrap_err().reason,
            RejectionReason::Interlocked
        );
        lease.report_camera(&agent, "front", true).unwrap();
        assert!(gated(&lease, Some(&agent), &arm, None).is_ok());
        lease.report_camera(&agent, "front", false).unwrap();
        let manual = take(&lease, LeaseHolder::Manual, "operator", now);
        assert_eq!(lease.interlocks(&arm), (Vec::new(), 2));
        lease.report_camera(&manual, "front", false).unwrap();
        assert!(gated(&lease, Some(&manual), &arm, None).is_ok());
        lease.raise(&arm, Interlock::ArmDisconnected);
        assert!(gated(&lease, Some(&manual), &arm, None).is_err());
    }

    #[test]
    fn only_the_holder_reports_its_own_required_cameras() {
        let (lease, now) = lease();
        let token = acquire_agent(&lease, now);
        assert_eq!(
            lease.report_camera("someone-else", "front", false),
            Err(CameraReportRefusal::LeaseLost)
        );
        assert_eq!(
            lease.report_camera(&token, "wrist", false),
            Err(CameraReportRefusal::CameraNotRequired)
        );
        assert_eq!(lease.camera_requirements().unwrap().cameras, ["front"]);
    }

    #[test]
    fn a_holder_excludes_everyone_else() {
        let (lease, now) = lease();
        let token = acquire_agent(&lease, now);
        assert!(command(&lease, Some(&token), now).is_ok());
        for other in [None, Some("someone-elses-token")] {
            let refused = command(&lease, other, now).unwrap_err();
            assert_eq!(refused.reason, RejectionReason::LeaseHeldElsewhere);
            assert!(refused.detail.contains("touch the block"));
        }
        let refused = lease
            .acquire(request(LeaseHolder::Manual, "viewer"), now)
            .unwrap_err();
        assert!(matches!(refused, AcquireError::Held(status) if status.label == "touch the block"));
    }

    #[test]
    fn the_lease_lapses_without_explicit_renewal() {
        let (lease, start) = lease();
        let token = acquire_agent(&lease, start);
        let almost = Duration::from_secs(9);
        let first = start + almost;
        assert!(command(&lease, Some(&token), first).is_ok());
        assert!(lease.status(first).unwrap().expires_in_seconds <= 1.0);
        assert!(lease.retain(Some(&token), first));
        let second = first + almost;
        assert!(command(&lease, Some(&token), second).is_ok());
        let status = lease.status(second).unwrap();
        assert!(status.expires_in_seconds > 0.0 && status.expires_in_seconds <= 1.0);
        let lapsed = second + Duration::from_secs(2);
        let (arm, inbox) = crate::handoff::arm_channel();
        lease.register_arm(
            arm,
            Arc::new(AtomicU64::new(0)),
            SharedInterlocks::default(),
        );
        assert!(lease.status(lapsed).is_some());
        assert!(!lease.retain(Some(&token), lapsed));
        assert!(command(&lease, Some(&token), lapsed).is_err());
        assert!(command(&lease, None, lapsed).is_err());
        assert!(inbox.next().is_none());
        lease.tick(lapsed);
        assert!(matches!(
            inbox.next(),
            Some(crate::handoff::ArmRequest::Single(
                crate::handoff::SingleArmRequest::Submit {
                    command: Command::Stop,
                    ..
                }
            ))
        ));
        assert!(command(&lease, Some(&token), lapsed).is_err());
        assert!(command(&lease, None, lapsed).is_err());
        let (new_token, ()) = lease
            .begin_takeover(
                request(LeaseHolder::Manual, "viewer"),
                TakeoverMode::Recover,
                lapsed,
                || (),
            )
            .unwrap();
        assert!(
            lease
                .finish_takeover(new_token, lapsed, TakeoverResult::Settled)
                .is_some()
        );
    }

    #[test]
    fn an_expired_manual_lease_stays_fenced_when_the_loop_cannot_confirm_its_stop() {
        for disconnect_before_expiry in [false, true] {
            let (lease, now) = lease();
            let token = token_of(lease.acquire(request(LeaseHolder::Manual, "viewer"), now));
            let (handle, inbox) = crate::handoff::arm_channel();
            lease.register_arm(
                handle,
                Arc::new(AtomicU64::new(0)),
                SharedInterlocks::default(),
            );
            let mut inbox = Some(inbox);
            if disconnect_before_expiry {
                drop(inbox.take());
            }
            let expired = now + LEASE_FOR_TESTS * 2;
            lease.tick(expired);
            assert!(lease.status(expired).is_some());
            drop(inbox);
            lease.tick(expired);
            assert!(lease.status(expired).is_some());
            assert!(!lease.retain(Some(&token), expired));
            assert!(command(&lease, None, expired).is_err());
            assert!(!lease.revoke());
        }
    }

    #[test]
    fn taking_replaces_the_holder_and_its_token() {
        let (lease, now) = lease();
        let first = acquire_agent(&lease, now);
        let second = take(&lease, LeaseHolder::Manual, "viewer", now);
        assert_ne!(first, second);
        assert_eq!(
            command(&lease, Some(&first), now).unwrap_err().reason,
            RejectionReason::LeaseHeldElsewhere
        );
        assert!(command(&lease, Some(&second), now).is_ok());
        assert!(!lease.retain(Some(&first), now));
    }

    #[test]
    fn handoff_excludes_commands_until_stops_finish_and_revokes_the_old_token() {
        let (lease, now) = lease();
        let old_token = acquire_agent(&lease, now);
        let (reservation, ()) = lease
            .begin_takeover(
                request(LeaseHolder::Manual, "viewer"),
                TakeoverMode::Replace,
                now,
                || (),
            )
            .unwrap();
        for token in [
            None,
            Some(old_token.as_str()),
            Some(reservation.token.as_str()),
        ] {
            assert_eq!(
                command(&lease, token, now).unwrap_err().reason,
                RejectionReason::ArmBusy
            );
        }
        assert!(
            lease
                .begin_takeover(
                    request(LeaseHolder::Agent, "other"),
                    TakeoverMode::Replace,
                    now,
                    || ()
                )
                .is_err()
        );
        assert!(!lease.revoke());
        let grant = lease
            .finish_takeover(reservation, now, TakeoverResult::Settled)
            .unwrap();
        assert!(command(&lease, Some(&grant.lease_token), now).is_ok());
        assert!(command(&lease, Some(&old_token), now).is_err());
    }

    #[test]
    fn controller_loss_revokes_a_transfer_and_requires_a_fresh_lease_after_recovery() {
        let (lease, now) = lease();
        let original = acquire_agent(&lease, now);
        let (transferring, ()) = lease
            .begin_takeover(
                request(LeaseHolder::Manual, "viewer"),
                TakeoverMode::Replace,
                now,
                || (),
            )
            .unwrap();
        lease.controller_lost();
        assert!(!lease.retain(Some(&original), now));
        assert!(
            lease
                .acquire_stopping_arms(
                    request(LeaseHolder::Agent, "offline"),
                    AcquireMode::WaitOrFail
                )
                .is_err()
        );
        assert!(command(&lease, None, now).is_err());
        lease.controller_ready();
        assert!(command(&lease, Some(&original), now).is_err());
        let fresh = lease
            .acquire_stopping_arms(
                request(LeaseHolder::Agent, "fresh"),
                AcquireMode::WaitOrFail,
            )
            .unwrap();
        assert!(
            lease
                .finish_takeover(transferring, now, TakeoverResult::Settled)
                .is_none()
        );
        assert!(lease.retain(Some(&fresh.lease_token), now));
    }

    #[test]
    fn controller_reconnect_rejects_old_shutdown_and_recovery_approvals() {
        let (lease, _) = lease();
        let (arm, _inbox) = crate::handoff::arm_channel();
        let generation = Arc::new(AtomicU64::new(0));
        lease.register_arm(arm, Arc::clone(&generation), SharedInterlocks::default());
        let captured = vec![GenerationTicket::capture(&generation)];
        lease.controller_lost();
        lease.controller_ready();
        assert!(lease.admit_recovery_approval(&captured, || ()).is_err());
        assert!(
            lease
                .interrupt_if_current(&generation, &captured, || ())
                .is_err()
        );
        let fresh = vec![GenerationTicket::capture(&generation)];
        assert!(lease.admit_recovery_approval(&fresh, || ()).is_ok());
        assert!(
            lease
                .interrupt_if_current(&generation, &fresh, || ())
                .is_ok()
        );
    }

    #[test]
    fn sequence_stop_refreshes_only_the_request_that_started_it() {
        let (lease, now) = lease();
        let token = acquire_agent(&lease, now);
        let (arm, _inbox) = crate::handoff::arm_channel();
        let generation = Arc::new(AtomicU64::new(0));
        lease.register_arm(arm, Arc::clone(&generation), SharedInterlocks::default());
        let mut pending = vec![GenerationTicket::capture(&generation)];
        lease
            .stop_leased_motion_for_submission(&token, &mut pending, || true)
            .unwrap();
        assert!(lease.admit_recovery_approval(&pending, || ()).is_ok());
        let mut stale = pending.clone();
        lease.interrupt(&generation, || ());
        assert!(lease.admit_recovery_approval(&pending, || ()).is_err());
        let mut sequence_cancelled = false;
        assert!(
            lease
                .stop_leased_motion_for_submission(&token, &mut stale, || {
                    sequence_cancelled = true;
                    true
                })
                .is_err()
        );
        assert!(!sequence_cancelled);
    }

    #[test]
    fn stopping_one_arm_does_not_invalidate_another_arms_submission() {
        let (lease, now) = lease();
        let token = acquire_agent(&lease, now);
        let first_generation = Arc::new(AtomicU64::new(0));
        let second_generation = Arc::new(AtomicU64::new(0));
        let first_submission = [GenerationTicket::capture(&first_generation)];
        let second_submission = [GenerationTicket::capture(&second_generation)];

        lease.interrupt(&first_generation, || ());

        assert!(
            lease
                .admit_generation(
                    Some(&token),
                    now,
                    &[],
                    &first_submission,
                    LeaseAdmission::Normal,
                    || (),
                )
                .is_err()
        );
        assert!(
            lease
                .admit_generation(
                    Some(&token),
                    now,
                    &[],
                    &second_submission,
                    LeaseAdmission::Normal,
                    || (),
                )
                .is_ok()
        );
    }

    #[test]
    fn failed_handoff_blocks_commands_until_a_successful_retry() {
        let (lease, now) = lease();
        let (reservation, ()) = lease
            .begin_takeover(
                request(LeaseHolder::Manual, "viewer"),
                TakeoverMode::Replace,
                now,
                || (),
            )
            .unwrap();
        assert!(
            lease
                .finish_takeover(reservation, now, TakeoverResult::Failed)
                .is_none()
        );
        let later = now + LEASE_FOR_TESTS * 2;
        assert!(command(&lease, None, later).is_err());
        assert!(
            lease
                .admit_generation(None, later, &[], &[], LeaseAdmission::ClearFault, || ())
                .is_ok()
        );
        let (retry, ()) = lease
            .begin_takeover(
                request(LeaseHolder::Manual, "viewer"),
                TakeoverMode::Recover,
                later,
                || (),
            )
            .unwrap();
        let grant = lease
            .finish_takeover(retry, later, TakeoverResult::Settled)
            .unwrap();
        assert!(command(&lease, Some(&grant.lease_token), later).is_ok());
    }

    #[test]
    fn a_missing_control_loop_refuses_takeover_and_keeps_commands_blocked() {
        let (lease, now) = lease();
        let old_token = acquire_agent(&lease, now);
        let (arm, inbox) = crate::handoff::arm_channel();
        drop(inbox);
        lease.register_arm(
            arm,
            Arc::new(AtomicU64::new(0)),
            SharedInterlocks::default(),
        );

        assert!(matches!(
            lease.acquire_stopping_arms(
                request(LeaseHolder::Manual, "viewer"),
                AcquireMode::Takeover
            ),
            Err(AcquireError::StopFailed {
                reason: RejectionReason::NotEnabled,
                ..
            })
        ));
        assert!(command(&lease, Some(&old_token), now).is_err());
        assert!(command(&lease, None, now).is_err());
    }

    #[test]
    fn only_the_holder_can_return_and_the_operator_can_always_revoke() {
        let (lease, now) = lease();
        let token = acquire_agent(&lease, now);
        lease.return_lease(Some("not-the-token"), now);
        lease.return_lease(None, now);
        assert!(lease.status(now).is_some());
        lease.return_lease(Some(&token), now);
        assert_eq!(lease.status(now), None);
        assert!(command(&lease, Some(&token), now).is_err());
        acquire_agent(&lease, now);
        assert!(lease.revoke());
        assert_eq!(lease.status(now), None);
    }

    #[test]
    fn only_stopping_is_open_to_everyone() {
        assert!(matches!(
            CommandAuthority::for_command(&Command::Stop),
            CommandAuthority::Interrupt
        ));
        assert!(matches!(
            CommandAuthority::for_command(&Command::Shutdown),
            CommandAuthority::Interrupt
        ));
        assert!(matches!(
            CommandAuthority::for_command(&Command::Enable),
            CommandAuthority::Lease(LeaseAdmission::Normal)
        ));
        assert!(matches!(
            CommandAuthority::for_command(&Command::Disable),
            CommandAuthority::Lease(LeaseAdmission::Disable)
        ));
    }

    #[test]
    fn a_refused_command_does_not_run() {
        let (lease, now) = lease();
        acquire_agent(&lease, now);
        let mut ran = false;
        let _ = lease.admit_generation(None, now, &[], &[], LeaseAdmission::Normal, || {
            ran = true;
        });
        assert!(!ran);
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]

        #[test]
        fn generated_lease_interruptions_never_restore_an_old_tokens_authority(
            operations in proptest::collection::vec((0_u8..10, 0_u64..15000), 1..48)
        ) {
            use proptest::{prop_assert, prop_assert_eq};
            enum Authority {
                Offline,
                Vacant,
                Active { token: String, expires: Instant },
                Fenced,
                Transferring(usize),
            }
            let (lease, mut now) = lease();
            let mut authority = Authority::Vacant;
            let mut issued = Vec::new();
            let mut pending = std::collections::VecDeque::new();
            let mut next_transfer = 0;
            for (operation, elapsed_millis) in operations {
                now += Duration::from_millis(elapsed_millis);
                if matches!(&authority, Authority::Active { expires, .. } if *expires <= now) {
                    // INVARIANT: An expired agent lease remains fenced until recovery or takeover.
                    authority = Authority::Fenced;
                }
                match operation {
                    0 if !matches!(authority, Authority::Offline) => {
                        let answer = lease.acquire(request(LeaseHolder::Agent, "generated"), now);
                        prop_assert_eq!(answer.is_ok(), matches!(authority, Authority::Vacant));
                        if let Ok(grant) = answer {
                            issued.push(grant.lease_token.clone());
                            authority = Authority::Active { token: grant.lease_token, expires: now + LEASE_FOR_TESTS };
                        }
                    }
                    1 if !matches!(authority, Authority::Offline) => {
                        let answer = lease.begin_takeover(request(LeaseHolder::Agent, "generated-take"), TakeoverMode::Replace, now, || ());
                        prop_assert_eq!(answer.is_ok(), !matches!(authority, Authority::Transferring(_)));
                        if let Ok((reservation, ())) = answer {
                            pending.push_back((next_transfer, reservation));
                            authority = Authority::Transferring(next_transfer);
                            next_transfer += 1;
                        }
                    }
                    2 => {
                        if let Some(token) = issued.last() {
                            lease.return_lease(Some(token), now);
                            if matches!(&authority, Authority::Active { token: held, .. } if held == token) {
                                authority = Authority::Vacant;
                            }
                        }
                    }
                    3 => {
                        if let Authority::Active { token, expires } = &mut authority {
                            prop_assert!(lease.retain(Some(token), now));
                            *expires = now + LEASE_FOR_TESTS;
                        }
                    }
                    4 => { lease.controller_lost(); authority = Authority::Offline; }
                    5 => {
                        lease.controller_ready();
                        if matches!(authority, Authority::Offline) { authority = Authority::Vacant; }
                    }
                    6..=8 => {
                        let feedback = if operation == 8 { pending.pop_back() } else { pending.pop_front() };
                        if let Some((transfer, reservation)) = feedback {
                            let current = matches!(authority, Authority::Transferring(active) if active == transfer);
                            let stopped = operation != 7;
                            let result = if stopped { TakeoverResult::Settled } else { TakeoverResult::Failed };
                            let grant = lease.finish_takeover(reservation, now, result);
                            prop_assert_eq!(grant.is_some(), current && stopped);
                            if let Some(grant) = grant {
                                issued.push(grant.lease_token.clone());
                                authority = Authority::Active { token: grant.lease_token, expires: now + LEASE_FOR_TESTS };
                            } else if current {
                                authority = Authority::Fenced;
                            }
                        }
                    }
                    _ => { prop_assert!(!lease.retain(Some("never-issued"), now)); }
                }
                prop_assert_eq!(command(&lease, None, now).is_ok(), matches!(authority, Authority::Vacant));
                for token in &issued {
                    let owns_current = matches!(&authority, Authority::Active { token: held, .. } if held == token);
                    prop_assert_eq!(command(&lease, Some(token), now).is_ok(), owns_current);
                }
            }
        }
    }
}
