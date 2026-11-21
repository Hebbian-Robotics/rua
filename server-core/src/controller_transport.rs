// INVARIANT: Bind each connection to one controller instance. Never replay requests after disconnect: resolve outstanding work as interrupted or unknown so old commands cannot execute on a replacement.

use std::io::{self, Read, Write};

use crate::action_step::PreparedArmAction;
use hebbian_contracts::{
    ArmMove, ArmRecord, Command, CommandRecord, Event, RejectionReason, TraceContext,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ControllerInstanceId(pub [u8; 16]);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub struct ControllerRequestId(pub u64);

#[derive(Debug, Deserialize, Serialize)]
pub enum ControllerRequest {
    ControlHandoffReadiness,
    CancelCommand {
        command_id: String,
    },
    Submit {
        command: Command,
        trace_context: Option<TraceContext>,
    },
    ActionStep {
        actions: Vec<PreparedArmAction>,
        trace_context: Option<TraceContext>,
    },
    MoveArms {
        label: String,
        moves: Vec<ArmMove>,
        trace_context: Option<TraceContext>,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub enum ControllerAnswer {
    ControlHandoffReadiness(bool),
    CancelledCommand,
    Submitted(Box<CommandRecord>),
    MovedArms(Vec<CommandRecord>),
    ActionStep(Vec<ArmRecord>),
    Refused {
        reason: RejectionReason,
        detail: String,
    },
}

#[derive(Debug, Deserialize, Serialize)]
pub enum ControllerMessage {
    Ready,
    Shutdown,
    Request {
        request_id: ControllerRequestId,
        arm: String,
        request: ControllerRequest,
    },
    Answer {
        request_id: ControllerRequestId,
        answer: ControllerAnswer,
    },
    Event {
        arm: String,
        event: Box<Event>,
    },
}

#[derive(Deserialize, Serialize)]
struct ControllerEnvelope {
    instance: ControllerInstanceId,
    message: ControllerMessage,
}

// CONSTRAINT: Bound allocations even when a peer sends an invalid or corrupt length.
const MAXIMUM_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

// CONSTRAINT: Framed send/receive perform blocking I/O and belong on transport threads, never the motor loop.
pub struct ControllerTransport<Stream> {
    stream: Stream,
    instance: ControllerInstanceId,
}

impl<Stream> ControllerTransport<Stream> {
    #[must_use]
    pub fn new(stream: Stream, instance: ControllerInstanceId) -> Self {
        Self { stream, instance }
    }
}

impl<Stream: Write> ControllerTransport<Stream> {
    pub fn send(&mut self, message: ControllerMessage) -> io::Result<()> {
        let body = serde_json::to_vec(&ControllerEnvelope {
            instance: self.instance,
            message,
        })?;
        if body.len() > MAXIMUM_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Controller message is too large",
            ));
        }
        let length = u32::try_from(body.len())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        self.stream.write_all(&length.to_be_bytes())?;
        self.stream.write_all(&body)?;
        self.stream.flush()
    }
}

impl<Stream: Read> ControllerTransport<Stream> {
    pub fn receive(&mut self) -> io::Result<ControllerMessage> {
        let mut length = [0_u8; 4];
        self.stream.read_exact(&mut length)?;
        let length = u32::from_be_bytes(length) as usize;
        if length > MAXIMUM_MESSAGE_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Controller message is too large",
            ));
        }
        let mut body = vec![0; length];
        self.stream.read_exact(&mut body)?;
        let envelope: ControllerEnvelope = serde_json::from_slice(&body)?;
        if envelope.instance != self.instance {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Controller instance does not match the active connection",
            ));
        }
        Ok(envelope.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    #[test]
    fn a_command_reaches_only_its_controller_instance() {
        let (station, controller) = UnixStream::pair().unwrap();
        let mut station = ControllerTransport::new(station, ControllerInstanceId([1; 16]));
        let mut controller = ControllerTransport::new(controller, ControllerInstanceId([1; 16]));
        station
            .send(ControllerMessage::Request {
                request_id: ControllerRequestId(3),
                arm: "left".into(),
                request: ControllerRequest::Submit {
                    command: Command::Enable,
                    trace_context: None,
                },
            })
            .unwrap();
        assert!(matches!(
            controller.receive().unwrap(),
            ControllerMessage::Request {
                request_id: ControllerRequestId(3),
                request: ControllerRequest::Submit {
                    command: Command::Enable,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn a_message_from_an_earlier_instance_is_rejected() {
        let (old_station, new_controller) = UnixStream::pair().unwrap();
        let mut old_station = ControllerTransport::new(old_station, ControllerInstanceId([1; 16]));
        let mut new_controller =
            ControllerTransport::new(new_controller, ControllerInstanceId([2; 16]));
        old_station
            .send(ControllerMessage::Request {
                request_id: ControllerRequestId(1),
                arm: "left".into(),
                request: ControllerRequest::ControlHandoffReadiness,
            })
            .unwrap();
        assert_eq!(
            new_controller.receive().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
