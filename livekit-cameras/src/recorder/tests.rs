use super::archive::{CompressedVideo, RecordingArchive};
use cameras::conversion::I420Frame;
use livekit::webrtc::video_frame::VideoRotation;
use livekit_camera_recorder::CameraWriter;
use prost::Message;
use serde_json::Value;
use std::sync::Arc;
use std::{fs, path::Path, process::Command};

fn probe(path: &Path) -> Result<Value, Box<dyn std::error::Error>> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_frames",
            "-show_streams",
            "-of",
            "json",
        ])
        .arg(path)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(serde_json::from_slice(&output.stdout)?)
}

#[test]
fn recording_preserves_irregular_timestamps_and_rotates_segments()
-> Result<(), Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    let archive = RecordingArchive::create(directory.path())?;
    archive.metadata("synthetic-test")?;
    let mut writer = CameraWriter::new("chest".into(), false, Arc::new(archive.clone()))?;
    let first_at = 1_700_000_000_123_456_000_u64;
    let input_timestamps = [first_at, first_at + 66_667_000, first_at + 175_111_000];
    let frame = I420Frame {
        width: 16,
        height: 8,
        data: [vec![80; 128], vec![128; 64]].concat(),
    };
    for timestamp in input_timestamps {
        writer.write(&frame, VideoRotation::VideoRotation0, timestamp)?;
    }
    writer.write(
        &frame,
        VideoRotation::VideoRotation90,
        first_at + 250_000_000,
    )?;
    let resized = I420Frame {
        width: 32,
        height: 16,
        data: [vec![80; 512], vec![128; 256]].concat(),
    };
    writer.write(
        &resized,
        VideoRotation::VideoRotation0,
        first_at + 350_000_000,
    )?;
    writer.finish()?;
    archive.finish()?;

    check_archive(directory.path(), first_at)?;
    Ok(())
}

fn assert_chunks_are_grouped(data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    // Video and telemetry never share a chunk, so a reader fetches only what it wants.
    let summary = mcap::Summary::read(data)?.ok_or("missing summary")?;
    let (mut video_chunks, mut telemetry_chunks) = (0, 0);
    for chunk in &summary.chunk_indexes {
        let video = chunk
            .message_index_offsets
            .keys()
            .map(|id| summary.channels[id].topic.ends_with("/video"))
            .collect::<Vec<_>>();
        assert!(
            video.iter().all(|is_video| *is_video) || video.iter().all(|is_video| !is_video),
            "a chunk mixes video and telemetry"
        );
        if video.first() == Some(&true) {
            video_chunks += 1;
        } else {
            telemetry_chunks += 1;
        }
    }
    assert!(video_chunks > 0 && telemetry_chunks > 0);
    Ok(())
}

fn check_archive(directory: &Path, first_at: u64) -> Result<(), Box<dyn std::error::Error>> {
    let data = fs::read(directory.join("recording.mcap"))?;
    assert_chunks_are_grouped(&data)?;
    let mut timestamps = Vec::new();
    let mut encoded_video = Vec::new();
    let mut segments = Vec::new();
    let mut statistics = Vec::new();
    for message in mcap::MessageStream::new(&data)? {
        let message = message?;
        if message.channel.topic == "/cameras/chest/segments" {
            segments.push(serde_json::from_slice::<Value>(&message.data)?);
        }
        if message.channel.topic == "/station/recording_statistics" {
            statistics.push(serde_json::from_slice::<Value>(&message.data)?);
        }
        if message.channel.topic == "/cameras/chest/video" {
            assert_eq!(message.channel.message_encoding, "protobuf");
            let schema = message
                .channel
                .schema
                .as_ref()
                .ok_or("missing video schema")?;
            assert_eq!(schema.name, "foxglove.CompressedVideo");
            assert_eq!(schema.encoding, "protobuf");
            let video = CompressedVideo::decode(message.data.as_ref())?;
            assert_eq!(video.frame_id, "chest");
            assert_eq!(video.format, "h264");
            assert!(video.data.starts_with(&[0, 0, 0, 1]) || video.data.starts_with(&[0, 0, 1]));
            let timestamp = video.timestamp.ok_or("missing video timestamp")?;
            assert_eq!(
                u64::try_from(timestamp.seconds)? * 1_000_000_000 + u64::try_from(timestamp.nanos)?,
                message.log_time
            );
            timestamps.push(message.log_time);
            if timestamps.len() <= 3 {
                encoded_video.extend_from_slice(&video.data);
            }
        }
    }
    // Each picture change starts a segment, and every received frame was encoded.
    let starts: Vec<&Value> = segments
        .iter()
        .filter(|entry| entry.get("started_unix_nano").is_some())
        .collect();
    let ends: Vec<&Value> = segments
        .iter()
        .filter(|entry| entry.get("ended_unix_nano").is_some())
        .collect();
    assert_eq!((starts.len(), ends.len()), (3, 3));
    assert_eq!(starts[0]["segment"], "chest/2023-11-14T22-13-20-123456Z");
    assert_eq!(
        (starts[1]["width"].clone(), starts[1]["height"].clone()),
        (8.into(), 16.into())
    );
    assert_eq!(ends[0]["segment"], starts[0]["segment"]);
    assert_eq!(
        ends[0]["ended_unix_nano"],
        (first_at + 175_111_000).to_string()
    );
    assert_eq!(statistics.len(), 3);
    assert!(
        statistics
            .iter()
            .all(|entry| entry["received"] == entry["encoded"])
    );
    timestamps.sort_unstable();
    assert_eq!(
        timestamps,
        [
            first_at,
            first_at + 66_667_000,
            first_at + 175_111_000,
            first_at + 250_000_000,
            first_at + 350_000_000
        ]
    );
    let annex_b = directory.join("archive.h264");
    fs::write(&annex_b, encoded_video)?;
    assert_eq!(
        probe(&annex_b)?["frames"]
            .as_array()
            .ok_or("missing Annex B frames")?
            .len(),
        3
    );
    Ok(())
}
