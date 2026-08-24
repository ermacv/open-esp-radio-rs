//! Production owner graph for one connected ESP32-S31 station epoch.
//!
//! Board/HIL code supplies already allocated storage, a network RX sink and
//! executor task placement. This module owns the driver relationships between
//! the associated peer, RX dispatcher/protocol, control-TX handoff,
//! ordinary/A-MPDU TX, BlockAck control and the final [`SingleRoleServices`].

use embassy_sync::channel::Receiver;
use open_esp_radio_embassy_net::{PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
use open_esp_radio_esp32s31_wifi_mac::{
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    init::{StaEspNowRxPolicyHardware, configure_sta_esp_now_receive_policy},
    rate_control::{StaRateControlAssociation, StaTxRatePolicy},
    rx::RxIngressConfig,
    rx_ampdu::{RxBlockAckSessions, RxBlockAckSessionsError},
    tx::{
        HeDcmRate, HeEdcaTxopLimit, HeMcs, HeTriggerBasedTxConfig, HtGuardInterval, HtMcs,
        LegacyRate, TxPhyRate, TxSlotState,
    },
    tx_ampdu::{StaTxBlockAckSessions, TxBlockAckError},
};
use open_esp_radio_esp32s31_wifi_sta::connected_rx::{ConnectedRxConfig, ConnectedRxDispatcher};
use open_esp_radio_esp32s31_wifi_sta::peer::{Esp32s31ConnectedStaPeer, Esp32s31StaConnectedLink};
use open_esp_radio_esp32s31_wifi_sta::{
    control_tx::Esp32s31ControlTx,
    single_mpdu_tx::{ConnectedTxHandoff, SingleMpduTxConfig},
};
use open_esp_radio_ieee80211::{
    he::HeDcmConstellation,
    station::{StaAssociationPhy, StaTxSequenceCounters},
    wmm::WmmAccessCategory,
};
use open_esp_radio_wifi_softmac::{
    EspNowRxEpoch, MacServiceCapabilities, MacTxPlan, WifiPlan,
    interface::{BoundVirtualInterface, VifRole},
};
use open_esp_radio_wifi_sta::{
    link_monitor::{StaBeaconLossConfig, StaBeaconLossConfigError},
    power_save::{StaPowerSavePolicy, StaPowerSavePolicyError},
    request::StationPowerMode,
};

#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::{
    aggregate_tx::AggregateTxObserver,
    rx_pipeline::{RxPipelineObserver, RxReorderAgreementObserver},
};
use crate::{
    datapath::irq::EmbassyMacIrqRuntime,
    datapath::rx::reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandReceiver, RxReorderCommandSender,
        RxReorderFrameStorage,
    },
    datapath::rx::staging::Esp32s31StagedRxFrame,
    datapath::services::SingleRoleServices,
    datapath::tx::resources::AggregateTxResources,
    roles::concurrent::Esp32s31StaApRxBlockAck,
    roles::station::control::Esp32s31ConnectedControl,
    roles::station::control_mailbox::ConnectedControlReceiver,
    roles::station::rx_protocol::{
        ConnectedRxProtocolSink, Esp32s31ConnectedRxProcessor, Esp32s31ConnectedRxProtocol,
        Esp32s31ConnectedRxProtocolStorage,
    },
    roles::station::tx::{AggregateTxConfig, Esp32s31ConnectedTx, StationTxBlockAckStatusSink},
};
/// Stateless namespace for preparing and composing a connected owner graph.
pub struct Esp32s31ConnectedStaPort;

mod composition;
mod plan;
mod resources;

pub use plan::{
    Esp32s31ConnectedStaBlockAckPolicy, Esp32s31ConnectedStaConfig,
    Esp32s31ConnectedStaConfigError, Esp32s31ConnectedStaEspNowRxError, Esp32s31ConnectedStaPlan,
    Esp32s31ConnectedStaPrepareFailure, Esp32s31ConnectedStaRateConfig,
    Esp32s31ConnectedStaRxPolicy, Esp32s31ConnectedStaTxPolicy,
};
pub use resources::{
    Esp32s31ConnectedStaCompositionFailure, Esp32s31ConnectedStaControlResources,
    Esp32s31ConnectedStaDriverParts, Esp32s31ConnectedStaDrivers,
    Esp32s31ConnectedStaNetworkTxDomain, Esp32s31ConnectedStaReport,
    Esp32s31ConnectedStaRxProcessorResources, Esp32s31ConnectedStaRxProtocolResources,
    Esp32s31ConnectedStaTxHandoffFailure, Esp32s31ConnectedStaTxResources,
};

#[cfg(test)]
mod tests;
