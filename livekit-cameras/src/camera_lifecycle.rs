use hebbian_contracts::{CameraPublication, CameraStatus};
use livekit_camera_publisher::error::PublisherError as CaptureError;

use crate::{config::CameraConfiguration, error::PublisherError, usb_camera::UsbMatch};

pub(super) enum CaptureDecision {
    Open(CameraConfiguration),
    Retry(CameraStatus),
}

pub(super) fn after_lookup(
    outcome: Result<UsbMatch<CameraConfiguration>, PublisherError>,
) -> CaptureDecision {
    match outcome {
        Ok(UsbMatch::Unique(camera)) => CaptureDecision::Open(camera),
        Ok(UsbMatch::Disconnected) => CaptureDecision::Retry(CameraStatus::Disconnected),
        Ok(UsbMatch::Ambiguous) => CaptureDecision::Retry(CameraStatus::Ambiguous),
        Err(error) => CaptureDecision::Retry(capture_failed(error)),
    }
}

pub(super) fn after_capture(result: Result<(), CaptureError>) -> CameraStatus {
    match result {
        Ok(()) => CameraStatus::Disconnected,
        Err(CaptureError::CaptureTimeout(_)) => CameraStatus::Stalled,
        Err(error) => capture_failed(error),
    }
}

pub(super) fn capture_failed(error: impl std::fmt::Display) -> CameraStatus {
    CameraStatus::Failed {
        detail: error.to_string(),
    }
}

pub(super) enum PublicationDecision {
    End,
    Retry(CameraPublication),
}

pub(super) fn after_publication(
    result: Result<(), PublisherError>,
    stopping: bool,
) -> PublicationDecision {
    match (result, stopping) {
        (Ok(()), _) | (Err(_), true) => PublicationDecision::End,
        (Err(error), false) => PublicationDecision::Retry(CameraPublication::Reconnecting {
            detail: Some(error.to_string()),
        }),
    }
}

pub(super) enum ActiveCapture {
    WaitingForFrame(CameraPublication),
    ReceivingFrames(CameraPublication),
}

pub(super) enum CaptureEvent {
    FrameReceived,
    PublicationChanged(CameraPublication),
}

impl ActiveCapture {
    pub(super) fn transition(self, event: CaptureEvent) -> Self {
        match (self, event) {
            (
                Self::WaitingForFrame(publication) | Self::ReceivingFrames(publication),
                CaptureEvent::FrameReceived,
            ) => Self::ReceivingFrames(publication),
            (Self::WaitingForFrame(_), CaptureEvent::PublicationChanged(publication)) => {
                Self::WaitingForFrame(publication)
            }
            (Self::ReceivingFrames(_), CaptureEvent::PublicationChanged(publication)) => {
                Self::ReceivingFrames(publication)
            }
        }
    }

    pub(super) fn status(&self) -> CameraStatus {
        match self {
            Self::WaitingForFrame(_) => CameraStatus::Starting,
            Self::ReceivingFrames(publication) => CameraStatus::Capturing {
                publication: publication.clone(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    #[allow(
        clippy::panic,
        reason = "The test fails if publication recovery ends instead of retrying."
    )]
    fn room_failure_and_recovery_keep_capture_active() {
        let capture = ActiveCapture::WaitingForFrame(CameraPublication::Connecting)
            .transition(CaptureEvent::FrameReceived);
        let decision = after_publication(
            Err(CaptureError::Disconnected("connection lost".to_owned()).into()),
            false,
        );
        let PublicationDecision::Retry(publication) = decision else {
            panic!("a LiveKit disconnect should retry publication");
        };
        let capture = capture.transition(CaptureEvent::PublicationChanged(publication));
        assert!(matches!(
            capture.status(),
            CameraStatus::Capturing {
                publication: CameraPublication::Reconnecting { .. },
            }
        ));
        let capture = capture.transition(CaptureEvent::PublicationChanged(
            CameraPublication::Publishing,
        ));
        assert_eq!(
            capture.status(),
            CameraStatus::Capturing {
                publication: CameraPublication::Publishing
            }
        );
    }

    #[test]
    fn a_stalled_capture_can_recover_only_after_a_unique_lookup()
    -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            after_capture(Err(CaptureError::CaptureTimeout(Duration::from_secs(5)))),
            CameraStatus::Stalled
        );
        assert!(matches!(
            after_lookup(Ok(UsbMatch::Disconnected)),
            CaptureDecision::Retry(CameraStatus::Disconnected)
        ));
        assert!(matches!(
            after_lookup(Ok(UsbMatch::Ambiguous)),
            CaptureDecision::Retry(CameraStatus::Ambiguous)
        ));
        let new_device = CameraConfiguration::new("wrist".into(), "2".into())?;
        assert!(matches!(
            after_lookup(Ok(UsbMatch::Unique(new_device))),
            CaptureDecision::Open(camera) if camera.device() == "2"
        ));
        Ok(())
    }
}
