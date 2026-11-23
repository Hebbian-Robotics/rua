//! Format model-facing results while retaining SI values in station records.

use serde_json::{Map, Number, Value};

fn scaled(value: Value, factor: f64) -> Value {
    match value {
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|item| scaled(item, factor)).collect())
        }
        Value::Number(number) => number
            .as_f64()
            .and_then(|number| Number::from_f64((number * factor * 10.0).round() / 10.0))
            .map_or(Value::Null, Value::Number),
        other => other,
    }
}

/// What the concise layer keeps. The default is the model view; the CLI's `state` also keeps
/// the station context an agent has no other way to learn.
#[derive(Debug, Clone, Copy, Default)]
pub struct ConciseOptions {
    /// Keep a station state's `description` (frames and conventions) and `lease` holder.
    /// Observations leave them out because the model is given the conventions separately.
    pub keep_station_context: bool,
}

const STATE_FIELDS: [&str; 4] = ["robot", "joint_names", "max_joint_speed", "surface_guard"];
const STATION_CONTEXT_FIELDS: [&str; 2] = ["description", "lease"];
const ARM_FIELDS: [&str; 12] = [
    "arm",
    "mode",
    "status",
    "fault",
    "active_trip",
    "interlocks",
    "recovery_issue",
    "revision",
    "sampled_at_unix_seconds",
    "measured_joints",
    "tool",
    "gripper_opening",
];
const DIAGNOSTIC_FIELDS: [&str; 11] = [
    "id",
    "command_id",
    "group_id",
    "stream_command_id",
    "sequence_id",
    "action_sequence_id",
    "checkpoints",
    "trace_context",
    "model_points",
    "accepted_at",
    "finished_at",
];

fn keep_fields(object: &Map<String, Value>, kept: &[&str]) -> Map<String, Value> {
    kept.iter()
        .filter_map(|key| {
            object
                .get(*key)
                .map(|value| ((*key).to_owned(), value.clone()))
        })
        .collect()
}

fn model_command_record(mut record: Map<String, Value>) -> Map<String, Value> {
    let Some((command, phase)) = record
        .get("command")
        .and_then(Value::as_object)
        .zip(record.get("phase").and_then(Value::as_object))
        .filter(|(command, phase)| {
            record.get("command_id").is_some_and(Value::is_string)
                && command.get("kind").is_some_and(Value::is_string)
                && matches!(
                    phase.get("kind").and_then(Value::as_str),
                    Some("pending" | "not_started" | "finished" | "outcome_unknown")
                )
        })
    else {
        return record;
    };
    let is_move = command["kind"] == "move";
    let target = command
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "kind" | "target_pose"));
    let mut fields: Map<String, Value> = target
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    if let Some(pose) = command.get("target_pose").and_then(Value::as_object) {
        fields.insert("target_position".into(), pose["position"].clone());
        fields.insert("target_quaternion".into(), pose["quaternion"].clone());
    }
    match phase["kind"].as_str() {
        Some("finished") => {
            let completion = &phase["completion"];
            fields.insert("completion".into(), completion.clone());
            fields.insert("achieved".into(), phase["achieved"].clone());
            if is_move {
                let status = match completion["outcome"].as_str() {
                    Some("completed")
                        if ["pose_limited", "path_limited"].iter().any(|field| {
                            command.get(*field).is_some_and(|value| !value.is_null())
                        }) =>
                    {
                        "limited"
                    }
                    Some("completed") => "reached",
                    Some("stopped_short") => "stopped_short",
                    _ => "partial",
                };
                fields.insert("target_status".into(), status.into());
            }
        }
        Some(kind @ ("not_started" | "outcome_unknown")) => {
            let outcome = if kind == "outcome_unknown" {
                "interrupted".into()
            } else {
                phase["outcome"].clone()
            };
            fields.insert(
                "completion".into(),
                serde_json::json!({
                    "outcome": outcome,
                    "detail": phase["detail"],
                    "finished_at": phase["finished_at"],
                }),
            );
            if kind == "not_started" {
                fields.insert(
                    if is_move { "target_status" } else { "phase" }.into(),
                    "not_started".into(),
                );
            }
        }
        _ => {}
    }
    record.remove("command");
    record.remove("phase");
    record.extend(fields);
    record
}

// Earlier steps retain the command outcome and limitation; the final step
// includes full concise details for the model's next decision.
fn compact_earlier_step(value: Value) -> Value {
    let Value::Array(entries) = value else {
        return concise_output(value, ConciseOptions::default());
    };
    Value::Array(
        entries
            .into_iter()
            .map(|entry| {
                let Some(object) = entry.as_object() else {
                    return concise_output(entry, ConciseOptions::default());
                };
                let Some(record) = object.get("record").and_then(Value::as_object) else {
                    return concise_output(entry, ConciseOptions::default());
                };
                let mut compact = keep_fields(object, &["arm"]);
                compact.insert(
                    "record".to_owned(),
                    Value::Object(keep_fields(
                        &model_command_record(record.clone()),
                        &[
                            "label",
                            "completion",
                            "phase",
                            "target_status",
                            "pose_limited",
                            "path_limited",
                        ],
                    )),
                );
                concise_output(Value::Object(compact), ConciseOptions::default())
            })
            .collect(),
    )
}

/// Keep outcomes and actionable state; omit history and rendering geometry. The shared
/// specification is `tests/fixtures/model-concise.json`, also run by Python and TypeScript.
#[must_use]
pub fn concise_output(value: Value, options: ConciseOptions) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| concise_output(item, options))
                .collect(),
        ),
        Value::Object(object)
            if object.contains_key("arms") && object.contains_key("joint_names") =>
        {
            let mut state = keep_fields(&object, &STATE_FIELDS);
            if options.keep_station_context {
                state.extend(keep_fields(&object, &STATION_CONTEXT_FIELDS));
            }
            let arms = object
                .get("arms")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|arm| {
                    let fields = arm
                        .as_object()
                        .map(|arm| keep_fields(arm, &ARM_FIELDS))
                        .unwrap_or_default();
                    concise_output(Value::Object(fields), ConciseOptions::default())
                })
                .collect();
            state.insert("arms".to_owned(), Value::Array(arms));
            Value::Object(state)
        }
        Value::Object(object)
            if object.contains_key("requested_steps") && object.contains_key("steps") =>
        {
            let steps = object["steps"].as_array().cloned().unwrap_or_default();
            let mut summary = keep_fields(&object, &["arms", "phase"]);
            summary.insert(
                "requested_step_count".to_owned(),
                object["requested_steps"]
                    .as_array()
                    .map_or(0, Vec::len)
                    .into(),
            );
            summary.insert("recorded_step_count".to_owned(), steps.len().into());
            let last_index = steps.len().saturating_sub(1);
            summary.insert(
                "steps".to_owned(),
                Value::Array(
                    steps
                        .into_iter()
                        .enumerate()
                        .map(|(index, step)| {
                            if index == last_index {
                                concise_output(step, ConciseOptions::default())
                            } else {
                                compact_earlier_step(step)
                            }
                        })
                        .collect(),
                ),
            );
            Value::Object(summary)
        }
        Value::Object(object) => Value::Object(
            model_command_record(object)
                .into_iter()
                .filter(|(key, _)| !DIAGNOSTIC_FIELDS.contains(&key.as_str()))
                .map(|(key, value)| (key, concise_output(value, options)))
                .collect(),
        ),
        other => other,
    }
}

/// The model view of a station record: the concise layer, then millimetres and degrees,
/// rounded, with null fields dropped. Inputs to the station stay in metres and radians.
#[must_use]
pub fn concise_response(value: Value) -> Value {
    concise_response_with(value, ConciseOptions::default())
}

/// [`concise_response`] with explicit [`ConciseOptions`].
#[must_use]
pub fn concise_response_with(value: Value, options: ConciseOptions) -> Value {
    model_response(concise_output(value, options))
}

/// Convert an SI payload to readable model units without summarizing its history.
/// Compose with [`concise_output`] or your own payload transforms before serialization.
#[must_use]
pub fn model_response(value: Value) -> Value {
    format_model_field(value, "")
}

fn model_unit_field(field: &str) -> Option<(&'static str, f64)> {
    hebbian_contracts::MODEL_OUTPUT_UNITS.iter().find_map(
        |&(station_field, model_field, factor)| {
            (station_field == field).then_some((model_field, factor))
        },
    )
}

fn format_model_field(value: Value, field: &str) -> Value {
    if matches!(field, "approach" | "jaw_yaw_degrees") {
        return value;
    }
    match value {
        Value::Object(object) => {
            let object = model_command_record(object);
            let mut formatted = Map::new();
            for (key, value) in object {
                if value.is_null()
                    || (key == "checkpoints" && value.as_array().is_some_and(Vec::is_empty))
                {
                    continue;
                }
                if let Some((name, factor)) = model_unit_field(&key) {
                    formatted.insert(name.to_owned(), scaled(value, factor));
                } else {
                    formatted.insert(key.clone(), format_model_field(value, &key));
                }
            }
            Value::Object(formatted)
        }
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| format_model_field(item, field))
                .collect(),
        ),
        Value::Number(number) if number.is_f64() => {
            let scale = if field.contains("unix_seconds")
                || matches!(field, "accepted_at" | "sampled_at" | "finished_at")
            {
                1000.0
            } else {
                10_000.0
            };
            number
                .as_f64()
                .and_then(|value| Number::from_f64((value * scale).round() / scale))
                .map_or(Value::Null, Value::Number)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{ConciseOptions, concise_output, concise_response_with};

    fn fixtures(text: &str) -> Vec<serde_json::Value> {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn model_responses_match_the_shared_unit_and_precision_examples() {
        for fixture in fixtures(include_str!("../../tests/fixtures/model-response.json")) {
            assert_eq!(
                crate::model_response(fixture["input"].clone()),
                fixture["expected"]
            );
        }
    }

    #[test]
    fn concise_views_match_the_shared_examples() {
        for fixture in fixtures(include_str!("../../tests/fixtures/model-concise.json")) {
            let options = ConciseOptions {
                keep_station_context: fixture["keep_station_context"].as_bool().unwrap(),
            };
            let name = &fixture["name"];
            assert_eq!(
                concise_output(fixture["input"].clone(), options),
                fixture["concise"],
                "{name}"
            );
            assert_eq!(
                concise_response_with(fixture["input"].clone(), options),
                fixture["expected"],
                "{name}"
            );
        }
    }
}
