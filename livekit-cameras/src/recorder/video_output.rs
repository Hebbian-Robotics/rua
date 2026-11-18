use super::archive::RecordingArchive;
use livekit_camera_recorder::{RecorderError, VideoEvent, VideoOutput};
use serde_json::json;

impl VideoOutput for RecordingArchive {
    fn camera_name(&self, participant_identity: &str, _track_sid: &str) -> Option<String> {
        Some(participant_identity.to_owned())
    }

    fn video(&self, camera: &str, timestamp: u64, packet: Vec<u8>) -> Result<(), RecorderError> {
        RecordingArchive::video(self, camera, timestamp, packet)
            .map_err(|error| RecorderError::Output(error.to_string()))
    }

    fn event(&self, event: VideoEvent<'_>) -> Result<(), RecorderError> {
        let result = match event {
            VideoEvent::CameraError { camera, detail, timestamp } => self.json("/station/recording_errors", &json!({"camera": camera, "error": detail}), timestamp),
            VideoEvent::SegmentStarted { camera, segment, timestamp, width, height } => self.json(&format!("/cameras/{camera}/segments"), &json!({"camera": camera, "segment": segment, "started_unix_nano": timestamp.to_string(), "width": width, "height": height}), timestamp),
            VideoEvent::SegmentEnded { camera, segment, timestamp } => self.json(&format!("/cameras/{camera}/segments"), &json!({"camera": camera, "segment": segment, "ended_unix_nano": timestamp.to_string()}), timestamp),
            VideoEvent::Statistics { camera, received, encoded, timestamp } => self.json("/station/recording_statistics", &json!({"camera": camera, "received": received, "encoded": encoded}), timestamp),
            VideoEvent::TimestampSources { camera, publisher_stamped, arrival_stamped, maximum_latency_nanoseconds, timestamp } => self.json("/station/recording_timestamp_sources", &json!({"camera": camera, "publisher_stamped": publisher_stamped, "arrival_stamped": arrival_stamped, "maximum_latency_nanoseconds": maximum_latency_nanoseconds}), timestamp),
        };
        result.map_err(|error| RecorderError::Output(error.to_string()))
    }
}
