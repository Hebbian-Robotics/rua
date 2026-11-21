use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::Context;
use hebbian_contracts::JawClosingAxis;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RobotAssemblySelection {
    manifest: PathBuf,
    variant: String,
}

#[derive(Deserialize)]
struct RobotAssemblies {
    joint_names: Vec<String>,
    tool_frame: String,
    jaw_closing_axis: JawClosingAxis,
    variants: BTreeMap<String, RobotAssembly>,
}

#[derive(Deserialize)]
struct RobotAssembly {
    urdf: PathBuf,
}

pub(super) fn apply_robot_assembly(
    profile: &mut toml::Table,
    repository_root: &Path,
) -> anyhow::Result<()> {
    let Some(selection) = profile.remove("robot_assembly") else {
        return Ok(());
    };
    let selection: RobotAssemblySelection = selection
        .try_into()
        .context("Invalid [robot_assembly] selection")?;
    let manifest_path = std::path::absolute(repository_root.join(selection.manifest))?;
    let text = std::fs::read_to_string(&manifest_path).with_context(|| {
        format!(
            "Cannot read robot assemblies manifest {}",
            manifest_path.display()
        )
    })?;
    let manifest: RobotAssemblies = toml::from_str(&text).with_context(|| {
        format!(
            "Cannot parse robot assemblies manifest {}",
            manifest_path.display()
        )
    })?;
    let variant = manifest.variants.get(&selection.variant).with_context(|| {
        format!(
            "Unknown robot assembly {:?} in {}",
            selection.variant,
            manifest_path.display()
        )
    })?;
    let urdf = manifest_path
        .parent()
        .context("Robot assemblies manifest has no parent directory")?
        .join(&variant.urdf);

    // INVARIANT: Explicit profile fields still override defaults from the model.
    for (key, value) in [
        ("urdf", toml::Value::try_from(urdf)?),
        ("joint_names", toml::Value::try_from(manifest.joint_names)?),
        ("tool_frame", toml::Value::String(manifest.tool_frame)),
        (
            "jaw_closing_axis",
            toml::Value::try_from(manifest.jaw_closing_axis)?,
        ),
    ] {
        profile.entry(key.to_owned()).or_insert(value);
    }
    Ok(())
}
