use serde::Deserialize;

use crate::{AccessError, Credentials};

#[derive(Deserialize, Default)]
#[serde(default)]
struct CliConfiguration {
    default_project: String,
    projects: Vec<Project>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Project {
    name: String,
    url: String,
    api_key: String,
    api_secret: String,
}

pub(super) fn load(home: Option<&std::path::Path>) -> Result<Option<Credentials>, AccessError> {
    let Some(home) = home else {
        return Ok(None);
    };
    let configuration = match std::fs::read(home.join(".livekit/cli-config.yaml")) {
        Ok(configuration) => configuration,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    parse(&configuration)
}

fn parse(configuration: &[u8]) -> Result<Option<Credentials>, AccessError> {
    // WHY: YAML diagnostics can include the input, which contains secrets.
    let configuration: CliConfiguration = serde_yaml_ng::from_slice(configuration)
        .map_err(|_| AccessError::InvalidCliConfiguration)?;
    if configuration.projects.is_empty() {
        return Ok(None);
    }
    let use_only_project =
        configuration.default_project.is_empty() && configuration.projects.len() == 1;
    let project = configuration
        .projects
        .into_iter()
        .find(|project| {
            use_only_project
                || (!configuration.default_project.is_empty()
                    && project.name == configuration.default_project)
        })
        .ok_or(AccessError::MissingDefaultProject)?;
    Credentials::from_values([
        Some(project.url),
        Some(project.api_key),
        Some(project.api_secret),
    ])
    .map(Some)
    .map_err(|_| AccessError::InvalidCliConfiguration)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_default_project_without_combining_credentials() -> Result<(), AccessError> {
        let projects = "projects:\n  - {name: staging}\n  - {name: station, url: 'wss://camera.example', api_key: key, api_secret: secret}\n";
        assert!(matches!(
            parse(projects.as_bytes()),
            Err(AccessError::MissingDefaultProject)
        ));
        let configuration = format!("default_project: station\n{projects}");
        let credentials =
            parse(configuration.as_bytes())?.ok_or(AccessError::InvalidCliConfiguration)?;
        assert_eq!(credentials.url, "wss://camera.example");
        assert_eq!(credentials.api_key, "key");
        assert_eq!(credentials.api_secret, "secret");
        Ok(())
    }
}
