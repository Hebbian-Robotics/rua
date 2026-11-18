use super::activity::Activity;
use super::runner::{PendingCycle, Runner};
use super::trajectory::{ActiveGroupMember, GroupReference, MotionStatus};
use crate::arm::{GroupMembership, Now};
use crate::config::ExecutorConfig;
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
        if record.group_id.as_deref() != Some(group_id.as_str()) {
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

pub(super) struct ActiveGroup {
    pub(super) id: GroupId,
    pub(super) members: Vec<usize>,
    pub(super) joint_counts: Vec<usize>,
    pub(super) supervisor: MotionSupervisor,
    pub(super) checkpoints_sent: usize,
}

impl ActiveGroup {
    pub(super) fn form(
        id: &GroupId,
        members: Vec<usize>,
        plans: &[&ValidatedPlan],
        configs: &[&ExecutorConfig],
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
                label: "group_target".into(),
                corners: vec![initial, target],
                minimum_duration_seconds: None,
                target_position: None,
                target_quaternion: None,
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
        })
    }

    pub(super) fn step(
        &mut self,
        measured: &[f64],
        interval: f64,
    ) -> Result<Vec<GroupReference>, Trip> {
        self.supervisor.advance(measured, interval)?;
        let checkpoints = self.supervisor.take_new_checkpoints(self.checkpoints_sent);
        self.checkpoints_sent += checkpoints.len();
        let segment = self.supervisor.segment_label().map(str::to_owned);
        let status = MotionStatus::of(&self.supervisor);
        let final_pose = (status != MotionStatus::Running).then(|| self.supervisor.final_pose());
        let reference = self.supervisor.reference();
        let mut start = 0;
        Ok(self
            .joint_counts
            .iter()
            .map(|count| {
                let part = start..start + count;
                start += count;
                GroupReference {
                    reference: Reference {
                        positions: reference.positions[part.clone()].to_vec(),
                        velocities: reference.velocities[part.clone()].to_vec(),
                        accelerations: reference.accelerations[part.clone()].to_vec(),
                        speed_limit: None,
                    },
                    checkpoints: checkpoints
                        .iter()
                        .map(|checkpoint| Checkpoint {
                            measured_joints: checkpoint.measured_joints[part.clone()].to_vec(),
                            ..checkpoint.clone()
                        })
                        .collect(),
                    segment: segment.clone(),
                    status,
                    final_pose: final_pose
                        .map_or_else(Vec::new, |positions| positions[part].to_vec()),
                }
            })
            .collect())
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
        let configs: Vec<&ExecutorConfig> =
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

pub(super) fn step_groups<P: Plant>(
    runners: &[Runner<'_, P>],
    groups: &mut [ActiveGroup],
    begun: &[Option<PendingCycle>],
    steps: &mut Vec<Option<Result<GroupReference, Trip>>>,
) {
    steps.clear();
    steps.extend((0..runners.len()).map(|_| None));
    for group in groups {
        // INVARIANT: A member that did not read this cycle (its plant failed) cannot be
        // stepped with; the others hold until the group is settled.
        let Some(pendings) = group
            .members
            .iter()
            .map(|index| begun[*index].as_ref())
            .collect::<Option<Vec<&PendingCycle>>>()
        else {
            continue;
        };
        let measured: Vec<f64> = group
            .members
            .iter()
            .flat_map(|index| runners[*index].feedback.positions.iter().copied())
            .collect();
        let interval = pendings
            .iter()
            .map(|pending| pending.interval)
            .fold(f64::INFINITY, f64::min);
        match group.step(&measured, interval) {
            Ok(shares) => {
                for (index, share) in group.members.iter().zip(shares) {
                    steps[*index] = Some(Ok(share));
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

// SAFETY: When one arm of a group has ended without completing, stop the others where
// they are; forget groups no arm is following any more.
pub(super) fn settle_groups<P: Plant>(
    runners: &mut [Runner<'_, P>],
    groups: &mut Vec<ActiveGroup>,
) {
    groups.retain(|group| {
        let following: Vec<bool> = group
            .members
            .iter()
            .map(|index| {
                runners[*index]
                    .group_member()
                    .is_some_and(|member| member.group.id == group.id)
            })
            .collect();
        if !following.contains(&true) {
            return false;
        }
        let ended_badly = group
            .members
            .iter()
            .zip(&following)
            .filter(|(_, following)| !**following)
            .find_map(|(index, _)| runners[*index].last_group_failure(&group.id));
        if let Some(detail) = ended_badly {
            let now = Now::current();
            for (index, following) in group.members.iter().zip(&following) {
                if *following {
                    runners[*index].state.cancel_group_member(&detail, now);
                }
            }
            return false;
        }
        true
    });
}
