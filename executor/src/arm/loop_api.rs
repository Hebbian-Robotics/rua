//! The control loop's side of the state machine: what it takes and reports.

use std::time::{Duration, Instant};

use super::state::ArmCore;
use super::types::{
    ArmFault, GripperTarget, Job, Lifecycle, Now, Observation, Poses, PowerState, ShutdownState,
};
use hebbian_contracts::{Checkpoint, CommandOutcome};

#[derive(Clone, Copy)]
enum FaultKind {
    HardFault,
    Disconnected,
}

impl ArmCore {
    // ----- the control loop's side -----------------------------------------

    /// Record what the plant reported this cycle, reusing the last record's
    /// buffers.
    pub(crate) fn observe(
        &mut self,
        sampled_at: f64,
        positions: &[f64],
        tool_pose: Option<([f64; 3], [f64; 4])>,
        fault: Option<&str>,
    ) {
        let Some(observation) = &mut self.observation else {
            self.observation = Some(Observation {
                sampled_at,
                positions: positions.to_vec(),
                tool_pose,
                fault: fault.map(str::to_owned),
            });
            return;
        };
        observation.sampled_at = sampled_at;
        observation.positions.clear();
        observation.positions.extend_from_slice(positions);
        observation.tool_pose = tool_pose;
        match (&mut observation.fault, fault) {
            (Some(held), Some(new)) => {
                held.clear();
                held.push_str(new);
            }
            (held, new) => *held = new.map(str::to_owned),
        }
    }

    /// Whether a stop asked the jaw to hold where it is, once.
    pub(crate) fn take_gripper_stop(&mut self) -> bool {
        std::mem::take(&mut self.gripper_stop_requested)
    }

    /// The jaw angle a gripper command asked for, once; the loop acts on it.
    pub(crate) fn take_gripper_target(&mut self) -> Option<(String, GripperTarget)> {
        let command = self.gripper_command.as_mut()?;
        Some((command.command_id.clone(), command.pending_target.take()?))
    }

    /// End a gripper command after a guard refusal, arrival, or contact.
    pub(crate) fn finish_gripper(
        &mut self,
        command_id: &str,
        outcome: CommandOutcome,
        detail: Option<String>,
        now: Now,
    ) {
        if self
            .gripper_command
            .as_ref()
            .is_some_and(|command| command.command_id == command_id)
        {
            self.gripper_command = None;
            self.finish(command_id, outcome, detail, now);
        }
    }

    /// The next queued command to carry out; it becomes the active one. The loop
    /// calls this only while nothing is active.
    pub(crate) fn next_job(&mut self) -> Option<Job> {
        let job = match std::mem::replace(&mut self.lifecycle, Lifecycle::Idle) {
            Lifecycle::Queued(job) => job,
            other => {
                self.lifecycle = other;
                return None;
            }
        };
        let id = job.command_id().to_owned();
        self.lifecycle = Lifecycle::Starting(id.clone());
        if let Some(record) = self.records.get(&id).cloned() {
            self.store(record);
        }
        Some(job)
    }

    /// The active command is now running on the arm.
    pub(crate) fn begin_moving(&mut self) {
        if let Lifecycle::Starting(id) = &self.lifecycle {
            self.lifecycle = Lifecycle::Executing {
                command_id: id.clone(),
                settling: false,
            };
        }
        self.transition_power(|power| match power {
            PowerState::Holding(poses) => PowerState::Moving(poses),
            other => other,
        });
    }

    /// Whether the running motion only waits for the arm to settle.
    pub(crate) fn set_settling(&mut self, settling: bool) {
        if let Lifecycle::Executing {
            settling: current, ..
        } = &mut self.lifecycle
        {
            *current = settling;
        }
    }

    /// Whether a new move replaced the settling motion, once.
    pub(crate) fn take_supersede(&mut self) -> bool {
        if let Lifecycle::Superseding(queued) = &mut self.lifecycle {
            self.lifecycle = queued.take().map_or(Lifecycle::Idle, Lifecycle::Queued);
            true
        } else {
            false
        }
    }

    /// The loop let go of the motion a new move replaced; the arm holds `hold`.
    pub(crate) fn superseded_motion_ended(&mut self, hold: &[f64]) {
        self.set_hold(hold);
        self.transition_power(|power| match power {
            PowerState::Moving(poses) => PowerState::Holding(poses),
            other => other,
        });
    }

    /// Name the segment being followed; none when idle.
    pub(crate) fn set_segment(&mut self, segment: Option<String>) {
        self.segment = segment;
    }

    /// Add a motion's settled checkpoint to its record.
    pub(crate) fn add_checkpoint(&mut self, checkpoint: Checkpoint) {
        let Some(id) = self.active_command_id().map(str::to_owned) else {
            return;
        };
        if let Some(mut record) = self.records.get(&id).cloned() {
            record.checkpoints.push(checkpoint);
            self.store(record);
        }
    }

    /// The arm's torque is on, holding `initial`: enabling succeeded.
    pub fn enabled(&mut self, initial: &[f64], now: Now) {
        self.set_power(PowerState::Holding(Poses {
            rest: initial.to_vec(),
            hold: initial.to_vec(),
        }));
        self.end_active(CommandOutcome::Completed, None, now);
    }

    /// Where the arm holds between commands changed (a motion ended, a trip).
    pub(crate) fn set_hold(&mut self, hold: &[f64]) {
        if let PowerState::Holding(poses) | PowerState::Moving(poses) = &mut self.power {
            poses.hold = hold.to_vec();
        }
    }

    /// The motion or stream ended; the arm holds again.
    pub(crate) fn motion_ended(
        &mut self,
        outcome: CommandOutcome,
        detail: Option<String>,
        hold: &[f64],
        now: Now,
    ) {
        self.set_hold(hold);
        self.transition_power(|power| match power {
            PowerState::Moving(poses) => PowerState::Holding(poses),
            other => other,
        });
        self.end_active(outcome, detail, now);
    }

    /// Torque is off.
    pub fn released(&mut self, cleanup_errors: &[String], now: Now) {
        if let Some(command) = self.gripper_command.take() {
            self.finish(
                &command.command_id,
                CommandOutcome::Stopped,
                Some("The motors were released".into()),
                now,
            );
        }
        self.set_power(PowerState::Disabled);
        if cleanup_errors.is_empty() {
            self.end_active(CommandOutcome::Completed, None, now);
        } else {
            self.end_active(CommandOutcome::Failed, Some(cleanup_errors.join("; ")), now);
        }
    }

    /// Enabling failed, or a queued command found the arm not enabled.
    pub(crate) fn failed(&mut self, detail: &str, now: Now) {
        self.end_active(CommandOutcome::Failed, Some(detail.to_owned()), now);
    }

    /// A hard fault: the motors were released at once; everything refuses until
    /// `clear_fault`.
    pub(crate) fn enter_fault(&mut self, reason: &str, cleanup_errors: &[String], now: Now) {
        self.stop_on_fault(FaultKind::HardFault, reason, cleanup_errors, now);
    }

    /// The bus stopped answering; interrupt commands without changing torque.
    pub(crate) fn bus_lost(&mut self, reason: &str, cleanup_errors: &[String], now: Now) {
        self.stop_on_fault(FaultKind::Disconnected, reason, cleanup_errors, now);
    }

    /// A disabled arm's bus answers again; it waits for `enable`.
    pub(crate) fn bus_reconnected(&mut self) {
        self.set_power(PowerState::Reconnected);
    }

    fn stop_on_fault(
        &mut self,
        kind: FaultKind,
        reason: &str,
        cleanup_errors: &[String],
        now: Now,
    ) {
        self.request_stop();
        self.cancel_queued(
            now,
            match kind {
                FaultKind::HardFault => "Stopped before start because the arm faulted",
                FaultKind::Disconnected => "The bus stopped answering",
            },
        );
        let outcome = match kind {
            FaultKind::HardFault => CommandOutcome::Faulted,
            FaultKind::Disconnected => CommandOutcome::Interrupted,
        };
        if let Some(command) = self.gripper_command.take() {
            self.finish(&command.command_id, outcome, Some(reason.to_owned()), now);
        }
        let detail = std::iter::once(reason.to_owned())
            .chain(cleanup_errors.iter().cloned())
            .collect::<Vec<_>>()
            .join("; ");
        let fault = ArmFault {
            detail: detail.clone(),
            entered_at: now.unix_seconds,
        };
        self.set_power(match kind {
            FaultKind::HardFault => PowerState::Faulted(fault),
            FaultKind::Disconnected => PowerState::Disconnected(fault),
        });
        self.end_active(outcome, Some(detail), now);
    }

    pub(super) fn end_active(&mut self, outcome: CommandOutcome, detail: Option<String>, now: Now) {
        if let Some(id) = self.active_command_id().map(str::to_owned) {
            self.finish(&id, outcome, detail, now);
        }
        self.lifecycle = Lifecycle::Idle;
        self.segment = None;
        self.stream = None;
    }

    /// The shutdown return to rest may run: `stop_requested` no longer stops it.
    pub(crate) fn allow_shutdown_return(&mut self) {
        if self.shutdown == ShutdownState::Stopping {
            self.shutdown = ShutdownState::ReturningToRest;
        }
    }

    /// The loop's polling limit for a stream that stops getting goals.
    #[must_use]
    pub(crate) fn stream_timed_out(
        &self,
        command_id: &str,
        now: Instant,
        timeout: Duration,
    ) -> bool {
        self.stream
            .as_ref()
            .filter(|stream| stream.command_id == command_id)
            .is_some_and(|stream| now.duration_since(stream.last_goal_at) > timeout)
    }
}
