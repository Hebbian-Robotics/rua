use std::fs::{DirBuilder, File, OpenOptions};
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::Context;

const SESSION_PREFIX: &str = "rua-compiler-";
const INACTIVE_RETENTION: Duration = Duration::from_hours(24);

pub(super) struct ArtifactWorkspace {
    directory: PathBuf,
    _session_lock: File,
}

impl ArtifactWorkspace {
    pub fn open(instance_id: &str) -> anyhow::Result<Self> {
        clean_expired_workspaces();
        let directory = std::env::temp_dir().join(format!("{SESSION_PREFIX}{instance_id}"));
        Self::create(directory)
    }

    fn create(directory: PathBuf) -> anyhow::Result<Self> {
        DirBuilder::new()
            .mode(0o700)
            .create(&directory)
            .context("Cannot create compilation artifact workspace")?;
        let session_lock = match OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(directory.join(".station-lock"))
        {
            Ok(session_lock) => session_lock,
            Err(error) => {
                // WHY: A leftover empty directory would refuse every later open for this instance.
                let _ = std::fs::remove_dir(&directory);
                return Err(error).context("Cannot open compilation workspace lock");
            }
        };
        session_lock
            .try_lock()
            .context("Compilation workspace is already in use")?;
        Ok(Self {
            directory,
            _session_lock: session_lock,
        })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

fn clean_expired_workspaces() {
    if let Err(error) = cleanup_inactive_sessions(&std::env::temp_dir(), SystemTime::now()) {
        tracing::warn!(error = %format_args!("{error:#}"), "Cannot inspect inactive compilation workspaces");
    }
}

fn cleanup_inactive_sessions(parent: &Path, now: SystemTime) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(parent).context("Cannot inspect temporary artifact sessions")? {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(instance) = name
            .to_str()
            .and_then(|name| name.strip_prefix(SESSION_PREFIX))
        else {
            continue;
        };
        if uuid::Uuid::parse_str(instance).is_err()
            || !entry.file_type().is_ok_and(|kind| kind.is_dir())
        {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= INACTIVE_RETENTION);
        if !old {
            continue;
        }
        let Ok(session_lock) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry.path().join(".station-lock"))
        else {
            continue;
        };
        if session_lock.try_lock().is_err() {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(entry.path()) {
            tracing::warn!(%error, "Cannot clean inactive compilation workspace");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn old_sessions_are_cleaned_only_after_the_owner_releases_its_lock() {
        let parent =
            std::env::temp_dir().join(format!("rua-artifact-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&parent).unwrap();
        let directory = parent.join(format!("{SESSION_PREFIX}{}", uuid::Uuid::new_v4()));
        let workspace = ArtifactWorkspace::create(directory.clone()).unwrap();
        assert_eq!(
            directory.metadata().unwrap().permissions().mode() & 0o077,
            0
        );
        std::fs::write(directory.join("inspection.json"), b"saved source").unwrap();
        let future = SystemTime::now() + INACTIVE_RETENTION + Duration::from_secs(1);
        cleanup_inactive_sessions(&parent, future).unwrap();
        assert!(directory.join("inspection.json").is_file());
        drop(workspace);
        cleanup_inactive_sessions(&parent, future).unwrap();
        assert!(!directory.exists());
        std::fs::remove_dir(parent).unwrap();
    }
}
