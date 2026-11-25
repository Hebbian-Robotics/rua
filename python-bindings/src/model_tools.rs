use std::{collections::BTreeMap, sync::Arc};

use crate::{error::station_error, lease::NativeLease};
use hebbian_rua::{
    LeaseGuard, RobotToolCall, RobotToolOutcome, RobotTools,
    contracts::{
        RejectionReason, RobotToolCapability, SequenceAction, SequenceScope,
        SequenceToolConfiguration,
    },
};
use pyo3::{
    exceptions::PyValueError,
    prelude::*,
    types::{PyBytes, PyDict},
};
use pyo3_async_runtimes::tokio::future_into_py;
use pythonize::{depythonize, pythonize};
use std::collections::BTreeSet;

#[derive(serde::Deserialize)]
struct ToolConfiguration {
    tools: Vec<String>,
    actions: BTreeSet<SequenceAction>,
    arms: Option<Vec<String>>,
}

impl ToolConfiguration {
    fn capabilities(self) -> PyResult<Vec<RobotToolCapability>> {
        let sequence = SequenceToolConfiguration {
            scope: self
                .arms
                .map_or(SequenceScope::SingleArm, SequenceScope::NamedArms),
            actions: self.actions,
        };
        let available: BTreeMap<_, _> = hebbian_rua::contracts::all_robot_tool_capabilities()
            .into_iter()
            .map(|capability| (capability.name(), capability))
            .collect();
        self.tools
            .into_iter()
            .map(|name| match available.get(name.as_str()) {
                Some(RobotToolCapability::ActionSequence(_)) => {
                    sequence.validate().map_err(PyValueError::new_err)?;
                    Ok(RobotToolCapability::ActionSequence(sequence.clone()))
                }
                Some(capability) => Ok(capability.clone()),
                None => Err(PyValueError::new_err(format!("Unknown tool {name}"))),
            })
            .collect()
    }
}

#[pyfunction]
pub fn tool_names() -> Vec<&'static str> {
    hebbian_rua::contracts::all_robot_tool_capabilities()
        .iter()
        .map(RobotToolCapability::name)
        .collect()
}

#[pyfunction]
pub fn tool_catalog<'py>(configuration: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let capabilities = depythonize::<ToolConfiguration>(configuration)?.capabilities()?;
    let definitions = capabilities
        .iter()
        .map(RobotToolCapability::definition)
        .collect::<Result<Vec<_>, _>>()
        .map_err(PyValueError::new_err)?;
    Ok(pythonize(configuration.py(), &definitions)?)
}

#[pyclass(module = "hebbian_rua.bindings._native")]
pub struct NativeTools {
    tools: RobotTools<Arc<LeaseGuard>>,
}

#[pymethods]
impl NativeTools {
    #[new]
    fn new(
        lease: &NativeLease,
        arm: String,
        cameras: Vec<String>,
        optional_cameras: Vec<String>,
    ) -> PyResult<Self> {
        Ok(Self {
            tools: RobotTools::new(lease.current()?, arm)
                .with_cameras(cameras)
                .with_optional_cameras(optional_cameras),
        })
    }

    fn configured(&self, configuration: &Bound<'_, PyAny>) -> PyResult<Self> {
        let capabilities = depythonize::<ToolConfiguration>(configuration)?.capabilities()?;
        Ok(Self {
            tools: self
                .tools
                .clone()
                .map_tools(|_| capabilities)
                .map_err(PyValueError::new_err)?,
        })
    }

    fn history<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        Ok(pythonize(py, &self.tools.sequence_images())?)
    }

    fn context<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let tools = self.tools.clone();
        future_into_py(py, async move {
            let context = tools.context().await.map_err(station_error)?;
            Python::attach(|py| Ok(pythonize(py, &context)?.unbind()))
        })
    }

    fn execute<'py>(
        &self,
        py: Python<'py>,
        call_id: String,
        name: String,
        arguments: &Bound<'_, PyAny>,
        options: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let call = RobotToolCall {
            id: call_id,
            name,
            arguments: depythonize(arguments)?,
        };
        let options = depythonize(options)?;
        let tools = self.tools.clone();
        future_into_py(py, async move {
            let result = tools.execute(call, options).await.map_err(station_error)?;
            let disposition = match &result.outcome {
                RobotToolOutcome::Returned(_) => "returned",
                RobotToolOutcome::Faulted(_)
                | RobotToolOutcome::Refused {
                    reason: RejectionReason::Faulted,
                    ..
                } => "faulted",
                RobotToolOutcome::Interrupted(_)
                | RobotToolOutcome::Failed(_)
                | RobotToolOutcome::Refused {
                    reason:
                        RejectionReason::StaleRevision
                        | RejectionReason::Interlocked
                        | RejectionReason::LeaseHeldElsewhere,
                    ..
                } => "interrupted",
                RobotToolOutcome::InvalidInput(_)
                | RobotToolOutcome::NotExecuted
                | RobotToolOutcome::Refused { .. }
                | RobotToolOutcome::Incomplete(_) => "incomplete",
            };
            Python::attach(|py| {
                let output = PyDict::new(py);
                if let RobotToolOutcome::InvalidInput(detail) = &result.outcome {
                    output.set_item("data", format!("Invalid input: {detail}"))?;
                } else {
                    output.set_item("data", pythonize(py, &result.data())?)?;
                }
                output.set_item("disposition", disposition)?;
                let cameras = result
                    .cameras
                    .iter()
                    .map(|(name, jpeg)| (name, PyBytes::new(py, jpeg)))
                    .collect::<BTreeMap<_, _>>();
                output.set_item("cameras", cameras)?;
                output.set_item("sequence_images", pythonize(py, &result.sequence_images)?)?;
                Ok(output.unbind())
            })
        })
    }
}
