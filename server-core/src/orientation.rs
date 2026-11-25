use std::f64::consts::TAU;

// CONSTRAINT: Tool +Z points palm-to-fingertips. Jaw yaw turns right-handed about minus approach; zero projects world +X, switching to +Y near parallel. See the public orientation contract.

use hebbian_contracts::{
    Approach, GoalComparison, JawClosingAxis, JawYawReference, NamedApproach, OrientationSummary,
    PoseOrientation, ToolOrientation,
};
use nalgebra::{Matrix3, Quaternion, Rotation3, Unit, UnitQuaternion, Vector3};

pub use hebbian_contracts::JAW_REFERENCE_SWITCH_APPROACH_X;
pub use hebbian_robot_geometry::kinematics::UNIT_QUATERNION_TOLERANCE;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationRefusal {
    ZeroApproach,
    NonFiniteYaw,
    NotAUnitQuaternion,
}

impl std::fmt::Display for OrientationRefusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroApproach => {
                write!(
                    formatter,
                    "The approach must be a finite, nonzero world direction"
                )
            }
            Self::NonFiniteYaw => write!(formatter, "The jaw yaw must be finite"),
            Self::NotAUnitQuaternion => write!(formatter, "The quaternion must be normalized"),
        }
    }
}

impl std::error::Error for OrientationRefusal {}

fn approach_direction(approach: Approach) -> Result<Unit<Vector3<f64>>, OrientationRefusal> {
    match approach {
        Approach::Named(NamedApproach::Down) => Ok(-Vector3::z_axis()),
        Approach::Named(NamedApproach::Forward) => Ok(Vector3::x_axis()),
        Approach::Direction(direction) => {
            let direction = Vector3::from(direction);
            // WHY: Scaling by the largest component first keeps extreme but finite
            // inputs from overflowing or underflowing the norm.
            let largest = direction.amax();
            if !largest.is_finite() || largest == 0.0 {
                return Err(OrientationRefusal::ZeroApproach);
            }
            Ok(Unit::new_normalize(direction / largest))
        }
    }
}

fn jaw_reference(direction: &Vector3<f64>) -> (JawYawReference, Vector3<f64>) {
    let (name, axis) = if direction.x.abs() > JAW_REFERENCE_SWITCH_APPROACH_X {
        (JawYawReference::WorldY, Vector3::y())
    } else {
        (JawYawReference::WorldX, Vector3::x())
    };
    let projected = axis - direction * axis.dot(direction);
    (name, projected.normalize())
}

fn closing_column(rotation: &Matrix3<f64>, axis: JawClosingAxis) -> Vector3<f64> {
    rotation
        .column(match axis {
            JawClosingAxis::X => 0,
            JawClosingAxis::Y => 1,
        })
        .into_owned()
}

fn jaw_yaw_radians(rotation: &Matrix3<f64>, axis: JawClosingAxis) -> f64 {
    let direction = rotation.column(2).into_owned();
    let (_, reference) = jaw_reference(&direction);
    let closing = closing_column(rotation, axis);
    closing
        .dot(&reference.cross(&direction))
        .atan2(closing.dot(&reference))
}

pub fn grasp_rotation(
    approach: Approach,
    jaw_yaw_radians: f64,
    axis: JawClosingAxis,
) -> Result<UnitQuaternion<f64>, OrientationRefusal> {
    if !jaw_yaw_radians.is_finite() {
        return Err(OrientationRefusal::NonFiniteYaw);
    }
    let direction = approach_direction(approach)?.into_inner();
    let (_, reference) = jaw_reference(&direction);
    let yaw = jaw_yaw_radians.rem_euclid(TAU);
    let closing = reference * yaw.cos() + reference.cross(&direction) * yaw.sin();
    let columns = match axis {
        JawClosingAxis::X => [closing, direction.cross(&closing), direction],
        JawClosingAxis::Y => [closing.cross(&direction), closing, direction],
    };
    Ok(UnitQuaternion::from_rotation_matrix(
        &Rotation3::from_matrix_unchecked(Matrix3::from_columns(&columns)),
    ))
}

pub fn quaternion_rotation(
    quaternion: [f64; 4],
) -> Result<UnitQuaternion<f64>, OrientationRefusal> {
    let [x, y, z, w] = quaternion;
    let raw = Quaternion::new(w, x, y, z);
    if !quaternion.iter().all(|value| value.is_finite())
        || (raw.norm() - 1.0).abs() > UNIT_QUATERNION_TOLERANCE
    {
        return Err(OrientationRefusal::NotAUnitQuaternion);
    }
    Ok(UnitQuaternion::new_normalize(raw))
}

pub fn pose_orientation_rotation(
    orientation: PoseOrientation,
    axis: JawClosingAxis,
) -> Result<UnitQuaternion<f64>, OrientationRefusal> {
    match orientation {
        PoseOrientation::Grasp {
            approach,
            jaw_yaw_radians,
        } => grasp_rotation(approach, jaw_yaw_radians, axis),
        PoseOrientation::Quaternion { quaternion } => quaternion_rotation(quaternion),
    }
}

// INVARIANT: Return client-supplied quaternions unchanged rather than introducing rounding through a rotation conversion.
pub fn pose_orientation_quaternion(
    orientation: PoseOrientation,
    axis: JawClosingAxis,
) -> Result<[f64; 4], OrientationRefusal> {
    match orientation {
        PoseOrientation::Quaternion { quaternion } => {
            quaternion_rotation(quaternion).map(|_| quaternion)
        }
        PoseOrientation::Grasp { .. } => {
            pose_orientation_rotation(orientation, axis).map(|rotation| xyzw(&rotation))
        }
    }
}

#[must_use]
pub fn from_xyzw(quaternion: [f64; 4]) -> UnitQuaternion<f64> {
    let [x, y, z, w] = quaternion;
    UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z))
}

#[must_use]
pub fn xyzw(rotation: &UnitQuaternion<f64>) -> [f64; 4] {
    [rotation.i, rotation.j, rotation.k, rotation.w]
}

#[must_use]
pub fn tool_orientation(rotation: &UnitQuaternion<f64>, axis: JawClosingAxis) -> ToolOrientation {
    let matrix = rotation.to_rotation_matrix().into_inner();
    let yaw = wrap_yaw(jaw_yaw_radians(&matrix, axis));
    ToolOrientation {
        approach: matrix.column(2).into_owned().into(),
        jaw_yaw_radians: yaw,
    }
}

#[must_use]
pub fn rounded(value: f64, digits: i32) -> f64 {
    let scale = 10_f64.powi(digits);
    (value * scale).round() / scale
}

fn wrap_yaw(radians: f64) -> f64 {
    let wrapped = radians.rem_euclid(TAU);
    // CONSTRAINT: Floating point wrap can return a full turn for a tiny negative yaw.
    if wrapped >= TAU { 0.0 } else { wrapped }
}

fn angle_between(first: &Vector3<f64>, second: &Vector3<f64>) -> f64 {
    first.cross(second).norm().atan2(first.dot(second))
}

#[must_use]
pub fn orientation_summary(
    rotation: &UnitQuaternion<f64>,
    axis: JawClosingAxis,
    goal: Option<&UnitQuaternion<f64>>,
) -> OrientationSummary {
    let matrix = rotation.to_rotation_matrix().into_inner();
    let direction = matrix.column(2).into_owned();
    let (reference, _) = jaw_reference(&direction);
    OrientationSummary {
        approach: direction.map(|value| rounded(value, 4)).into(),
        radians_from_down: angle_between(&direction, &-Vector3::z()),
        jaw_yaw_radians: wrap_yaw(jaw_yaw_radians(&matrix, axis)),
        jaw_yaw_reference: reference,
        goal: goal.map(|goal| {
            let goal_matrix = goal.to_rotation_matrix().into_inner();
            let goal_direction = goal_matrix.column(2).into_owned();
            GoalComparison {
                jaw_yaw_radians: wrap_yaw(jaw_yaw_radians(&goal_matrix, axis)),
                jaw_yaw_reference: jaw_reference(&goal_direction).0,
                approach_error_radians: angle_between(&direction, &goal_direction),
                orientation_error_radians: rotation.angle_to(goal),
            }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closing_axis(rotation: &UnitQuaternion<f64>, axis: JawClosingAxis) -> Vector3<f64> {
        closing_column(&rotation.to_rotation_matrix().into_inner(), axis)
    }

    #[test]
    fn down_closes_along_world_x_at_zero_and_world_y_at_ninety() {
        for axis in [JawClosingAxis::X, JawClosingAxis::Y] {
            let down = Approach::Named(NamedApproach::Down);
            let at_zero = grasp_rotation(down, 0.0, axis).unwrap();
            let at_ninety = grasp_rotation(down, 90_f64.to_radians(), axis).unwrap();
            assert!((closing_axis(&at_zero, axis) - Vector3::x()).norm() < 1e-12);
            assert!((closing_axis(&at_ninety, axis) - Vector3::y()).norm() < 1e-12);
            assert!((at_zero * Vector3::z() + Vector3::z()).norm() < 1e-12);
        }
    }

    #[test]
    fn examples_point_the_fingers_and_close_the_jaws_in_documented_directions() {
        let y_closing = [
            (
                Approach::Named(NamedApproach::Down),
                0.0,
                [0.0, 0.0, -1.0],
                [1.0, 0.0, 0.0],
            ),
            (
                Approach::Named(NamedApproach::Down),
                90_f64.to_radians(),
                [0.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
            ),
            (
                Approach::Named(NamedApproach::Forward),
                0.0,
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
            ),
            (
                Approach::Named(NamedApproach::Forward),
                180_f64.to_radians(),
                [1.0, 0.0, 0.0],
                [0.0, -1.0, 0.0],
            ),
            (
                Approach::Direction([0.0, 0.0, -20.0]),
                450_f64.to_radians(),
                [0.0, 0.0, -1.0],
                [0.0, 1.0, 0.0],
            ),
            (
                Approach::Direction([0.0, -1.0, 0.0]),
                0.0,
                [0.0, -1.0, 0.0],
                [1.0, 0.0, 0.0],
            ),
            (
                Approach::Direction([1e300, 0.0, 0.0]),
                0.0,
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
            ),
        ];
        for (approach, yaw, pointing, closing) in y_closing {
            let rotation = grasp_rotation(approach, yaw, JawClosingAxis::Y).unwrap();
            assert!((rotation * Vector3::z() - Vector3::from(pointing)).norm() < 1e-12);
            assert!(
                (closing_axis(&rotation, JawClosingAxis::Y) - Vector3::from(closing)).norm()
                    < 1e-12,
                "{approach:?} at {yaw}"
            );
        }
        let forward = grasp_rotation(
            Approach::Named(NamedApproach::Forward),
            90_f64.to_radians(),
            JawClosingAxis::X,
        )
        .unwrap();
        assert!((closing_axis(&forward, JawClosingAxis::X) + Vector3::z()).norm() < 1e-12);
    }

    #[test]
    fn a_summary_tells_a_tilt_from_a_jaw_yaw() {
        // CONSTRAINT: Down at 0, yawed 87 degrees about world z, then tilted 12 about y.
        let down =
            grasp_rotation(Approach::Named(NamedApproach::Down), 0.0, JawClosingAxis::Y).unwrap();
        let rotation = UnitQuaternion::from_euler_angles(0.0, 12_f64.to_radians(), 0.0)
            * UnitQuaternion::from_euler_angles(0.0, 0.0, 87_f64.to_radians())
            * down;
        let target = grasp_rotation(
            Approach::Named(NamedApproach::Down),
            90_f64.to_radians(),
            JawClosingAxis::Y,
        )
        .unwrap();
        let summary = orientation_summary(&rotation, JawClosingAxis::Y, Some(&target));
        assert!((summary.radians_from_down - 12_f64.to_radians()).abs() < 1e-9);
        assert!((summary.jaw_yaw_radians - 87_f64.to_radians()).abs() < 1e-9);
        let comparison = summary.goal.unwrap();
        assert!((comparison.jaw_yaw_radians - 90_f64.to_radians()).abs() < 1e-9);
        assert!((comparison.approach_error_radians - 12_f64.to_radians()).abs() < 1e-9);
        assert!(comparison.orientation_error_radians > 12_f64.to_radians());
    }

    #[test]
    fn forward_measures_yaw_from_world_y() {
        let forward = grasp_rotation(
            Approach::Named(NamedApproach::Forward),
            0.0,
            JawClosingAxis::X,
        )
        .unwrap();
        assert!((closing_axis(&forward, JawClosingAxis::X) - Vector3::y()).norm() < 1e-12);
        let summary = orientation_summary(&forward, JawClosingAxis::X, None);
        assert_eq!(summary.jaw_yaw_reference, JawYawReference::WorldY);
        assert!((summary.radians_from_down - 90_f64.to_radians()).abs() < 1e-9);
    }

    #[test]
    fn an_orientation_read_back_reproduces_its_rotation() {
        for axis in [JawClosingAxis::X, JawClosingAxis::Y] {
            for (approach, yaw_degrees) in [
                (Approach::Direction([0.3, -0.2, -0.9]), 37.5),
                (Approach::Direction([0.99, 0.05, 0.1]), 300.0),
                (Approach::Named(NamedApproach::Down), -45.0),
            ] {
                let rotation =
                    grasp_rotation(approach, f64::to_radians(yaw_degrees), axis).unwrap();
                let read = tool_orientation(&rotation, axis);
                assert!((0.0..TAU).contains(&read.jaw_yaw_radians));
                let again = grasp_rotation(
                    Approach::Direction(read.approach),
                    read.jaw_yaw_radians,
                    axis,
                )
                .unwrap();
                assert!(rotation.angle_to(&again) < 1e-9);
            }
        }
    }

    #[test]
    fn rounded_preview_quaternions_are_normalized_for_motion() {
        for quaternion in [
            [-0.9099, 0.2167, -0.2759, 0.2215],
            [0.5000, 0.5000, 0.5000, 0.5001],
        ] {
            let rotation = quaternion_rotation(quaternion).unwrap();
            assert!((rotation.quaternion().norm() - 1.0).abs() < 1e-12);
        }
        for quaternion in [
            [0.0; 4],
            [0.0, 0.0, 0.0, 1.001],
            [f64::NAN, 0.0, 0.0, 1.0],
            [f64::INFINITY, 0.0, 0.0, 1.0],
        ] {
            assert_eq!(
                quaternion_rotation(quaternion),
                Err(OrientationRefusal::NotAUnitQuaternion)
            );
        }
    }

    #[test]
    fn a_zero_approach_and_a_long_quaternion_are_refused() {
        for yaw in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                grasp_rotation(Approach::Named(NamedApproach::Down), yaw, JawClosingAxis::X),
                Err(OrientationRefusal::NonFiniteYaw)
            );
        }
        assert_eq!(
            grasp_rotation(Approach::Direction([0.0; 3]), 0.0, JawClosingAxis::X),
            Err(OrientationRefusal::ZeroApproach)
        );
        assert_eq!(
            quaternion_rotation([0.0, 0.0, 0.0, 1.1]),
            Err(OrientationRefusal::NotAUnitQuaternion)
        );
    }

    #[test]
    fn a_summary_against_its_target_reports_the_turn_between() {
        let down = Approach::Named(NamedApproach::Down);
        let achieved = grasp_rotation(down, 10_f64.to_radians(), JawClosingAxis::X).unwrap();
        let target = grasp_rotation(down, 40_f64.to_radians(), JawClosingAxis::X).unwrap();
        let summary = orientation_summary(&achieved, JawClosingAxis::X, Some(&target));
        let comparison = summary.goal.unwrap();
        assert!((summary.jaw_yaw_radians - 10_f64.to_radians()).abs() < 1e-9);
        assert!((comparison.jaw_yaw_radians - 40_f64.to_radians()).abs() < 1e-9);
        assert!(comparison.approach_error_radians.abs() < 1e-9);
        assert!((comparison.orientation_error_radians - 30_f64.to_radians()).abs() < 1e-9);
    }
}
