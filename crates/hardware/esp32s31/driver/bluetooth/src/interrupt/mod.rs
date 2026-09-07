//! CPU route policy, interrupt classification and deferred publication.

pub(crate) mod classifier;
pub(crate) mod nrt;
pub(crate) mod primary;
pub(crate) mod route;
pub(crate) mod wake;

pub use classifier::{
    BluetoothSchedulerReferenceGateObservation, BluetoothSchedulerWorkObservation,
    PrimaryControllerFault, PrimaryInterruptClassification, PrimarySchedulerTrigger,
    SchedulerReferenceAction, SchedulerReferenceGate, SchedulerWorkClassifier, SchedulerWorkerWake,
    SchedulerWorkerWakeClass,
};

pub use nrt::{NrtDefaultInterruptEpoch, step_nrt_default_interrupt};

pub use primary::{
    PrimaryInterruptStep, PrimaryNoSchedulerWork, PrimaryPublishedInterruptStep,
    PrimarySchedulerEvent, step_primary_interrupt,
};

pub use route::{CpuInterruptRoutePolicy, CpuInterruptSource, InterruptHandlerResidency};

pub use wake::{SchedulerWakeBatch, SchedulerWakeCell, SchedulerWakePublication};
