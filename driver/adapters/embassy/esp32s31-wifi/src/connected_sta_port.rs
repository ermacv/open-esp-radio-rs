//! Production owner graph for one connected ESP32-S31 station epoch.
//!
//! Board/HIL code supplies already allocated storage, a network RX sink and
//! executor task placement. This module owns the driver relationships between
//! the associated peer, RX dispatcher/protocol, control-TX handoff,
//! ordinary/A-MPDU TX, BlockAck control and the final [`Esp32s31ConnectedServices`].

use embassy_sync::channel::Receiver;
use open_esp_radio_embassy_net::RawMutex;
use open_esp_radio_esp32s31_wifi_mac::{
    capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES,
    connected_rx::{ConnectedRxConfig, ConnectedRxDispatcher},
    rate_control::StaTxRatePolicy,
    rx::RxIngressConfig,
    rx_ampdu::{StaRxBlockAckSessions, StaRxBlockAckSessionsError},
    tx::{
        HeDcmRate, HeEdcaTxopLimit, HeMcs, HtGuardInterval, HtMcs, LegacyRate, TxPhyRate,
        TxSlotState,
    },
    tx_ampdu::{StaTxBlockAckSessions, TxBlockAckError},
};
use open_esp_radio_esp32s31_wifi_sta::peer::{Esp32s31ConnectedStaPeer, Esp32s31StaConnectedLink};
use open_esp_radio_ieee80211::{
    he::HeDcmConstellation,
    station::{StaAssociationPhy, StaTxSequenceCounters},
    wmm::WmmAccessCategory,
};
use open_esp_radio_wifi_softmac::{
    MacServiceCapabilities, MacTxPlan,
    interface::{BoundVirtualInterface, ChannelContextId, VifId, VifRole, VirtualInterface},
};
use open_esp_radio_wifi_sta::link_monitor::{StaBeaconLossConfig, StaBeaconLossConfigError};

use crate::{
    aggregate_tx::{AggregateTxConfig, AggregateTxResources, Esp32s31ConnectedTx},
    aggregate_tx_observer::AggregateTxObserver,
    connected_control::Esp32s31ConnectedControl,
    connected_rx_protocol::{
        ConnectedRxProtocolSink, Esp32s31ConnectedRxProtocol, Esp32s31StagedRxFrame,
    },
    connected_services::Esp32s31ConnectedServices,
    control_mailbox::ConnectedControlReceiver,
    control_tx::Esp32s31ControlTx,
    embassy_irq::EmbassyMacIrqRuntime,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxTimer},
    rx_pipeline_observer::RxPipelineObserver,
    rx_reorder::{
        RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandReceiver, RxReorderCommandSender,
        RxReorderFrameStorage,
    },
    single_mpdu_tx::{ConnectedTxHandoff, SingleMpduTxConfig},
};
/// Stateless namespace for preparing and composing a connected owner graph.
pub struct Esp32s31ConnectedStaPort;

mod composition;
mod plan;
mod resources;

pub use plan::{
    Esp32s31ConnectedStaConfig, Esp32s31ConnectedStaConfigError, Esp32s31ConnectedStaPlan,
    Esp32s31ConnectedStaPrepareFailure, Esp32s31ConnectedStaRateConfig,
};
pub use resources::{
    Esp32s31ConnectedStaControlResources, Esp32s31ConnectedStaDriverParts,
    Esp32s31ConnectedStaDrivers, Esp32s31ConnectedStaNetworkTxDomain, Esp32s31ConnectedStaReport,
    Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxHandoffFailure,
    Esp32s31ConnectedStaTxResources,
};

#[cfg(test)]
mod tests;
