use std::sync::Arc;

use hebbian_rua::{ConnectOptions, Error, RunUpdate, Station, contracts::ServerMessage};
use pyo3::{
    exceptions::{PyConnectionError, PyRuntimeError, PyTimeoutError},
    prelude::*,
};
use pyo3_async_runtimes::tokio::{future_into_py, get_runtime};
use pythonize::{depythonize, pythonize};
use serde::Serialize;
use tokio::sync::{Mutex, broadcast, watch};

pyo3::create_exception!(_native, HttpError, PyRuntimeError);
pyo3::create_exception!(_native, CommandRefused, PyRuntimeError);
pyo3::create_exception!(_native, CommandIncomplete, PyRuntimeError);
pyo3::create_exception!(_native, LeaseHeld, PyRuntimeError);
pyo3::create_exception!(_native, MissingCamera, PyRuntimeError);
pub(crate) fn station_error(error: Error) -> PyErr {
    match error {
        Error::Connection(_) => PyConnectionError::new_err(error.to_string()),
        Error::FrameTooLarge(_) => pyo3::exceptions::PyValueError::new_err(error.to_string()),
        Error::Timeout(_) => PyTimeoutError::new_err(error.to_string()),
        Error::Http { status, body } => Python::attach(|py| {
            let error = HttpError::new_err(format!("HTTP {status}: {body}"));
            if let Err(attribute_error) = error
                .value(py)
                .setattr("status", status)
                .and_then(|()| error.value(py).setattr("body", body))
            {
                return attribute_error;
            }
            error
        }),
        Error::Refused { reason, detail } => Python::attach(|py| match pythonize(py, &reason) {
            Ok(reason) => CommandRefused::new_err((reason.unbind(), detail)),
            Err(error) => error.into(),
        }),
        Error::Incomplete { record, .. } => Python::attach(|py| match pythonize(py, &record) {
            Ok(record) => CommandIncomplete::new_err(record.unbind()),
            Err(error) => error.into(),
        }),
        Error::LeaseHeldElsewhere(status) => Python::attach(|py| match pythonize(py, &status) {
            Ok(status) => LeaseHeld::new_err((status.unbind(),)),
            Err(error) => error.into(),
        }),
        Error::CameraUnavailable {
            name,
            listed,
            last_error,
        } => Python::attach(|py| match pythonize(py, &listed) {
            Ok(listed) => MissingCamera::new_err((name, listed.unbind(), last_error)),
            Err(error) => error.into(),
        }),
    }
}

#[pyclass(module = "hebbian_rua.bindings._native")]
pub struct NativeStation {
    station: Option<Station>,
    closed: watch::Sender<bool>,
    initial_telemetry: Option<broadcast::Receiver<RunUpdate>>,
}

impl NativeStation {
    fn current(&self) -> PyResult<Station> {
        self.station
            .clone()
            .ok_or_else(|| PyConnectionError::new_err("Station closed"))
    }

    fn operation<'py, T, F, Fut>(
        &self,
        py: Python<'py>,
        operation: F,
    ) -> PyResult<Bound<'py, PyAny>>
    where
        T: Serialize + Send,
        F: FnOnce(Station) -> Fut + Send + 'static,
        Fut: Future<Output = hebbian_rua::Result<T>> + Send,
    {
        let station = self.current()?;
        let mut closed = self.closed.subscribe();
        future_into_py(py, async move {
            tokio::select! {
                result = operation(station) => {
                    let value = result.map_err(station_error)?;
                    Python::attach(|py| Ok(pythonize(py, &value)?.unbind()))
                }
                _ = closed.changed() => Err(PyConnectionError::new_err("Station closed")),
            }
        })
    }
}

#[pymethods]
impl NativeStation {
    #[new]
    #[pyo3(signature = (server, media_server, run_after=None))]
    fn new(server: String, media_server: String, run_after: Option<u64>) -> PyResult<Self> {
        let _runtime = get_runtime().enter();
        let (station, initial_telemetry) = Station::open(ConnectOptions {
            server,
            media_server,
            run_after,
        })
        .map_err(station_error)?;
        Ok(Self {
            station: Some(station),
            closed: watch::channel(false).0,
            initial_telemetry: Some(initial_telemetry),
        })
    }

    fn close(&mut self) {
        self.closed.send_replace(true);
        if let Some(station) = self.station.take() {
            station.close_event_stream();
        }
        self.initial_telemetry.take();
    }

    fn telemetry(&mut self) -> PyResult<Telemetry> {
        let station = self.current()?;
        let receiver = self
            .initial_telemetry
            .take()
            .unwrap_or_else(|| station.run_updates());
        Ok(Telemetry {
            receiver: Arc::new(Mutex::new(receiver)),
            closed: self.closed.subscribe(),
        })
    }

    fn wait_until_connected<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.wait_until_connected().await
        })
    }

    fn revoke_lease<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(
            py,
            move |station| async move { station.revoke_lease().await },
        )
    }

    fn camera_names<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(
            py,
            move |station| async move { station.camera_names().await },
        )
    }

    fn ensure_recording<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.ensure_recording().await
        })
    }

    fn start_recording<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(
            py,
            move |station| async move { station.start_recording().await },
        )
    }

    fn stop_recording<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(
            py,
            move |station| async move { station.stop_recording().await },
        )
    }

    fn recording_status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.recording_status().await
        })
    }

    fn list_traces<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(
            py,
            move |station| async move { station.list_traces().await },
        )
    }

    fn get_operation<'py>(
        &self,
        py: Python<'py>,
        identifier: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.get_operation(&identifier).await
        })
    }

    fn get_action_sequence<'py>(
        &self,
        py: Python<'py>,
        identifier: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.get_action_sequence(&identifier).await
        })
    }

    fn wait_for_action_sequence<'py>(
        &self,
        py: Python<'py>,
        identifier: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.wait_for_action_sequence(&identifier).await
        })
    }

    fn cancel_action_sequence<'py>(
        &self,
        py: Python<'py>,
        identifier: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.cancel_action_sequence(&identifier).await
        })
    }

    fn inspect_compilation<'py>(
        &self,
        py: Python<'py>,
        identifier: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.inspect_compilation(&identifier).await
        })
    }

    fn compile_action_sequence<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |station| async move {
            station.compile_action_sequence(&request).await
        })
    }

    fn import_compilation<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |station| async move {
            station.import_compilation(&request).await
        })
    }

    fn capture_observation_http<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |station| async move {
            station.capture_observation_http(&request).await
        })
    }

    fn preview_poses<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |station| async move {
            station.preview_poses(&request).await
        })
    }

    fn phone_invite<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |station| async move {
            station.phone_invite(&request).await
        })
    }

    fn record_run_event<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |station| async move {
            station.record_run_event(request).await
        })
    }

    fn record_run_spans<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |station| async move {
            station.record_run_spans(request).await
        })
    }

    fn wait_for_outcome<'py>(
        &self,
        py: Python<'py>,
        command_id: String,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.wait_for_outcome(&command_id).await
        })
    }

    fn acquire_lease<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
        take: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        let station = self.current()?;
        let mut closed = self.closed.subscribe();
        future_into_py(py, async move {
            let acquire = async {
                station.wait_until_connected().await?;
                if take {
                    station.take_lease(request).await
                } else {
                    station.acquire_lease(request).await
                }
            };
            tokio::select! {
                result = acquire => Ok(crate::lease::NativeLease::new(result.map_err(station_error)?)),
                _ = closed.changed() => Err(PyConnectionError::new_err("Station closed")),
            }
        })
    }

    fn run_command<'py>(
        &self,
        py: Python<'py>,
        arm: String,
        command: &Bound<'_, PyAny>,
        options: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let command = depythonize(command)?;
        let options = depythonize(options)?;
        self.operation(py, move |station| async move {
            station.run_station_command(&arm, command, options).await
        })
    }
    fn observation_cursor(&self) -> PyResult<(u64, u64)> {
        Ok(self.current()?.observation_cursor())
    }

    fn wait_for_observation<'py>(
        &self,
        py: Python<'py>,
        cursor: (u64, u64),
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            station.wait_for_observation(cursor).await;
            Ok(())
        })
    }

    fn next_sequence<'py>(&self, py: Python<'py>, revision: u64) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |station| async move {
            Ok(station.next_sequence(revision).await)
        })
    }

    fn camera_jpeg<'py>(
        &self,
        py: Python<'py>,
        name: String,
        maximum_bytes: Option<usize>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let station = self.current()?;
        future_into_py(py, async move {
            let frame = station
                .camera_jpeg_bounded(&name, maximum_bytes)
                .await
                .map_err(station_error)?;
            Python::attach(|py| {
                Ok((
                    pyo3::types::PyBytes::new(py, &frame.jpeg).unbind(),
                    frame.captured_at_unix_ns,
                ))
            })
        })
    }

    fn download_trace<'py>(&self, py: Python<'py>, name: String) -> PyResult<Bound<'py, PyAny>> {
        let station = self.current()?;
        future_into_py(py, async move {
            let bytes = station.download_trace(&name).await.map_err(station_error)?;
            Python::attach(|py| Ok(pyo3::types::PyBytes::new(py, &bytes).unbind()))
        })
    }

    fn state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, |station| async move {
            station.wait_until_connected().await?;
            station.state().await
        })
    }
}

#[pyclass(module = "hebbian_rua.bindings._native")]
pub struct Telemetry {
    receiver: Arc<Mutex<broadcast::Receiver<RunUpdate>>>,
    closed: watch::Receiver<bool>,
}

#[pymethods]
impl Telemetry {
    fn receive<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let receiver = self.receiver.clone();
        let mut closed = self.closed.clone();
        future_into_py(py, async move {
            if *closed.borrow() {
                return Ok(None);
            }
            let mut receiver = receiver.lock().await;
            let update = tokio::select! {
                result = receiver.recv() => match result {
                    Ok(update) => update,
                    Err(broadcast::error::RecvError::Closed) => return Ok(None),
                    Err(error) => return Err(PyRuntimeError::new_err(error.to_string())),
                },
                _ = closed.changed() => return Ok(None),
            };
            let message = match update {
                RunUpdate::Event {
                    seq,
                    received_unix_nano,
                    event,
                } => ServerMessage::RunEvent {
                    seq,
                    received_unix_nano,
                    event,
                },
                RunUpdate::Span {
                    seq,
                    received_unix_nano,
                    span,
                } => ServerMessage::RunSpan {
                    seq,
                    received_unix_nano,
                    span: *span,
                },
                RunUpdate::Resync {
                    oldest_retained,
                    newest,
                } => ServerMessage::RunResync {
                    oldest_retained,
                    newest,
                },
                RunUpdate::Snapshot(run) => ServerMessage::Run { run },
            };
            Python::attach(|py| Ok(Some(pythonize(py, &message)?.unbind())))
        })
    }
}
