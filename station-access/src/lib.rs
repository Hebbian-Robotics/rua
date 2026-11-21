use std::time::Duration;

use hebbian_contracts::{PhoneInvite, RoomAccess};
use livekit_api::{
    access_token::{AccessToken, VideoGrants},
    services::room::{CreateRoomOptions, RoomClient},
};

const VARIABLES: [&str; 3] = ["LIVEKIT_URL", "LIVEKIT_API_KEY", "LIVEKIT_API_SECRET"];

#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    #[error("Set LiveKit credentials: {}", .0.join(", "))]
    MissingCredentials(Vec<&'static str>),
    #[error("invalid camera configuration: {0}")]
    Camera(#[from] hebbian_contracts::InvalidCameraName),
    #[error("LiveKit token: {0}")]
    Token(#[from] livekit_api::access_token::AccessTokenError),
    #[error("LiveKit room service: {0}")]
    Room(#[from] livekit_api::services::ServiceError),
    #[error("LiveKit room metadata update exceeded 10 seconds")]
    Timeout,
    #[error(transparent)]
    Invite(#[from] livekit_camera_invite::InviteError),
}

// INVARIANT: Secrets deliberately do not implement `Debug`, `Serialize` or `JsonSchema`.
pub struct Credentials {
    pub url: String,
    api_key: String,
    api_secret: String,
}

// WHY: The server can distinguish an intentionally unconfigured station from a partial setup.
pub enum CameraConfiguration {
    Off { reason: AccessError },
    Enabled(Credentials),
}

impl CameraConfiguration {
    pub fn from_environment() -> Result<Self, AccessError> {
        Self::from_values(VARIABLES.map(|name| std::env::var(name).ok()))
    }

    fn from_values(values: [Option<String>; 3]) -> Result<Self, AccessError> {
        let credentials = Credentials::from_values(values);
        match credentials {
            Err(AccessError::MissingCredentials(missing)) if missing.len() == VARIABLES.len() => {
                Ok(Self::Off {
                    reason: AccessError::MissingCredentials(missing),
                })
            }
            other => other.map(Self::Enabled),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CameraLifetime {
    PhoneInvite,
    Station,
}
impl CameraLifetime {
    fn duration(self) -> Duration {
        match self {
            Self::PhoneInvite => Duration::from_hours(1),
            Self::Station => Duration::from_hours(12),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum WatcherRole {
    Viewer,
    Model,
    Recorder,
}
impl WatcherRole {
    fn name(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Model => "model",
            Self::Recorder => "recorder",
        }
    }
    fn duration(self) -> Duration {
        match self {
            Self::Viewer | Self::Model => Duration::from_hours(1),
            Self::Recorder => Duration::from_hours(12),
        }
    }
}

#[must_use]
pub fn new_room_name() -> String {
    format!(
        "station-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..12]
    )
}

impl Credentials {
    pub fn from_environment() -> Result<Self, AccessError> {
        Self::from_values(VARIABLES.map(|name| std::env::var(name).ok()))
    }

    pub fn from_values(values: [Option<String>; 3]) -> Result<Self, AccessError> {
        let missing: Vec<_> = VARIABLES
            .into_iter()
            .zip(&values)
            .filter_map(|(name, value)| value.as_ref().is_none_or(String::is_empty).then_some(name))
            .collect();
        if !missing.is_empty() {
            return Err(AccessError::MissingCredentials(missing));
        }
        let [url, api_key, api_secret] = values.map(Option::unwrap_or_default);
        Ok(Self {
            url,
            api_key,
            api_secret,
        })
    }

    pub fn camera_access(
        &self,
        room: &str,
        camera_name: &str,
        lifetime: CameraLifetime,
    ) -> Result<RoomAccess, AccessError> {
        let camera_name = hebbian_contracts::CameraName::try_from(camera_name)?;
        self.room_access(
            room,
            camera_name.as_str(),
            lifetime.duration(),
            VideoGrants {
                can_publish: Some(true),
                can_publish_sources: vec!["camera".into()],
                can_subscribe: Some(false),
                can_publish_data: Some(false),
                ..Default::default()
            },
        )
    }

    pub fn watcher_access(&self, room: &str, role: WatcherRole) -> Result<RoomAccess, AccessError> {
        let identity = format!(
            "{}-{}",
            role.name(),
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        );
        self.room_access(
            room,
            &identity,
            role.duration(),
            VideoGrants {
                can_publish: Some(false),
                can_publish_data: Some(false),
                can_subscribe: Some(true),
                hidden: true,
                ..Default::default()
            },
        )
    }

    fn room_access(
        &self,
        room: &str,
        identity: &str,
        lifetime: Duration,
        mut grants: VideoGrants,
    ) -> Result<RoomAccess, AccessError> {
        grants.room_join = true;
        room.clone_into(&mut grants.room);
        let token = AccessToken::with_api_key(&self.api_key, &self.api_secret)
            .with_identity(identity)
            .with_ttl(lifetime)
            .with_grants(grants)
            .to_jwt()?;
        Ok(RoomAccess {
            server_url: self.url.clone(),
            token,
        })
    }

    pub fn phone_invite(
        &self,
        room: &str,
        label: &str,
        phone_page_url: &str,
    ) -> Result<PhoneInvite, AccessError> {
        let camera_name = format!("phone_{label}");
        let access = self.camera_access(room, &camera_name, CameraLifetime::PhoneInvite)?;
        let invite = livekit_camera_invite::camera_invite(
            phone_page_url,
            &access.server_url,
            &access.token,
        )?;
        Ok(PhoneInvite {
            camera_name,
            url: invite.url,
            qr_code_image: invite.qr_code_image,
        })
    }

    // WHY: The room exists before recording state is announced, which also covers phones joining later.
    pub async fn announce_recording(&self, room: &str, recording: bool) -> Result<(), AccessError> {
        let client = RoomClient::with_api_key(&self.url, &self.api_key, &self.api_secret);
        tokio::time::timeout(Duration::from_secs(10), async {
            client
                .create_room(room, CreateRoomOptions::default())
                .await?;
            client
                .update_room_metadata(
                    room,
                    &serde_json::json!({ "recording": recording }).to_string(),
                )
                .await?;
            Ok::<_, livekit_api::services::ServiceError>(())
        })
        .await
        .map_err(|_| AccessError::Timeout)??;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
