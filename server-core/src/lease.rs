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

pub const LEASE: Duration = Duration::from_secs(15);

#[must_use]
pub fn is_always_allowed(command: &Command) -> bool {
    matches!(command, Command::Stop | Command::Shutdown)
}

#[must_use]
pub fn is_allowed_while_interlocked(command: &Command) -> bool {
    matches!(
        command,
        Command::Stop
            | Command::Shutdown
            | Command::Disable
            | Command::ClearFault
            | Command::ApproveRecovery
    )
}

pub use hebbian_contracts::LeaseRequest;

struct Held {
    token: String,
    request: LeaseRequest,
    unavailable_cameras: BTreeSet<String>,
    expires_at: Instant,
    phase: LeasePhase,
}

enum LeasePhase {
    Active,
    Transferring,
    Interrupted,
    ExpiredManual(Vec<PendingArmStop>),
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
    held: Mutex<Option<Held>>,
    arms: Mutex<Vec<LeasedArm>>,
    // INVARIANT: Even while a controller is available, odd while it is unavailable.
    // Changes on every transition so an acquisition cannot cross a reconnect.
    controller_epoch: AtomicU64,
    authority_changes: tokio::sync::watch::Sender<()>,
}

#[derive(Clone)]
pub struct CameraRequirements {
    pub token: String,
    pub cameras: Vec<String>,
}

#[derive(Default)]
pub(crate) struct AdmissionOptions {
    pub allow_fault_recovery: bool,
    pub allow_while_interlocked: bool,
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
            held: Mutex::new(None),
            arms: Mutex::new(Vec::new()),
            controller_epoch: AtomicU64::new(0),
            authority_changes: tokio::sync::watch::Sender::new(()),
        }
    }

    pub fn controller_lost(&self) {
        let mut held = self.held.lock_unpoisoned();
        if self.controller_epoch.load(Ordering::Acquire) & 1 == 0 {
            self.controller_epoch.fetch_add(1, Ordering::AcqRel);
        }
        *held = None;
        for arm in self.arms.lock_unpoisoned().iter() {
            arm.generation.fetch_add(1, Ordering::AcqRel);
        }
        self.authority_changes.send_replace(());
    }

    pub fn controller_ready(&self) {
        let _held = self.held.lock_unpoisoned();
        if self.controller_epoch.load(Ordering::Acquire) & 1 == 1 {
            self.controller_epoch.fetch_add(1, Ordering::AcqRel);
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
        let mut held = self.held.lock_unpoisoned();
        if !self
            .live(&mut held, Instant::now())
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
        generations: &mut [(Arc<AtomicU64>, u64)],
        cancel_sequence: impl FnOnce() -> bool,
    ) -> Result<(), Rejection> {
        let mut held = self.held.lock_unpoisoned();
        if !self
            .live(&mut held, Instant::now())
            .is_some_and(|claim| claim.is_active_holder(Some(token)))
        {
            return Err(Rejection {
                reason: RejectionReason::LeaseHeldElsewhere,
                detail: "The lease is no longer held".into(),
            });
        }
        self.check_controller_generation(generations)?;
        if !cancel_sequence() {
            return Ok(());
        }
        if !self.stop_all_arms() {
            return Err(crate::handoff::loop_not_running());
        }
        for (generation, captured) in generations {
            *captured = generation.load(Ordering::Acquire);
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
        let mut held = self.held.lock_unpoisoned();
        if self.controller_epoch.load(Ordering::Acquire) & 1 == 1 {
            return false;
        }
        let claim = self.live(&mut held, Instant::now());
        let authorized = match (claim, token) {
            (None, None) => true,
            (Some(claim), Some(token)) => claim.is_active_holder(Some(token)),
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
        take: bool,
    ) -> Result<LeaseGrant, AcquireError> {
        let epoch = self.controller_epoch.load(Ordering::Acquire);
        if epoch & 1 == 1 {
            return Err(controller_unavailable());
        }
        let result = self.acquire_stopping_arms_inner(request, take, epoch);
        if self.controller_epoch.load(Ordering::Acquire) != epoch {
            if let Ok(grant) = &result {
                self.return_lease(Some(&grant.lease_token), Instant::now());
            }
            return Err(controller_unavailable());
        }
        result
    }

    fn acquire_stopping_arms_inner(
        &self,
        request: LeaseRequest,
        take: bool,
        epoch: u64,
    ) -> Result<LeaseGrant, AcquireError> {
        if !take {
            match self.acquire(request.clone(), Instant::now()) {
                Ok(grant) => return Ok(grant),
                Err(AcquireError::Held(_)) => {}
                Err(error) => return Err(error),
            }
        }
        let arms = self.arms.lock_unpoisoned().clone();
        let (token, stops) = self.begin_takeover(request, take, Instant::now(), || {
            for arm in &arms {
                arm.generation.fetch_add(1, Ordering::AcqRel);
            }
            arms.iter()
                .map(|arm| arm.arm.submit_unawaited(Command::Stop, None))
                .collect::<Vec<_>>()
        })?;
        if self.controller_epoch.load(Ordering::Acquire) != epoch {
            let mut held = self.held.lock_unpoisoned();
            if held
                .as_ref()
                .is_some_and(|claim| claim.is_held_by(Some(&token)))
            {
                *held = None;
            }
            return Err(controller_unavailable());
        }
        let deadline = Instant::now() + TAKEOVER_TIMEOUT;
        let result = (|| -> Result<(), Rejection> {
            for stop in stops {
                let answer = stop?;
                answer
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| crate::handoff::loop_not_running())??;
            }
            loop {
                let mut all_ready = true;
                for arm in &arms {
                    let answer = arm.arm.control_handoff_readiness()?;
                    let ready = answer
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .map_err(|_| crate::handoff::loop_not_running())??;
                    all_ready &= ready;
                }
                if all_ready {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(Rejection {
                        reason: RejectionReason::ArmBusy,
                        detail: "Timed out waiting for the arms to stop".into(),
                    });
                }
                std::thread::sleep(TAKEOVER_POLL_INTERVAL);
            }
        })();
        let grant = self.finish_takeover(&token, Instant::now(), result.is_ok());
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

    // SAFETY: Keep expired manual control fenced until every arm confirms it has stopped.
    fn live<'a>(&self, held: &'a mut Option<Held>, now: Instant) -> Option<&'a mut Held> {
        if held.as_ref().is_some_and(|claim| {
            matches!(claim.phase, LeasePhase::Active) && claim.expires_at <= now
        }) {
            let stops_submitted = self.stop_all_arms();
            if let Some(claim) = held.as_mut() {
                claim.phase = if claim.request.holder == LeaseHolder::Manual && stops_submitted {
                    self.arms
                        .lock_unpoisoned()
                        .iter()
                        .map(|arm| {
                            Ok(PendingArmStop {
                                arm: arm.arm.clone(),
                                readiness: arm.arm.control_handoff_readiness()?,
                            })
                        })
                        .collect::<Result<Vec<_>, Rejection>>()
                        .map_or(LeasePhase::Interrupted, LeasePhase::ExpiredManual)
                } else {
                    LeasePhase::Interrupted
                };
            }
            self.authority_changes.send_replace(());
        }
        let mut release = false;
        if let Some(claim) = held.as_mut()
            && let LeasePhase::ExpiredManual(pending) = &mut claim.phase
        {
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
                claim.phase = LeasePhase::Interrupted;
            } else {
                release = pending.is_empty();
            }
        }
        if release {
            *held = None;
        }
        held.as_mut()
    }

    #[must_use]
    pub fn status(&self, now: Instant) -> Option<LeaseStatus> {
        let mut held = self.held.lock_unpoisoned();
        self.live(&mut held, now).map(|held| held.status(now))
    }

    #[must_use]
    pub fn camera_requirements(&self) -> Option<CameraRequirements> {
        let mut held = self.held.lock_unpoisoned();
        self.live(&mut held, Instant::now())
            .filter(|claim| matches!(claim.phase, LeasePhase::Active))
            .map(|claim| CameraRequirements {
                token: claim.token.clone(),
                cameras: claim.request.required_cameras.clone(),
            })
    }

    pub fn report_camera(&self, token: &str, camera: &str, available: bool) -> Result<(), String> {
        let mut held = self.held.lock_unpoisoned();
        let Some(claim) = self
            .live(&mut held, Instant::now())
            .filter(|claim| claim.is_active_holder(Some(token)))
        else {
            return Err("This token does not hold the lease".into());
        };
        if !claim
            .request
            .required_cameras
            .iter()
            .any(|name| name == camera)
        {
            return Err(format!("The lease does not require camera {camera}"));
        }
        if available {
            claim.unavailable_cameras.remove(camera);
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
            let mut held = self.held.lock_unpoisoned();
            self.live(&mut held, Instant::now())
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
                    false
                }
                None => {
                    interlocks.active.push(interlock.clone());
                    interlocks.revision = interlocks.revision.saturating_add(1);
                    true
                }
            }
        };
        if raised && matches!(interlock, Interlock::RecoveryApprovalRequired { .. }) {
            let mut held = self.held.lock_unpoisoned();
            if self.live(&mut held, Instant::now()).is_some_and(|claim| {
                matches!(claim.phase, LeasePhase::Active)
                    && claim.request.on_interruption == OnInterruption::ReturnLease
            }) {
                *held = None;
            }
        }
        if raised {
            self.authority_changes.send_replace(());
        }
    }

    pub fn clear(&self, arm: &SharedInterlocks, matches: impl Fn(&Interlock) -> bool) {
        arm.lock_unpoisoned()
            .active
            .retain(|interlock| !matches(interlock));
    }

    fn acquire(&self, request: LeaseRequest, now: Instant) -> Result<LeaseGrant, AcquireError> {
        let mut held = self.held.lock_unpoisoned();
        if let Some(existing) = self.live(&mut held, now) {
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
    pub(crate) fn begin_takeover<T>(
        &self,
        request: LeaseRequest,
        replace_active: bool,
        now: Instant,
        enqueue_stops: impl FnOnce() -> T,
    ) -> Result<(String, T), AcquireError> {
        let mut held = self.held.lock_unpoisoned();
        if let Some(existing) = self.live(&mut held, now)
            && (matches!(existing.phase, LeasePhase::Transferring)
                || (matches!(existing.phase, LeasePhase::Active) && !replace_active))
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
        let token = reservation.token.clone();
        *held = Some(reservation);
        self.authority_changes.send_replace(());
        Ok((token, enqueue_stops()))
    }

    // SAFETY: Complete the reservation only after every arm has stopped. A failed
    // transfer stays reserved, preventing unleased commands until a retry.
    pub(crate) fn finish_takeover(
        &self,
        token: &str,
        now: Instant,
        stopped: bool,
    ) -> Option<LeaseGrant> {
        let mut held = self.held.lock_unpoisoned();
        let reservation = held.as_mut().filter(|reservation| {
            matches!(reservation.phase, LeasePhase::Transferring)
                && reservation.is_held_by(Some(token))
        })?;
        if !stopped {
            reservation.phase = LeasePhase::Interrupted;
            return None;
        }
        reservation.phase = LeasePhase::Active;
        reservation.expires_at = now + self.duration;
        Some(LeaseGrant {
            lease_token: reservation.token.clone(),
            lease_seconds: self.duration.as_secs_f64(),
            status: reservation.status(now),
        })
    }

    pub fn retain(&self, token: Option<&str>, now: Instant) -> bool {
        let mut held = self.held.lock_unpoisoned();
        let duration = self.duration;
        self.live(&mut held, now)
            .filter(|claim| claim.is_active_holder(token))
            .map(|claim| claim.expires_at = now + duration)
            .is_some()
    }

    pub fn return_lease(&self, token: Option<&str>, now: Instant) {
        let mut held = self.held.lock_unpoisoned();
        if self
            .live(&mut held, now)
            .is_some_and(|claim| claim.is_active_holder(token))
        {
            *held = None;
            self.authority_changes.send_replace(());
        }
    }

    pub(crate) fn release_with(&self, token: &str, cancel_sequence: impl FnOnce() -> bool) -> bool {
        let mut held = self.held.lock_unpoisoned();
        if !self
            .live(&mut held, Instant::now())
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
        let mut held = self.held.lock_unpoisoned();
        if self
            .live(&mut held, now)
            .is_some_and(|claim| claim.is_active_holder(Some(token)))
        {
            stop();
            *held = None;
            self.authority_changes.send_replace(());
        }
    }

    // INVARIANT: An incomplete handoff cannot be revoked: its stop must finish before control can become available.
    pub fn revoke(&self) -> bool {
        let mut held = self.held.lock_unpoisoned();
        if held
            .as_ref()
            .is_none_or(|claim| matches!(claim.phase, LeasePhase::Active))
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
        let _held = self.held.lock_unpoisoned();
        generation.fetch_add(1, Ordering::AcqRel);
        enqueue()
    }

    pub(crate) fn interrupt_if_current<T>(
        &self,
        arm_generation: &AtomicU64,
        generations: &[(Arc<AtomicU64>, u64)],
        enqueue: impl FnOnce() -> T,
    ) -> Result<T, Rejection> {
        let _held = self.held.lock_unpoisoned();
        self.check_controller_generation(generations)?;
        arm_generation.fetch_add(1, Ordering::AcqRel);
        Ok(enqueue())
    }

    fn check_controller_generation(
        &self,
        generations: &[(Arc<AtomicU64>, u64)],
    ) -> Result<(), Rejection> {
        if self.controller_epoch.load(Ordering::Acquire) & 1 == 1 {
            return Err(crate::handoff::loop_not_running());
        }
        if generations
            .iter()
            .any(|(current, expected)| current.load(Ordering::Acquire) != *expected)
        {
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
        generations: &[(Arc<AtomicU64>, u64)],
        enqueue: impl FnOnce() -> T,
    ) -> Result<T, Rejection> {
        let _held = self.held.lock_unpoisoned();
        self.check_controller_generation(generations)?;
        Ok(enqueue())
    }

    pub(crate) fn admit_generation<T>(
        &self,
        token: Option<&str>,
        now: Instant,
        gates: &[ArmGate],
        generations: &[(Arc<AtomicU64>, u64)],
        options: AdmissionOptions,
        act: impl FnOnce() -> T,
    ) -> Result<T, Rejection> {
        let mut held = self.held.lock_unpoisoned();
        if self.controller_epoch.load(Ordering::Acquire) & 1 == 1 {
            return Err(crate::handoff::loop_not_running());
        }
        let (manual_holder, cameras_unavailable) =
            self.live(&mut held, now).map_or((false, false), |claim| {
                (
                    claim.is_active_holder(token) && claim.request.holder == LeaseHolder::Manual,
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
                        "An interlock was raised since revision {}; the arm is at revision {}. \
                         Read the state again and retry.",
                        gate.expected_revision.unwrap_or_default(),
                        interlocks.revision
                    ),
                });
            }
            if !options.allow_while_interlocked
                && let Some(interlock) = interlocks.active.first()
            {
                return Err(Rejection {
                    reason: RejectionReason::Interlocked,
                    detail: interlock.to_string(),
                });
            }
        }
        if !options.allow_while_interlocked && cameras_unavailable && !manual_holder {
            return Err(Rejection {
                reason: RejectionReason::Interlocked,
                detail: "A camera the lease requires is unavailable. Motion resumes when it gives a fresh frame".to_owned(),
            });
        }
        if generations
            .iter()
            .any(|(current, expected)| current.load(Ordering::Acquire) != *expected)
        {
            return Err(Rejection {
                reason: RejectionReason::ArmBusy,
                detail: "The command was superseded by a stop".to_owned(),
            });
        }
        if let Some(claim) = self.live(&mut held, now) {
            if matches!(
                claim.phase,
                LeasePhase::Interrupted | LeasePhase::ExpiredManual(_)
            ) && options.allow_fault_recovery
            {
                return Ok(act());
            }
            if !matches!(claim.phase, LeasePhase::Active) {
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
            .begin_takeover(request(holder, label), true, now, || ())
            .unwrap();
        lease
            .finish_takeover(&reservation, now, true)
            .unwrap()
            .lease_token
    }

    fn command(lease: &Lease, token: Option<&str>, now: Instant) -> Result<(), Rejection> {
        lease.admit_generation(token, now, &[], &[], AdmissionOptions::default(), || ())
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
            AdmissionOptions::default(),
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
        lease.clear(&arm, |interlock| *interlock == Interlock::ArmDisconnected);
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
        assert!(lease.report_camera("someone-else", "front", false).is_err());
        assert!(lease.report_camera(&token, "wrist", false).is_err());
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
        assert!(lease.status(lapsed).is_some());
        assert!(!lease.retain(Some(&token), lapsed));
        assert!(command(&lease, Some(&token), lapsed).is_err());
        assert!(command(&lease, None, lapsed).is_err());
        let (new_token, ()) = lease
            .begin_takeover(request(LeaseHolder::Manual, "viewer"), false, lapsed, || ())
            .unwrap();
        assert!(lease.finish_takeover(&new_token, lapsed, true).is_some());
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
            assert!(lease.status(expired).is_some());
            drop(inbox);
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
            .begin_takeover(request(LeaseHolder::Manual, "viewer"), true, now, || ())
            .unwrap();
        for token in [None, Some(old_token.as_str()), Some(reservation.as_str())] {
            assert_eq!(
                command(&lease, token, now).unwrap_err().reason,
                RejectionReason::ArmBusy
            );
        }
        assert!(
            lease
                .begin_takeover(request(LeaseHolder::Agent, "other"), true, now, || ())
                .is_err()
        );
        assert!(!lease.revoke());
        let grant = lease.finish_takeover(&reservation, now, true).unwrap();
        assert!(command(&lease, Some(&grant.lease_token), now).is_ok());
        assert!(command(&lease, Some(&old_token), now).is_err());
    }

    #[test]
    fn controller_loss_revokes_a_transfer_and_requires_a_fresh_lease_after_recovery() {
        let (lease, now) = lease();
        let original = acquire_agent(&lease, now);
        let (transferring, ()) = lease
            .begin_takeover(request(LeaseHolder::Manual, "viewer"), true, now, || ())
            .unwrap();
        lease.controller_lost();
        assert!(!lease.retain(Some(&original), now));
        assert!(lease.finish_takeover(&transferring, now, true).is_none());
        assert!(
            lease
                .acquire_stopping_arms(request(LeaseHolder::Agent, "offline"), false)
                .is_err()
        );
        assert!(command(&lease, None, now).is_err());
        lease.controller_ready();
        assert!(command(&lease, Some(&original), now).is_err());
        assert!(
            lease
                .acquire_stopping_arms(request(LeaseHolder::Agent, "fresh"), false)
                .is_ok()
        );
    }

    #[test]
    fn controller_reconnect_rejects_old_shutdown_and_recovery_approvals() {
        let (lease, _) = lease();
        let (arm, _inbox) = crate::handoff::arm_channel();
        let generation = Arc::new(AtomicU64::new(0));
        lease.register_arm(arm, Arc::clone(&generation), SharedInterlocks::default());
        let captured = vec![(Arc::clone(&generation), 0)];
        lease.controller_lost();
        lease.controller_ready();
        assert!(lease.admit_recovery_approval(&captured, || ()).is_err());
        assert!(
            lease
                .interrupt_if_current(&generation, &captured, || ())
                .is_err()
        );
        let fresh = vec![(Arc::clone(&generation), generation.load(Ordering::Acquire))];
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
        let mut pending = vec![(Arc::clone(&generation), 0)];
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
        let first_submission = [(Arc::clone(&first_generation), 0)];
        let second_submission = [(Arc::clone(&second_generation), 0)];

        lease.interrupt(&first_generation, || ());

        assert!(
            lease
                .admit_generation(
                    Some(&token),
                    now,
                    &[],
                    &first_submission,
                    AdmissionOptions::default(),
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
                    AdmissionOptions::default(),
                    || (),
                )
                .is_ok()
        );
    }

    #[test]
    fn failed_handoff_blocks_commands_until_a_successful_retry() {
        let (lease, now) = lease();
        let (reservation, ()) = lease
            .begin_takeover(request(LeaseHolder::Manual, "viewer"), true, now, || ())
            .unwrap();
        assert!(lease.finish_takeover(&reservation, now, false).is_none());
        let later = now + LEASE_FOR_TESTS * 2;
        assert!(command(&lease, None, later).is_err());
        assert!(
            lease
                .admit_generation(
                    None,
                    later,
                    &[],
                    &[],
                    AdmissionOptions {
                        allow_fault_recovery: true,
                        ..AdmissionOptions::default()
                    },
                    || ()
                )
                .is_ok()
        );
        let (retry, ()) = lease
            .begin_takeover(request(LeaseHolder::Manual, "viewer"), false, later, || ())
            .unwrap();
        let grant = lease.finish_takeover(&retry, later, true).unwrap();
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
            lease.acquire_stopping_arms(request(LeaseHolder::Manual, "viewer"), true),
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
        assert!(is_always_allowed(&Command::Stop));
        assert!(is_always_allowed(&Command::Shutdown));
        assert!(!is_always_allowed(&Command::Enable));
        assert!(!is_always_allowed(&Command::Disable));
    }

    #[test]
    fn a_refused_command_does_not_run() {
        let (lease, now) = lease();
        acquire_agent(&lease, now);
        let mut ran = false;
        let _ = lease.admit_generation(None, now, &[], &[], AdmissionOptions::default(), || {
            ran = true;
        });
        assert!(!ran);
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]

        #[test]
        fn generated_lease_interruptions_never_restore_an_old_tokens_authority(
            operations in proptest::collection::vec((0_u8..7, 0_u64..15000), 1..48)
        ) {
            use proptest::{prop_assert, prop_assert_eq};
            enum Authority {
                Offline,
                Vacant,
                Active { token: String, expires: Instant },
                Fenced,
            }
            let (lease, mut now) = lease();
            let mut authority = Authority::Vacant;
            let mut issued = Vec::new();
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
                        let (token, ()) = lease.begin_takeover(request(LeaseHolder::Agent, "generated-take"), true, now, || ()).unwrap();
                        let grant = lease.finish_takeover(&token, now, true).unwrap();
                        issued.push(grant.lease_token.clone());
                        authority = Authority::Active { token: grant.lease_token, expires: now + LEASE_FOR_TESTS };
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
