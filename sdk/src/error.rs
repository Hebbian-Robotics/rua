use hebbian_contracts::{
    CameraListing, CommandOutcome, CommandRecord, LeaseStatus, RejectionReason,
};

pub type Result<T> = std::result::Result<T, Error>;

/// The failures a caller branches on.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The station could not be reached, the connection was lost before an
    /// answer arrived, or the station sent something unreadable. A command that
    /// failed this way may or may not have been accepted; the SDK never retries it.
    #[error("station connection: {0}")]
    Connection(String),
    /// Someone else holds the lease. `None` when the holder is not known.
    #[error("the lease is held elsewhere: {}", describe_holder(.0.as_deref()))]
    LeaseHeldElsewhere(Option<Box<LeaseStatus>>),
    /// The station refused a command, preview or message; nothing moved.
    #[error("refused ({reason:?}): {detail}")]
    Refused {
        reason: RejectionReason,
        detail: String,
    },
    /// An accepted command ended without completing: stopped, tripped, timed out...
    #[error("command {} ended {outcome:?}{}", .record.command_id, detail_suffix(.detail.as_deref()))]
    Incomplete {
        outcome: CommandOutcome,
        detail: Option<String>,
        record: Box<CommandRecord>,
    },
    /// The station did not answer in time.
    #[error("timed out waiting for {0}")]
    Timeout(&'static str),
    /// The media service has no fresh frame for this camera. `listed` is what it reports, so a
    /// camera missing from it was never published or the name is wrong, while one listed as
    /// unavailable stopped streaming. `last_error` is the latest failure the station saw in
    /// the media programs' output for it, when the station could be asked.
    #[error("camera '{name}' is not streaming ({}){}", describe_cameras(.listed), describe_last_error(.last_error.as_deref()))]
    CameraUnavailable {
        name: String,
        listed: Vec<CameraListing>,
        last_error: Option<String>,
    },
    /// An HTTP route answered with an error status.
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
}

impl From<hebbian_contracts::InvalidCameraName> for Error {
    fn from(error: hebbian_contracts::InvalidCameraName) -> Self {
        Self::Refused {
            reason: RejectionReason::InvalidTarget,
            detail: error.to_string(),
        }
    }
}

fn describe_holder(status: Option<&LeaseStatus>) -> String {
    status.map_or_else(
        || "an unknown holder".to_owned(),
        |status| format!("{:?} ({})", status.holder, status.label),
    )
}

fn describe_cameras(listed: &[CameraListing]) -> String {
    if listed.is_empty() {
        return "the media service lists no cameras; check that the media service is running and \
                the station's --camera device ids are current"
            .to_owned();
    }
    let names: Vec<String> = listed
        .iter()
        .map(|camera| {
            format!(
                "{} ({})",
                camera.name,
                if camera.available {
                    "streaming"
                } else {
                    "unavailable"
                }
            )
        })
        .collect();
    format!("the media service lists: {}", names.join(", "))
}

fn describe_last_error(last_error: Option<&str>) -> String {
    last_error.map_or_else(String::new, |detail| format!("; last error: {detail}"))
}

fn detail_suffix(detail: Option<&str>) -> String {
    detail.map_or_else(String::new, |detail| format!(": {detail}"))
}

impl From<reqwest::Error> for Error {
    fn from(error: reqwest::Error) -> Self {
        Self::Connection(error.to_string())
    }
}
