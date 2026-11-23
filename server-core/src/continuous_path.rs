use anyhow::{Result, ensure};
use hebbian_contracts::{CornerPlan, MotionLimits};

use crate::reference_filter::Reference;

struct Section {
    controls: Vec<[f64; 6]>,
}

pub(crate) struct ContinuousPath {
    sections: Vec<Section>,
    section_seconds: f64,
}

fn interpolate<const N: usize>(mut controls: [f64; N], fraction: f64) -> f64 {
    for remaining in (1..N).rev() {
        for index in 0..remaining {
            controls[index] += fraction * (controls[index + 1] - controls[index]);
        }
    }
    controls[0]
}

fn differences<const N: usize>(controls: &[f64]) -> [f64; N] {
    std::array::from_fn(|index| (controls[index + 1] - controls[index]) * crate::numeric::count(N))
}

fn tangent(previous: f64, next: f64) -> f64 {
    if previous.signum() == next.signum() {
        previous.signum() * previous.abs().min(next.abs())
    } else {
        0.0
    }
}

impl ContinuousPath {
    pub(crate) fn new(plan: &CornerPlan, limits: &MotionLimits) -> Result<Self> {
        ensure!(
            plan.is_continuous() && plan.segments.len() >= 2,
            "A continuous move needs one boundary per intermediate waypoint"
        );
        ensure!(
            plan.segments
                .iter()
                .all(|segment| segment.corners.len() == 2
                    && segment.minimum_duration_seconds.is_none()),
            "A continuous section needs two joint configurations"
        );
        let mut knots = vec![plan.segments[0].corners[0].as_slice()];
        knots.extend(
            plan.segments
                .iter()
                .map(|segment| segment.corners[1].as_slice()),
        );
        let mut sections = Vec::with_capacity(plan.segments.len());
        let mut section_seconds: f64 = 0.0;
        for (index, pair) in knots.windows(2).enumerate() {
            let mut controls = Vec::with_capacity(pair[0].len());
            let mut passes = false;
            for (joint, (&start, &end)) in pair[0].iter().zip(pair[1]).enumerate() {
                let delta = end - start;
                let entry = index.checked_sub(1).map_or(0.0, |previous| {
                    tangent(start - knots[previous][joint], delta)
                });
                let exit = knots
                    .get(index + 2)
                    .map_or(0.0, |next| tangent(delta, next[joint] - end));
                passes |= exit.abs() > 1e-12;
                // INVARIANT: Bernstein controls stay ordered within the joint interval. Shared
                // first differences set the knot velocity; second differences vanish there.
                let positions = [
                    start,
                    start + entry / 5.0,
                    start + 2.0 * entry / 5.0,
                    end - 2.0 * exit / 5.0,
                    end - exit / 5.0,
                    end,
                ];
                let velocity = differences::<5>(&positions);
                let acceleration = differences::<4>(&velocity);
                let jerk = differences::<3>(&acceleration);
                for value in velocity {
                    section_seconds = section_seconds.max(value.abs() / plan.peak_speed);
                }
                for value in acceleration {
                    section_seconds =
                        section_seconds.max((value.abs() / limits.acceleration_limit).sqrt());
                }
                for value in jerk {
                    section_seconds = section_seconds.max((value.abs() / limits.jerk_limit).cbrt());
                }
                controls.push(positions);
            }
            ensure!(
                index + 1 == plan.segments.len() || passes,
                "Waypoint {index} requires a stop or reversal; use a separate step"
            );
            sections.push(Section { controls });
        }
        ensure!(
            section_seconds.is_finite() && section_seconds > 0.0,
            "Invalid continuous path duration"
        );
        Ok(Self {
            sections,
            section_seconds,
        })
    }

    pub(crate) fn section_seconds(&self) -> f64 {
        self.section_seconds
    }

    pub(crate) fn sample(&self, section: usize, fraction: f64, reference: &mut Reference) {
        for (joint, controls) in self.sections[section].controls.iter().enumerate() {
            let velocity = differences::<5>(controls);
            let acceleration = differences::<4>(&velocity);
            reference.positions[joint] = interpolate(*controls, fraction);
            reference.velocities[joint] = interpolate(velocity, fraction) / self.section_seconds;
            reference.accelerations[joint] =
                interpolate(acceleration, fraction) / self.section_seconds.powi(2);
        }
    }
}
