use std::process::ExitCode;

use hebbian_robot_use_sdk::contracts::{LeaseStatus, RejectionReason};
use hebbian_robot_use_sdk::{ConciseOptions, Error, concise_response_with};
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Debug)]
pub enum Failure {
    Sdk {
        error: Error,
        holder: Option<Box<LeaseStatus>>,
    },
    ActionSequenceStopped {
        detail: String,
        record: Value,
    },
    Usage(String),
    Io(String),
}

impl From<Error> for Failure {
    fn from(error: Error) -> Self {
        Self::Sdk {
            error,
            holder: None,
        }
    }
}

pub fn json_value(value: &impl Serialize) -> Result<Value, Failure> {
    serde_json::to_value(value).map_err(|error| Failure::Io(error.to_string()))
}

// WHY: A station state keeps its description and lease holder, which an agent has no other way
// to learn; no other CLI output has that shape.
pub fn print_compactable(value: Value, raw: bool) -> String {
    if raw {
        value.to_string()
    } else {
        concise_response_with(
            value,
            ConciseOptions {
                keep_station_context: true,
            },
        )
        .to_string()
    }
}

impl Failure {
    pub fn exit_code(&self) -> ExitCode {
        ExitCode::from(match self {
            Self::Usage(_) => 2,
            Self::Io(_) | Self::ActionSequenceStopped { .. } => 1,
            Self::Sdk { error, .. } => match error {
                Error::Connection(_) | Error::Timeout(_) => 3,
                Error::LeaseHeldElsewhere(_) => 4,
                Error::Refused { reason, .. } if *reason == RejectionReason::LeaseHeldElsewhere => {
                    4
                }
                Error::Refused { .. }
                | Error::Incomplete { .. }
                | Error::CameraUnavailable { .. }
                | Error::Http { .. } => 1,
            },
        })
    }

    pub fn to_json(&self) -> Value {
        let mut error = serde_json::Map::new();
        let mut put = |key: &str, value: Value| {
            error.insert(key.to_owned(), value);
        };
        match self {
            Self::Usage(detail) => {
                put("kind", json!("usage"));
                put("detail", json!(detail));
            }
            Self::ActionSequenceStopped { detail, record } => {
                put("kind", json!("incomplete"));
                put("detail", json!(detail));
                put("record", record.clone());
            }
            Self::Io(detail) => {
                put("kind", json!("io"));
                put("detail", json!(detail));
            }
            Self::Sdk { error, holder } => {
                put("detail", json!(error.to_string()));
                match error {
                    Error::Connection(_) => put("kind", json!("connection")),
                    Error::Timeout(_) => put("kind", json!("timeout")),
                    Error::LeaseHeldElsewhere(status) => {
                        put("kind", json!("lease_held_elsewhere"));
                        put("holder", json!(status));
                    }
                    Error::Refused { reason, detail } => {
                        let elsewhere = *reason == RejectionReason::LeaseHeldElsewhere;
                        put(
                            "kind",
                            json!(if elsewhere {
                                "lease_held_elsewhere"
                            } else {
                                "refused"
                            }),
                        );
                        put("reason", json!(reason));
                        put("detail", json!(detail));
                        if elsewhere {
                            put("holder", json!(holder));
                        }
                    }
                    Error::Incomplete {
                        outcome, record, ..
                    } => {
                        put("kind", json!("incomplete"));
                        put("outcome", json!(outcome));
                        put("record", json!(record));
                    }
                    Error::CameraUnavailable {
                        name,
                        listed,
                        last_error,
                    } => {
                        put("kind", json!("camera_unavailable"));
                        put("camera", json!(name));
                        put("listed", json!(listed));
                        put("last_error", json!(last_error));
                    }
                    Error::Http { status, body } => {
                        put("kind", json!("http"));
                        put("status", json!(status));
                        put("detail", json!(body));
                    }
                }
            }
        }
        json!({ "error": error })
    }
}
