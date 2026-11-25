//! Shared scales between native station units and human-facing motion units.

/// Metres in one millimetre.
pub const METRES_PER_MILLIMETRE: f64 = 0.001;

/// Radians in one degree.
pub const RADIANS_PER_DEGREE: f64 = std::f64::consts::PI / 180.0;

/// Millimetres in one metre.
pub const MILLIMETRES_PER_METRE: f64 = 1.0 / METRES_PER_MILLIMETRE;

/// Degrees in one radian.
pub const DEGREES_PER_RADIAN: f64 = 1.0 / RADIANS_PER_DEGREE;
