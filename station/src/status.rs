use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, ensure};
use hebbian_contracts::{ArmState, CameraName, RoomAccess, StationState};
use reqwest::{Client, StatusCode, Url, header};
use serde::Deserialize;
use tokio::{io::AsyncWriteExt, process::Command, time::Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
const FEEDBACK_WINDOW: Duration = Duration::from_secs(2);
const FEEDBACK_INTERVAL: Duration = Duration::from_millis(200);
const ROOM_PROBE_TIMEOUT: Duration = Duration::from_secs(8);

pub(crate) async fn check(
    server: &Url,
    media_server: Option<&Url>,
    camera: Option<&str>,
) -> Result<()> {
    let camera = camera.map(CameraName::try_from).transpose()?;
    let client = Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let state_url = endpoint(server, &["state"])?;
    let initial_state = read_state(&client, &state_url)
        .await
        .context("Station did not return arm state")?;
    println!("Station: responding");

    let (arms, snapshot, livekit) = tokio::join!(
        check_arm_feedback(&client, &state_url, &initial_state),
        async {
            let camera = camera?;
            let result = check_camera(&client, media_server.unwrap_or(server), camera).await;
            Some((camera, result))
        },
        check_livekit(&client, server),
    );
    let arms_ready = report("Arm feedback", arms);
    let camera_ready = snapshot
        .is_none_or(|(camera, result)| report(&format!("Camera {}", camera.as_str()), result));
    let livekit_ready = report("LiveKit", livekit);
    ensure!(
        arms_ready && camera_ready && livekit_ready,
        "Station checks failed"
    );
    Ok(())
}

fn endpoint(base: &Url, segments: &[&str]) -> Result<Url> {
    let mut url = base.clone();
    url.path_segments_mut()
        .map_err(|()| anyhow::anyhow!("Expected an HTTP server URL"))?
        .pop_if_empty()
        .extend(segments);
    Ok(url)
}

async fn read_state(client: &Client, state_url: &Url) -> Result<StationState> {
    Ok(client
        .get(state_url.clone())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?)
}

async fn check_arm_feedback(
    client: &Client,
    state_url: &Url,
    initial: &StationState,
) -> Result<String> {
    if initial.arms.is_empty() {
        anyhow::bail!("No arms reported");
    }
    let deadline = Instant::now() + FEEDBACK_WINDOW;
    loop {
        tokio::time::sleep_until((Instant::now() + FEEDBACK_INTERVAL).min(deadline)).await;
        let current = read_state(client, state_url).await?;
        let mut modes = Vec::with_capacity(initial.arms.len());
        let mut unavailable = Vec::new();
        for arm in &initial.arms {
            let sample = current.arms.iter().find(|sample| sample.arm == arm.arm);
            if let Some(sample) = sample {
                ensure!(
                    !sample.mode.refuses_everything(),
                    "{} is {:?} ({})",
                    sample.arm,
                    sample.mode,
                    sample.status
                );
                if feedback_advanced(arm, sample) {
                    modes.push(format!("{} ({:?})", sample.arm, sample.mode));
                    continue;
                }
            }
            unavailable.push(arm.arm.as_str());
        }
        if unavailable.is_empty() {
            return Ok(format!("fresh for {}", modes.join(", ")));
        }
        if Instant::now() >= deadline {
            anyhow::bail!("No fresh feedback from {}", unavailable.join(", "));
        }
    }
}

fn feedback_advanced(initial: &ArmState, current: &ArmState) -> bool {
    !current.measured_joints.is_empty()
        && current.sampled_at_unix_seconds.is_some_and(|timestamp| {
            initial
                .sampled_at_unix_seconds
                .is_none_or(|previous| timestamp > previous)
        })
}

async fn check_camera(client: &Client, server: &Url, name: CameraName<'_>) -> Result<String> {
    let url = endpoint(server, &["cameras", name.as_str()])?;
    let response = client.get(url).send().await?;
    if response.status() == StatusCode::NOT_FOUND {
        anyhow::bail!("No fresh snapshot");
    }
    let response = response.error_for_status()?;
    ensure!(
        response
            .headers()
            .get(header::CONTENT_TYPE)
            .is_some_and(|kind| kind == "image/jpeg")
            && response.bytes().await?.starts_with(&[0xff, 0xd8]),
        "Snapshot response was not a JPEG"
    );
    Ok("fresh snapshot available".to_owned())
}

#[derive(Deserialize)]
struct Refusal {
    detail: String,
}

async fn check_livekit(client: &Client, server: &Url) -> Result<String> {
    let response = client
        .get(endpoint(server, &["viewer", "api", "token"])?)
        .send()
        .await?;
    if response.status() == StatusCode::SERVICE_UNAVAILABLE {
        let refusal: Refusal = response.json().await.context("Could not get room access")?;
        if let Some(reason) = refusal.detail.strip_prefix("Cameras are off: ") {
            return Ok(format!("off. {reason}"));
        }
        anyhow::bail!("Room access is unavailable");
    }
    let access: RoomAccess = response.error_for_status()?.json().await?;
    let has_video = tokio::time::timeout(ROOM_PROBE_TIMEOUT, probe_room(&access))
        .await
        .context("Room join timed out")??;
    Ok(if has_video {
        "connected, video published"
    } else {
        "connected, waiting for cameras"
    }
    .to_owned())
}

async fn probe_room(access: &RoomAccess) -> Result<bool> {
    let executable = crate::media::find_media(std::env::var_os("HEBBIAN_MEDIA").map(PathBuf::from));
    let mut probe = Command::new(&executable)
        .arg("probe-livekit")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("Cannot run media companion {}", executable.display()))?;
    let mut stdin = probe
        .stdin
        .take()
        .context("Cannot open media companion input")?;
    stdin.write_all(&serde_json::to_vec(access)?).await?;
    drop(stdin);
    let output = probe.wait_with_output().await?;
    ensure!(
        output.status.success(),
        "Could not join the room. Check the station's LiveKit URL, project credentials and network"
    );
    serde_json::from_slice(&output.stdout).context("Invalid LiveKit probe response")
}

fn report(label: &str, result: Result<String>) -> bool {
    match result {
        Ok(message) => {
            println!("{label}: {message}");
            true
        }
        Err(error) => {
            println!("{label}: {error:#}");
            false
        }
    }
}
