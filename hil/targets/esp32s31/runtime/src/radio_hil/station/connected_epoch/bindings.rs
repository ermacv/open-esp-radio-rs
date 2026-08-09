#![forbid(unsafe_code)]

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use open_esp_radio::esp32s31::wifi::device::register_arena::Esp32s31RadioRegistersArena;
use open_esp_radio::esp32s31::wifi::mac::tx_ampdu::HtAmpduTxError;
use open_esp_radio_esp32s31_wifi_embassy::{
    connected_sta_port::Esp32s31ConnectedStaConfig, embassy_irq::EmbassyMacIrqRuntime,
    station::Esp32s31InitialConnectedEpochResources,
};
use open_esp_radio_hil_esp32s31_telemetry::{
    aggregate_tx::AggregateTxCounters, rx_pipeline::RxPipelineCounters,
};
use open_esp_radio_hil_protocol::NetworkIpv4Configuration;
use static_cell::ConstStaticCell;

use crate::{
    radio_fault::StationFaultControl,
    radio_hil::{
        ConnectedAmpduStorage, ConnectedRxEpochResources, ConnectedRxProtocolStorage,
        ConnectedRxReorderCommands, ConnectedRxReorderStorage, ConnectedRxStagePool,
        ConnectedStackResources, ConnectedTrafficStartChannel, ControlResources, StagedRxQueue,
    },
};

use super::super::reporting::RadioHilStationEpochReporter;

/// Initial-only owners materialized before the station lifecycle can activate
/// IRQ or DMA for a connected epoch.
pub(in crate::radio_hil) struct RadioHilInitialConnectedStaticResources {
    registers: &'static Esp32s31RadioRegistersArena,
    aggregate: ConnectedAmpduStorage,
    control: &'static ControlResources,
}

impl RadioHilInitialConnectedStaticResources {
    pub(in crate::radio_hil) const fn new(
        registers: &'static Esp32s31RadioRegistersArena,
        aggregate: ConnectedAmpduStorage,
        control: &'static ControlResources,
    ) -> Self {
        Self {
            registers,
            aggregate,
            control,
        }
    }

    pub(in crate::radio_hil) fn with_rx(
        self,
        rx: ConnectedRxEpochResources,
    ) -> Esp32s31InitialConnectedEpochResources<
        'static,
        ConnectedRxEpochResources,
        ConnectedAmpduStorage,
        &'static ControlResources,
    > {
        Esp32s31InitialConnectedEpochResources::new(
            self.registers,
            rx,
            self.aggregate,
            self.control,
        )
    }
}

/// Boot-time failure to materialize an initial-only connected resource.
#[derive(Debug)]
pub(in crate::radio_hil) enum RadioHilConnectedStaticResourceError {
    PrimaryMetadataUnavailable,
    PrimaryDmaUnavailable,
    PrimaryAmpduInvalid { _error: HtAmpduTxError },
    StandbyMetadataUnavailable,
    StandbyDmaUnavailable,
    StandbyAmpduInvalid { _error: HtAmpduTxError },
    PrimaryRetentionUnavailable,
    StandbyRetentionUnavailable,
    ControlUnavailable,
    RegisterArenaUnavailable,
}

/// Storage retained across connected epochs. Initial-only resources are
/// present exactly once and become structurally unavailable after first use.
pub(in crate::radio_hil) struct RadioHilConnectedEpochStorage {
    pub(in crate::radio_hil) stack: &'static ConstStaticCell<ConnectedStackResources>,
    pub(in crate::radio_hil) initial: Option<RadioHilInitialConnectedStaticResources>,
    pub(in crate::radio_hil) rx_protocol: &'static mut ConnectedRxProtocolStorage,
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
    pub(in crate::radio_hil) station_reporter: RadioHilStationEpochReporter,
}

/// Scenario policy selected by the HIL composition root.
#[derive(Clone, Copy)]
pub(in crate::radio_hil) struct RadioHilConnectedEpochPolicy {
    pub(in crate::radio_hil) station: Esp32s31ConnectedStaConfig,
    pub(in crate::radio_hil) ipv4: NetworkIpv4Configuration,
}

/// Complete HIL adapter supplied to the production connected transaction.
pub(in crate::radio_hil) struct RadioHilConnectedEpochBindings {
    pub(in crate::radio_hil) storage: RadioHilConnectedEpochStorage,
    pub(in crate::radio_hil) services: RadioHilConnectedEpochServices,
    pub(in crate::radio_hil) policy: RadioHilConnectedEpochPolicy,
}
