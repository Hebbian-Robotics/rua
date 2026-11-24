mod lease;
mod model_tools;
mod station;
use hebbian_rua::ConciseOptions;
use pyo3::prelude::*;
use pythonize::{depythonize, pythonize};

#[pyfunction]
fn model_response<'py>(value: &Bound<'py, PyAny>) -> PyResult<Bound<'py, PyAny>> {
    let response = hebbian_rua::model_response(depythonize(value)?);
    Ok(pythonize(value.py(), &response)?)
}

#[pyfunction(signature = (value, *, keep_station_context = false))]
fn concise_output<'py>(
    value: &Bound<'py, PyAny>,
    keep_station_context: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let response = hebbian_rua::concise_output(
        depythonize(value)?,
        ConciseOptions {
            keep_station_context,
        },
    );
    Ok(pythonize(value.py(), &response)?)
}

#[pyfunction(signature = (value, *, keep_station_context = false))]
fn concise_response<'py>(
    value: &Bound<'py, PyAny>,
    keep_station_context: bool,
) -> PyResult<Bound<'py, PyAny>> {
    let response = hebbian_rua::concise_response_with(
        depythonize(value)?,
        ConciseOptions {
            keep_station_context,
        },
    );
    Ok(pythonize(value.py(), &response)?)
}

#[pymodule(name = "_native")]
fn native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add("HttpError", module.py().get_type::<station::HttpError>())?;
    module.add(
        "CommandRefused",
        module.py().get_type::<station::CommandRefused>(),
    )?;
    module.add(
        "CommandIncomplete",
        module.py().get_type::<station::CommandIncomplete>(),
    )?;
    module.add("LeaseHeld", module.py().get_type::<station::LeaseHeld>())?;
    module.add(
        "MissingCamera",
        module.py().get_type::<station::MissingCamera>(),
    )?;
    module.add_function(wrap_pyfunction!(model_tools::tool_names, module)?)?;
    module.add_class::<model_tools::NativeTools>()?;
    module.add_function(wrap_pyfunction!(model_tools::tool_catalog, module)?)?;
    module.add_class::<lease::NativeLease>()?;
    module.add_class::<station::NativeStation>()?;
    module.add_class::<station::Telemetry>()?;
    module.add_function(wrap_pyfunction!(model_response, module)?)?;
    module.add_function(wrap_pyfunction!(concise_response, module)?)?;
    module.add_function(wrap_pyfunction!(concise_output, module)?)?;
    Ok(())
}
