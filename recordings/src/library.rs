use crate::{Error, Recording, Recordings};
use std::{
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Clone)]
pub struct RecordingLibrary {
    pub(crate) directory: PathBuf,
    active_recording: Arc<dyn Fn() -> Option<String> + Send + Sync>,
}
impl RecordingLibrary {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
            active_recording: Arc::new(|| None),
        }
    }
    /// Use the station's current recording name for listing and lifecycle guards.
    #[must_use]
    pub fn with_active_recording(
        mut self,
        active_recording: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.active_recording = Arc::new(active_recording);
        self
    }
    pub(crate) fn active_recording(&self) -> Option<String> {
        (self.active_recording)()
    }
    /// List visible, real recording directories, newest name first.
    ///
    /// # Errors
    /// Returns an error if the recordings directory cannot be read.
    pub fn recordings(&self) -> Result<Recordings, Error> {
        let mut recordings = Vec::new();
        if !self.directory.is_dir() {
            return Ok(Recordings {
                active: self.active_recording(),
                recordings,
            });
        }
        for directory in visible_entries(&self.directory)? {
            if !directory.is_dir() {
                continue;
            }
            let name = file_name(&directory)?;
            let mcap_path = format!("{name}/recording.mcap");
            let size_bytes = self
                .safe_file(&mcap_path)
                .ok()
                .and_then(|path| path.metadata().ok())
                .map(|metadata| metadata.len());
            recordings.push(Recording {
                name,
                mcap_path: size_bytes.map(|_| mcap_path),
                size_bytes: size_bytes.unwrap_or(0),
            });
        }
        recordings.sort_by(|left, right| right.name.cmp(&left.name));
        Ok(Recordings {
            active: self.active_recording(),
            recordings,
        })
    }
    /// Delete an exact listed recording.
    ///
    /// # Errors
    /// Returns `NotFound` for an unlisted name, or a filesystem error.
    pub fn delete_recording(&self, name: &str) -> Result<(), Error> {
        if self.active_recording().as_deref() == Some(name) {
            return Err(Error::RecordingInProgress);
        }
        // Resolve only the exact directory name returned by the listing.
        let recording = self.listed(name)?;
        fs::remove_dir_all(self.directory.join(&recording.name))?;
        Ok(())
    }
    pub(crate) fn listed(&self, name: &str) -> Result<Recording, Error> {
        self.recordings()?
            .recordings
            .into_iter()
            .find(|recording| recording.name == name)
            .ok_or(Error::NotFound)
    }
    /// Resolve a visible recording file while refusing symlinks and traversal.
    ///
    /// # Errors
    /// Returns `NotFound` for missing, hidden or unsafe paths.
    pub fn recording_file(&self, relative: &str) -> Result<PathBuf, Error> {
        self.safe_file(relative)
    }
    pub(crate) fn safe_file(&self, relative: &str) -> Result<PathBuf, Error> {
        let path = Path::new(relative);
        if !path.components().all(|component| matches!(component, Component::Normal(part) if !part.to_string_lossy().starts_with('.'))) || !matches!(path.extension().and_then(|extension| extension.to_str()), Some("mcap")) { return Err(Error::NotFound); }
        let root = self.directory.canonicalize().map_err(|_| Error::NotFound)?;
        let mut resolved = root.clone();
        for component in path.components() {
            resolved.push(component);
            if fs::symlink_metadata(&resolved)
                .map_err(|_| Error::NotFound)?
                .file_type()
                .is_symlink()
            {
                return Err(Error::NotFound);
            }
        }
        if !resolved.is_file() || !resolved.starts_with(root) {
            return Err(Error::NotFound);
        }
        Ok(resolved)
    }
}
fn visible_entries(directory: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with('.') && !entry.file_type()?.is_symlink()
        {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}
fn file_name(path: &Path) -> Result<String, Error> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or(Error::NotFound)
}
