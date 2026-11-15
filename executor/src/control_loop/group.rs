//! Grouped motions: one trajectory stepped over every member arm.

use super::activity::Activity;
use super::runner::{PendingCycle, Runner};
use super::trajectory::{ActiveGroupMember, GroupReference, MotionStatus};
use crate::arm::{GroupMembership, Now};
use crate::config::ExecutorConfig;
use crate::corner_path::MotionSupervisor;
use crate::limits::Trip;
use crate::plant::Plant;
use crate::reference_filter::Reference;
use hebbian_contracts::{Checkpoint, CommandOutcome, CornerPlan, Segment};

impl<P: Plant> Runner<'_, P> {
    /// This arm's part of a grouped motion, if it is following one.
    pub(super) fn group_member(&self) -> Option<&ActiveGroupMember> {
        match &self.state.activity {
            Activity::Group(member) => Some(member),
            _ => None,
        }
    }

    /// Why this arm's part of the group `group_id` ended without completing,
    /// if its last finished command is that part and it did not complete.
    pub(super) fn last_group_failure(&self, group_id: &str) -> Option<String> {
        let arm = &self.state.core;
        let record = arm.last_finished()?;
        if record.group_id.as_deref() != Some(group_id) {
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

/// A grouped motion being followed: one trajectory over every member's joints,
/// joined in the order of the group's arms, with one tracking gate.
pub(super) struct ActiveGroup {
    pub(super) id: String,
    /// Runner indexes, in the group's order.
    pub(super) members: Vec<usize>,
    pub(super) joint_counts: Vec<usize>,
    pub(super) supervisor: MotionSupervisor,
    pub(super) checkpoints_sent: usize,
}

impl ActiveGroup {
    /// Join the members' plans into one straight run from where every arm holds
    /// to where every arm is going, at the first plan's speed.
    pub(super) fn form(
        id: &str,
        members: Vec<usize>,
        plans: &[&CornerPlan],
        configs: &[&ExecutorConfig],
    ) -> anyhow::Result<Self> {
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
            peak_speed: plans[0].peak_speed,
        };
        let joint_count = configs.iter().map(|config| config.joint_count).sum();
        let supervisor = MotionSupervisor::new(plan, &configs[0].motion, joint_count)?;
        Ok(Self {
            id: id.to_owned(),
            members,
            joint_counts: configs.iter().map(|config| config.joint_count).collect(),
            supervisor,
            checkpoints_sent: 0,
        })
    }

    /// The trajectory's next reference, split into one share per member, from the
    /// members' measured joints. Each member's own loop checks its tracking.
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

/// Start the groups whose arms have all begun their parts. Arms are admitted
/// into a group together, but each begins on its own cycle, so a group forms on
/// the first cycle when all of them have.
pub(super) fn form_groups<P: Plant>(runners: &[Runner<'_, P>], groups: &mut Vec<ActiveGroup>) {
    let mut candidates: Vec<&GroupMembership> = Vec::new();
    for runner in runners {
        if let Some(member) = runner.group_member()
            && !groups.iter().any(|group| group.id == member.group.id)
            && !candidates.iter().any(|group| group.id == member.group.id)
        {
            candidates.push(&member.group);
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
        let plans: Vec<&CornerPlan> = indexes
            .iter()
            .filter_map(|index| runners[*index].group_member().map(|member| &member.plan))
            .collect();
        let configs: Vec<&ExecutorConfig> =
            indexes.iter().map(|index| runners[*index].config).collect();
        if let Ok(group) = ActiveGroup::form(&membership.id, indexes, &plans, &configs) {
            groups.push(group);
        }
        // A group that cannot form (its plans were validated at admission, so
        // not expected) never gets a reference: its arms hold, and the first
        // arm to end takes the rest with it.
    }
}

/// Step every group once, from this cycle's readings, into one share per arm,
/// left in `steps`; with no group nothing is allocated.
pub(super) fn step_groups<P: Plant>(
    runners: &[Runner<'_, P>],
    groups: &mut [ActiveGroup],
    begun: &[Option<PendingCycle>],
    steps: &mut Vec<Option<Result<GroupReference, Trip>>>,
) {
    steps.clear();
    steps.extend((0..runners.len()).map(|_| None));
    for group in groups {
        // A member that did not read this cycle (its plant failed) cannot be
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

/// When one arm of a group has ended without completing, stop the others where
/// they are; forget groups no arm is following any more.
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
