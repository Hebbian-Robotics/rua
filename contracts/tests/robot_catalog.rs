//! The distributable artifacts must describe the current Rust catalog.

#[test]
fn generated_robot_artifacts_match_contract() -> Result<(), Box<dyn std::error::Error>> {
    let catalog: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/robot-tools.json"))?;
    let context: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/robot-context.json"))?;
    let input_units: serde_json::Value =
        serde_json::from_str(include_str!("../../schemas/robot-tool-input-units.json"))?;
    assert_eq!(
        catalog,
        serde_json::to_value(hebbian_contracts::robot_tool_catalog())?,
        "regenerate the robot tool catalog"
    );
    assert_eq!(
        context,
        hebbian_contracts::robot_tool_context(),
        "regenerate the robot context"
    );
    assert_eq!(
        input_units,
        serde_json::to_value(hebbian_contracts::MODEL_INPUT_UNITS)?,
        "regenerate the robot tool input units"
    );
    Ok(())
}
