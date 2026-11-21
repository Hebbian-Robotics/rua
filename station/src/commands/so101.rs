use std::path::PathBuf;

#[derive(clap::Args, Default)]
pub(crate) struct SelectionArguments {
    /// Local SO-101 config checkout containing arms/<USB-serial>/{setup.toml,calibration.json}.
    /// Default: `$HEBBIAN_SO101_CONFIG_DIRECTORY`, else `$XDG_CONFIG_HOME/rua/so101-arm-configs`.
    #[arg(long)]
    pub(crate) so101_config_directory: Option<PathBuf>,
    /// Select one connected USB adapter when several arms are registered. With a named
    /// simulator profile or `--no-physics-stand-in`: the registered arm whose calibrated joint
    /// travel the simulated arm takes, so it stops where that arm does.
    #[arg(long)]
    pub(crate) so101_serial: Option<String>,
}
