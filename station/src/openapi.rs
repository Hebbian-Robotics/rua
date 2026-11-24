use hebbian_contracts::{
    ActionSequenceRecord, ActionSequenceRequest, ActionSequenceStartRequest, CameraListing,
    ClientMessage, CommandAnswer, CommandCancellationRequest, CommandSubmissionRequest,
    CompilationArtifact, CompilationOutcome, ControlCapabilities, ControllerStatus,
    GroupCommandSubmissionRequest, LeaseAcquireRequest, LeaseCameraReport, LeaseGrant, LeaseToken,
    MediaStatusReport, ObservationAnswer, ObservationRequest, OperationStatus, PhoneInvite,
    PhoneInviteRequest, PosePreviewBatchRequest, PosePreviewBatchResponse, ReconnectRequest,
    RecordingStatus, RoomAccess, RunCompiledProgramRequest, RunEvent, RunSpansRequest,
    ServerMessage, StationInstance, StationState, StreamGoalUpdateRequest, TraceFile,
};
use schemars::generate::SchemaSettings;
use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

type SchemaOf = fn(&mut SchemaGenerator) -> Value;

fn schema_of<T: JsonSchema>(generator: &mut SchemaGenerator) -> Value {
    generator.subschema_for::<T>().to_value()
}

enum Reply {
    Json(SchemaOf),
    File(&'static [&'static str]),
    NoContent,
}

pub struct Route {
    pub operation: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    pub direct_media: bool,
    summary: &'static str,
    request: Option<SchemaOf>,
    reply: Reply,
    sdk: bool,
}

const fn route(
    operation: &'static str,
    method: &'static str,
    path: &'static str,
    summary: &'static str,
    request: Option<SchemaOf>,
    reply: Reply,
    sdk: bool,
) -> Route {
    Route {
        operation,
        method,
        path,
        direct_media: false,
        summary,
        request,
        reply,
        sdk,
    }
}

#[must_use]
#[allow(clippy::too_many_lines)]
pub fn routes() -> Vec<Route> {
    let table = [
        route(
            "stop_leased_arm_http",
            "post",
            "/arms/{arm}/stop",
            "Stop one arm while holding the lease.",
            Some(schema_of::<LeaseToken>),
            Reply::NoContent,
            true,
        ),
        route(
            "report_lease_camera_http",
            "post",
            "/leases/camera",
            "Report a required camera's availability.",
            Some(schema_of::<LeaseCameraReport>),
            Reply::NoContent,
            true,
        ),
        route(
            "release_lease_http",
            "post",
            "/leases/release",
            "Release a current station lease.",
            Some(schema_of::<LeaseToken>),
            Reply::NoContent,
            true,
        ),
        route(
            "renew_lease_http",
            "post",
            "/leases/renew",
            "Renew a current station lease.",
            Some(schema_of::<LeaseToken>),
            Reply::NoContent,
            true,
        ),
        route(
            "acquire_lease_http",
            "post",
            "/leases",
            "Acquire or take the station lease.",
            Some(schema_of::<LeaseAcquireRequest>),
            Reply::Json(schema_of::<LeaseGrant>),
            true,
        ),
        route(
            "preview_poses",
            "post",
            "/pose_previews",
            "Preview independent pose candidates for one arm in input order.",
            Some(schema_of::<PosePreviewBatchRequest>),
            Reply::Json(schema_of::<PosePreviewBatchResponse>),
            true,
        ),
        route(
            "stream_events",
            "get",
            "/events",
            "Stream station events with resumable event IDs.",
            None,
            Reply::File(&["text/event-stream"]),
            false,
        ),
        route(
            "get_station_instance",
            "get",
            "/instance",
            "Identify the current station process.",
            None,
            Reply::Json(schema_of::<StationInstance>),
            true,
        ),
        route(
            "get_health",
            "get",
            "/health",
            "Answers while the process runs.",
            None,
            Reply::Json(schema_of::<std::collections::BTreeMap<String, String>>),
            false,
        ),
        route(
            "list_traces",
            "get",
            "/traces",
            "The controller's trace files, newest first.",
            None,
            Reply::Json(schema_of::<Vec<TraceFile>>),
            true,
        ),
        route(
            "get_trace_file",
            "get",
            "/traces/{name}",
            "A trace file's bytes exactly as served: gzip-compressed.",
            None,
            Reply::File(&["application/gzip"]),
            true,
        ),
        route(
            "get_robot_model_asset",
            "get",
            "/model/{arm}/{asset}",
            "The configured arm's URDF as robot.urdf, or a referenced mesh (asset may contain slashes).",
            None,
            Reply::File(&["application/xml", "application/octet-stream"]),
            false,
        ),
        route(
            "get_state",
            "get",
            "/state",
            "The whole station.",
            None,
            Reply::Json(schema_of::<StationState>),
            true,
        ),
        route(
            "compile_action_sequence",
            "post",
            "/compilations/compile",
            "Compile an action sequence without acquiring control or moving hardware.",
            Some(schema_of::<ActionSequenceRequest>),
            Reply::Json(schema_of::<CompilationOutcome>),
            true,
        ),
        route(
            "import_compilation",
            "post",
            "/compilations/import",
            "Load an exported compilation by recompiling its source against this station.",
            Some(schema_of::<CompilationArtifact>),
            Reply::Json(schema_of::<CompilationOutcome>),
            true,
        ),
        route(
            "inspect_compilation",
            "get",
            "/compilations/{id}",
            "Inspect a retained station-owned compilation.",
            None,
            Reply::Json(schema_of::<CompilationArtifact>),
            true,
        ),
        route(
            "run_compiled_sequence_http",
            "post",
            "/compilations/run",
            "Run a retained compilation under a current lease.",
            Some(schema_of::<RunCompiledProgramRequest>),
            Reply::Json(schema_of::<String>),
            true,
        ),
        route(
            "start_action_sequence_http",
            "post",
            "/action_sequence",
            "Start an action sequence under a lease.",
            Some(schema_of::<ActionSequenceStartRequest>),
            Reply::Json(schema_of::<String>),
            true,
        ),
        route(
            "cancel_action_sequence_http",
            "post",
            "/action_sequence/{id}/cancel",
            "Cancel an action sequence.",
            None,
            Reply::NoContent,
            true,
        ),
        route(
            "get_current_action_sequence",
            "get",
            "/action_sequence",
            "The newest action sequence, if any.",
            None,
            Reply::Json(schema_of::<Option<ActionSequenceRecord>>),
            false,
        ),
        route(
            "get_action_sequence",
            "get",
            "/action_sequence/{id}",
            "A running or recently finished action sequence by identifier.",
            None,
            Reply::Json(schema_of::<ActionSequenceRecord>),
            true,
        ),
        route(
            "submit_group_command_http",
            "post",
            "/group_commands",
            "Submit a coordinated arm command.",
            Some(schema_of::<GroupCommandSubmissionRequest>),
            Reply::Json(schema_of::<CommandAnswer>),
            true,
        ),
        route(
            "submit_command_http",
            "post",
            "/commands",
            "Submit an individual arm command.",
            Some(schema_of::<CommandSubmissionRequest>),
            Reply::Json(schema_of::<CommandAnswer>),
            true,
        ),
        route(
            "cancel_operation_http",
            "post",
            "/operations/{id}/cancel",
            "Cancel an operation while holding its lease.",
            Some(schema_of::<CommandCancellationRequest>),
            Reply::NoContent,
            true,
        ),
        route(
            "capture_observation_http",
            "post",
            "/observations",
            "Capture selected arm state and camera frames under the same observation revisions.",
            Some(schema_of::<ObservationRequest>),
            Reply::Json(schema_of::<ObservationAnswer>),
            true,
        ),
        route(
            "get_operation",
            "get",
            "/operations/{id}",
            "The retained status and outcomes of an operation.",
            None,
            Reply::Json(schema_of::<OperationStatus>),
            true,
        ),
        route(
            "update_stream_goal_http",
            "post",
            "/operations/{id}/goal",
            "Update a live stream using its operation identifier and a newer goal sequence.",
            Some(schema_of::<StreamGoalUpdateRequest>),
            Reply::Json(schema_of::<CommandAnswer>),
            true,
        ),
        route(
            "record_run_event_http",
            "post",
            "/run/events",
            "Record a model-loop event.",
            Some(schema_of::<RunEvent>),
            Reply::NoContent,
            true,
        ),
        route(
            "replay_run_records",
            "get",
            "/run/records",
            "Replay retained run records after the `after` query cursor.",
            None,
            Reply::Json(schema_of::<Vec<ServerMessage>>),
            false,
        ),
        route(
            "record_run_spans_http",
            "post",
            "/run/spans",
            "Record model-loop span updates.",
            Some(schema_of::<RunSpansRequest>),
            Reply::NoContent,
            true,
        ),
        route(
            "get_live_run",
            "get",
            "/run",
            "The model loop's live telemetry.",
            None,
            Reply::Json(schema_of::<hebbian_contracts::LiveRun>),
            false,
        ),
        route(
            "get_recording_enabled",
            "get",
            "/recording/enabled",
            "Whether the station can record.",
            None,
            Reply::Json(schema_of::<bool>),
            false,
        ),
        route(
            "get_room_access",
            "get",
            "/media/access-token",
            "Watch-only LiveKit access to every camera in the room.",
            None,
            Reply::Json(schema_of::<RoomAccess>),
            false,
        ),
        route(
            "phone_invite",
            "post",
            "/media/phone-invites",
            "Create an invite from a loopback connection to stream a phone as camera `phone_<label>`.",
            Some(schema_of::<PhoneInviteRequest>),
            Reply::Json(schema_of::<PhoneInvite>),
            true,
        ),
        route(
            "get_control_capabilities",
            "get",
            "/control/capabilities",
            "Whether the station is configured for manual pose control.",
            None,
            Reply::Json(schema_of::<ControlCapabilities>),
            false,
        ),
        route(
            "get_controller_status",
            "get",
            "/controller",
            "The controller's availability and guard reports.",
            None,
            Reply::Json(schema_of::<ControllerStatus>),
            false,
        ),
        route(
            "reconnect_controller",
            "post",
            "/controller/reconnect",
            "Reconnect after the operator supports the arm.",
            Some(schema_of::<ReconnectRequest>),
            Reply::Json(schema_of::<ControllerStatus>),
            false,
        ),
        Route {
            direct_media: true,
            ..route(
                "get_media_status",
                "get",
                "/media/status",
                "The media service and the current state of each configured camera.",
                None,
                Reply::Json(schema_of::<MediaStatusReport>),
                false,
            )
        },
        route(
            "start_recording",
            "post",
            "/recording/start",
            "Start recording and return the recording's name.",
            None,
            Reply::Json(schema_of::<String>),
            true,
        ),
        route(
            "recording_status",
            "get",
            "/recording/status",
            "Whether the station is recording.",
            None,
            Reply::Json(schema_of::<RecordingStatus>),
            true,
        ),
        route(
            "stop_recording",
            "post",
            "/recording/stop",
            "Stop recording.",
            None,
            Reply::Json(schema_of::<()>),
            true,
        ),
        route(
            "stop_recording_for_lease",
            "post",
            "/recording/stop-for-lease",
            "Stop recording only while the token still holds the lease.",
            Some(schema_of::<LeaseToken>),
            Reply::Json(schema_of::<()>),
            true,
        ),
        route(
            "list_recordings",
            "get",
            "/recordings",
            "The recordings on disk.",
            None,
            Reply::Json(schema_of::<recordings::Recordings>),
            false,
        ),
        route(
            "get_recording_file",
            "get",
            "/recordings/{path}",
            "A file of a recording.",
            None,
            Reply::File(&["application/octet-stream"]),
            false,
        ),
        route(
            "delete_recording",
            "delete",
            "/recordings/{path}",
            "Delete a recording.",
            None,
            Reply::NoContent,
            false,
        ),
    ];
    let mut routes: Vec<Route> = table.into_iter().collect();
    for media in [
        Route {
            direct_media: true,
            ..route(
                "list_cameras",
                "get",
                "/cameras",
                "The cameras the media service knows.",
                None,
                Reply::Json(schema_of::<Vec<CameraListing>>),
                true,
            )
        },
        Route {
            direct_media: true,
            ..route(
                "get_camera",
                "get",
                "/cameras/{name}",
                "The newest JPEG frame from a camera, with `X-Captured-At-Unix-Ns` when known.",
                None,
                Reply::File(&["image/jpeg"]),
                true,
            )
        },
    ] {
        routes.push(media);
    }
    routes
}

fn path_parameters(path: &str) -> Vec<Value> {
    path.split('/')
        .filter_map(|segment| segment.strip_prefix('{')?.strip_suffix('}'))
        .map(|name| {
            let schema = if path == "/cameras/{name}" {
                json!({"type": "string", "pattern": hebbian_contracts::SnapshotName::PATTERN})
            } else {
                json!({"type":"string"})
            };
            json!({"name": name, "in": "path", "required": true, "schema": schema})
        })
        .collect()
}

fn response(reply: &Reply, generator: &mut SchemaGenerator) -> Value {
    match reply {
        Reply::Json(schema) => {
            json!({"200": {"description": "OK", "content": {"application/json": {"schema": schema(generator)}}}})
        }
        Reply::File(media_types) => {
            let content: Map<String, Value> = media_types
                .iter()
                .map(|media_type| {
                    (
                        (*media_type).to_owned(),
                        json!({"schema": {"type": "string", "format": "binary"}}),
                    )
                })
                .collect();
            json!({"200": {"description": "OK", "content": content}})
        }
        Reply::NoContent => json!({"204": {"description": "No content"}}),
    }
}

#[must_use]
pub fn openapi_json() -> String {
    let mut generator = SchemaSettings::draft2020_12()
        .with(|settings| settings.definitions_path = "/components/schemas".into())
        .into_generator();
    // WHY: These shared message types also drive SDK convenience APIs and SSE parsing.
    let _ = generator.subschema_for::<ClientMessage>();
    let _ = generator.subschema_for::<ServerMessage>();
    let _ = generator.subschema_for::<hebbian_contracts::CommandOutcome>();
    let mut paths: Map<String, Value> = Map::new();
    for route in routes() {
        let mut operation = json!({
            "operationId": route.operation,
            "summary": route.summary,
            "x-sdk": route.sdk,
            "x-server": "station",
        });
        if route.direct_media {
            operation["x-direct-media"] = json!(true);
        }
        let parameters = path_parameters(route.path);
        if !parameters.is_empty() {
            operation["parameters"] = Value::Array(parameters);
        }
        if let Some(request) = route.request {
            operation["requestBody"] = json!({
                "required": true,
                "content": {"application/json": {"schema": request(&mut generator)}},
            });
        }
        let mut responses = response(&route.reply, &mut generator);
        responses["default"] = json!({
            "description": "An error: {\"detail\": ...}",
            "content": {"application/json": {"schema": {
                "type": "object",
                "properties": {"detail": {"type": "string"}},
                "required": ["detail"],
            }}},
        });
        operation["responses"] = responses;
        paths
            .entry(route.path.to_owned())
            .or_insert_with(|| json!({}))[route.method] = operation;
    }
    let document = json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Hebbian robot-use station",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Generated by `hebbian-rua-server schema --openapi`.",
        },
        "paths": paths,
        "components": {"schemas": Value::Object(generator.take_definitions(true))},
    });
    #[expect(
        clippy::expect_used,
        reason = "The document is a serde_json Value, which cannot fail JSON serialization."
    )]
    let mut text = serde_json::to_string_pretty(&document).expect("a document serializes");
    text.push('\n');
    text
}
