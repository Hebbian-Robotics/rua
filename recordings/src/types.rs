use schemars::JsonSchema;
use serde::Serialize;

/// One recording directory. Its data, video included, is `recording.mcap`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Recording {
    pub name: String,
    pub mcap_path: Option<String>,
    pub size_bytes: u64,
}
#[derive(Debug, Serialize, JsonSchema)]
pub struct Recordings {
    pub active: Option<String>,
    pub recordings: Vec<Recording>,
}
