//! Grasp orientations: the approach direction and jaw yaw a client names a tool
//! orientation by, and the rotation they stand for.
//!
//! Local +Z of every supported tool frame points from palm to fingertips. The
//! jaws close along the gripper's jaw closing axis, local X or Y. Jaw yaw is a
//! right-handed rotation about minus the approach direction; its zero is world
//! +X projected onto the plane across the approach, or world +Y when the
//! approach is within `JAW_REFERENCE_SWITCH_APPROACH_X` of world X. So down at 0
//! closes along world X and down at 90 along world Y.

use hebbian_contracts::{
    Approach, JawClosingAxis, JawYawReference, NamedApproach, OrientationSummary, PoseOrientation,
    TargetComparison, ToolOrientation,
};
use nalgebra::{Matrix3, Quaternion, Rotation3, Unit, UnitQuaternion, Vector3};

pub use hebbian_contracts::JAW_REFERENCE_SWITCH_APPROACH_X;
/// How far from unit length a client's quaternion may be.
pub const UNIT_QUATERNION_TOLERANCE: f64 = 1e-5;

/// Why an orientation cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrientationRefusal {
    ZeroApproach,
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
            // Scaling by the largest component first keeps extreme but finite
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

/// The jaw yaw of a tool rotation about minus its approach, in (-180, 180].
fn jaw_yaw_degrees(rotation: &Matrix3<f64>, axis: JawClosingAxis) -> f64 {
    let direction = rotation.column(2).into_owned();
    let (_, reference) = jaw_reference(&direction);
    let closing = closing_column(rotation, axis);
    closing
        .dot(&reference.cross(&direction))
        .atan2(closing.dot(&reference))
        .to_degrees()
}

/// The rotation for an approach and a jaw yaw.
///
/// # Errors
/// The approach is zero or not finite.
pub fn grasp_rotation(
    approach: Approach,
    jaw_yaw_degrees: f64,
    axis: JawClosingAxis,
) -> Result<UnitQuaternion<f64>, OrientationRefusal> {
    let direction = approach_direction(approach)?.into_inner();
    let (_, reference) = jaw_reference(&direction);
    let yaw = jaw_yaw_degrees.rem_euclid(360.0).to_radians();
    let closing = reference * yaw.cos() + reference.cross(&direction) * yaw.sin();
    let columns = match axis {
        JawClosingAxis::X => [closing, direction.cross(&closing), direction],
        JawClosingAxis::Y => [closing.cross(&direction), closing, direction],
    };
    Ok(UnitQuaternion::from_rotation_matrix(
        &Rotation3::from_matrix_unchecked(Matrix3::from_columns(&columns)),
    ))
}

/// The rotation a client's quaternion `[x, y, z, w]` stands for.
///
/// # Errors
/// It is not finite or not of unit length.
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

/// The rotation an orientation names for a gripper closing along `axis`.
///
/// # Errors
/// The approach is zero, or the quaternion is not a unit quaternion.
pub fn pose_orientation_rotation(
    orientation: PoseOrientation,
    axis: JawClosingAxis,
) -> Result<UnitQuaternion<f64>, OrientationRefusal> {
    match orientation {
        PoseOrientation::Grasp {
            approach,
            jaw_yaw_degrees,
        } => grasp_rotation(approach, jaw_yaw_degrees, axis),
        PoseOrientation::Quaternion { quaternion } => quaternion_rotation(quaternion),
    }
}

/// The quaternion `[x, y, z, w]`: as given when it is one, so a client's
/// own quaternion comes back unchanged.
///
/// # Errors
/// As for [`pose_orientation_rotation`].
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

/// The rotation of a quaternion `[x, y, z, w]` the executor computed itself,
/// normalized against rounding.
#[must_use]
pub fn from_xyzw(quaternion: [f64; 4]) -> UnitQuaternion<f64> {
    let [x, y, z, w] = quaternion;
    UnitQuaternion::new_normalize(Quaternion::new(w, x, y, z))
}

/// A rotation as `[x, y, z, w]`.
#[must_use]
pub fn xyzw(rotation: &UnitQuaternion<f64>) -> [f64; 4] {
    [rotation.i, rotation.j, rotation.k, rotation.w]
}

/// The approach and jaw yaw a rotation stands for, at full precision.
#[must_use]
pub fn tool_orientation(rotation: &UnitQuaternion<f64>, axis: JawClosingAxis) -> ToolOrientation {
    let matrix = rotation.to_rotation_matrix().into_inner();
    let yaw = jaw_yaw_degrees(&matrix, axis).rem_euclid(360.0);
    ToolOrientation {
        approach: matrix.column(2).into_owned().into(),
        // A negative yaw just under a full turn can come out as 360 itself.
        jaw_yaw_degrees: if yaw >= 360.0 { 0.0 } else { yaw },
    }
}

/// `value` rounded to `digits` decimal places.
#[must_use]
pub fn rounded(value: f64, digits: i32) -> f64 {
    let scale = 10_f64.powi(digits);
    (value * scale).round() / scale
}

fn rounded_in_full_turn(degrees: f64) -> f64 {
    // The second remainder maps a value that rounds up to 360 back to 0.
    rounded(degrees.rem_euclid(360.0), 1).rem_euclid(360.0)
}

fn degrees_between(first: &Vector3<f64>, second: &Vector3<f64>) -> f64 {
    first.dot(second).clamp(-1.0, 1.0).acos().to_degrees()
}

/// A rotation in grasp terms, compared with `target` when there is one.
#[must_use]
pub fn orientation_summary(
    rotation: &UnitQuaternion<f64>,
    axis: JawClosingAxis,
    target: Option<&UnitQuaternion<f64>>,
) -> OrientationSummary {
    let matrix = rotation.to_rotation_matrix().into_inner();
    let direction = matrix.column(2).into_owned();
    let (reference, _) = jaw_reference(&direction);
    OrientationSummary {
        approach: direction.map(|value| rounded(value, 4)).into(),
        degrees_from_down: rounded(degrees_between(&direction, &-Vector3::z()), 1),
        jaw_yaw_degrees: rounded_in_full_turn(jaw_yaw_degrees(&matrix, axis)),
        jaw_yaw_reference: reference,
        target: target.map(|target| {
            let target_matrix = target.to_rotation_matrix().into_inner();
            let target_direction = target_matrix.column(2).into_owned();
            TargetComparison {
                jaw_yaw_degrees: rounded_in_full_turn(jaw_yaw_degrees(&target_matrix, axis)),
                jaw_yaw_reference: jaw_reference(&target_direction).0,
                approach_error_degrees: rounded(degrees_between(&direction, &target_direction), 1),
                orientation_error_degrees: rounded(rotation.angle_to(target).to_degrees(), 1),
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
            let at_ninety = grasp_rotation(down, 90.0, axis).unwrap();
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
                90.0,
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
                180.0,
                [1.0, 0.0, 0.0],
                [0.0, -1.0, 0.0],
            ),
            (
                Approach::Direction([0.0, 0.0, -20.0]),
                450.0,
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
            90.0,
            JawClosingAxis::X,
        )
        .unwrap();
        assert!((closing_axis(&forward, JawClosingAxis::X) + Vector3::z()).norm() < 1e-12);
    }

    #[test]
    fn a_summary_tells_a_tilt_from_a_jaw_yaw() {
        // Down at 0, yawed 87 degrees about world z, then tilted 12 about y.
        let down =
            grasp_rotation(Approach::Named(NamedApproach::Down), 0.0, JawClosingAxis::Y).unwrap();
        let rotation = UnitQuaternion::from_euler_angles(0.0, 12_f64.to_radians(), 0.0)
            * UnitQuaternion::from_euler_angles(0.0, 0.0, 87_f64.to_radians())
            * down;
        let target = grasp_rotation(
            Approach::Named(NamedApproach::Down),
            90.0,
            JawClosingAxis::Y,
        )
        .unwrap();
        let summary = orientation_summary(&rotation, JawClosingAxis::Y, Some(&target));
        assert!((summary.degrees_from_down - 12.0).abs() < 1e-9);
        assert!((summary.jaw_yaw_degrees - 87.0).abs() < 1e-9);
        let comparison = summary.target.unwrap();
        assert!((comparison.jaw_yaw_degrees - 90.0).abs() < 1e-9);
        assert!((comparison.approach_error_degrees - 12.0).abs() < 1e-9);
        assert!(comparison.orientation_error_degrees > 12.0);
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
        assert!((summary.degrees_from_down - 90.0).abs() < 1e-9);
    }

    #[test]
    fn an_orientation_read_back_reproduces_its_rotation() {
        for axis in [JawClosingAxis::X, JawClosingAxis::Y] {
            for (approach, yaw) in [
                (Approach::Direction([0.3, -0.2, -0.9]), 37.5),
                (Approach::Direction([0.99, 0.05, 0.1]), 300.0),
                (Approach::Named(NamedApproach::Down), -45.0),
            ] {
                let rotation = grasp_rotation(approach, yaw, axis).unwrap();
                let read = tool_orientation(&rotation, axis);
                assert!((0.0..360.0).contains(&read.jaw_yaw_degrees));
                let again = grasp_rotation(
                    Approach::Direction(read.approach),
                    read.jaw_yaw_degrees,
                    axis,
                )
                .unwrap();
                assert!(rotation.angle_to(&again) < 1e-9);
            }
        }
    }

    #[test]
    fn a_zero_approach_and_a_long_quaternion_are_refused() {
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
        let achieved = grasp_rotation(down, 10.0, JawClosingAxis::X).unwrap();
        let target = grasp_rotation(down, 40.0, JawClosingAxis::X).unwrap();
        let summary = orientation_summary(&achieved, JawClosingAxis::X, Some(&target));
        let comparison = summary.target.unwrap();
        assert!((summary.jaw_yaw_degrees - 10.0).abs() < 1e-9);
        assert!((comparison.jaw_yaw_degrees - 40.0).abs() < 1e-9);
        assert!(comparison.approach_error_degrees.abs() < 1e-9);
        assert!((comparison.orientation_error_degrees - 30.0).abs() < 1e-9);
    }
}
