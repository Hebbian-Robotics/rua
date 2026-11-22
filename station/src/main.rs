mod commands;
mod media;
mod run;
mod serve;
mod so101;
mod tools;

use clap::Parser;

use crate::commands::{Arguments, Command};

fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new(
                    "warn,hebbian_robot_use_server=info,hebbian_robot_use_core=info",
                )
            }),
        )
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    match Arguments::parse().command {
        Command::Serve { arguments, web } => serve::serve(arguments, web.settings()?),
        Command::Controller {
            arms,
            station_socket,
            station_pid,
            instance,
            socket_directory,
            plant_lag_seconds,
            hardware,
        } => run::run_configured(
            &run::load_arms(&arms)?,
            &station_socket,
            station_pid,
            instance,
            socket_directory,
            plant_lag_seconds,
            hardware,
        ),
        Command::So101Discover {
            so101_config_directory,
        } => so101::print_discovered(so101_config_directory.as_deref()),
        Command::CheckArm { config } => tools::check_arm(&config),
        Command::Disable { config } => tools::disable_arm(&config),
        Command::ProbeRobot { trace } => tools::probe_robot(&trace),
        Command::ReplayPolicy { trace } => tools::replay_policy(&trace),
        Command::CheckConfig { config } => tools::check_config(&config),
        Command::Config {
            profile,
            selection,
            arm,
            profile_directory,
            urdf_limits,
        } => tools::print_profile_config(
            &profile,
            &selection,
            &arm,
            profile_directory.as_deref(),
            urdf_limits,
        ),
        Command::Guard {
            config,
            controller_pid,
        } => tools::run_guard(&config, controller_pid),
        Command::Schema { sim_plant, .. } => {
            let kind = if sim_plant {
                tools::SchemaKind::SimPlant
            } else {
                tools::SchemaKind::OpenApi
            };
            tools::print_schema(kind);
            Ok(())
        }
    }
}
