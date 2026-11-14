//! Print model-facing definitions or shared context for SDK generation.
fn main() -> Result<(), serde_json::Error> {
    let value = if std::env::args().nth(1).as_deref() == Some("--context") {
        hebbian_contracts::robot_tool_context()
    } else {
        serde_json::to_value(hebbian_contracts::robot_tool_catalog())?
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
