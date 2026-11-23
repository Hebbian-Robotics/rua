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

#[pymodule]
fn _native(module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_function(wrap_pyfunction!(model_response, module)?)?;
    module.add_function(wrap_pyfunction!(concise_output, module)?)?;
    Ok(())
}
