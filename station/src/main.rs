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
    match Arguments::parse().command {
        Command::Serve { arguments, web } => serve::serve(arguments, web.settings()?),
        Command::Run {
            arms,
            seconds,
            socket_directory,
            plant_lag_seconds,
            hardware,
            http_address,
            web,
        } => run::run_loop(
            &run::load_arms(&arms)?,
            seconds,
            socket_directory.as_deref(),
            plant_lag_seconds,
            hardware,
            http_address.as_deref(),
            web.settings()?,
        ),
        Command::Controller {
            arms,
            station_socket,
            instance,
            socket_directory,
            plant_lag_seconds,
            hardware,
        } => run::run_controller(
            &run::load_arms(&arms)?,
            &station_socket,
            instance,
            socket_directory.as_deref(),
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
            profile_json: _,
            selection,
            arm,
            profile_directory,
            urdf_limits,
        } => tools::print_profile_config(
            profile.as_deref(),
            &selection,
            &arm,
            profile_directory.as_deref(),
            urdf_limits,
        ),
        Command::Guard {
            config,
            executor_pid,
        } => tools::run_guard(&config, executor_pid),
        Command::Schema { sim_plant, openapi } => {
            let kind = if sim_plant {
                tools::SchemaKind::SimPlant
            } else if openapi {
                tools::SchemaKind::OpenApi
            } else {
                tools::SchemaKind::Contract
            };
            tools::print_schema(kind);
            Ok(())
        }
    }
}
