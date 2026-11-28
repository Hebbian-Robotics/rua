//! Format model-facing results while retaining SI values in station records.

use hebbian_contracts::MODEL_RESPONSE_FIELDS;
use serde_json::{Map, Number, Value};

fn scaled(value: Value, factor: f64, round: bool) -> Value {
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| scaled(item, factor, round))
                .collect(),
        ),
        Value::Number(number) => number
            .as_f64()
            .and_then(|number| {
                let converted = number * factor;
                Number::from_f64(if round {
                    (converted * 10.0).round() / 10.0
                } else {
                    converted
                })
            })
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
    use hebbian_contracts::{CommandPhase, CommandRecord, RecordedCommand};
    if !record.contains_key("command_id") {
        return record;
    }
    let Ok(command_record) = serde_json::from_value::<CommandRecord>(Value::Object(record.clone()))
    else {
        return record;
    };
    let Some(mut fields) = record
        .remove("command")
        .and_then(|command| command.as_object().cloned())
    else {
        return record;
    };
    fields.remove("kind");
    if let RecordedCommand::PolicyReach { goal_pose } = &command_record.command {
        fields.remove("goal_pose");
        fields.insert(
            "goal_position".into(),
            serde_json::json!(goal_pose.position),
        );
        fields.insert(
            "goal_quaternion".into(),
            serde_json::json!(goal_pose.quaternion),
        );
    }
    if let Some(status) = command_record.goal_status() {
        fields.insert("goal_status".into(), serde_json::json!(status));
    }
    match &command_record.phase {
        CommandPhase::Pending => {}
        CommandPhase::Finished {
            completion,
            achieved,
        } => {
            fields.insert("completion".into(), serde_json::json!(completion));
            fields.insert("achieved".into(), serde_json::json!(achieved));
        }
        CommandPhase::NotStarted {
            detail,
            finished_unix_nano,
            ..
        }
        | CommandPhase::OutcomeUnknown {
            detail,
            finished_unix_nano,
        } => {
            fields.insert("completion".into(), serde_json::json!({
                "outcome": command_record.outcome(), "detail": detail, "finished_unix_nano": finished_unix_nano,
            }));
            if matches!(command_record.phase, CommandPhase::NotStarted { .. })
                && command_record.goal_status().is_none()
            {
                fields.insert("phase".into(), "not_started".into());
            }
        }
    }
    record.remove("phase");
    record.extend(fields);
    record
}

// INVARIANT: Earlier steps retain the command outcome and limitation; the final step
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
                        MODEL_RESPONSE_FIELDS.earlier_step,
                    )),
                );
                concise_output(Value::Object(compact), ConciseOptions::default())
            })
            .collect(),
    )
}

fn add_waypoint_reversal_hint(summary: &mut Map<String, Value>) {
    let Some(failure) = summary
        .get_mut("phase")
        .and_then(|phase| phase.get_mut("failure"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    // WHY: The station reports this as text, so match the known error exactly.
    let is_reversal = failure.get("stage").and_then(Value::as_str) == Some("preparation")
        && failure.get("reason").and_then(Value::as_str) == Some("invalid_target")
        && failure
            .get("detail")
            .and_then(Value::as_str)
            .and_then(|detail| detail.strip_prefix("Waypoint "))
            .and_then(|detail| {
                detail.strip_suffix(" requires a stop or reversal; use a separate step")
            })
            .is_some_and(|index| {
                !index.is_empty() && index.bytes().all(|byte| byte.is_ascii_digit())
            });
    if is_reversal {
        failure.insert("hint".into(), Value::String(
            "Waypoint index is zero-based. Split this move into two steps at that waypoint; each step can still have waypoints.".into(),
        ));
    }
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
            let mut state = keep_fields(&object, MODEL_RESPONSE_FIELDS.state);
            if options.keep_station_context {
                state.extend(keep_fields(&object, MODEL_RESPONSE_FIELDS.station_context));
            }
            let arms = object
                .get("arms")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|arm| {
                    let fields = arm
                        .as_object()
                        .map(|arm| keep_fields(arm, MODEL_RESPONSE_FIELDS.arm))
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
            add_waypoint_reversal_hint(&mut summary);
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
                .filter(|(key, _)| !MODEL_RESPONSE_FIELDS.diagnostic.contains(&key.as_str()))
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
    if field == "approach" {
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
                    formatted.insert(
                        name.to_owned(),
                        scaled(value, factor, name != "jaw_yaw_degrees"),
                    );
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
            let scale = if field.contains("unix_seconds") || matches!(field, "sampled_at") {
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
