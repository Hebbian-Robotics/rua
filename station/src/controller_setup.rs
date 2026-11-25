use std::path::Path;

use anyhow::Context;
use hebbian_rua_core::config::ControllerConfig;
use hebbian_rua_core::so101::driver::{SerialBus, Servos};

pub(crate) const DEFAULT_STAND_IN_LAG_SECONDS: f64 = 0.05;

pub(crate) fn load_config(path: &Path) -> anyhow::Result<ControllerConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read the config {}", path.display()))?;
    ControllerConfig::parse(&text).with_context(|| format!("Invalid config {}", path.display()))
}

#[allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "Startup and explicit maintenance commands open the configured controller hardware."
)]
pub(crate) fn feetech_servos(config: &ControllerConfig) -> anyhow::Result<Servos<SerialBus>> {
    let bus = config
        .feetech
        .clone()
        .context("This command needs a config with a feetech section")?;
    Servos::open(bus)
}
