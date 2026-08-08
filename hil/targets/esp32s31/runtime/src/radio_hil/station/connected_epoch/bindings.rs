#![forbid(unsafe_code)]

use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Duration;
use open_esp_radio::{
    adapters::esp32s31::wifi_embassy::{
        connected_sta_port::Esp32s31ConnectedStaConfig, embassy_irq::EmbassyMacIrqRuntime,
    },
    esp32s31::hal::RadioRegisters,
};
use open_esp_radio_hil_esp32s31_telemetry::{
    aggregate_tx::AggregateTxCounters, rx_pipeline::RxPipelineCounters,
};
use open_esp_radio_hil_protocol::NetworkIpv4Configuration;
use static_cell::StaticCell;

use crate::{
    radio_fault::StationFaultControl,
    radio_hil::{
        ConnectedAmpduDmaBacking, ConnectedAmpduMetadataBacking, ConnectedRxReorderCommands,
        ConnectedRxReorderStorage, ConnectedRxStagePool, ConnectedStackResources,
        ConnectedTrafficStartChannel, ControlResources, StagedRxQueue,
    },
};

use super::super::reporting::RadioHilStationEpochReporter;

/// Static cells initialized only by the first connected epoch.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedEpochStorage {
    pub(in crate::radio_hil) stack: &'static StaticCell<ConnectedStackResources>,
    pub(in crate::radio_hil) ampdu_metadata: &'static StaticCell<ConnectedAmpduMetadataBacking>,
    pub(in crate::radio_hil) ampdu_dma: &'static StaticCell<ConnectedAmpduDmaBacking>,
    pub(in crate::radio_hil) ampdu_standby_metadata:
        &'static StaticCell<ConnectedAmpduMetadataBacking>,
    pub(in crate::radio_hil) ampdu_standby_dma: &'static StaticCell<ConnectedAmpduDmaBacking>,
    pub(in crate::radio_hil) control: &'static StaticCell<ControlResources>,
    pub(in crate::radio_hil) registers: &'static StaticCell<RefCell<&'static mut RadioRegisters>>,
}

/// Persistent queues, observers and fault controls borrowed by every epoch.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedEpochServices {
    pub(in crate::radio_hil) staged_rx: &'static StagedRxQueue,
    pub(in crate::radio_hil) rx_stage_pool: &'static ConnectedRxStagePool,
    pub(in crate::radio_hil) rx_pipeline: &'static RxPipelineCounters,
    pub(in crate::radio_hil) rx_reorder_commands: &'static ConnectedRxReorderCommands,
    pub(in crate::radio_hil) rx_reorder_storage: &'static ConnectedRxReorderStorage,
    pub(in crate::radio_hil) irq: &'static EmbassyMacIrqRuntime<CriticalSectionRawMutex>,
    pub(in crate::radio_hil) aggregate_tx: &'static AggregateTxCounters,
    pub(in crate::radio_hil) faults: &'static StationFaultControl,
    pub(in crate::radio_hil) traffic_start: &'static ConnectedTrafficStartChannel,
    pub(in crate::radio_hil) task_stop_timeout: Duration,
    pub(in crate::radio_hil) station_reporter: RadioHilStationEpochReporter,
}

/// Scenario policy selected by the HIL composition root.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedEpochPolicy {
    pub(in crate::radio_hil) station: Esp32s31ConnectedStaConfig,
    pub(in crate::radio_hil) ipv4: NetworkIpv4Configuration,
}

/// Complete HIL adapter supplied to the production connected transaction.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedEpochBindings {
    pub(in crate::radio_hil) storage: RadioHilConnectedEpochStorage,
    pub(in crate::radio_hil) services: RadioHilConnectedEpochServices,
    pub(in crate::radio_hil) policy: RadioHilConnectedEpochPolicy,
}
