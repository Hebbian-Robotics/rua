//! Who may command the arms right now (the lease), and what inhibits motion
//! (interlocks). See `docs/control-model.md`.
//!
//! One [`Lease`] is shared by every arm of an executor, so every client,
//! whether an agent run, the viewer's manual controls or a script, is
//! arbitrated the same way, and nothing can bypass it by going around the
//! station.
//!
//! - With no holder, a client without a token may command (scripts and the
//!   command line keep working). A stale token cannot authorize a command.
//! - With a holder, only commands carrying its token are accepted.
//! - `stop` and `shutdown` are always accepted from anyone, and
//!   `approve_recovery` is the operator's: stopping and recovering are never
//!   what needs protecting.
//! - The holder renews the lease explicitly. A lapse stops leased motion and
//!   leaves the arms holding, so a crashed client cannot keep control.
//! - Each arm carries a list of interlocks and a revision bumped whenever one
//!   is raised. Motion is refused while any is active; a command carrying an
//!   `expected_revision` the arm has moved past is refused as `stale_revision`.
//!   A required camera's interlock belongs to the lease and ends with it.
//!
//! Every operation takes the instant it happens at, so the rules have no clock of
//! their own.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::arm::Rejection;
use crate::handoff::ArmHandle;
use crate::sync::LockUnpoisoned;
use hebbian_contracts::{
    Command, Interlock, LeaseGrant, LeaseHolder, LeaseStatus, OnInterruption, RejectionReason,
};

/// How long the lease lasts after its holder's last command or retain.
pub const LEASE: Duration = Duration::from_secs(15);

/// Whether anyone may send this command, whoever holds the lease.
#[must_use]
pub fn is_always_allowed(command: &Command) -> bool {
    matches!(command, Command::Stop | Command::Shutdown)
}

/// Whether a command is accepted while an interlock is active: it stops,
/// powers off or recovers the arm rather than moving it.
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
    /// Required cameras currently reported unavailable.
    unavailable_cameras: BTreeSet<String>,
    expires_at: Instant,
    phase: LeasePhase,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LeasePhase {
    Active,
    Transferring,
    Interrupted,
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
        self.phase == LeasePhase::Active && self.is_held_by(token)
    }
}

/// Whether two tokens are equal, taking as long for every pair of the same length.
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

/// A token no one can guess: 24 random bytes in hex.
fn new_token() -> String {
    let mut bytes = [0_u8; 24];
    std::fs::File::open("/dev/urandom")
        .and_then(|mut random| random.read_exact(&mut bytes))
        .expect("the system provides random bytes");
    bytes.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// One arm's own interlocks (those not tied to the lease) and its revision.
#[derive(Debug, Default)]
pub struct ArmInterlocks {
    active: Vec<Interlock>,
    revision: u64,
}

/// One arm's interlocks, shared between its link and the lease.
pub type SharedInterlocks = Arc<Mutex<ArmInterlocks>>;

/// An arm a command would move, and the revision the caller last saw on it.
#[derive(Clone)]
pub(crate) struct ArmGate {
    pub interlocks: SharedInterlocks,
    pub expected_revision: Option<u64>,
}

/// The lease on all of an executor's arms, and their interlocks.
pub struct Lease {
    duration: Duration,
    held: Mutex<Option<Held>>,
    arms: Mutex<Vec<LeasedArm>>,
    /// Even while a controller is available, odd while it is unavailable.
    /// Changes on every transition so an acquisition cannot cross a reconnect.
    controller_epoch: AtomicU64,
}

/// The holder's token and the cameras it requires.
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

/// Acquiring can be refused by another holder or by an unsuccessful stop.
#[derive(Debug)]
pub enum AcquireError {
    Held(LeaseStatus),
    StopFailed {
        reason: RejectionReason,
        detail: String,
    },
}

impl std::fmt::Display for AcquireError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Held(status) => write!(formatter, "The lease is held by '{}'.", status.label),
            Self::StopFailed { detail, .. } => {
                write!(formatter, "Could not stop to take the lease: {detail}")
            }
        }
    }
}

impl std::error::Error for AcquireError {}

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
        }
    }

    /// Revoke every holder and pending handoff after controller loss.
    pub fn controller_lost(&self) {
        let mut held = self.held.lock_unpoisoned();
        if self.controller_epoch.load(Ordering::Acquire) & 1 == 0 {
            self.controller_epoch.fetch_add(1, Ordering::AcqRel);
        }
        *held = None;
        for arm in self.arms.lock_unpoisoned().iter() {
            arm.generation.fetch_add(1, Ordering::AcqRel);
        }
    }

    /// Allow fresh leases after a new controller completes recovery checks.
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

    /// Stop every arm only while the specified holder still holds the lease.
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

    /// Stop a lease holder's sequence without invalidating the command that
    /// requested the stop. Any earlier stop still invalidates that command.
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

    /// Run an action while the specified holder still holds the lease.
    pub fn with_lease(&self, token: &str, action: &mut dyn FnMut()) -> bool {
        self.with_captured_lease(Some(token), action)
    }

    /// Run an action if the captured lease is current, or an unheld lease remains unheld.
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

    /// Acquire the lease, or with `take` take it, stopping every arm before
    /// granting it. This waits for the control loop and must run outside
    /// asynchronous workers.
    ///
    /// # Errors
    /// Another client holds the lease, or an arm cannot finish stopping.
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
        if !take && let Ok(grant) = self.acquire(request.clone(), Instant::now()) {
            return Ok(grant);
        }
        let arms = self.arms.lock_unpoisoned().clone();
        let (token, stops) = self
            .begin_takeover(request, take, Instant::now(), || {
                for arm in &arms {
                    arm.generation.fetch_add(1, Ordering::AcqRel);
                }
                arms.iter()
                    .map(|arm| arm.arm.submit_unawaited(Command::Stop, None))
                    .collect::<Vec<_>>()
            })
            .map_err(AcquireError::Held)?;
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

    /// Fence and stop the previous holder before making an expired lease available.
    fn live<'a>(&self, held: &'a mut Option<Held>, now: Instant) -> Option<&'a mut Held> {
        if held
            .as_ref()
            .is_some_and(|held| held.phase == LeasePhase::Active && held.expires_at <= now)
        {
            self.stop_all_arms();
            if let Some(claim) = held.as_mut() {
                claim.phase = LeasePhase::Interrupted;
            }
        }
        held.as_mut()
    }

    #[must_use]
    pub fn status(&self, now: Instant) -> Option<LeaseStatus> {
        let mut held = self.held.lock_unpoisoned();
        self.live(&mut held, now).map(|held| held.status(now))
    }

    /// The holder's token and required cameras, while the lease is active.
    #[must_use]
    pub fn camera_requirements(&self) -> Option<CameraRequirements> {
        let mut held = self.held.lock_unpoisoned();
        self.live(&mut held, Instant::now())
            .filter(|claim| claim.phase == LeasePhase::Active)
            .map(|claim| CameraRequirements {
                token: claim.token.clone(),
                cameras: claim.request.required_cameras.clone(),
            })
    }

    /// Report whether a camera the holder requires gives frames. A camera
    /// newly unavailable raises its interlock, bumps every arm's revision and
    /// stops the arms; an available one clears it.
    ///
    /// # Errors
    /// The token does not hold the lease, or the lease does not require the camera.
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
        }
        Ok(())
    }

    /// One arm's active interlocks, its own and the lease's, and its revision.
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

    /// Raise an arm's interlock, bumping its revision when it is new. A
    /// recovery that needs approval returns the lease when the holder asked
    /// for that.
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
                claim.phase == LeasePhase::Active
                    && claim.request.on_interruption == OnInterruption::ReturnLease
            }) {
                *held = None;
            }
        }
    }

    /// Clear an arm's interlocks that match.
    pub fn clear(&self, arm: &SharedInterlocks, matches: impl Fn(&Interlock) -> bool) {
        arm.lock_unpoisoned()
            .active
            .retain(|interlock| !matches(interlock));
    }

    /// Acquire the lease when no one holds it.
    ///
    /// # Errors
    /// Someone else holds the lease: their status.
    fn acquire(&self, request: LeaseRequest, now: Instant) -> Result<LeaseGrant, LeaseStatus> {
        let mut held = self.held.lock_unpoisoned();
        if let Some(existing) = self.live(&mut held, now) {
            return Err(existing.status(now));
        }
        let claim = Held {
            token: new_token(),
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

    /// Reserve the lease and queue all stops while command admission is locked.
    /// The reservation's token is never given to a client until the stop ends.
    pub(crate) fn begin_takeover<T>(
        &self,
        request: LeaseRequest,
        replace_active: bool,
        now: Instant,
        enqueue_stops: impl FnOnce() -> T,
    ) -> Result<(String, T), LeaseStatus> {
        let mut held = self.held.lock_unpoisoned();
        if let Some(existing) = self.live(&mut held, now)
            && (existing.phase == LeasePhase::Transferring
                || (existing.phase == LeasePhase::Active && !replace_active))
        {
            return Err(existing.status(now));
        }
        let reservation = Held {
            token: new_token(),
            request,
            unavailable_cameras: BTreeSet::new(),
            expires_at: now + self.duration,
            phase: LeasePhase::Transferring,
        };
        let token = reservation.token.clone();
        *held = Some(reservation);
        Ok((token, enqueue_stops()))
    }

    /// Complete the reservation only after every arm has stopped. A failed
    /// transfer stays reserved, preventing unleased commands until a retry.
    pub(crate) fn finish_takeover(
        &self,
        token: &str,
        now: Instant,
        stopped: bool,
    ) -> Option<LeaseGrant> {
        let mut held = self.held.lock_unpoisoned();
        let reservation = held.as_mut().filter(|reservation| {
            reservation.phase == LeasePhase::Transferring && reservation.is_held_by(Some(token))
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

    /// Extend the holder's lease. False if the token is not the holder's.
    pub fn retain(&self, token: Option<&str>, now: Instant) -> bool {
        let mut held = self.held.lock_unpoisoned();
        let duration = self.duration;
        self.live(&mut held, now)
            .filter(|claim| claim.is_active_holder(token))
            .map(|claim| claim.expires_at = now + duration)
            .is_some()
    }

    /// Give the lease up. Returning without holding it does nothing.
    pub fn return_lease(&self, token: Option<&str>, now: Instant) {
        let mut held = self.held.lock_unpoisoned();
        if self
            .live(&mut held, now)
            .is_some_and(|claim| claim.is_active_holder(token))
        {
            *held = None;
        }
    }

    /// Return the current lease and stop any cancelled action sequence atomically.
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
        true
    }

    /// Queue the holder's disconnect stops before allowing another holder.
    pub(crate) fn return_after(&self, token: &str, now: Instant, stop: impl FnOnce()) {
        let mut held = self.held.lock_unpoisoned();
        if self
            .live(&mut held, now)
            .is_some_and(|claim| claim.is_active_holder(Some(token)))
        {
            stop();
            *held = None;
        }
    }

    /// End an active lease. An incomplete handoff cannot be revoked.
    pub fn revoke(&self) -> bool {
        let mut held = self.held.lock_unpoisoned();
        if held
            .as_ref()
            .is_none_or(|claim| claim.phase == LeasePhase::Active)
        {
            *held = None;
            true
        } else {
            false
        }
    }

    /// Invalidate prior submissions for this arm and queue its stop atomically.
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

    /// Approving recovery is the operator's, whoever holds the lease.
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
            if claim.phase == LeasePhase::Interrupted && options.allow_fault_recovery {
                return Ok(act());
            }
            if claim.phase != LeasePhase::Active {
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

/// Manual control needs no interlock but a required camera's on any arm.
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

    fn token_of(acquired: Result<LeaseGrant, LeaseStatus>) -> String {
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
        // The camera interlock ended with the agent's lease.
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
        assert_eq!(refused.label, "touch the block");
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
}
