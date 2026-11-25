use std::f64::consts::{FRAC_PI_2, PI, TAU};

use nalgebra::Vector3;

use super::Planner;
use super::orientation::{from_xyzw, grasp_rotation, rounded, tool_orientation, xyzw};
use hebbian_contracts::{Approach, IkResult, JawYawRange, OrientationGoal, ProjectionOutcome};

// WHY: Keep suggested yaw inside the reachable arc so rounding does not place it on the wrist-roll limit.
const SUGGESTION_EDGE_INSET_RADIANS: f64 = PI / 180.0;

fn circular_angle_between(first: f64, second: f64) -> f64 {
    let gap = (first - second).abs().rem_euclid(TAU);
    gap.min(TAU - gap)
}

fn nearest_reachable_yaws(ranges: &[JawYawRange], wanted: f64) -> Vec<f64> {
    let mut candidates: Vec<f64> = ranges
        .iter()
        .filter(|span| span.reachable)
        .map(|span| {
            let inset =
                SUGGESTION_EDGE_INSET_RADIANS.min((span.to_radians - span.from_radians) / 2.0);
            let (low, high) = (span.from_radians + inset, span.to_radians - inset);
            let nearest = if (low..=high).contains(&wanted) {
                wanted
            } else if circular_angle_between(low, wanted) <= circular_angle_between(high, wanted) {
                low
            } else {
                high
            };
            rounded_model_yaw(nearest)
        })
        .collect();
    candidates.sort_by(|a, b| {
        circular_angle_between(*a, wanted).total_cmp(&circular_angle_between(*b, wanted))
    });
    candidates
}

fn json_number(value: f64) -> String {
    if value.is_finite() && value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        format!("{value}")
    }
}

fn projected_position_advice(found: [f64; 3], requested: [f64; 3]) -> String {
    format!(
        "The solver did not find a pose reaching this position; the closest position it found \
         with an unconstrained orientation is position_mm=[{}] ({:.1} mm away)",
        found
            .map(|value| json_number(rounded(value * 1000.0, 1)))
            .join(", "),
        (Vector3::from(found) - Vector3::from(requested)).norm() * 1000.0
    )
}

// WHY: Verify the same tenth-of-a-degree suggestion the model will receive.
fn rounded_model_yaw(radians: f64) -> f64 {
    rounded(radians.rem_euclid(TAU).to_degrees(), 1)
        .rem_euclid(360.0)
        .to_radians()
}

fn orientation_json(approach: [f64; 3], yaw: f64) -> String {
    format!(
        "\"orientation\": {{\"approach\": [{}], \"jaw_yaw_degrees\": {}}}",
        approach.map(json_number).join(", "),
        json_number(rounded(yaw.to_degrees(), 1))
    )
}

impl Planner {
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

    // INVARIANT: Solve every suggested alternative again before offering it so the client can submit it from this start.
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
        // WHY: The ranges depend on position and approach only, so any yaw that solves
        // for this approach gives them; the tilt the solve settles at shifts them a
        // little, so each anchor is tried until one suggestion verifies.
        for turn in [PI, FRAC_PI_2, PI + FRAC_PI_2] {
            let anchor_yaw = (wanted.jaw_yaw_radians + turn).rem_euclid(TAU);
            let Some(anchor) = grasp_rotation(Approach::Direction(approach), anchor_yaw, axis)
                .ok()
                .and_then(|rotation| self.joints_reaching(start, position, xyzw(&rotation)))
            else {
                continue;
            };
            for yaw in nearest_reachable_yaws(&self.jaw_yaw_ranges(&anchor), wanted.jaw_yaw_radians)
            {
                if let Some(found) = suggestion(approach, yaw) {
                    return format!(
                        "jaw_yaw_degrees {:.0} is not reachable with this approach at this \
                         position; the same approach works at the nearest reachable yaw. Try {found}",
                        wanted.jaw_yaw_radians.to_degrees()
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
                    rounded_model_yaw(chosen.jaw_yaw_radians),
                ) {
                    return format!(
                        "The solver did not find a reachable jaw yaw with the requested approach; it \
                         found this alternative approach. Try {found}"
                    );
                }
                "The solver did not find a pose reaching this position with the orientations it tried".to_owned()
            }
            ProjectionOutcome::Projected {
                pose,
                within_tolerance: false,
                ..
            } => projected_position_advice(pose.position, position),
            ProjectionOutcome::Unavailable { .. } => {
                "The solver did not find a pose reaching this position with the orientations it tried".to_owned()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(from_degrees: f64, to_degrees: f64, reachable: bool) -> JawYawRange {
        JawYawRange {
            from_radians: from_degrees.to_radians(),
            to_radians: to_degrees.to_radians(),
            reachable,
            reason: None,
        }
    }

    #[test]
    fn projected_position_advice_uses_explicit_millimeters() {
        let advice = projected_position_advice([0.134, 0.011, 0.083], [0.127, 0.010, 0.087]);
        assert!(advice.contains("position_mm=[134.0, 11.0, 83.0]"));
        assert!(advice.contains("8.1 mm away"));
    }

    #[test]
    fn a_suggested_yaw_is_the_nearest_point_inside_each_reachable_run() {
        let ranges = [
            span(0.0, 100.0, true),
            span(100.0, 200.0, false),
            span(200.0, 360.0, true),
        ];
        let already_reachable = nearest_reachable_yaws(&ranges, 50_f64.to_radians());
        assert!((already_reachable[0].to_degrees() - 50.0).abs() < 1e-9);
        assert_eq!(
            nearest_reachable_yaws(&ranges, 120_f64.to_radians())
                .into_iter()
                .map(|yaw| rounded(yaw.to_degrees(), 1))
                .collect::<Vec<_>>(),
            vec![99.0, 201.0]
        );
    }

    #[test]
    fn a_suggestion_writes_whole_numbers_with_a_decimal_point() {
        assert_eq!(
            orientation_json([0.0, 0.0, -1.0], FRAC_PI_2),
            "\"orientation\": {\"approach\": [0.0, 0.0, -1.0], \"jaw_yaw_degrees\": 90.0}"
        );
    }
}
