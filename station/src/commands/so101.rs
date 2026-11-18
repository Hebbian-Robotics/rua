use std::path::PathBuf;

#[derive(clap::Args, Default)]
pub(crate) struct SelectionArguments {
    /// Local SO-101 config checkout containing arms/<USB-serial>/{setup.toml,calibration.json}.
    /// Default: `$HEBBIAN_SO101_CONFIG_DIRECTORY`, else `$XDG_CONFIG_HOME/rua/so101-arm-configs`.
    #[arg(long)]
    pub(crate) so101_config_directory: Option<PathBuf>,
    /// Select one connected USB controller when several arms are registered.
    #[arg(long)]
    pub(crate) so101_serial: Option<String>,
}
