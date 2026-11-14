//! When a pose is out of reach, what would be, in the terms a pose takes.

use nalgebra::Vector3;

use super::Planner;
use super::orientation::{from_xyzw, grasp_rotation, rounded, tool_orientation, xyzw};
use hebbian_contracts::{Approach, IkResult, JawYawRange, OrientationGoal, ProjectionOutcome};

/// How far inside a reachable run's edge a suggested jaw yaw sits, so it does
/// not land on the wrist roll's limit itself.
const SUGGESTION_EDGE_INSET_DEGREES: f64 = 1.0;

fn circular_degrees_between(first: f64, second: f64) -> f64 {
    let gap = (first - second).abs().rem_euclid(360.0);
    gap.min(360.0 - gap)
}

/// One candidate yaw per reachable run, the run's point nearest `wanted`, nearest first.
fn nearest_reachable_yaws(ranges: &[JawYawRange], wanted: f64) -> Vec<f64> {
    let mut candidates: Vec<f64> = ranges
        .iter()
        .filter(|span| span.reachable)
        .map(|span| {
            let inset =
                SUGGESTION_EDGE_INSET_DEGREES.min((span.to_degrees - span.from_degrees) / 2.0);
            let (low, high) = (span.from_degrees + inset, span.to_degrees - inset);
            let nearest = if (low..=high).contains(&wanted) {
                wanted
            } else if circular_degrees_between(low, wanted)
                <= circular_degrees_between(high, wanted)
            {
                low
            } else {
                high
            };
            rounded(nearest.rem_euclid(360.0), 1)
        })
        .collect();
    candidates.sort_by(|a, b| {
        circular_degrees_between(*a, wanted).total_cmp(&circular_degrees_between(*b, wanted))
    });
    candidates
}

/// A number as JSON writes it, with a decimal point on whole numbers, so a
/// suggestion reads the same as the floats a client sends.
fn json_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

/// The orientation a client sends back, as JSON.
fn orientation_json(approach: [f64; 3], yaw: f64) -> String {
    format!(
        "\"orientation\": {{\"approach\": [{}], \"jaw_yaw_degrees\": {}}}",
        approach.map(json_number).join(", "),
        json_number(yaw)
    )
}

impl Planner {
    /// Joints that reach the full pose from `seed`, if the solver finds them.
    fn joints_reaching(
        &self,
        seed: &[f64],
        position: [f64; 3],
        quaternion: [f64; 4],
    ) -> Option<Vec<f64>> {
        match self
            .solver
            .solve(position, OrientationGoal::Required { quaternion }, seed)
        {
            Ok(IkResult::Solved(pose)) => Some(pose.joints),
            _ => None,
        }
    }

    /// Why a pose was not reachable and what would be.
    ///
    /// First the same position and approach at the nearest jaw yaw the wrist
    /// roll's range allows, then the orientation a position-only solve landed
    /// on, else that the position is out of reach. Every suggestion is solved
    /// again before it is offered, so sending it back is accepted from this start.
    pub(super) fn unreachable_advice(
        &self,
        start: &[f64],
        position: [f64; 3],
        quaternion: [f64; 4],
    ) -> String {
        let axis = self.jaw_axis;
        let wanted = tool_orientation(&from_xyzw(quaternion), axis);
        let approach = wanted.approach.map(|value| rounded(value, 4));
        let suggestion = |approach: [f64; 3], yaw: f64| -> Option<String> {
            let rotation = grasp_rotation(Approach::Direction(approach), yaw, axis).ok()?;
            self.joints_reaching(start, position, xyzw(&rotation))?;
            Some(orientation_json(approach, yaw))
        };
        // The ranges depend on position and approach only, so any yaw that solves
        // for this approach gives them; the tilt the solve settles at shifts them a
        // little, so each anchor is tried until one suggestion verifies.
        for turn in [180.0, 90.0, 270.0] {
            let anchor_yaw = (wanted.jaw_yaw_degrees + turn).rem_euclid(360.0);
            let Some(anchor) = grasp_rotation(Approach::Direction(approach), anchor_yaw, axis)
                .ok()
                .and_then(|rotation| self.joints_reaching(start, position, xyzw(&rotation)))
            else {
                continue;
            };
            for yaw in nearest_reachable_yaws(&self.jaw_yaw_ranges(&anchor), wanted.jaw_yaw_degrees)
            {
                if let Some(found) = suggestion(approach, yaw) {
                    return format!(
                        "jaw_yaw_degrees {:.0} is not reachable with this approach at this \
                         position; the same approach works at the nearest reachable yaw. Try {found}",
                        wanted.jaw_yaw_degrees
                    );
                }
            }
        }
        match self.solve_toward(start, position, None) {
            ProjectionOutcome::Projected {
                pose,
                within_tolerance: true,
                ..
            } => {
                let chosen = tool_orientation(&from_xyzw(pose.quaternion), axis);
                if let Some(found) = suggestion(
                    chosen.approach.map(|value| rounded(value, 4)),
                    rounded(chosen.jaw_yaw_degrees, 1),
                ) {
                    return format!(
                        "No jaw yaw reaches this position with the requested approach; the \
                         solver reached it with another approach. Try {found}"
                    );
                }
                "The position is out of reach for any orientation".to_owned()
            }
            ProjectionOutcome::Projected {
                pose,
                within_tolerance: false,
                ..
            } => format!(
                "The position is out of reach for any orientation; the nearest the arm gets \
                 is [{}] ({:.1} cm away)",
                pose.position
                    .map(|value| json_number(rounded(value, 3)))
                    .join(", "),
                (Vector3::from(pose.position) - Vector3::from(position)).norm() * 100.0
            ),
            ProjectionOutcome::Unavailable { .. } => {
                "The position is out of reach for any orientation".to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(from_degrees: f64, to_degrees: f64, reachable: bool) -> JawYawRange {
        JawYawRange {
            from_degrees,
            to_degrees,
            reachable,
            reason: None,
        }
    }

    #[test]
    fn a_suggested_yaw_is_the_nearest_point_inside_each_reachable_run() {
        let ranges = [
            span(0.0, 100.0, true),
            span(100.0, 200.0, false),
            span(200.0, 360.0, true),
        ];
        // Inside a run, the wanted yaw itself.
        assert_eq!(nearest_reachable_yaws(&ranges, 50.0)[0], 50.0);
        // In the blocked run, the nearer edge of each run, inset from the limit.
        assert_eq!(nearest_reachable_yaws(&ranges, 120.0), vec![99.0, 201.0]);
    }

    #[test]
    fn a_suggestion_writes_whole_numbers_with_a_decimal_point() {
        assert_eq!(
            orientation_json([0.0, 0.0, -1.0], 90.0),
            "\"orientation\": {\"approach\": [0.0, 0.0, -1.0], \"jaw_yaw_degrees\": 90.0}"
        );
    }
}
