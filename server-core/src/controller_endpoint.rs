use std::sync::mpsc::Receiver;

use hebbian_contracts::{LoggedEvent, Sample};

use crate::handoff::ArmInbox;

pub struct ControllerEndpoint {
    pub(crate) inbox: ArmInbox,
    pub(crate) samples: SampleRecycler,
    pub(crate) publish_sample: Box<dyn Fn(Sample) + Send>,
    pub(crate) publish_log: std::sync::Arc<dyn Fn(LoggedEvent) + Send + Sync>,
}

// WHY: Recycle sample buffers after the writer sends them to avoid allocations in the control loop.
pub(crate) struct SampleRecycler {
    pub(crate) returned: Receiver<Sample>,
}

impl SampleRecycler {
    pub(crate) fn checkout(&self) -> Sample {
        self.returned.try_recv().unwrap_or_else(|_| Sample::blank())
    }
}
