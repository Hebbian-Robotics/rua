use super::{RecorderError, unix_nanoseconds};
#[cfg(test)]
pub(super) use hebbian_mcap::messages::CompressedVideo;
use hebbian_mcap::{
    GroupedWriter, WriterOptions,
    messages::{self, VIDEO_SCHEMA, joint_state},
};
use mcap::records::MessageHeader;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::File,
    io::BufWriter,
    path::Path,
    sync::{Arc, Mutex},
    time::Duration,
};

/// What a message's channel declares about its payload.
#[derive(Clone, Copy)]
enum Payload {
    Json,
    Video,
    JointState,
}

pub(super) struct Archive {
    writer: GroupedWriter<BufWriter<File>>,
    video_schema: u16,
    joint_state_schema: u16,
    channels: BTreeMap<String, u16>,
    started_at: u64,
    controller_interrupted: bool,
}

#[derive(Clone)]
pub(crate) struct RecordingArchive(Arc<Mutex<Archive>>);

impl RecordingArchive {
    pub fn create(directory: &Path) -> Result<Self, RecorderError> {
        std::fs::create_dir_all(directory)?;
        let mut writer = GroupedWriter::new(
            BufWriter::new(File::create(directory.join("recording.mcap"))?),
            WriterOptions {
                chunk_size: 512 * 1024,
                max_chunk_age: Some(Duration::from_secs(1)),
                ..WriterOptions::default()
            },
        )?;
        let video_schema =
            writer.add_schema("foxglove.CompressedVideo", "protobuf", VIDEO_SCHEMA)?;
        let joint_state_schema =
            writer.add_schema("sensor_msgs/msg/JointState", "ros2msg", joint_state::SCHEMA)?;
        Ok(Self(Arc::new(Mutex::new(Archive {
            writer,
            video_schema,
            joint_state_schema,
            channels: BTreeMap::new(),
            started_at: unix_nanoseconds(),
            controller_interrupted: false,
        }))))
    }

    pub fn mark_controller_interrupted(
        &self,
        event_time: u64,
        unavailable_at_start: bool,
    ) -> Result<(), RecorderError> {
        let mut archive = self.0.lock().map_err(|_| RecorderError::WriterPoisoned)?;
        if unavailable_at_start || event_time >= archive.started_at {
            archive.controller_interrupted = true;
        }
        Ok(())
    }

    pub fn controller_interrupted(&self) -> Result<bool, RecorderError> {
        Ok(self
            .0
            .lock()
            .map_err(|_| RecorderError::WriterPoisoned)?
            .controller_interrupted)
    }

    pub fn json(&self, topic: &str, data: &Value, timestamp: u64) -> Result<(), RecorderError> {
        self.write(
            topic,
            "json",
            timestamp,
            &serde_json::to_vec(data)?,
            Payload::Json,
        )
    }

    pub fn video(
        &self,
        camera: &str,
        timestamp: u64,
        packet: Vec<u8>,
    ) -> Result<(), RecorderError> {
        self.write(
            &format!("/cameras/{camera}/video"),
            "protobuf",
            timestamp,
            &messages::encode_compressed_video(timestamp, camera, &packet, "h264"),
            Payload::Video,
        )
    }

    /// One arm's joints as a ROS 2 `sensor_msgs/JointState`, for Foxglove and Rerun.
    /// Velocities and efforts are each empty or one per joint.
    pub fn joint_states(
        &self,
        arm: &str,
        timestamp: u64,
        names: &[String],
        positions: &[f64],
        velocities: &[f64],
        efforts: &[f64],
    ) -> Result<(), RecorderError> {
        self.write(
            &format!("/arms/{arm}/joint_states"),
            "cdr",
            timestamp,
            &joint_state::encode(timestamp, arm, names, positions, velocities, efforts)?,
            Payload::JointState,
        )
    }

    fn write(
        &self,
        topic: &str,
        encoding: &str,
        timestamp: u64,
        data: &[u8],
        payload: Payload,
    ) -> Result<(), RecorderError> {
        let mut archive = self.0.lock().map_err(|_| RecorderError::WriterPoisoned)?;
        let schema = match payload {
            Payload::Json => 0,
            Payload::Video => archive.video_schema,
            Payload::JointState => archive.joint_state_schema,
        };
        let group = match payload {
            Payload::Video => "video",
            Payload::Json | Payload::JointState => "telemetry",
        };
        let channel_id = if let Some(channel_id) = archive.channels.get(topic) {
            *channel_id
        } else {
            let channel_id =
                archive
                    .writer
                    .add_channel(schema, topic, encoding, &BTreeMap::new(), group)?;
            archive.channels.insert(topic.into(), channel_id);
            channel_id
        };
        archive.writer.write_message(
            MessageHeader {
                channel_id,
                sequence: 0,
                log_time: timestamp,
                publish_time: timestamp,
            },
            data,
        )?;
        Ok(())
    }

    pub fn metadata(&self, room: &str) -> Result<(), RecorderError> {
        self.json("/recording/metadata", &json!({
            "room": room,
            "timestampSource": "video: publisher capture time or arrival; arm frames: executor emission; run telemetry: server receipt; station snapshots: recorder arrival",
            "stateSamplingHz": 10,
            "limitations": [
                "Unsequenced control samples and decimated station state cannot be replayed across a disconnect",
            ],
        }), unix_nanoseconds())
    }

    pub fn finish(&self) -> Result<(), RecorderError> {
        let mut archive = self.0.lock().map_err(|_| RecorderError::WriterPoisoned)?;
        archive.writer.finish()?;
        Ok(())
    }
}
