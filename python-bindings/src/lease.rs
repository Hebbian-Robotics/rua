use std::sync::Arc;

use crate::error::station_error;
use hebbian_rua::LeaseGuard;
use pyo3::{exceptions::PyRuntimeError, prelude::*};
use pyo3_async_runtimes::tokio::future_into_py;
use pythonize::{depythonize, pythonize};
use serde::Serialize;

#[pyclass(module = "hebbian_rua.bindings._native")]
pub struct NativeLease {
    guard: Option<Arc<LeaseGuard>>,
}

impl NativeLease {
    pub fn new(guard: LeaseGuard) -> Self {
        Self {
            guard: Some(Arc::new(guard)),
        }
    }
    pub(crate) fn current(&self) -> PyResult<Arc<LeaseGuard>> {
        self.guard
            .clone()
            .ok_or_else(|| PyRuntimeError::new_err("Lease released"))
    }
    fn operation<'py, T, F, Fut>(
        &self,
        py: Python<'py>,
        operation: F,
    ) -> PyResult<Bound<'py, PyAny>>
    where
        T: Serialize + Send,
        F: FnOnce(Arc<LeaseGuard>) -> Fut + Send + 'static,
        Fut: Future<Output = hebbian_rua::Result<T>> + Send,
    {
        let guard = self.current()?;
        future_into_py(py, async move {
            let value = operation(guard).await.map_err(station_error)?;
            Python::attach(|py| Ok(pythonize(py, &value)?.unbind()))
        })
    }
}

#[pymethods]
impl NativeLease {
    fn status<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        Ok(pythonize(py, self.current()?.status())?)
    }
    fn lost<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, |guard| async move {
            guard.lost().await;
            Ok(())
        })
    }
    fn release<'py>(&mut self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let release = self.guard.take().map(|guard| {
            pyo3_async_runtimes::tokio::get_runtime()
                .spawn(async move { guard.return_lease().await.map_err(station_error) })
        });
        future_into_py(py, async move {
            if let Some(release) = release {
                release
                    .await
                    .map_err(|error| PyRuntimeError::new_err(error.to_string()))??;
            }
            Ok(())
        })
    }
    fn wait_for_observation<'py>(
        &self,
        py: Python<'py>,
        selection: &Bound<'_, PyAny>,
        arm: String,
        timeout_milliseconds: u32,
    ) -> PyResult<Bound<'py, PyAny>> {
        let selection = depythonize(selection)?;
        self.operation(py, move |guard| async move {
            guard
                .wait_for_observation(selection, &arm, timeout_milliseconds)
                .await
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
        self.operation(py, move |guard| async move {
            guard.run_with_lease(&arm, command, options).await
        })
    }
    fn move_arms<'py>(
        &self,
        py: Python<'py>,
        command: &Bound<'_, PyAny>,
        options: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let command = depythonize(command)?;
        let options = depythonize(options)?;
        self.operation(py, move |guard| async move {
            guard.run_arms_records(command, options).await
        })
    }
    fn start_sequence_execution<'py>(
        &self,
        py: Python<'py>,
        request: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let request = depythonize(request)?;
        self.operation(py, move |guard| async move {
            let execution = guard.start_sequence_execution(request, Vec::new()).await?;
            Ok((
                execution.station_instance_id().to_owned(),
                execution.id().to_owned(),
            ))
        })
    }
    fn start_compiled_execution<'py>(
        &self,
        py: Python<'py>,
        identifier: String,
        revisions: &Bound<'_, PyAny>,
        continuation: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let revisions = depythonize(revisions)?;
        let continuation = depythonize(continuation)?;
        self.operation(py, move |guard| async move {
            let execution = guard
                .start_compiled_execution(&identifier, revisions, continuation)
                .await?;
            Ok((
                execution.station_instance_id().to_owned(),
                execution.id().to_owned(),
            ))
        })
    }
    fn send_operation<'py>(
        &self,
        py: Python<'py>,
        arm: String,
        identifier: &Bound<'_, PyAny>,
        command: &Bound<'_, PyAny>,
        revision: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let identifier = depythonize(identifier)?;
        let command = depythonize(command)?;
        self.operation(py, move |guard| async move {
            guard
                .send_operation(&arm, identifier, command, revision)
                .await
        })
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The stream update carries the operation, sequence, targets and revision."
    )]
    fn update_stream_goal<'py>(
        &self,
        py: Python<'py>,
        identifier: &Bound<'_, PyAny>,
        sequence: u32,
        joints: Vec<f64>,
        gripper_opening: &Bound<'_, PyAny>,
        revision: Option<u64>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let identifier = depythonize(identifier)?;
        let gripper_opening = depythonize(gripper_opening)?;
        self.operation(py, move |guard| async move {
            guard
                .update_stream_goal(identifier, sequence, joints, gripper_opening, revision)
                .await
        })
    }
    fn cancel_operation<'py>(
        &self,
        py: Python<'py>,
        identifier: &Bound<'_, PyAny>,
    ) -> PyResult<Bound<'py, PyAny>> {
        let identifier = depythonize(identifier)?;
        self.operation(py, move |guard| async move {
            guard.cancel_operation(identifier).await
        })
    }
    fn report_camera<'py>(
        &self,
        py: Python<'py>,
        camera: String,
        available: bool,
    ) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |guard| async move {
            guard.report_camera(&camera, available).await
        })
    }
    fn stop_arm<'py>(&self, py: Python<'py>, arm: String) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |guard| async move { guard.stop_arm(&arm).await })
    }
    fn stop_recording<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        self.operation(py, move |guard| async move { guard.stop_recording().await })
    }
}
