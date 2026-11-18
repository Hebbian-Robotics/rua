use std::time::Instant;

use nalgebra::{UnitQuaternion, Vector3};

use super::Planner;
use super::orientation::{from_xyzw, pose_orientation_rotation, tool_orientation, xyzw};
use crate::preview::{PreviewOutcome, ReachPreview};
use crate::robot::PathCheck;
use hebbian_contracts::{
    JawClosingAxis, JawYawRange, PoseOrientation, PoseProjection, ProjectedPose, ProjectionOutcome,
    RequestedOrientation,
};

fn pieces_in_full_turn(start: f64, end: f64) -> Vec<(f64, f64)> {
    let (start, end) = if start >= 360.0 {
        (start - 360.0, end - 360.0)
    } else {
        (start, end)
    };
    let pieces = if end <= 360.0 {
        vec![(start, end)]
    } else {
        vec![(start, 360.0), (0.0, end - 360.0)]
    };
    pieces.into_iter().filter(|(from, to)| to > from).collect()
}

impl Planner {
    fn requested(
        &self,
        orientation: Option<PoseOrientation>,
    ) -> Result<Option<UnitQuaternion<f64>>, String> {
        orientation
            .map(|orientation| pose_orientation_rotation(orientation, self.jaw_axis))
            .transpose()
            .map_err(|why| why.to_string())
    }

    fn described(&self, rotation: &UnitQuaternion<f64>) -> RequestedOrientation {
        RequestedOrientation {
            orientation: tool_orientation(rotation, self.jaw_axis),
            quaternion: xyzw(rotation),
        }
    }

    // CONSTRAINT: Only wrist roll changes jaw yaw: its limit arc uses positive roll toward positive yaw while other joints stay fixed.
    pub(super) fn jaw_yaw_ranges(&self, joints: &[f64]) -> Vec<JawYawRange> {
        let Some(roll) = self.wrist_roll else {
            return Vec::new();
        };
        let (lower, upper) = (self.lower[roll], self.upper[roll]);
        let mut at_lower = joints.to_vec();
        at_lower[roll] = lower;
        let start = tool_orientation(&self.tool_rotation(&at_lower), self.jaw_axis).jaw_yaw_degrees;
        let arc = (upper - lower).to_degrees().min(360.0);
        let reason = format!(
            "Needs a wrist roll outside its calibrated range of {:.0} to {:.0} degrees",
            lower.to_degrees(),
            upper.to_degrees()
        );
        let mut arcs = vec![(start, start + arc, true)];
        if arc < 360.0 {
            arcs.push((start + arc, start + 360.0, false));
        }
        let mut ranges: Vec<JawYawRange> = arcs
            .into_iter()
            .flat_map(|(from, to, reachable)| {
                pieces_in_full_turn(from, to)
                    .into_iter()
                    .map(move |(from_degrees, to_degrees)| (from_degrees, to_degrees, reachable))
            })
            .map(|(from_degrees, to_degrees, reachable)| JawYawRange {
                from_degrees,
                to_degrees,
                reachable,
                reason: (!reachable).then(|| reason.clone()),
            })
            .collect();
        ranges.sort_by(|a, b| a.from_degrees.total_cmp(&b.from_degrees));
        ranges
    }

    #[must_use]
    pub fn project(
        &self,
        joints: &[f64],
        position: [f64; 3],
        orientation: Option<PoseOrientation>,
        gripper_angle: Option<f64>,
    ) -> PoseProjection {
        let started = Instant::now();
        let finish = |outcome, requested: Option<RequestedOrientation>, ranges| PoseProjection {
            outcome,
            requested,
            jaw_yaw_ranges: ranges,
            calculation_milliseconds: started.elapsed().as_secs_f64() * 1000.0,
        };
        let requested = match self.requested(orientation) {
            Ok(requested) => requested,
            Err(reason) => {
                return finish(ProjectionOutcome::Unavailable { reason }, None, Vec::new());
            }
        };
        let described = requested.as_ref().map(|rotation| self.described(rotation));
        let outcome = self.solve_toward(joints, position, requested.as_ref());
        let solved = match &outcome {
            ProjectionOutcome::Unavailable { .. } => return finish(outcome, described, Vec::new()),
            ProjectionOutcome::Projected { pose, .. } => pose.joints.clone(),
        };
        let ranges = self.jaw_yaw_ranges(&solved);
        let outcome = match self.path_check(joints, &solved, gripper_angle) {
            Ok(PathCheck::Clear) => outcome,
            Ok(PathCheck::StartRefused { reason }) => ProjectionOutcome::Unavailable {
                reason: format!(
                    "{reason}; the measured pose needs its geometry or calibration reviewed \
                     before the arm can move from it"
                ),
            },
            Ok(PathCheck::StopsAt { last_safe, reason }) => ProjectionOutcome::Projected {
                pose: self.projected(last_safe, position, requested.as_ref()),
                within_tolerance: false,
                reason: Some(format!(
                    "{reason}; limited to the last collision-free sample on the joint path"
                )),
            },
            Err(error) => ProjectionOutcome::Unavailable {
                reason: format!("{error:#}"),
            },
        };
        finish(outcome, described, ranges)
    }

    // INVARIANT: The ghost rehearses the same controller and goal as live motion. IK supplies yaw bounds and an orientation only when the goal leaves it free.
    pub fn project_by_policy(
        &self,
        joints: &[f64],
        position: [f64; 3],
        orientation: Option<PoseOrientation>,
        rehearse: impl FnOnce([f64; 3], [f64; 4]) -> Result<ReachPreview, String>,
    ) -> PoseProjection {
        let started = Instant::now();
        let finish = |outcome, requested, ranges| PoseProjection {
            outcome,
            requested,
            jaw_yaw_ranges: ranges,
            calculation_milliseconds: started.elapsed().as_secs_f64() * 1000.0,
        };
        let requested = match self.requested(orientation) {
            Ok(requested) => requested,
            Err(reason) => {
                return finish(ProjectionOutcome::Unavailable { reason }, None, Vec::new());
            }
        };
        // WHY: A requested orientation is the goal as given; only a free one needs the
        // solver to pick it. The jaw yaw ranges are then read where the policy
        // ends, which is the pose the ghost shows.
        let (goal, solved_joints) = match requested {
            Some(rotation) => (rotation, None),
            None => match self.solve_toward(joints, position, None) {
                ProjectionOutcome::Unavailable { .. } => (self.tool_rotation(joints), None),
                ProjectionOutcome::Projected { pose, .. } => {
                    (from_xyzw(pose.quaternion), Some(pose.joints))
                }
            },
        };
        let described = Some(self.described(&goal));
        let (outcome, ranges) = match rehearse(position, xyzw(&goal)) {
            Err(reason) => (ProjectionOutcome::Unavailable { reason }, Vec::new()),
            Ok(rehearsal) => {
                let ranges =
                    self.jaw_yaw_ranges(solved_joints.as_ref().unwrap_or(&rehearsal.joints));
                (policy_outcome(rehearsal, position, self.jaw_axis), ranges)
            }
        };
        finish(outcome, described, ranges)
    }
}

fn policy_outcome(
    rehearsal: ReachPreview,
    goal_position: [f64; 3],
    axis: JawClosingAxis,
) -> ProjectionOutcome {
    let position_error = (Vector3::from(rehearsal.position) - Vector3::from(goal_position)).norm();
    let orientation = tool_orientation(&from_xyzw(rehearsal.quaternion), axis);
    let pose = |rehearsal: ReachPreview| ProjectedPose {
        joints: rehearsal.joints,
        position: rehearsal.position,
        quaternion: rehearsal.quaternion,
        orientation,
        position_error_meters: position_error,
        orientation_error_radians: Some(rehearsal.orientation_error_radians),
    };
    match rehearsal.outcome {
        PreviewOutcome::Blocked => ProjectionOutcome::Unavailable {
            reason: rehearsal
                .reason
                .unwrap_or_else(|| "A guard stopped the policy".to_owned()),
        },
        PreviewOutcome::Settled => ProjectionOutcome::Projected {
            pose: pose(rehearsal),
            within_tolerance: true,
            reason: None,
        },
        PreviewOutcome::TimedOut => {
            let reason = format!(
                "{}; the policy would stop about {:.1} mm short of the goal, shown here",
                rehearsal
                    .reason
                    .as_deref()
                    .unwrap_or("The policy did not settle"),
                position_error * 1000.0
            );
            ProjectionOutcome::Projected {
                pose: pose(rehearsal),
                within_tolerance: false,
                reason: Some(reason),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::pieces_in_full_turn;

    #[test]
    fn an_arc_past_a_full_turn_continues_from_zero() {
        assert_eq!(
            pieces_in_full_turn(300.0, 420.0),
            vec![(300.0, 360.0), (0.0, 60.0)]
        );
        assert_eq!(pieces_in_full_turn(370.0, 400.0), vec![(10.0, 40.0)]);
        assert_eq!(pieces_in_full_turn(10.0, 10.0), Vec::new());
    }
}
