#![expect(
    clippy::too_many_arguments,
    reason = "AP ingress and lifecycle boundaries expose independent borrowed owners without dynamic erasure"
)]

//! Embassy-owned AP MAC and network handoff service.
//!
//! The service handles beacons, management frames, WPA2 EAPOL and authorized
//! Ethernet traffic through one bounded RX/TX owner.

use core::{convert::Infallible, future::Future};

use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Instant, Timer};

use open_esp_radio_dma::StableDmaBacking;
use open_esp_radio_embassy_net::{
    FrameLengthError, LinkState, OwnedNetworkTxFrame, RxEnqueueError,
};

use crate::datapath::{DatapathTxConsumer, PinnedTxFrame};
use open_esp_radio_esp32s31_wifi::{
    ampdu_tx::HtAmpduTxRolePolicy,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::{WifiTxProgress, WifiTxWake},
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_ap::mac::Esp32s31ApMacObservation;
use open_esp_radio_esp32s31_wifi_ap::protocol::{
    AP_MAX_CLIENTS, AccessPointServiceStatus, ApAssociationIdentity, ApBufferedGroupRelease,
    ApBufferedUnicastRelease, ApDownlinkDisposition, ApPeerClose, ApPeerPhase, ApPeerPowerState,
    ApPowerSaveAction, ApWpa2RetryProgress,
};
use open_esp_radio_esp32s31_wifi_ap::{
    ampdu::{Esp32s31ApAggregateAdmission, Esp32s31ApAmpduError, Esp32s31ApAmpduProgress},
    engine::{Esp32s31ApEngine, Esp32s31ApRuntimeHardware, Esp32s31ApWpa2Outcome},
    mac::{
        Esp32s31ApMac, Esp32s31ApMacError, Esp32s31ApMacParked, Esp32s31ApPeerDisconnectStage,
        Esp32s31ApTxCompletionAction,
    },
    rx::{
        Esp32s31ApRxAdmission, Esp32s31ApRxAdmissionRequest, Esp32s31ApRxConfig,
        Esp32s31ApRxDispatch, Esp32s31ApRxDispatcher, Esp32s31ApRxError, Esp32s31ApRxEvent,
        Esp32s31ApRxSink,
    },
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_mac::rx::{
    HtDuplicateRxClassification, HtSignal, RxDescriptorSnapshot,
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_mac::tx::HtMcs;
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_mac::tx::{HtChannelWidth, HtRate};
use open_esp_radio_esp32s31_wifi_mac::{
    MacInterface,
    init::MAC_COLD_RX_INTERRUPT_MASK,
    irq::MacInterruptRoute,
    rx::{RxDma, RxIngressConfig, view_normalized_rx_frame},
    rx_ampdu::{
        RX_BLOCK_ACK_MAX_WINDOW, RxBlockAckActivation, RxBlockAckRequest, RxBlockAckSessionsError,
        write_declined_addba_response,
    },
    rx_ampdu_hw::{RxBlockAckHardware, S31RxBlockAckAgreementError},
    tx::TxHardware,
};
use open_esp_radio_ieee80211::data::{
    DataInterfaceRole, EthernetFrameParts, IEEE80211_LEGACY_DATA_HEADER_LEN,
    IEEE80211_QOS_DATA_HEADER_LEN, plan_data_decapsulation,
};
use open_esp_radio_ieee80211::{
    ap::{
        ApManagementRequest, ApPowerSaveObservation,
        observe_ap_null_data_power_save_for_access_point, observe_ap_power_save_for_access_point,
        parse_ap_management_request,
    },
    block_ack::BlockAckAction,
    security::WifiSecurityMode,
};
use open_esp_radio_wifi_embassy::await_stack_boundary;
use open_esp_radio_wifi_softmac::MacRxEvidence;
use open_esp_radio_wpa2::{OwnedEapolFrame, Wpa2Interface};

#[cfg(any(feature = "diagnostics", test))]
use crate::datapath::irq::Esp32s31MacInterruptEpochDrain;
#[cfg(any(feature = "diagnostics", test))]
use crate::datapath::rx::frontier::Esp32s31RxFrontierSchedulerSnapshot;
#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::access_point::{
    AccessPointObservationStorage, AccessPointTerminalObservation, AccessPointTerminalObserver,
};
#[cfg(any(feature = "diagnostics", test))]
use crate::diagnostics::aggregate_tx::{
    AggregateBuildStop, AggregateTxObservation, AggregateTxObserver, PreparedTxSchedulerPhase,
};
#[cfg(feature = "tx-phase-telemetry")]
use crate::diagnostics::core0_rx_performance::CORE0_PERFORMANCE;
#[cfg(feature = "diagnostics")]
use crate::diagnostics::network::{RxNetworkDeliveryEvent, RxNetworkDeliveryObserver};
use crate::{
    datapath::irq::{
        Esp32s31MacInterruptEpoch, Esp32s31MacInterruptEpochActivateError,
        Esp32s31MacInterruptEpochQuiesceError,
    },
    datapath::rx::reorder::{RX_REORDER_BACKING_SLOT_COUNT, RxReorderFrameStorage},
    datapath::rx::staging::StagedEthernetPublication,
    datapath::tx::aggregate::AggregateTxServiceEvent,
    datapath::{
        DatapathControlContext, DatapathControlProgress, DatapathRunner, DatapathRxProgress,
        DatapathRxServiceContext, DatapathServices, DatapathStopProgress,
        network::{DatapathNetworkLink, DatapathNetworkRx},
    },
    roles::concurrent::Esp32s31StaApRxBlockAck,
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_ap::ampdu::Esp32s31ApAmpduCompletion;

const EAPOL_ETHERTYPE: u16 = 0x888e;
const EAPOL_CAPACITY: usize = 512;

fn access_point_tx_batch_target(operational_window: Option<u16>, arena_capacity: usize) -> usize {
    operational_window.map_or(1, |window| usize::from(window).min(arena_capacity).max(1))
}

/// Publish the AP network endpoint only while at least one peer has completed
/// authorization. Standalone and paired lifecycles use this same policy so a
/// role composition cannot leave one permanent logical device down.
pub const fn access_point_network_link_state(authorized_peers: u8) -> LinkState {
    if authorized_peers == 0 {
        LinkState::Down
    } else {
        LinkState::Up
    }
}

#[cfg(any(feature = "diagnostics", test))]
fn observe_aggregate_rate(observer: &dyn AggregateTxObserver, rate: HtRate) {
    observer.observe(AggregateTxObservation::RateSelected {
        bandwidth_mhz: match rate.channel_width {
            HtChannelWidth::Mhz20 => 20,
            HtChannelWidth::Mhz40 => 40,
        },
        nominal_kbps: rate.nominal_kbps(),
    });
}

/// Preserve a real DMA continuation, but do not report an ordered AP protocol
/// head as a descriptor-writeback probe once the physical frontier is drained.
const fn ap_rx_progress_while_protocol_tx_blocked(dma: DatapathRxProgress) -> DatapathRxProgress {
    match dma {
        DatapathRxProgress::Drained | DatapathRxProgress::UpperLayerBlockedButDroppable => {
            DatapathRxProgress::ProtocolBlockedByTx
        }
        pending => pending,
    }
}

/// An active TX keeps hardware out of the protocol consumer. The enclosing
/// radio owner remains responsible for executing the consumer's typed mailbox
/// actions after the protocol borrow ends.
const fn rx_protocol_consumer_has_hardware(tx_pending: bool) -> bool {
    !tx_pending
}

/// Keep one reorder release on a single ordered publication path after an
/// older cold frame has entered the deferred batch.
const fn can_publish_ap_rx_in_place(
    current_staging_owner: bool,
    current_is_amsdu: bool,
    deferred_bytes: usize,
) -> bool {
    current_staging_owner && !current_is_amsdu && deferred_bytes == 0
}

mod ampdu;
pub mod concurrent;
mod datapath;
pub mod network_tx;
mod protocol_mailbox;
mod rx_pipeline;
mod rx_reorder;

pub use self::rx_reorder::{Esp32s31AccessPointRxReorder, Esp32s31AccessPointRxReorderError};
pub use ampdu::Esp32s31AccessPointAmpdu;
pub use concurrent::{
    AccessPointRoleRuntime, Esp32s31StaApAccessPointFinishFailure,
    Esp32s31StaApAccessPointFinished, Esp32s31StaApAccessPointParkError,
    Esp32s31StaApAccessPointParkFailure, Esp32s31StaApAccessPointTxActive,
    Esp32s31StaApAccessPointTxParked, finish_sta_ap_access_point_role,
    park_sta_ap_access_point_role,
};
#[cfg(any(feature = "diagnostics", test))]
use datapath::BlockAckObservationState;
use datapath::Esp32s31AccessPointDatapathServices;
use network_tx::Esp32s31AccessPointNetworkTx;
pub use protocol_mailbox::{
    Esp32s31AccessPointHardwareAction, Esp32s31AccessPointProtocolAction,
    Esp32s31AccessPointProtocolMailbox, Esp32s31AccessPointProtocolPublisher,
    Esp32s31AccessPointProtocolReceiver,
};
#[doc(hidden)]
pub use rx_pipeline::{
    AccessPointRxProducer, AccessPointRxProducerObservation, AccessPointRxProtocolConsumer,
    AccessPointStagedRxFrame, Esp32s31AccessPointRxConsumer, Esp32s31AccessPointRxProducer,
};

// One protected frame can produce a BlockAck-window reset and one peer
// activity update. The active-TX protocol quantum owns four frames, so the
// mailbox covers exactly one complete bounded turn.
const AP_PROTOCOL_ACTION_CAPACITY: usize = 8;
// Only a reorder-window reset crosses the protocol-to-PAC boundary. Peer
// activity and power-save state are role-local values and are committed
// directly by the AP protocol owner.
const AP_PROTOCOL_ACTIONS_PER_RX_FRAME: usize = 1;

include!("access_point/control_types.rs");
include!("access_point/rx_dispatch.rs");
include!("access_point/protocol_owner.rs");
include!("access_point/control_owner.rs");
include!("access_point/protocol_service.rs");
include!("access_point/control_readiness.rs");
include!("access_point/protocol_shutdown.rs");
include!("access_point/service_epoch.rs");
include!("access_point/ethernet_diagnostics.rs");

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[test]
    fn network_link_requires_an_authorized_peer_in_every_ap_composition() {
        assert!(matches!(
            access_point_network_link_state(0),
            LinkState::Down
        ));
        assert!(matches!(access_point_network_link_state(1), LinkState::Up));
        assert!(matches!(
            access_point_network_link_state(AP_MAX_CLIENTS as u8),
            LinkState::Up
        ));
    }
}
