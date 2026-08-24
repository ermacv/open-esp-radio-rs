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
    FrameLengthError, LinkState, PinnedTxFrame, PinnedTxInterfaceConsumer, RxEnqueueError,
};

use open_esp_radio_esp32s31_wifi::{
    ampdu_tx::HtAmpduTxRolePolicy,
    ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxResources, WifiTxTimer},
    tx::{WifiTxProgress, WifiTxWake},
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_ap::mac::Esp32s31ApMacObservation;
use open_esp_radio_esp32s31_wifi_ap::protocol::{
    AccessPointServiceStatus, ApBufferedUnicastRelease, ApDownlinkDisposition, ApPeerClose,
    ApPeerPhase, ApPeerPowerState, ApPowerSaveAction, ApWpa2RetryProgress,
};
use open_esp_radio_esp32s31_wifi_ap::{
    ampdu::{Esp32s31ApAggregateAdmission, Esp32s31ApAmpduError, Esp32s31ApAmpduProgress},
    engine::{Esp32s31ApRuntimeHardware, Esp32s31ApWpa2Outcome},
    mac::{
        Esp32s31ApMac, Esp32s31ApMacError, Esp32s31ApMacParked, Esp32s31ApPeerDisconnectStage,
        Esp32s31ApTxCompletionAction,
    },
    rx::{
        Esp32s31ApRxConfig, Esp32s31ApRxDispatch, Esp32s31ApRxDispatcher, Esp32s31ApRxError,
        Esp32s31ApRxEvent, Esp32s31ApRxSink,
    },
};
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_esp32s31_wifi_mac::rx::RxDescriptorSnapshot;
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
#[cfg(any(feature = "diagnostics", test))]
use open_esp_radio_ieee80211::data::EthernetFrameParts;
use open_esp_radio_ieee80211::data::{
    DataInterfaceRole, IEEE80211_LEGACY_DATA_HEADER_LEN, IEEE80211_QOS_DATA_HEADER_LEN,
    plan_data_decapsulation,
};
use open_esp_radio_ieee80211::{
    ap::{
        ApManagementRequest, ApPowerSaveObservation, observe_ap_power_save,
        parse_ap_management_request,
    },
    block_ack::BlockAckAction,
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
    PreparedTxSchedulerTrace,
};
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
        network::DatapathNetworkRx,
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

/// Avoid per-frame MMIO polling while preserving a batched producer refill.
/// The DATAPATH owner explicitly services DMA again at each protocol-quantum
/// boundary before yielding.
const fn should_observe_ap_rx_dma(protocol_blocked: bool, queued_frames: usize) -> bool {
    protocol_blocked || queued_frames == 0
}

/// An active TX keeps hardware out of the protocol consumer. The enclosing
/// radio owner remains responsible for executing the consumer's typed mailbox
/// actions after the protocol borrow ends.
const fn rx_protocol_consumer_has_hardware(tx_pending: bool) -> bool {
    !tx_pending
}

/// Keep one reorder release on a single ordered publication path after an
/// older cold frame has entered the deferred batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AccessPointRxPublication {
    /// Lend the SRAM staging owner directly to the sole standalone endpoint.
    SharedStaging,
    /// Copy into the paired endpoint's PSRAM pool and release SRAM immediately.
    OwnedNetworkPool,
}

const fn can_publish_ap_rx_in_place(
    publication: AccessPointRxPublication,
    current_staging_owner: bool,
    current_is_amsdu: bool,
    deferred_bytes: usize,
) -> bool {
    matches!(publication, AccessPointRxPublication::SharedStaging)
        && current_staging_owner
        && !current_is_amsdu
        && deferred_bytes == 0
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
    Esp32s31AccessPointControlAction, Esp32s31AccessPointHardwareAction,
    Esp32s31AccessPointProtocolAction, Esp32s31AccessPointProtocolMailbox,
    Esp32s31AccessPointProtocolPublisher, Esp32s31AccessPointProtocolReceiver,
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
const AP_PROTOCOL_ACTIONS_PER_RX_FRAME: usize = 2;

include!("access_point/control_types.rs");
include!("access_point/rx_dispatch.rs");
include!("access_point/protocol_owner.rs");
include!("access_point/control_owner.rs");
include!("access_point/protocol_service.rs");
include!("access_point/control_readiness.rs");
include!("access_point/protocol_shutdown.rs");
include!("access_point/service_epoch.rs");
include!("access_point/ethernet_diagnostics.rs");
