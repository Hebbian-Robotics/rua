use super::activity::Activity;
use super::runner::{GroupWrite, PendingCycle, Runner};
use super::trajectory::{ActiveGroupMember, GroupReference, GroupWriteAttempt, MotionStatus};
use crate::arm::{GroupMembership, Now};
use crate::config::ControllerConfig;
use crate::corner_path::{MotionSupervisor, ValidatedPlan};
use crate::limits::Trip;
use crate::motion_values::GroupId;
use crate::plant::Plant;
use crate::reference_filter::Reference;
use hebbian_contracts::{Checkpoint, CommandOutcome, CornerPlan, Segment};

impl<P: Plant> Runner<'_, P> {
    pub(super) fn group_member(&self) -> Option<&ActiveGroupMember> {
        match &self.state.activity {
            Activity::Group(member) => Some(member),
            _ => None,
        }
    }

    pub(super) fn last_group_failure(&self, group_id: &GroupId) -> Option<String> {
        let arm = &self.state.core;
        let record = arm.last_finished()?;
        if record.group_id() != Some(group_id.as_str()) {
            return None;
        }
        let outcome = record
            .outcome()
            .filter(|outcome| *outcome != CommandOutcome::Completed)?;
        let outcome = serde_json::to_string(&outcome)
            .unwrap_or_default()
            .trim_matches('"')
            .to_owned();
        Some(match record.detail() {
            Some(detail) => format!("{} ended {outcome}: {detail}", self.name),
            None => format!("{} ended {outcome}", self.name),
        })
    }
}

const GROUP_SEGMENT_LABEL: &str = "group_target";

pub(super) struct ActiveGroup {
    pub(super) id: GroupId,
    pub(super) members: Vec<usize>,
    pub(super) joint_counts: Vec<usize>,
    pub(super) supervisor: MotionSupervisor,
    checkpoints_sent: usize,
    step_pending: bool,
    shares: Vec<GroupReference>,
}

impl ActiveGroup {
    pub(super) fn form(
        id: &GroupId,
        members: Vec<usize>,
        plans: &[&ValidatedPlan],
        configs: &[&ControllerConfig],
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !members.is_empty() && members.len() == plans.len() && plans.len() == configs.len(),
            "A group needs matching nonempty members, plans and configurations"
        );
        let unique_members: std::collections::BTreeSet<_> = members.iter().collect();
        anyhow::ensure!(
            unique_members.len() == members.len(),
            "A group cannot repeat a member"
        );
        for (plan, config) in plans.iter().zip(configs) {
            anyhow::ensure!(
                plan.segments.len() == 1 && plan.segments[0].corners.len() == 2,
                "A group member needs one straight section"
            );
            anyhow::ensure!(
                plan.initial_corner().len() == config.joint_count,
                "A group member's plan does not match its arm's joint count"
            );
        }
        let joined = |corner: fn(&Segment) -> &Vec<f64>| -> Vec<f64> {
            plans
                .iter()
                .flat_map(|plan| corner(&plan.segments[0]).iter().copied())
                .collect()
        };
        let initial = joined(|segment| &segment.corners[0]);
        let target = joined(|segment| &segment.corners[1]);
        let plan = CornerPlan {
            segments: vec![Segment {
                pass_through: None,
                label: GROUP_SEGMENT_LABEL.into(),
                corners: vec![initial, target],
                minimum_duration_seconds: None,
                destination_position: None,
                destination_quaternion: None,
            }],
            peak_speed: plans
                .iter()
                .map(|plan| plan.peak_speed)
                .fold(f64::INFINITY, f64::min),
        };
        let joint_count = configs.iter().map(|config| config.joint_count).sum();
        let mut motion_limits = configs[0].motion.clone();
        motion_limits.acceleration_limit = configs
            .iter()
            .map(|config| config.motion.acceleration_limit)
            .fold(f64::INFINITY, f64::min);
        motion_limits.jerk_limit = configs
            .iter()
            .map(|config| config.motion.jerk_limit)
            .fold(f64::INFINITY, f64::min);
        let supervisor = MotionSupervisor::new(plan, &motion_limits, joint_count)?;
        Ok(Self {
            id: id.clone(),
            members,
            joint_counts: configs.iter().map(|config| config.joint_count).collect(),
            supervisor,
            checkpoints_sent: 0,
            step_pending: false,
            shares: configs
                .iter()
                .map(|config| GroupReference {
                    attempt: GroupWriteAttempt::First,
                    reference: Reference::at_rest(&config.lower_limits, None),
                    checkpoints: Vec::new(),
                    segment: None,
                    status: MotionStatus::Running,
                    final_pose: Vec::new(),
                })
                .collect(),
        })
    }

    pub(super) fn step(
        &mut self,
        measured: &[f64],
        interval: f64,
    ) -> Result<&[GroupReference], Trip> {
        // INVARIANT: A partial batch keeps its setpoint and checkpoints until all members
        // confirm that same step in one cycle.
        if self.step_pending {
            for share in &mut self.shares {
                share.attempt = GroupWriteAttempt::Retry;
            }
            return Ok(&self.shares);
        }
        self.supervisor.advance(measured, interval)?;
        self.step_pending = true;
        let checkpoints = self.supervisor.new_checkpoints(self.checkpoints_sent);
        let segment = self.supervisor.segment_label().map(|_| GROUP_SEGMENT_LABEL);
        let status = MotionStatus::of(&self.supervisor);
        let final_pose = (status != MotionStatus::Running).then(|| self.supervisor.final_pose());
        let reference = self.supervisor.reference();
        let mut start = 0;
        for (count, share) in self.joint_counts.iter().zip(&mut self.shares) {
            let part = start..start + count;
            start += count;
            share
                .reference
                .positions
                .copy_from_slice(&reference.positions[part.clone()]);
            share
                .reference
                .velocities
                .copy_from_slice(&reference.velocities[part.clone()]);
            share
                .reference
                .accelerations
                .copy_from_slice(&reference.accelerations[part.clone()]);
            share.checkpoints.clear();
            share
                .checkpoints
                .extend(checkpoints.iter().map(|checkpoint| Checkpoint {
                    measured_joints: checkpoint.measured_joints[part.clone()].to_vec(),
                    label: checkpoint.label.clone(),
                    destination_position: checkpoint.destination_position,
                    destination_quaternion: checkpoint.destination_quaternion,
                }));
            share.attempt = GroupWriteAttempt::First;
            share.segment = segment;
            share.status = status;
            share.final_pose.clear();
            if let Some(positions) = final_pose {
                share.final_pose.extend_from_slice(&positions[part]);
            }
        }
        Ok(&self.shares)
    }

    pub(super) fn confirm_step(&mut self) {
        self.checkpoints_sent += self.supervisor.new_checkpoints(self.checkpoints_sent).len();
        self.step_pending = false;
    }

    pub(super) fn member_reference(&self, index: usize) -> &GroupReference {
        &self.shares[index]
    }
}

// INVARIANT: Start the groups whose arms have all begun their parts. Arms are admitted
// into a group together, but each begins on its own cycle, so a group forms on
// the first cycle when all of them have.
pub(super) fn form_groups<P: Plant>(runners: &mut [Runner<'_, P>], groups: &mut Vec<ActiveGroup>) {
    let mut candidates: Vec<GroupMembership> = Vec::new();
    for runner in runners.iter() {
        if let Some(member) = runner.group_member()
            && !groups.iter().any(|group| group.id == member.group.id)
            && !candidates.iter().any(|group| group.id == member.group.id)
        {
            candidates.push(member.group.clone());
        }
    }
    for membership in candidates {
        let indexes: Option<Vec<usize>> = membership
            .arms
            .iter()
            .map(|arm| {
                runners.iter().position(|runner| {
                    runner.name == *arm
                        && runner
                            .group_member()
                            .is_some_and(|member| member.group.id == membership.id)
                })
            })
            .collect();
        let Some(indexes) = indexes else { continue };
        let plans: Vec<&ValidatedPlan> = indexes
            .iter()
            .filter_map(|index| runners[*index].group_member().map(|member| &*member.plan))
            .collect();
        let configs: Vec<&ControllerConfig> =
            indexes.iter().map(|index| runners[*index].config).collect();
        let result = ActiveGroup::form(&membership.id, indexes.clone(), &plans, &configs);
        match result {
            Ok(group) => groups.push(group),
            Err(error) => {
                let detail = format!("Could not construct grouped motion: {error:#}");
                let now = Now::current();
                for index in indexes {
                    runners[index].state.end_activity(
                        CommandOutcome::Failed,
                        Some(detail.clone()),
                        None,
                        now,
                    );
                }
            }
        }
    }
}

pub(super) struct GroupMemberReferenceIndex {
    pub(super) group: usize,
    pub(super) member: usize,
}

pub(super) fn step_groups<P: Plant>(
    runners: &mut [Runner<'_, P>],
    groups: &mut [ActiveGroup],
    begun: &[Option<PendingCycle>],
    steps: &mut Vec<Option<Result<GroupMemberReferenceIndex, Trip>>>,
    measured: &mut Vec<f64>,
) {
    steps.clear();
    steps.resize_with(runners.len(), || None);
    for (group_index, group) in groups.iter_mut().enumerate() {
        if let Some(stopping) = group.members.iter().copied().find(|&index| {
            runners[index]
                .group_member()
                .is_some_and(|member| member.group.id == group.id)
                && runners[index].state.core.stop_requested()
        }) {
            let reason = format!("{} requested stop", runners[stopping].name);
            for &index in &group.members {
                if index != stopping
                    && runners[index]
                        .group_member()
                        .is_some_and(|member| member.group.id == group.id)
                    && !runners[index].state.core.stop_requested()
                {
                    runners[index].state.request_group_stop(&reason);
                }
            }
            continue;
        }
        // INVARIANT: A member that did not read this cycle (its plant failed) cannot be
        // stepped with; the others hold until the group is settled.
        let interval = group
            .members
            .iter()
            .try_fold(f64::INFINITY, |interval, index| {
                let runner = &runners[*index];
                let member = runner.group_member()?;
                if member.group.id != group.id
                    || member.stop_reason.is_some()
                    || runner.state.core.stop_requested()
                {
                    return None;
                }
                Some(interval.min(begun[*index].as_ref()?.interval))
            });
        let Some(interval) = interval else { continue };
        measured.clear();
        measured.extend(
            group
                .members
                .iter()
                .flat_map(|index| runners[*index].feedback.positions.iter().copied()),
        );
        match group.step(measured, interval) {
            Ok(_) => {
                for (member, index) in group.members.iter().enumerate() {
                    steps[*index] = Some(Ok(GroupMemberReferenceIndex {
                        group: group_index,
                        member,
                    }));
                }
            }
            Err(trip) => {
                for index in &group.members {
                    steps[*index] = Some(Err(trip.clone()));
                }
            }
        }
    }
}

pub(super) fn confirm_group_writes<P: Plant>(
    runners: &mut [Runner<'_, P>],
    groups: &mut [ActiveGroup],
    writes: &mut [Option<GroupWrite>],
) {
    for group in groups {
        let confirmed = group.members.iter().all(|index| writes[*index].is_some());
        if confirmed {
            group.confirm_step();
        }
        for &index in &group.members {
            if let Some(write) = writes[index].take() {
                runners[index].finish_group_write(write, confirmed);
            }
        }
    }
}

// SAFETY: When one arm of a group has ended without completing, stop the others where
// they are; forget groups no arm is following any more.
pub(super) fn settle_groups<P: Plant>(
    runners: &mut [Runner<'_, P>],
    groups: &mut Vec<ActiveGroup>,
) {
    groups.retain(|group| {
        let is_following = |runner: &Runner<'_, P>| {
            runner
                .group_member()
                .is_some_and(|member| member.group.id == group.id)
        };
        if !group
            .members
            .iter()
            .any(|index| is_following(&runners[*index]))
        {
            return false;
        }
        let ended_badly = group
            .members
            .iter()
            .filter(|index| !is_following(&runners[**index]))
            .find_map(|index| runners[*index].last_group_failure(&group.id));
        if let Some(detail) = ended_badly {
            for index in &group.members {
                if is_following(&runners[*index]) {
                    runners[*index].state.request_group_stop(&detail);
                }
            }
            return false;
        }
        true
    });
}
