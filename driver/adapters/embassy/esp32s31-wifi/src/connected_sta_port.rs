//! Production owner graph for one connected ESP32-S31 station epoch.
//!
//! Board/HIL code supplies already allocated storage, a network RX sink and
//! executor task placement. This module owns the driver relationships between
//! the associated peer, RX dispatcher/protocol, control-TX handoff,
//! ordinary/A-MPDU TX, BlockAck control and the final [`WdevServiceSet`].

use embassy_sync::channel::Receiver;
use open_esp_radio_embassy_net::{PinnedTxFrame, RawMutex};
use open_esp_radio_esp32s31_wifi::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer};
use open_esp_radio_esp32s31_wifi_mac::{
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    rate_control::{StaRateControlAssociation, StaTxRatePolicy},
    rx::RxIngressConfig,
    rx_ampdu::{RxBlockAckSessions, RxBlockAckSessionsError},
    tx::{
        HeDcmRate, HeEdcaTxopLimit, HeMcs, HtGuardInterval, HtMcs, LegacyRate, TxPhyRate,
        TxSlotState,
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
    MacServiceCapabilities, MacTxPlan, WifiPlan,
    interface::{BoundVirtualInterface, VifRole},
};
use open_esp_radio_wifi_sta::link_monitor::{StaBeaconLossConfig, StaBeaconLossConfigError};

use crate::{
    aggregate_tx_observer::AggregateTxObserver,
    ampdu_resources::AggregateTxResources,
    connected_control::Esp32s31ConnectedControl,
    connected_rx_protocol::{
        ConnectedRxProtocolSink, Esp32s31ConnectedRxProcessor, Esp32s31ConnectedRxProtocol,
        Esp32s31ConnectedRxProtocolStorage, Esp32s31StagedRxFrame,
    },
    control_mailbox::ConnectedControlReceiver,
    embassy_irq::EmbassyMacIrqRuntime,
    rx_pipeline_observer::RxPipelineObserver,
    rx_reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandReceiver, RxReorderCommandSender,
        RxReorderFrameStorage,
    },
    station_tx::{AggregateTxConfig, Esp32s31ConnectedTx},
    wdev::services::WdevServiceSet,
};
/// Stateless namespace for preparing and composing a connected owner graph.
pub struct Esp32s31ConnectedStaPort;

mod composition;
mod plan;
mod resources;

pub use plan::{
    Esp32s31ConnectedStaBlockAckPolicy, Esp32s31ConnectedStaConfig,
    Esp32s31ConnectedStaConfigError, Esp32s31ConnectedStaPlan, Esp32s31ConnectedStaPrepareFailure,
    Esp32s31ConnectedStaRateConfig, Esp32s31ConnectedStaRxPolicy, Esp32s31ConnectedStaTxPolicy,
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
