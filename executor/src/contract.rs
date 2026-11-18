use crate::config::ExecutorConfigInput;

pub type Contract = hebbian_contracts::ExecutorContract<ExecutorConfigInput>;

#[must_use]
#[expect(
    clippy::expect_used,
    reason = "Schemars schemas contain only JSON-compatible data and cannot fail JSON serialization."
)]
pub fn schema_json() -> String {
    let schema = schemars::schema_for!(Contract);
    let mut text = serde_json::to_string_pretty(&schema).expect("a schema serializes");
    text.push('\n');
    text
}
