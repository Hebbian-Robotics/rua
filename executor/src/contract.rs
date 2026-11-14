//! Everything that crosses the executor's socket or its config file, as one type
//! so that one JSON Schema describes it. `hebbian-robot-use-server schema` writes that
//! schema; the Python (pydantic) and TypeScript (zod) types are generated from it
//! and checked for drift.

use schemars::JsonSchema;

use crate::config::ExecutorConfig;
use hebbian_contracts::{Envelope, Frame};

/// The root of the generated schema; it exists only to carry the three types.
#[derive(JsonSchema)]
pub struct Contract {
    /// What a client sends.
    pub request: Envelope,
    /// What the executor sends back, one per line.
    pub frame: Frame,
    /// What the executor is started with.
    pub config: ExecutorConfig,
}

/// The contract's JSON Schema, pretty-printed with a trailing newline.
///
/// # Panics
/// Never in practice: a schema is always serializable.
#[must_use]
pub fn schema_json() -> String {
    let schema = schemars::schema_for!(Contract);
    let mut text = serde_json::to_string_pretty(&schema).expect("a schema serializes");
    text.push('\n');
    text
}
