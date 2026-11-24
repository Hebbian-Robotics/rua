use hebbian_rua::Error;
use pyo3::{
    exceptions::{PyBaseException, PyConnectionError, PyRuntimeError, PyTimeoutError},
    prelude::*,
};
use pythonize::pythonize;

pyo3::create_exception!(_native, HttpError, PyRuntimeError);
pyo3::create_exception!(_native, CommandRefused, PyRuntimeError);
pyo3::create_exception!(_native, CommandIncomplete, PyRuntimeError);
pyo3::create_exception!(_native, LeaseHeld, PyRuntimeError);
pyo3::create_exception!(_native, MissingCamera, PyRuntimeError);
fn with_error_details(
    error: PyErr,
    set_details: impl FnOnce(&Bound<'_, PyBaseException>) -> PyResult<()>,
) -> PyErr {
    Python::attach(|py| match set_details(error.value(py)) {
        Ok(()) => error,
        Err(attribute_error) => attribute_error,
    })
}

pub(crate) fn station_error(error: Error) -> PyErr {
    let message = error.to_string();
    match error {
        Error::Connection(_) => PyConnectionError::new_err(message),
        Error::FrameTooLarge(_) => pyo3::exceptions::PyValueError::new_err(message),
        Error::Timeout(_) => PyTimeoutError::new_err(message),
        Error::Http { status, body } => with_error_details(HttpError::new_err(message), |error| {
            error.setattr("status", status)?;
            error.setattr("body", body)
        }),
        Error::Refused { reason, detail } => {
            with_error_details(CommandRefused::new_err(message), |error| {
                error.setattr("reason", pythonize(error.py(), &reason)?)?;
                error.setattr("detail", detail)
            })
        }
        Error::Incomplete { record, .. } => {
            with_error_details(CommandIncomplete::new_err(message), |error| {
                error.setattr("record", pythonize(error.py(), &record)?)
            })
        }
        Error::LeaseHeldElsewhere(status) => {
            with_error_details(LeaseHeld::new_err(message), |error| {
                error.setattr("holder_status", pythonize(error.py(), &status)?)
            })
        }
        Error::CameraUnavailable {
            listed, last_error, ..
        } => with_error_details(MissingCamera::new_err(message), |error| {
            error.setattr("listed", pythonize(error.py(), &listed)?)?;
            error.setattr("last_error", last_error)
        }),
    }
}
