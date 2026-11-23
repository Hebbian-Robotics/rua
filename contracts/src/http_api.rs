//! The JSON bodies of the station's HTTP routes that no other contract defines.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// One trace file, as `GET /traces` lists it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TraceFile {
    pub name: String,
    pub size_bytes: u64,
    pub modified_unix_seconds: f64,
}

/// A camera as the media service's `GET /cameras` lists it.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CameraListing {
    pub name: String,
    pub available: bool,
}

/// `POST /media/phone-invites`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PhoneInviteRequest {
    /// The phone streams as `phone_<label>`.
    pub label: String,
}

/// A phone invite: scan `url` to stream the phone as `camera_name`.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct PhoneInvite {
    pub camera_name: String,
    pub url: String,
    pub qr_code_image: String,
}
