//! Explicit motion units for callers constructing native station values.

use hebbian_contracts::{DEGREES_PER_RADIAN, METRES_PER_MILLIMETRE, RADIANS_PER_DEGREE};

/// Units accepted when constructing a Cartesian position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LengthUnit {
    Metres,
    Millimetres,
}

impl LengthUnit {
    /// Convert a world position to the metres expected by station motion commands.
    #[must_use]
    pub fn position_metres(self, position: [f64; 3]) -> [f64; 3] {
        match self {
            Self::Metres => position,
            Self::Millimetres => position.map(|coordinate| coordinate * METRES_PER_MILLIMETRE),
        }
    }
}

/// Units for joint angles, jaw yaw, and the angular part of a rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleUnit {
    Radians,
    Degrees,
}

impl AngleUnit {
    /// Convert an angle or angular rate to radians, retaining its time denominator.
    #[must_use]
    pub fn to_radians(self, value: f64) -> f64 {
        match self {
            Self::Radians => value,
            Self::Degrees => value * RADIANS_PER_DEGREE,
        }
    }

    /// Convert an angle or angular rate to degrees, retaining its time denominator.
    #[must_use]
    pub fn to_degrees(self, value: f64) -> f64 {
        match self {
            Self::Radians => value * DEGREES_PER_RADIAN,
            Self::Degrees => value,
        }
    }

    /// Convert a joint target to radians in the supplied joint order.
    #[must_use]
    pub fn joint_radians(self, mut joints: Vec<f64>) -> Vec<f64> {
        if self == Self::Degrees {
            for angle in &mut joints {
                *angle = self.to_radians(*angle);
            }
        }
        joints
    }
}
