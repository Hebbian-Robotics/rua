//! Print model-facing definitions or shared context for SDK generation.
fn main() -> Result<(), serde_json::Error> {
    let value = match std::env::args().nth(1).as_deref() {
        Some("--outcomes") => {
            let schema = schemars::schema_for!(hebbian_contracts::CommandOutcome);
            let names: Vec<String> = serde_json::from_value(schema.as_value()["enum"].clone())?;
            let mut rules = serde_json::Map::new();
            for name in names {
                let outcome: hebbian_contracts::CommandOutcome =
                    serde_json::from_value(name.clone().into())?;
                rules.insert(
                    name,
                    {
                        let class = hebbian_contracts::ModelOutcome::from(outcome);
                        serde_json::json!({"class": class, "target": class.goal_status(false), "limited_target": class.goal_status(true)})
                    },
                );
            }
            let incomplete = hebbian_contracts::ModelOutcome::Incomplete;
            rules.insert("pending".into(), serde_json::json!({"class": incomplete, "target": incomplete.goal_status(false), "limited_target": incomplete.goal_status(true)}));
            serde_json::json!({"commands": rules, "status_priority": hebbian_contracts::MODEL_GROUP_STATUSES})
        }
        Some("--output-units") => serde_json::to_value(hebbian_contracts::MODEL_OUTPUT_UNITS)?,
        Some("--response-fields") => {
            serde_json::to_value(hebbian_contracts::MODEL_RESPONSE_FIELDS)?
        }
        Some("--context") => hebbian_contracts::robot_tool_context(),
        Some("--input-units") => serde_json::to_value(hebbian_contracts::MODEL_INPUT_UNITS)?,
        _ => serde_json::to_value(hebbian_contracts::robot_tool_catalog())?,
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
