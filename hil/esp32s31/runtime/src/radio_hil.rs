use core::{
    cell::RefCell,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
    task::{Context, Waker},
};

use crate::console::emergency_log;
use embassy_executor::{SendSpawner, Spawner};
use embassy_futures::{select::select, yield_now};
use embassy_net::{
    Config as NetworkConfig, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_net_driver::{Driver, LinkState, RxToken as _, TxToken as _};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::efuse::{self, InterfaceMacAddress};
use esp_hal::rng::{Rng, Trng};
use open_esp_radio::esp32s31::phy::PhyTxTargetPowerProfile;
use open_esp_radio::wifi::ieee80211::wmm::WmmParameterSet;
use open_esp_radio::{
    esp32s31::{
        hal::{ColdRadioRegisters, Radio, RadioRegisters},
        pac::{
            MacInterruptRegisters, MacInterruptSetup, MacPowerInterruptRegisters,
            mac::{self as mac_pac, init as mac_registers},
        },
        phy::{
            PhyCalibrationIdentity, PhyCalibrationPath, PhyRegisterRunError, PhyRegisterTransition,
            PhyRfBoundary, PhyTargetObserver, TargetPhyRegisterPort,
            phy_cold::{PhyCalibrationRecord, PhyColdState},
            phy_rfpll::phy_get_rf_cal_version,
            run_phy_register, select_phy_channel, switch_phy_channel_with_mac_restart,
            target_executor::{PhyAsyncDelay, PhyTargetPortError},
        },
        wifi::mac::{
            connected_rx::{
                ConnectedRxConfig, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink,
            },
            crypto::{
                CcmpKeyHardware, CryptoKeyError, StaGroupCcmpSlot, StaPairwiseCcmpSlot,
                install_sta_group_ccmp, install_sta_pairwise_ccmp,
            },
            descriptor::{DESCRIPTOR_BYTES, length as descriptor_length, rx_done},
            edca::EdcaParametersError,
            he::{He20PeerHardware, program_he20_peer_state},
            init::{
                MAC_COLD_RX_INTERRUPT_MASK, StaLinkRxPolicyHardware, StaNoiseFloorHardware,
                StaPeerScanPolicy, StaWmmSource, configure_sta_link_receive_policy,
                initialize_promiscuous_receive,
            },
            irq::{
                IrqSink, MAC_INT_COLLISION, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
                MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT, handle_mac_irq,
                handle_power_irq,
            },
            rate_control::{
                BeamformingReportHardware, StaRateControlAssociation, StaTxRatePolicy,
            },
            rate_schedule::schedule_state,
            registers::{
                MAC_INT_RAW, MAC_INT_STATUS, Mmio, RX_CONTROL, RX_DESCRIPTOR_BASE,
                RX_LAST_DESCRIPTOR, RX_LAST_DESCRIPTOR_HIGH, RX_NEXT_DESCRIPTOR, TX_Q_CONTROL,
                TX_Q_ENABLE_VALID,
            },
            rx::{
                HeGuardIntervalAndLtf, PUBLIC_HEADER_SIZE, RxDma, RxError, RxIngressConfig,
                RxRingError, RxRingHalted, RxRingLive, RxRingStopped, RxSegment, build_cold_ring,
                decode_rx_phy_info, disable_receive, enable_receive, extract_ccmp_data, extract_data,
                extract_management, first_segment_layout, publish_cold_ring,
            },
            rx_pool::RxStagePool,
            scan::{ScanObservation, ScanRecord, ScanTable},
            tx::{
                HeBccDcmMcs, HeDcmRate, HeEdcaTxopLimit, HeLdpcDcmMcs, HeMcs, HtGuardInterval,
                HtMcs, HtPeerAmpduParameters, LegacyRate, LegacyTxQueue, TxCompletion, TxError,
                TxHardware, TxPhyRate, TxSlot,
            },
            tx_ampdu::{HtAmpduTxStorage, StaTxBlockAckSessions},
            tx_runtime::StaTxRuntimePolicy,
        },
    },
    integration::{
        esp32s31::wifi_embassy::{
            aggregate_tx::{
                AggregateTxConfig, AggregateTxCounterSnapshot, AggregateTxCounters,
                Esp32s31ConnectedTx,
            },
            backend::Esp32s31WifiBackend,
            connected_control::Esp32s31ConnectedControl,
            control_tx::{
                ConnectedTxHandoff, ControlTxConfig, ControlTxError, Esp32s31ControlTx,
                WifiTxResources,
            },
            cooperative_tx::CooperativeTxHardware,
            embassy_irq::{EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime},
            embassy_rx::RxReloadDelay,
            link_monitor::StaBeaconLossConfig,
            runner::{WifiRunner, WifiRunnerExit},
            rx_backend::{
                ConnectedControlPublisher, ConnectedControlResources, ESP32S31_RX_BUFFER_SIZE,
                EmbassyNetConnectedRxSink, Esp32s31ConnectedRx, Esp32s31RxDmaStorage,
                Esp32s31RxEpochResources, Esp32s31StoppedRx, RxEnqueueCounters,
            },
            rx_reorder::{
                RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandResources, RxReorderFrameStorage,
            },
            rx_telemetry::{RxPipelineCounterSnapshot, RxPipelineCounters},
            single_mpdu_tx::{EmbassyWifiTxTimer, SingleMpduTxConfig},
            sta_join::{
                EmbassyStaJoinTimer, StaJoinBackend, StaJoinRunner, StaJoinRxDirective,
                StaJoinRxObserver,
            },
            sta_scan::{
                Esp32s31ActiveProbeOutcome, Esp32s31StaScanBackend, Esp32s31StaScanConfig,
                Esp32s31StaScanPort,
            },
            staged_rx::{
                ConnectedRxProtocolStopped, Esp32s31ConnectedRxProtocol,
                Esp32s31StagedRxQueue,
            },
            wpa2::{
                EmbassyWpa2HandshakeTimer, Wpa2HandshakeBackend, Wpa2HandshakeConfig,
                Wpa2HandshakeRunner, Wpa2KeyInstallBackend, Wpa2KeyInstallRunner,
                Wpa2PendingKeyInstall, Wpa2RxProgress,
            },
        },
        network::embassy_net::{
            PinnedTxFrame as OpenRadioNetworkTxFrame, PinnedTxPool as OpenRadioNetworkTxPool,
            SplitPinnedDevice as OpenRadioNetworkDevice,
            SplitPinnedRadioRunner as OpenRadioNetworkRunner,
            SplitPinnedResources as OpenRadioNetworkResources,
        },
    },
    wifi::ieee80211::{
        data::{DataInterfaceRole, decapsulate_data},
        he::HeDcmConstellation,
        management::ProbeRequest,
        scan::best_matching_ssid,
        station::{
            AssociationRequest, HeUlMuPowerCapability, HeUlMuPowerCapabilityError,
            OpenAuthenticationRequest, STA_PROTECTED_QOS_ETHERNET_HEADROOM, StaAssociationAttempt,
            StaAssociationPhy, StaAssociationPreference, StaAuthenticationAttempt, StaDataFrame,
            StaPowerCapability, StaPowerCapabilityError, StaProtectedDataFrame, StaSequenceCounter,
            StaTxSequenceCounters, select_sta_association, select_wpa2_psk_rsn,
        },
    },
    wifi::lifecycle::station::{
        StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffOutcome,
        StaBackoffReason, StaFailureDisposition, StaLifecycleBackend, StaLifecycleExit,
        StaLifecycleService, StaLifecycleStage, StaNextCandidate, StaReconnectPolicy,
    },
    wifi::lifecycle::scan::{
        StaCandidateScanExit, StaCandidateScanService, StaScanChannelContext,
    },
    wifi::wpa2::{
        OwnedEapolFrame, Pmk, Wpa2Interface, aes::Wpa2SoftwareAes, frames::Wpa2TxFrame,
        keys::Wpa2KeyKind,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;
use open_esp_radio_hil_protocol::{
    Capabilities, Completion as HilCompletion, Direction as HilDirection, Event as HilEvent,
    FeatureCapabilities, MAX_WIRE_FRAME_BYTES, NetworkCredentials, NetworkInfo, ServiceInfo,
    StartupArtifactDisposition, Transport as HilTransport, TransportEvidence,
};

// Coarse crash breadcrumbs for the standalone HIL panic handler. The action
// ordinal is deterministic for a fixed PHY transition and can therefore be
// matched back to the exact external binding in a host replay.
//
// Stage values:
// 10 run entry, 20 radio claim, 30 power-up, 40 IRQ ownership,
// 100 local transition, 110 binding lowering, 120 hardware completion,
// 130 state advance, 200 PHY complete, 210 channel select,
// 220 post-PHY diagnostics, 230 MAC/RX/scan, 250 terminal halt.
static DIAGNOSTIC_STAGE: AtomicU32 = AtomicU32::new(0);
static DIAGNOSTIC_ACTION_ORDINAL: AtomicU32 = AtomicU32::new(0);
static AUTH_REGISTER_SNAPSHOT_CAPTURED: AtomicBool = AtomicBool::new(false);

fn set_diagnostic_stage(stage: u32) {
    DIAGNOSTIC_STAGE.store(stage, Ordering::Release);
}

/// Returns the last coarse HIL stage and deterministic external-action ordinal.
///
/// The standalone panic handler snapshots these atomics into reset-persistent
/// memory before printing. They contain no peripheral ownership and are safe
/// to read even if a panic interrupts a radio transition.
pub fn diagnostic_snapshot() -> (u32, u32) {
    (
        DIAGNOSTIC_STAGE.load(Ordering::Acquire),
        DIAGNOSTIC_ACTION_ORDINAL.load(Ordering::Acquire),
    )
}
use static_cell::StaticCell;

const MAC_HANDSHAKE_SAMPLE_LIMIT: u32 = 100_000;
// A reset-separated 48-by-1,700 experiment reduced BUFFER_FULL from four to
// one at 80 Mbit/s/full MTU, but turned 3,196 of 3,959 service calls into
// staging backpressure and did not improve end-to-end delivery (both rings
// delivered every datagram). Keep one complete 32-member A-MPDU here; more
// DMA descriptors are not useful without increasing downstream ownership too.
const RX_DESCRIPTOR_COUNT: usize = 32;
const RX_DESCRIPTOR_COMPLETE_MASK: u64 = (1_u64 << RX_DESCRIPTOR_COUNT) - 1;
// Match the recovered vendor large-RX payload object. A larger MPDU is carried
// by a bounded descriptor chain and becomes one contiguous staged unit before
// any descriptor is returned to DMA. This avoids reserving 4,608 bytes in all
// 32 DMA slots merely because a peer may occasionally send an A-MSDU.
const RX_BUFFER_SIZE: usize = 1_700;
const RX_BUFFER_STORAGE_SIZE: usize = RX_BUFFER_SIZE + 4;
// Reset-separated 70/80-Mbit/s HE20 HIL observed 184,719 ordinary units, no
// A-MSDU and no unit above 1,336 bytes. Keep the recovered 1,700-byte vendor
// large-RX capacity on the hot path. The descriptor-chain owner remains able
// to identify a larger unit, which is explicitly discarded until a distinct
// cold-jumbo pool is composed rather than reserving jumbo capacity per slot.
const RX_STAGE_CAPACITY: usize = 1_700;
// A maximum 32-entry BlockAck window can retain at most 31 frames behind a
// gap. Sixty-four slots therefore cover those 31 owners, the next complete
// 32-descriptor hardware burst and one current frame. At 1,700 bytes this
// costs about 106 KiB, substantially less than the former 40-by-4,608 arena,
// while letting ordinary reorder stay in SRAM instead of copying to PSRAM.
const RX_STAGE_SLOT_COUNT: usize = 64;
// Smaller library compositions may still select the independent PSRAM
// backing. This HIL composition uses its 64 hot slots first; the cold owner is
// retained as a correctness fallback and for explicit lower-memory profiles.
const RX_BLOCK_ACK_SOFTWARE_WINDOW: usize = 32;
const _: () = assert!(RX_BLOCK_ACK_SOFTWARE_WINDOW <= RX_REORDER_BACKING_SLOT_COUNT);
const _: () = assert!(RX_BLOCK_ACK_SOFTWARE_WINDOW <= 64);
const NETWORK_FRAME_CAPACITY: usize = 1_600;
const CONNECTED_CONTROL_QUEUE_DEPTH: usize = 32;
// Raw A-MSDU/A-MPDU HIL generates TX below the network stack, and its direct
// UDP RX meter consumes the benchmark stream before the Embassy handoff.
// Deep Ethernet queues therefore only waste memory in those diagnostic
// images. Production-shaped RX retains one complete 32-frame hardware burst
// plus eight overlap owners in ordinary/PSRAM storage, matching the qualified
// 40-slot staging profile. A 64-entry experiment removed network-ready waits
// but increased PSRAM/cache cost and did not improve the 80-Mbit/s boundary.
// Repeating it with RX BlockAck reorder made starvation worse while at least
// eight network slots remained free, proving that the staging pool rather
// than this PSRAM-backed queue was the constrained owner.
// TX depth is selected independently because its backing pool is DMA-visible
// internal SRAM: ordinary and RX-only images keep 32 TX leases, while the
// dedicated TX throughput image retains 64.
//
// SOURCE: ESP32-S31 ESP-IDF Wi-Fi buffer documentation identifies 1.6 KiB as
// the fixed TX-buffer size and says TX throughput scales with the Wi-Fi/LwIP
// buffer counts; complete `_oracles/libnet80211.a[ieee80211_output.o]::
// ieee80211_encap_amsdu` only consumes already queued `s_tx_cacheq` entries.
const NETWORK_RX_QUEUE_DEPTH: usize = if OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_RAW_MAC_BENCH {
    4
} else if OPEN_RADIO_THROUGHPUT_BENCH && !OPEN_RADIO_BIDIRECTIONAL_BENCH {
    32
} else {
    40
};
const NETWORK_TX_QUEUE_DEPTH: usize = if OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_RAW_MAC_BENCH {
    4
} else if OPEN_RADIO_THROUGHPUT_BENCH
    && !OPEN_RADIO_BIDIRECTIONAL_BENCH
    && !OPEN_RADIO_NETWORK_AMSDU_BENCH
{
    64
} else {
    32
};
const OPEN_RADIO_UDP_RX_PORT: u16 = 4_323;
// The production radio/network handoff can publish a complete 32-frame burst
// before the UDP task is polled. Retain two such bursts so smoltcp does not
// discard the second half of an A-MPDU merely because its application future
// follows the stack and radio futures in the cooperative executor.
const OPEN_RADIO_UDP_RX_QUEUE_DEPTH: usize = 64;
const OPEN_RADIO_UDP_PAYLOAD_CAPACITY: usize = 1_472;
const OPEN_RADIO_UDP_TX_QUEUE_DEPTH: usize = 16;
// The simultaneous qualification owns a second socket so RX can remain bound
// while the ordinary TX benchmark socket drives uplink traffic. It retains two
// complete 32-MPDU bursts for the same reason as the RX-only profile: a socket
// resource bound must not turn one cooperative executor interval into packet
// loss. This is PSRAM-backed socket storage, not a per-poll processing limit.
const OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH: usize = if OPEN_RADIO_BIDIRECTIONAL_BENCH {
    OPEN_RADIO_UDP_RX_QUEUE_DEPTH
} else {
    1
};
const OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH: usize = 1;
// Only one benchmark direction is compiled into a HIL image. Keep the
// inactive direction at one packet instead of reserving another 23 KiB of
// internal SRAM. HIL 2026-07-29: retaining two 16-packet payload rings reduced
// the executor stack frontier enough to corrupt TxStorage while the uplink
// benchmark and the 32-frame A-MPDU owner were live.
const OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH: usize = if option_env!("OPEN_RADIO_TX_BENCH").is_some() {
    1
} else {
    OPEN_RADIO_UDP_RX_QUEUE_DEPTH
};
const OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH: usize = if option_env!("OPEN_RADIO_TX_BENCH").is_some() {
    OPEN_RADIO_UDP_TX_QUEUE_DEPTH
} else {
    1
};
const OPEN_RADIO_UDP_TX_BENCH_PORT: u16 = 9_002;
const OPEN_RADIO_UDP_TX_BENCH_DURATION: Duration = Duration::from_secs(5);
const OPEN_RADIO_UDP_TX_DRAIN: Duration = Duration::from_millis(250);
const OPEN_RADIO_UDP_RX_IDLE: Duration = Duration::from_millis(750);
const OPEN_RADIO_TCP_RX_PORT: u16 = 4_325;
const OPEN_RADIO_TCP_RX_BUFFER_CAPACITY: usize = 65_536;
const OPEN_RADIO_TCP_TX_BUFFER_CAPACITY: usize = 1_024;
const OPEN_RADIO_TCP_READ_CAPACITY: usize = 32_768;
const OPEN_RADIO_TCP_CHUNK_CAPACITY: usize = 32_768;
const OPEN_RADIO_TCP_IDLE_TIMEOUT: Duration = Duration::from_secs(3);
const OPEN_RADIO_RX_APPLICATION_HANDOFF_BUDGET: Duration = Duration::from_micros(500);
const OPEN_RADIO_THROUGHPUT_BENCH: bool = option_env!("OPEN_RADIO_TX_BENCH").is_some();
const OPEN_RADIO_BIDIRECTIONAL_BENCH: bool =
    option_env!("OPEN_RADIO_BIDIRECTIONAL_BENCH").is_some();
const OPEN_RADIO_TCP_RX_BENCH: bool = option_env!("OPEN_RADIO_TCP_RX_BENCH").is_some();
const OPEN_RADIO_TASK_POLL_TELEMETRY: bool = cfg!(feature = "task-poll-telemetry");
const OPEN_RADIO_RX_ORDER_TELEMETRY: bool = cfg!(feature = "rx-order-telemetry");
const OPEN_RADIO_STACK_SOCKET_COUNT: usize = if OPEN_RADIO_BIDIRECTIONAL_BENCH { 5 } else { 4 };

/// Describes only behavior implemented by the current image. Every advertised
/// throughput profile uses runtime sessions and structured evidence.
pub const fn hil_capabilities() -> Capabilities {
    Capabilities {
        features: FeatureCapabilities {
            udp: !OPEN_RADIO_TCP_RX_BENCH,
            tcp: OPEN_RADIO_TCP_RX_BENCH,
            rx: !OPEN_RADIO_THROUGHPUT_BENCH || OPEN_RADIO_BIDIRECTIONAL_BENCH,
            tx: OPEN_RADIO_THROUGHPUT_BENCH && !OPEN_RADIO_TCP_RX_BENCH,
            bidirectional: OPEN_RADIO_BIDIRECTIONAL_BENCH && !OPEN_RADIO_TCP_RX_BENCH,
            network_provisioning: true,
            runtime_configuration: OPEN_RADIO_RUNTIME_SESSIONS,
            structured_evidence: OPEN_RADIO_RUNTIME_SESSIONS,
            startup_artifact: true,
            station_epoch_control: true,
        },
        maximum_payload_bytes: if OPEN_RADIO_TCP_RX_BENCH {
            OPEN_RADIO_TCP_CHUNK_CAPACITY as u16
        } else {
            OPEN_RADIO_UDP_PAYLOAD_CAPACITY as u16
        },
        maximum_wire_frame_bytes: MAX_WIRE_FRAME_BYTES as u16,
    }
}

// Every HE matrix owns a synthetic A-MPDU traffic source. Requiring a second
// independent build flag previously allowed the matrix selector and its log
// labels to be active while no matrix traffic was generated.
const OPEN_RADIO_RAW_MAC_BENCH: bool = option_env!("OPEN_RADIO_RAW_MAC_BENCH").is_some()
    || option_env!("OPEN_RADIO_HE_MATRIX_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_LDPC_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_DCM_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_TB_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_DELIMITER_HIL").is_some();
const OPEN_RADIO_RUNTIME_RX_SESSIONS: bool =
    !OPEN_RADIO_THROUGHPUT_BENCH && !OPEN_RADIO_RAW_MAC_BENCH;
const OPEN_RADIO_RUNTIME_TX_SESSIONS: bool =
    OPEN_RADIO_THROUGHPUT_BENCH && !OPEN_RADIO_BIDIRECTIONAL_BENCH;
const OPEN_RADIO_RUNTIME_BIDIRECTIONAL_SESSIONS: bool = OPEN_RADIO_BIDIRECTIONAL_BENCH;
const OPEN_RADIO_RUNTIME_SESSIONS: bool =
    OPEN_RADIO_RUNTIME_RX_SESSIONS
        || OPEN_RADIO_RUNTIME_TX_SESSIONS
        || OPEN_RADIO_RUNTIME_BIDIRECTIONAL_SESSIONS;
const OPEN_RADIO_AMSDU_BENCH: bool = option_env!("OPEN_RADIO_AMSDU_BENCH").is_some();
// Exercise the blob-exact cache-ESF A-MSDU ownership edge on real
// embassy-net frames. Unlike OPEN_RADIO_AMSDU_BENCH, this does not synthesize
// or reuse a body below the network stack.
const OPEN_RADIO_NETWORK_AMSDU_BENCH: bool =
    option_env!("OPEN_RADIO_NETWORK_AMSDU_BENCH").is_some();
const OPEN_RADIO_HE_DELIMITER_HIL: bool = option_env!("OPEN_RADIO_HE_DELIMITER_HIL").is_some();
const OPEN_RADIO_HE_MATRIX_HIL: bool =
    option_env!("OPEN_RADIO_HE_MATRIX_HIL").is_some() || OPEN_RADIO_HE_DELIMITER_HIL;
const OPEN_RADIO_HE_DCM_HIL: bool = option_env!("OPEN_RADIO_HE_DCM_HIL").is_some();
const OPEN_RADIO_HE_TB_HIL: bool = option_env!("OPEN_RADIO_HE_TB_HIL").is_some();
const _: () = assert!(
    !OPEN_RADIO_RAW_MAC_BENCH && !OPEN_RADIO_AMSDU_BENCH && !OPEN_RADIO_NETWORK_AMSDU_BENCH,
    "legacy raw/A-MPDU/A-MSDU HIL profiles are not wired to the production WifiRunner"
);
// One slot must admit the complete baseline 3,839-byte A-MSDU class plus the
// outer QoS/CCMP headers, hardware MIC/FCS and S31 private metadata.
const TX_BUFFER_SIZE: usize = if OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_NETWORK_AMSDU_BENCH {
    3_904
} else {
    1_700
};
// Production connected TX references pinned embassy-net allocations directly,
// so this owner needs only descriptor/scalar storage, never a second payload
// arena. The full negotiated BlockAck window remains statically bounded.
const TX_AMPDU_BUFFER_SIZE: usize = 0;
const TX_AMPDU_FRAME_COUNT: usize = 32;
// The recovered station connection-complete path negotiates a 32-MPDU
// receive window even while the first production TX owner emits one MPDU.
const TX_BLOCK_ACK_WINDOW: usize = 32;
const CONNECTED_BEACON_MISS_LIMIT: u8 = 10;
const fn selected_tx_bench_rate_kbps(value: Option<&str>) -> Option<u64> {
    let Some(value) = value else {
        return None;
    };
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        panic!("OPEN_RADIO_TX_BENCH_RATE_KBPS must be 1..1000000");
    }
    let mut result = 0_u64;
    let mut index = 0_usize;
    while index < bytes.len() {
        let digit = bytes[index];
        if digit < b'0' || digit > b'9' {
            panic!("OPEN_RADIO_TX_BENCH_RATE_KBPS must be 1..1000000");
        }
        result = result * 10 + (digit - b'0') as u64;
        index += 1;
    }
    if result == 0 || result > 1_000_000 {
        panic!("OPEN_RADIO_TX_BENCH_RATE_KBPS must be 1..1000000");
    }
    Some(result)
}
// Optional application-side offered-load bound for low-rate bidirectional
// certification. It does not alter EDCA, the selected PHY vector or driver
// retry policy; it merely prevents a saturated MCS0/DCM UDP producer from
// consuming nearly all airtime while the peer is expected to send a
// simultaneous downlink stream.
const OPEN_RADIO_TX_BENCH_RATE_KBPS: Option<u64> =
    selected_tx_bench_rate_kbps(option_env!("OPEN_RADIO_TX_BENCH_RATE_KBPS"));
const PROBE_TX_DESCRIPTOR_CAPACITY: usize = 88;
const TX_METADATA_SIZE: usize = 8;
const TX_FCS_SIZE: usize = 4;
const TX_CCMP_MIC_SIZE: usize = 8;
// The cache-TX/type-nine vendor path preserves the Ethernet payload in its
// netstack allocation. For the open equivalent, eight S31-private DMA bytes
// precede the 28-byte QoS/CCMP/LLC replacement prefix; MIC and FCS are emitted
// after the frame in the same permanent allocation.
//
// SOURCE: complete `_oracles/libnet80211.a[ieee80211_output.o]::
// ieee80211_alloc_tx_buf`, complete
// `_oracles/libpp.a[esf_buf.o]::{esf_buf_setup,esf_buf_alloc}`, and the
// in-place layout proof in `open-esp-radio-ieee80211::station`.
const NETWORK_TX_HEADROOM: usize = TX_METADATA_SIZE + STA_PROTECTED_QOS_ETHERNET_HEADROOM;
// Pair A-MSDU coalescing grows the first 1,600-byte network allocation to the
// negotiated 3,839-byte A-MSDU class. 2,304 bytes cover the exact maximum
// outer QoS/CCMP MPDU plus hardware MIC/FCS after accounting for the existing
// 36-byte metadata/headroom prefix. Ordinary builds retain only the hardware
// trailer and do not pay this SRAM cost.
//
// SOURCE: complete `_oracles/libnet80211.a[ieee80211_output.o]::
// ieee80211_encap_amsdu` grows the first cache ESF in place before copying and
// recycling later ESFs. The open `StaProtectedEthernetFrame`::
// `encode_amsdu_pair_in_place` owns the same bounded operation.
const NETWORK_TX_TRAILER: usize = if OPEN_RADIO_NETWORK_AMSDU_BENCH {
    2_304
} else {
    TX_CCMP_MIC_SIZE + TX_FCS_SIZE
};
const TX_COMPLETION_DEADLINE_MS: u64 = 250;
const UNICAST_TX_ATTEMPT_LIMIT: u8 = 4;
// First throughput baseline: legacy OFDM at its maximum PHY rate, HT20/HT40
// disabled. Management and EAPOL frames remain at the conservative 1-Mbit/s
// rate. This deliberately separates basic MAC/PHY/DMA performance from the
// still-unwired HT PLCP and A-MPDU paths.
//
// SOURCE: the hardware code is `WIFI_PHY_RATE_54M = 0x0c` in the sibling
// esp-wifi-sys S31 oracle; open-esp-radio-esp32s31-wifi-mac::tx::LegacyRate records
// the complete typed mapping and blob/ROM provenance.
//
// `OPEN_RADIO_LEGACY_RATE_MBIT` is deliberately limited to the non-HT OFDM
// rates qualified by this descriptor path. CCK rates remain available in
// `LegacyRate`, but are kept out of the performance selector until their
// long/short preamble policy is explicit. HT MCS values require a different
// PLCP/association path and cannot accidentally enter this raw legacy slot.
const fn selected_open_radio_data_rate(value: Option<&str>) -> LegacyRate {
    let Some(value) = value else {
        return LegacyRate::Ofdm54M;
    };
    let bytes = value.as_bytes();
    if bytes.len() == 1 {
        return match bytes[0] {
            b'6' => LegacyRate::Ofdm6M,
            b'9' => LegacyRate::Ofdm9M,
            _ => {
                panic!("OPEN_RADIO_LEGACY_RATE_MBIT must be 6, 9, 12, 18, 24, 36, 48 or 54")
            }
        };
    }
    if bytes.len() == 2 {
        return match (bytes[0], bytes[1]) {
            (b'1', b'2') => LegacyRate::Ofdm12M,
            (b'1', b'8') => LegacyRate::Ofdm18M,
            (b'2', b'4') => LegacyRate::Ofdm24M,
            (b'3', b'6') => LegacyRate::Ofdm36M,
            (b'4', b'8') => LegacyRate::Ofdm48M,
            (b'5', b'4') => LegacyRate::Ofdm54M,
            _ => {
                panic!("OPEN_RADIO_LEGACY_RATE_MBIT must be 6, 9, 12, 18, 24, 36, 48 or 54")
            }
        };
    }
    panic!("OPEN_RADIO_LEGACY_RATE_MBIT must be 6, 9, 12, 18, 24, 36, 48 or 54")
}

const OPEN_RADIO_DATA_RATE: LegacyRate =
    selected_open_radio_data_rate(option_env!("OPEN_RADIO_LEGACY_RATE_MBIT"));

const fn selected_open_radio_ht_mcs(value: Option<&str>) -> HtMcs {
    let Some(value) = value else {
        return HtMcs::Mcs7;
    };
    match value.as_bytes() {
        [b'0'] => HtMcs::Mcs0,
        [b'1'] => HtMcs::Mcs1,
        [b'2'] => HtMcs::Mcs2,
        [b'3'] => HtMcs::Mcs3,
        [b'4'] => HtMcs::Mcs4,
        [b'5'] => HtMcs::Mcs5,
        [b'6'] => HtMcs::Mcs6,
        [b'7'] => HtMcs::Mcs7,
        _ => panic!("OPEN_RADIO_HT_MCS must be 0..7 for the 1T1R HT path"),
    }
}

const OPEN_RADIO_HT_MCS: HtMcs = selected_open_radio_ht_mcs(option_env!("OPEN_RADIO_HT_MCS"));

const fn selected_open_radio_he_mcs(value: Option<&str>) -> HeMcs {
    let Some(value) = value else {
        return HeMcs::Mcs9;
    };
    match value.as_bytes() {
        [b'0'] => HeMcs::Mcs0,
        [b'1'] => HeMcs::Mcs1,
        [b'2'] => HeMcs::Mcs2,
        [b'3'] => HeMcs::Mcs3,
        [b'4'] => HeMcs::Mcs4,
        [b'5'] => HeMcs::Mcs5,
        [b'6'] => HeMcs::Mcs6,
        [b'7'] => HeMcs::Mcs7,
        [b'8'] => HeMcs::Mcs8,
        [b'9'] => HeMcs::Mcs9,
        _ => panic!("OPEN_RADIO_HE_MCS must be 0..9 for the 1T1R HE SU path"),
    }
}

const fn selected_open_radio_he_gi_ltf(value: Option<&str>) -> HeGuardIntervalAndLtf {
    let Some(value) = value else {
        // This is the exact active vendor HE20/MCS9 profile captured on the
        // same S31 and FRITZ!Box association.
        return HeGuardIntervalAndLtf::TwoLtf800Ns;
    };
    match value.as_bytes() {
        [b'0'] => HeGuardIntervalAndLtf::OneLtf800Ns,
        [b'1'] => HeGuardIntervalAndLtf::TwoLtf800Ns,
        [b'2'] => HeGuardIntervalAndLtf::TwoLtf1600Ns,
        [b'3'] => HeGuardIntervalAndLtf::FourLtf3200Ns,
        _ => panic!("OPEN_RADIO_HE_GI_LTF must be 0..3"),
    }
}

const OPEN_RADIO_HE_MCS: HeMcs = selected_open_radio_he_mcs(option_env!("OPEN_RADIO_HE_MCS"));
const OPEN_RADIO_HE_GI_LTF: HeGuardIntervalAndLtf =
    selected_open_radio_he_gi_ltf(option_env!("OPEN_RADIO_HE_GI_LTF"));

const fn selected_open_radio_he_dcm_rate(
    mcs: Option<&str>,
    ldpc: bool,
    guard_interval_and_ltf: HeGuardIntervalAndLtf,
) -> Option<HeDcmRate> {
    let Some(mcs) = mcs else {
        if ldpc {
            panic!("OPEN_RADIO_HE_DCM_LDPC requires OPEN_RADIO_HE_DCM_MCS");
        }
        return None;
    };
    match (mcs.as_bytes(), ldpc) {
        ([b'0'], false) => Some(HeDcmRate::bcc(HeBccDcmMcs::Mcs0, guard_interval_and_ltf)),
        ([b'1'], false) => Some(HeDcmRate::bcc(HeBccDcmMcs::Mcs1, guard_interval_and_ltf)),
        ([b'3'], false) => Some(HeDcmRate::bcc(HeBccDcmMcs::Mcs3, guard_interval_and_ltf)),
        ([b'0'], true) => Some(HeDcmRate::ldpc(HeLdpcDcmMcs::Mcs0, guard_interval_and_ltf)),
        ([b'1'], true) => Some(HeDcmRate::ldpc(HeLdpcDcmMcs::Mcs1, guard_interval_and_ltf)),
        ([b'3'], true) => Some(HeDcmRate::ldpc(HeLdpcDcmMcs::Mcs3, guard_interval_and_ltf)),
        ([b'4'], true) => Some(HeDcmRate::ldpc(HeLdpcDcmMcs::Mcs4, guard_interval_and_ltf)),
        (_, false) => panic!("BCC HE DCM MCS must be 0, 1 or 3"),
        (_, true) => panic!("LDPC HE DCM MCS must be 0, 1, 3 or 4"),
    }
}

const OPEN_RADIO_HE_DCM_RATE: Option<HeDcmRate> = selected_open_radio_he_dcm_rate(
    option_env!("OPEN_RADIO_HE_DCM_MCS"),
    option_env!("OPEN_RADIO_HE_DCM_LDPC").is_some(),
    OPEN_RADIO_HE_GI_LTF,
);

const OPEN_RADIO_HT_GI: HtGuardInterval = if option_env!("OPEN_RADIO_HT_SGI").is_some() {
    HtGuardInterval::Short400Ns
} else {
    HtGuardInterval::Long800Ns
};
const fn selected_max_tx_power_quarter_dbm(value: Option<&str>) -> i8 {
    let Some(value) = value else {
        return 80;
    };
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 2 {
        panic!("OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM must be 0..80");
    }
    let mut result = 0_u8;
    let mut index = 0;
    while index < bytes.len() {
        let digit = bytes[index];
        if digit < b'0' || digit > b'9' {
            panic!("OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM must be 0..80");
        }
        result = result * 10 + digit - b'0';
        index += 1;
    }
    if result > 80 {
        panic!("OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM must be 0..80");
    }
    result as i8
}

// Keep the ordinary open HIL at the board's 20-dBm limit, while allowing a
// build-time HIL override for reproducible near-field power sweeps. The public
// Wi-Fi API uses quarter-dBm, so 80 becomes MAC power code 20 in the currently
// recovered open profile. Three consecutive full open STA runs at code 20
// completed Authentication, Association, WPA2 and DHCP. Limiting this profile
// to 20 quarter-dBm/code 5 made the same close-range link intermittent,
// including complete 12-attempt Authentication failures.
//
// SOURCE: esp-wifi-sys `c/headers/esp32s31/esp_wifi.h` documents the
// quarter-dBm API; open-esp-radio-rs SVD provenance source
// `HIL_OPEN_TX_POWER_CONNECTIVITY_2026_07_28` records the reset, connection,
// ping and retry results. This is a HIL policy limit, not yet a claim that the
// open and vendor power-table encodings are fully equivalent.
const OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM: i8 =
    selected_max_tx_power_quarter_dbm(option_env!("OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM"));
// A common 100-TU beacon interval is 102.4 ms. The old 100-ms dwell could
// therefore miss an AP even during a full-domain scan when the wildcard
// probe response was lost. Cover almost two complete beacon intervals while
// keeping a bounded scan across all supported 2.4-GHz channels.
const SCAN_DWELL_MS: u16 = 200;
const PERF_AP_PROFILE: bool = option_env!("OPEN_RADIO_PERF_AP").is_some()
    || OPEN_RADIO_HE_MATRIX_HIL
    || OPEN_RADIO_HE_DCM_HIL
    || OPEN_RADIO_HE_TB_HIL
    || OPEN_RADIO_HE_DELIMITER_HIL;
const fn selected_sta_channel(value: Option<&str>, default: u16) -> u16 {
    let Some(value) = value else {
        return default;
    };
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 2 {
        return default;
    }

    let mut channel = 0_u16;
    let mut index = 0_usize;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte < b'0' || byte > b'9' {
            return default;
        }
        channel = channel * 10 + (byte - b'0') as u16;
        index += 1;
    }
    if channel >= 1 && channel <= 13 {
        channel
    } else {
        default
    }
}
const LISTEN_CHANNEL: u16 = selected_sta_channel(
    option_env!("OPEN_RADIO_STA_CHANNEL"),
    if PERF_AP_PROFILE { 11 } else { 6 },
);
const STA_SCAN_CHANNEL_COUNT: usize = 13;
// `OPEN_RADIO_STA_CHANNEL` remains a useful first-channel hint for controlled
// HIL setups, but it must never pin an ordinary connection. Scan that channel
// first and then every other ESP32-S31 2.4-GHz channel exactly once.
const fn sta_scan_channel(index: usize) -> u8 {
    let preferred = LISTEN_CHANNEL as u8;
    if index == 0 {
        preferred
    } else {
        let sequential = index as u8;
        if sequential >= preferred {
            sequential + 1
        } else {
            sequential
        }
    }
}

const fn sta_scan_channels() -> [u8; STA_SCAN_CHANNEL_COUNT] {
    let mut channels = [0; STA_SCAN_CHANNEL_COUNT];
    let mut index = 0;
    while index < STA_SCAN_CHANNEL_COUNT {
        channels[index] = sta_scan_channel(index);
        index += 1;
    }
    channels
}

const STA_SCAN_CHANNELS: [u8; STA_SCAN_CHANNEL_COUNT] = sta_scan_channels();
const WPA2_PROTECTED_ARP_TIMEOUT_MS: u32 = 1_500;
const WPA2_CONTROLLED_PORT_SETTLE_MS: u64 = 10;
const WPA2_PROTECTED_ARP_ATTEMPTS: u8 = 3;
const WPA2_PROTECTED_ARP_RETRY_DELAY_MS: u64 = 20;
// Migration installs both keys before queueing M4, but keeps STA EAPOL on its
// measured plaintext layout until the M4 TX-done edge opens the controlled
// port. Protected M4 remains a useful explicit negative control experiment.
const WPA2_MESSAGE_4_HARDWARE_PROTECTED: bool = false;
const LLC_SNAP_EAPOL: [u8; 8] = [0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e];

const fn selected_ipv4(value: Option<&str>, default: [u8; 4]) -> [u8; 4] {
    let Some(value) = value else {
        return default;
    };
    let bytes = value.as_bytes();
    let mut result = [0_u8; 4];
    let mut result_index = 0_usize;
    let mut value_index = 0_usize;
    let mut octet = 0_u16;
    let mut digits = 0_u8;

    while value_index < bytes.len() {
        let byte = bytes[value_index];
        if byte >= b'0' && byte <= b'9' {
            octet = octet * 10 + (byte - b'0') as u16;
            digits += 1;
            if octet > u8::MAX as u16 || digits > 3 {
                panic!("OPEN_RADIO IPv4 octet must be in 0..=255");
            }
        } else if byte == b'.' && digits != 0 && result_index < 3 {
            result[result_index] = octet as u8;
            result_index += 1;
            octet = 0;
            digits = 0;
        } else {
            panic!("OPEN_RADIO IPv4 value must contain four decimal octets");
        }
        value_index += 1;
    }
    if result_index != 3 || digits == 0 {
        panic!("OPEN_RADIO IPv4 value must contain four decimal octets");
    }
    result[3] = octet as u8;
    result
}

const DEFAULT_STA_ARP_TARGET_IPV4: [u8; 4] = if PERF_AP_PROFILE {
    [10, 42, 0, 1]
} else {
    [192, 168, 178, 1]
};
const DEFAULT_STA_HIL_IPV4: [u8; 4] = if PERF_AP_PROFILE {
    [10, 42, 0, 138]
} else {
    [192, 168, 178, 138]
};
const STA_ARP_TARGET_IPV4: [u8; 4] = selected_ipv4(
    option_env!("OPEN_RADIO_STA_GATEWAY_IPV4"),
    DEFAULT_STA_ARP_TARGET_IPV4,
);
const OPEN_RADIO_TX_BENCH_TARGET_IPV4: [u8; 4] = selected_ipv4(
    option_env!("OPEN_RADIO_TX_BENCH_TARGET_IPV4"),
    STA_ARP_TARGET_IPV4,
);
const STA_HIL_IPV4: [u8; 4] =
    selected_ipv4(option_env!("OPEN_RADIO_STA_IPV4"), DEFAULT_STA_HIL_IPV4);
// The controlled Linux AP normally serves both as the gateway and as the
// externally observed ARP/ping peer. Android tethering chooses a different
// RFC 1918 prefix on every platform and therefore needs an explicit peer.
// Keep the ordinary FRITZ profile's laptop target as the compatibility
// default, but never silently reuse the controlled-AP 10.42.0.1 address when
// a HIL run selected another reachable peer.
const LAN_PROBE_IPV4: [u8; 4] = selected_ipv4(
    option_env!("OPEN_RADIO_LAN_PROBE_IPV4"),
    if PERF_AP_PROFILE {
        [10, 42, 0, 1]
    } else {
        [192, 168, 178, 129]
    },
);
const PROBE_REQUEST_RATES: [u8; 12] = [
    0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
];

type RxStorage =
    Esp32s31RxDmaStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
const _: () = assert!(RX_BUFFER_SIZE <= ESP32S31_RX_BUFFER_SIZE);

type ControlTx = Esp32s31ControlTx<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;

struct TxStorage {
    control: Option<ControlTx>,
}

impl TxStorage {
    fn new(
        slot: Pin<&'static mut TxSlot<TX_BUFFER_SIZE>>,
        tx_power_profile: PhyTxTargetPowerProfile,
    ) -> Self {
        Self {
            control: Some(Esp32s31ControlTx::new(
                WifiTxResources {
                    slot,
                    policy: StaTxRuntimePolicy::vendor_defaults(),
                    power: tx_power_profile,
                    entropy: open_radio_tx_entropy as fn() -> u32,
                    timer: EmbassyWifiTxTimer,
                },
                ControlTxConfig {
                    unicast_attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
                    completion_timeout_us: TX_COMPLETION_DEADLINE_MS * 1_000,
                    poll_interval_us: 1,
                },
            )),
        }
    }

    fn control_mut(&mut self) -> &mut ControlTx {
        self.control
            .as_mut()
            .expect("control TX owner has not moved into the connected runner")
    }

    fn install_ht_ampdu_policy(&mut self, parameters: HtPeerAmpduParameters) {
        self.control_mut().install_ht_ampdu_policy(parameters);
    }

    fn install_he_bss_color(&mut self, bss_color: u8) {
        self.control_mut().install_he_bss_color(bss_color);
    }

    fn install_wmm_edca(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError> {
        self.control_mut().install_wmm_edca(parameters)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveScanTxError {
    PowerCapability(StaPowerCapabilityError),
    HeUlMuPower(HeUlMuPowerCapabilityError),
    Control(ControlTxError),
}

impl From<ControlTxError> for ActiveScanTxError {
    fn from(error: ControlTxError) -> Self {
        Self::Control(error)
    }
}

// The full PSRAM/PSRAM profile keeps ordinary state external, but the Wi-Fi
// DMA master consumes these descriptors and buffers directly. Their explicit
// section names retain both ownership objects in internal SRAM in every
// memory-profile image.
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_rx")]
static OPEN_RADIO_RX_DMA_STORAGE: StaticCell<RxStorage> = StaticCell::new();
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_tx")]
static OPEN_RADIO_TX_DMA_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
static OPEN_RADIO_TX_STATE: StaticCell<TxStorage> = StaticCell::new();
// The cold owner is moved here only after scan/authentication has consumed
// every polling-only MAC transition. Connected Embassy tasks may then borrow
// one permanently located running owner without manufacturing another PAC
// singleton or tying their lifetime to the parent HIL future's stack.
static OPEN_RADIO_RUNNING_REGISTERS: StaticCell<RadioRegisters> = StaticCell::new();
static OPEN_RADIO_REGISTER_CELL: StaticCell<RefCell<&'static mut RadioRegisters>> =
    StaticCell::new();
static OPEN_RADIO_RX_BUFFER_ADDRESSES: StaticCell<[u32; RX_DESCRIPTOR_COUNT]> =
    StaticCell::new();
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_tx_ampdu")]
static OPEN_RADIO_TX_AMPDU_STORAGE: StaticCell<
    HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
> = StaticCell::new();
static SCAN_TABLE: StaticCell<ScanTable> = StaticCell::new();
static SCAN_FRAME: StaticCell<[u8; RX_STAGE_CAPACITY]> = StaticCell::new();
static ETHERNET_FRAME: StaticCell<[u8; RX_STAGE_CAPACITY]> = StaticCell::new();
// The vendor `wDev_IndicateFrame` allocates an ESF buffer and copies the
// completed RX unit before `wDev_DiscardFrame` returns the DMA descriptors.
// Keep the same ownership boundary explicit in the open HIL. This hot staging
// object stays in internal CPU-owned SRAM; unlike OPEN_RADIO_RX_DMA_STORAGE,
// the Wi-Fi DMA master never addresses it. Its capacity follows the negotiated
// MPDU geometry rather than the narrower ordinary vendor singleton profile.
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.open_radio_rx_stage")]
static OPEN_RADIO_RX_STAGE_POOL: RxStagePool<
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
> = RxStagePool::new();
type StagedRxQueue = Esp32s31StagedRxQueue<
    'static,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
>;
static OPEN_RADIO_STAGED_RX_QUEUE: StagedRxQueue = StagedRxQueue::new();
static OPEN_RADIO_RX_REORDER_COMMANDS: RxReorderCommandResources<CriticalSectionRawMutex> =
    RxReorderCommandResources::new();
// Ordinary `.bss` belongs to PSRAM in the qualified profile. Only MPDUs that
// actually cross a sequence gap touch this cold backing; in-order frames stay
// on the internal SRAM staging fast path.
static OPEN_RADIO_RX_REORDER_STORAGE: RxReorderFrameStorage<RX_STAGE_CAPACITY> =
    RxReorderFrameStorage::new();
type NetworkResources = OpenRadioNetworkResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkDevice = OpenRadioNetworkDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkRunner = OpenRadioNetworkRunner<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkTxFrame = OpenRadioNetworkTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type NetworkTxPool = OpenRadioNetworkTxPool<
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type ControlResources =
    ConnectedControlResources<CriticalSectionRawMutex, CONNECTED_CONTROL_QUEUE_DEPTH>;
type ControlPublisher = ConnectedControlPublisher<
    'static,
    CriticalSectionRawMutex,
    CONNECTED_CONTROL_QUEUE_DEPTH,
>;
type ConnectedNetworkRxSink = EmbassyNetConnectedRxSink<
    'static,
    CriticalSectionRawMutex,
    HilConnectedRxObserver<ControlPublisher>,
    NETWORK_FRAME_CAPACITY,
    NETWORK_RX_QUEUE_DEPTH,
>;
type ConnectedRxProtocol = Esp32s31ConnectedRxProtocol<
    'static,
    'static,
    'static,
    'static,
    CriticalSectionRawMutex,
    ConnectedNetworkRxSink,
    RX_STAGE_SLOT_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
>;
type ConnectedNetworkStackRunner = embassy_net::Runner<'static, NetworkDevice>;
type ConnectedHardware = CooperativeTxHardware<'static, 'static>;

fn assert_join_hardware_capabilities<
    H: Mmio
        + RxDma
        + TxHardware
        + CcmpKeyHardware
        + He20PeerHardware
        + BeamformingReportHardware
        + StaLinkRxPolicyHardware
        + StaNoiseFloorHardware,
>(
    _: &H,
) {
}

type ConnectedStoppedRx = Esp32s31StoppedRx<
    'static,
    'static,
    'static,
    OpenRadioRxReloadDelay,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
>;
type ConnectedRxEpochResources = Esp32s31RxEpochResources<
    'static,
    'static,
    'static,
    OpenRadioRxReloadDelay,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
    RX_BUFFER_SIZE,
    RX_BUFFER_STORAGE_SIZE,
>;

/// Hardware/storage input for one production connected epoch.
///
/// Only the first variant may initialize static cells. The reconnect variant
/// is assembled exclusively from a completed disconnected epoch, making a
/// second `StaticCell::init` structurally impossible.
enum RadioHilConnectedEpochResources {
    Initial {
        registers: &'static mut RadioRegisters,
        rx: RadioHilJoinRx<'static>,
    },
    Reconnected {
        hardware: ConnectedHardware,
        rx: RadioHilJoinRx<'static>,
        rx_resources: ConnectedRxEpochResources,
        ampdu: Pin<
            &'static mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
        >,
        control_resources: &'static ControlResources,
    },
}

/// Resources returned after one connected HIL epoch is completely quiesced.
///
/// `embassy-net` itself remains alive with link-down; this owner carries only
/// the driver side needed by a later association. The register cell and
/// descriptor arenas retain stable addresses while their finite PAC/DMA
/// capabilities are no longer borrowed by spawned tasks.
struct RadioHilDisconnectedEpoch {
    network: RadioHilRunningNetwork,
    hardware: ConnectedHardware,
    rx: ConnectedStoppedRx,
    ampdu: Pin<
        &'static mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
    >,
    control_resources: &'static ControlResources,
}

/// Board and station state returned after all connected tasks have stopped.
///
/// The unique fixture borrows are returned together with PMK/sequence state;
/// an outer lifecycle can therefore start another finite join attempt instead
/// of parking the hardware merely because the connected runner consumed its
/// input values.
struct RadioHilConnectedEpochReturn<'fixture, 'security> {
    fixture: RadioHilConnectedTaskFixture<'fixture>,
    disconnected: RadioHilDisconnectedEpoch,
    security: StaAssociationSecurity<'security>,
    exit: RadioHilConnectedExit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilConnectedExit {
    Disconnected,
    Stopped,
    HardwareFailure,
}

/// Complete input frontier for the next Association/WPA2 epoch.
struct RadioHilReconnectReady<'fixture, 'security> {
    fixture: RadioHilConnectedTaskFixture<'fixture>,
    target: StaJoinTarget,
    network: RadioHilStaNetwork,
    epoch: RadioHilConnectedEpochResources,
    security: StaAssociationSecurity<'security>,
}

impl RadioHilDisconnectedEpoch {
    fn into_reconnected_resources(
        self,
    ) -> (RadioHilRunningNetwork, RadioHilConnectedEpochResources) {
        assert_join_hardware_capabilities(&self.hardware);
        let (rx, rx_resources) = self.rx.into_epoch_parts();
        (
            self.network,
            RadioHilConnectedEpochResources::Reconnected {
                hardware: self.hardware,
                rx: RadioHilJoinRx::Halted(rx),
                rx_resources,
                ampdu: self.ampdu,
                control_resources: self.control_resources,
            },
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StaConnectedLink {
    station_address: [u8; 6],
    bssid: [u8; 6],
    association_id: u16,
    beacon_interval_tu: u16,
    peer_qos: bool,
    association_phy: StaAssociationPhy,
    peer_supports_one_ltf_800ns_gi: bool,
    peer_supports_ldpc: bool,
    peer_dcm_receive: HeDcmConstellation,
}

struct StaConnectedSession<'rate, 'security> {
    link: StaConnectedLink,
    network: RadioHilStaNetwork,
    rate_control: &'rate mut StaRateControlAssociation,
    pmk: &'security Pmk,
    supplicant_nonce: [u8; 32],
    sequences: &'security mut StaTxSequenceCounters,
}

/// One-time versus persistent `embassy-net` ownership for STA epochs.
enum RadioHilStaNetwork {
    Unstarted {
        device: NetworkDevice,
        runner: NetworkRunner,
    },
    Running(RadioHilRunningNetwork),
}

struct RadioHilRunningNetwork {
    stack: Stack<'static>,
    runner: NetworkRunner,
}

#[derive(Clone, Copy)]
struct StaJoinTarget {
    station_address: [u8; 6],
    access_point: ScanRecord,
}

struct StaAssociationSecurity<'a> {
    pmk: &'a Pmk,
    supplicant_nonce: [u8; 32],
    sequences: &'a mut StaTxSequenceCounters,
}

/// Complete retry ownership returned by a failed Association/WPA2 attempt.
///
/// No field is reconstructed from a static address. The outer STA lifecycle
/// receives the exact board fixture, DMA frontier, persistent network owner
/// and security/sequence state that the failed finite runner stopped using.
struct RadioHilJoinRetry<'fixture, 'security> {
    fixture: RadioHilConnectedFixture<'fixture>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    network: RadioHilStaNetwork,
    security: StaAssociationSecurity<'security>,
}

/// Observable progress plus the resources needed for a bounded retry.
struct RadioHilJoinFailure<'fixture, 'security> {
    retry: RadioHilJoinRetry<'fixture, 'security>,
    associated: bool,
    message1: bool,
    message3: bool,
}

/// Finite result of an initial Association/WPA2/connected attempt.
///
/// Success crosses the one-time static-resource boundary and therefore
/// returns the later reconnect owner as a distinct variant. Failure keeps the
/// earlier join owner intact so the outer lifecycle can apply bounded retry
/// policy without reconstructing any board resource.
enum RadioHilJoinOutcome<'fixture, 'security> {
    Connected {
        ready: RadioHilReconnectReady<'fixture, 'security>,
        exit: RadioHilConnectedExit,
    },
    Failed(RadioHilJoinFailure<'fixture, 'security>),
}

impl<'fixture, 'security> From<RadioHilJoinFailure<'fixture, 'security>>
    for RadioHilJoinOutcome<'fixture, 'security>
{
    fn from(failure: RadioHilJoinFailure<'fixture, 'security>) -> Self {
        Self::Failed(failure)
    }
}

/// The initial join and later reconnect frontiers intentionally remain
/// different Rust types. This enum is only the outer lifecycle's sum type; it
/// does not erase either phase into a mutable vendor-style context.
enum RadioHilStaLifecycleOwner<'fixture, 'security> {
    Authenticate(RadioHilAuthenticationReady<'fixture, 'security>),
    Join(RadioHilJoinRetry<'fixture, 'security>),
    Reconnect(RadioHilReconnectReady<'fixture, 'security>),
}

impl<'fixture, 'security> RadioHilJoinFailure<'fixture, 'security> {
    const fn new(
        retry: RadioHilJoinRetry<'fixture, 'security>,
        associated: bool,
        message1: bool,
        message3: bool,
    ) -> Self {
        Self {
            retry,
            associated,
            message1,
            message3,
        }
    }

    const fn progress(&self) -> (bool, bool, bool) {
        (self.associated, self.message1, self.message3)
    }
}

fn failed_join<'fixture, 'security>(
    fixture: RadioHilConnectedFixture<'fixture>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    network: RadioHilStaNetwork,
    security: StaAssociationSecurity<'security>,
    associated: bool,
) -> RadioHilJoinFailure<'fixture, 'security> {
    RadioHilJoinFailure::new(
        RadioHilJoinRetry {
            fixture,
            target,
            rx,
            network,
            security,
        },
        associated,
        false,
        false,
    )
}

fn failed_join_from_session<'fixture, 'rate, 'security>(
    fixture: RadioHilConnectedFixture<'fixture>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    session: StaConnectedSession<'rate, 'security>,
    message1: bool,
) -> RadioHilJoinFailure<'fixture, 'security> {
    let StaConnectedSession {
        link: _,
        network,
        rate_control: _,
        pmk,
        supplicant_nonce,
        sequences,
    } = session;
    RadioHilJoinFailure::new(
        RadioHilJoinRetry {
            fixture,
            target,
            rx,
            network,
            security: StaAssociationSecurity {
                pmk,
                supplicant_nonce,
                sequences,
            },
        },
        true,
        message1,
        false,
    )
}

/// Board-owned resources required by the connected HIL path.
///
/// This is deliberately a HIL fixture, not a production service locator: all
/// protocol/link state lives in `StaConnectedSession`, and every field here
/// is a concrete hardware or scratch-buffer capability consumed by the same
/// WPA2-to-connected ownership transition.
struct RadioHilConnectedFixture<'a> {
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    platform: &'a mut EspHalRadioPeripheral,
    mmio: &'static mut RadioRegisters,
    interrupt_setup: &'a mut Option<MacInterruptSetup>,
    rx_storage: &'static RxStorage,
    tx_storage: &'static mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    frame: &'static mut [u8],
    ethernet: &'static mut [u8],
}

/// Connected-task resources which remain board-owned across association
/// epochs. Register ownership is carried separately because the first epoch
/// promotes the cold PAC owner into a cooperative cell, while every later
/// epoch must reuse that exact cell.
struct RadioHilConnectedTaskFixture<'a> {
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    platform: &'a mut EspHalRadioPeripheral,
    interrupt_setup: &'a mut Option<MacInterruptSetup>,
    rx_storage: &'static RxStorage,
    tx_storage: &'static mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    frame: &'static mut [u8],
    ethernet: &'static mut [u8],
}

impl<'a> RadioHilConnectedFixture<'a> {
    fn into_task_fixture(
        self,
    ) -> (
        RadioHilConnectedTaskFixture<'a>,
        &'static mut RadioRegisters,
    ) {
        (
            RadioHilConnectedTaskFixture {
                spawner: self.spawner,
                protocol_spawner: self.protocol_spawner,
                platform: self.platform,
                interrupt_setup: self.interrupt_setup,
                rx_storage: self.rx_storage,
                tx_storage: self.tx_storage,
                descriptor_base: self.descriptor_base,
                buffer_addresses: self.buffer_addresses,
                frame: self.frame,
                ethernet: self.ethernet,
            },
            self.mmio,
        )
    }
}

/// Join-time extension of the board fixture with the PHY channel owner.
///
/// Authentication needs to switch the PHY channel; after that transition the
/// same concrete radio resources continue through Association, WPA2 and the
/// connected runner without being repeated as positional arguments.
struct RadioHilJoinFixture<'a> {
    state: &'a mut PhyColdState,
    radio: RadioHilConnectedFixture<'a>,
}

/// Complete same-candidate frontier before Open Authentication.
///
/// The PHY channel owner is present only in this phase. Successful
/// authentication consumes it into the connected fixture; a failed finite
/// authentication returns the complete value so the outer lifecycle can wait
/// and retry without recreating DMA or security state.
struct RadioHilAuthenticationReady<'fixture, 'security> {
    fixture: RadioHilJoinFixture<'fixture>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    network: RadioHilStaNetwork,
    security: StaAssociationSecurity<'security>,
}

impl<'a> RadioHilJoinFixture<'a> {
    fn into_connected(self) -> RadioHilConnectedFixture<'a> {
        self.radio
    }
}
// The embassy-net RX slots and index queues are CPU-owned and are never
// presented to the Wi-Fi DMA engine. In the qualified
// `psram-code-psram-data` runtime ordinary `.bss` already lives in PSRAM; an
// explicit `.psram.bss` input section would bypass the runtime payload layout
// and overlap `.runtime.payload_end`.
//
// Standalone flash-XIP A-MSDU HIL previously left 33,160 bytes between
// `_stack_end` and `_stack_start`. The WPA2 path crossed that frontier and
// overwrote `SharedLinkState`; its next `set_link_state` failed with a
// misaligned waker load at 0x400679e2. The benchmark-specific queue depth above
// removes that false memory pressure, while production stays at depth 32.
static OPEN_RADIO_NETWORK_RESOURCES: StaticCell<NetworkResources> = StaticCell::new();
static OPEN_RADIO_CONTROL_RESOURCES: StaticCell<ControlResources> = StaticCell::new();
// Only allocations actually addressed by Wi-Fi DMA are forced into SRAM.
// Embassy RX slots, channels and link state remain ordinary PSRAM data.
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_network_tx")]
static OPEN_RADIO_NETWORK_TX_POOL: StaticCell<NetworkTxPool> = StaticCell::new();
// Keep one slot for each concurrently live socket: embassy-net DNS, DHCP,
// the selected UDP benchmark, and the post-DHCP external-network probe.
// The probe intentionally remains alive after it primes the neighbor cache,
// so its slot cannot be shared with the benchmark socket.
static OPEN_RADIO_STACK_RESOURCES: StaticCell<StackResources<OPEN_RADIO_STACK_SOCKET_COUNT>> =
    StaticCell::new();
static OPEN_RADIO_UDP_RX_METADATA: StaticCell<[PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]> =
    StaticCell::new();
static OPEN_RADIO_UDP_RX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();
static OPEN_RADIO_UDP_TX_METADATA: StaticCell<[PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]> =
    StaticCell::new();
static OPEN_RADIO_UDP_TX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();
static OPEN_RADIO_UDP_PACKET: StaticCell<[u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]> = StaticCell::new();
static OPEN_RADIO_TCP_RX_BUFFER: StaticCell<[u8; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY]> =
    StaticCell::new();
static OPEN_RADIO_TCP_TX_BUFFER: StaticCell<[u8; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY]> =
    StaticCell::new();
static OPEN_RADIO_TCP_READ_BUFFER: StaticCell<[u8; OPEN_RADIO_TCP_READ_CAPACITY]> =
    StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_RX_METADATA: StaticCell<
    [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH],
> = StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_RX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_TX_METADATA: StaticCell<
    [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH],
> = StaticCell::new();
static OPEN_RADIO_BIDIRECTIONAL_TX_BUFFER: StaticCell<
    [u8; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
> = StaticCell::new();
#[unsafe(link_section = ".critical.data.open_radio_bidirectional_session")]
static OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS: Channel<
    CriticalSectionRawMutex,
    crate::console::ActiveSession,
    1,
> = Channel::new();
#[unsafe(link_section = ".critical.data.open_radio_bidirectional_session")]
static OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS: Channel<
    CriticalSectionRawMutex,
    crate::console::ActiveSession,
    1,
> = Channel::new();
#[unsafe(link_section = ".critical.data.open_radio_bidirectional_session")]
static OPEN_RADIO_BIDIRECTIONAL_RESULTS: Channel<
    CriticalSectionRawMutex,
    OpenRadioBidirectionalResult,
    2,
> = Channel::new();
static OPEN_RADIO_LOCAL_IPV4: AtomicU32 = AtomicU32::new(0);
static OPEN_RADIO_LAN_PROBE_RESPONSE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Eq, PartialEq)]
enum OpenRadioBidirectionalDirection {
    Rx,
    Tx,
}

#[derive(Clone, Copy)]
struct OpenRadioBidirectionalResult {
    session_id: u64,
    direction: OpenRadioBidirectionalDirection,
    evidence: TransportEvidence,
    passed: bool,
}

#[derive(Clone, Copy, Default)]
struct OpenRadioTaskPollSnapshot {
    polls: u32,
    poll_micros: u32,
    lifetime_max_micros: u32,
    over_100_micros: u32,
    over_500_micros: u32,
    over_1_000_micros: u32,
    over_5_000_micros: u32,
}

impl OpenRadioTaskPollSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            polls: self.polls.wrapping_sub(earlier.polls),
            poll_micros: self.poll_micros.wrapping_sub(earlier.poll_micros),
            // A maximum is a lifetime observation, not an additive interval
            // counter. Its name in the UART record makes that distinction
            // explicit.
            lifetime_max_micros: self.lifetime_max_micros,
            over_100_micros: self.over_100_micros.wrapping_sub(earlier.over_100_micros),
            over_500_micros: self.over_500_micros.wrapping_sub(earlier.over_500_micros),
            over_1_000_micros: self
                .over_1_000_micros
                .wrapping_sub(earlier.over_1_000_micros),
            over_5_000_micros: self
                .over_5_000_micros
                .wrapping_sub(earlier.over_5_000_micros),
        }
    }
}

struct OpenRadioTaskPollCounters {
    polls: AtomicU32,
    poll_micros: AtomicU32,
    lifetime_max_micros: AtomicU32,
    over_100_micros: AtomicU32,
    over_500_micros: AtomicU32,
    over_1_000_micros: AtomicU32,
    over_5_000_micros: AtomicU32,
}

impl OpenRadioTaskPollCounters {
    const fn new() -> Self {
        Self {
            polls: AtomicU32::new(0),
            poll_micros: AtomicU32::new(0),
            lifetime_max_micros: AtomicU32::new(0),
            over_100_micros: AtomicU32::new(0),
            over_500_micros: AtomicU32::new(0),
            over_1_000_micros: AtomicU32::new(0),
            over_5_000_micros: AtomicU32::new(0),
        }
    }

    #[inline]
    fn record(&self, elapsed_micros: u64) {
        let elapsed_micros = elapsed_micros.min(u32::MAX.into()) as u32;
        self.polls.fetch_add(1, Ordering::Relaxed);
        self.poll_micros
            .fetch_add(elapsed_micros, Ordering::Relaxed);
        update_atomic_max(&self.lifetime_max_micros, elapsed_micros);
        if elapsed_micros > 100 {
            self.over_100_micros.fetch_add(1, Ordering::Relaxed);
        }
        if elapsed_micros > 500 {
            self.over_500_micros.fetch_add(1, Ordering::Relaxed);
        }
        if elapsed_micros > 1_000 {
            self.over_1_000_micros.fetch_add(1, Ordering::Relaxed);
        }
        if elapsed_micros > 5_000 {
            self.over_5_000_micros.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> OpenRadioTaskPollSnapshot {
        OpenRadioTaskPollSnapshot {
            polls: self.polls.load(Ordering::Relaxed),
            poll_micros: self.poll_micros.load(Ordering::Relaxed),
            lifetime_max_micros: self.lifetime_max_micros.load(Ordering::Relaxed),
            over_100_micros: self.over_100_micros.load(Ordering::Relaxed),
            over_500_micros: self.over_500_micros.load(Ordering::Relaxed),
            over_1_000_micros: self.over_1_000_micros.load(Ordering::Relaxed),
            over_5_000_micros: self.over_5_000_micros.load(Ordering::Relaxed),
        }
    }
}

#[inline]
fn update_atomic_max(maximum: &AtomicU32, value: u32) {
    let mut observed = maximum.load(Ordering::Relaxed);
    while value > observed {
        match maximum.compare_exchange_weak(
            observed,
            value,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(current) => observed = current,
        }
    }
}

#[derive(Clone, Copy, Default)]
struct OpenRadioTaskPollSetSnapshot {
    network: OpenRadioTaskPollSnapshot,
    protocol: OpenRadioTaskPollSnapshot,
    radio: OpenRadioTaskPollSnapshot,
    benchmark: OpenRadioTaskPollSnapshot,
}

struct OpenRadioTaskPollSet {
    network: OpenRadioTaskPollCounters,
    protocol: OpenRadioTaskPollCounters,
    radio: OpenRadioTaskPollCounters,
    benchmark: OpenRadioTaskPollCounters,
}

impl OpenRadioTaskPollSet {
    const fn new() -> Self {
        Self {
            network: OpenRadioTaskPollCounters::new(),
            protocol: OpenRadioTaskPollCounters::new(),
            radio: OpenRadioTaskPollCounters::new(),
            benchmark: OpenRadioTaskPollCounters::new(),
        }
    }

    fn snapshot(&self) -> OpenRadioTaskPollSetSnapshot {
        OpenRadioTaskPollSetSnapshot {
            network: self.network.snapshot(),
            protocol: self.protocol.snapshot(),
            radio: self.radio.snapshot(),
            benchmark: self.benchmark.snapshot(),
        }
    }
}

// Poll telemetry is HIL-only and deliberately sits in internal SRAM. Reading
// these counters once per completed traffic interval must not add PSRAM
// traffic to the executor hot path being diagnosed.
#[unsafe(link_section = ".critical.bss.open_radio_task_poll_telemetry")]
static OPEN_RADIO_TASK_POLLS: OpenRadioTaskPollSet = OpenRadioTaskPollSet::new();

const OPEN_RADIO_MAC_TX_IRQ_MASK: u32 =
    MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT | MAC_INT_COLLISION;

#[derive(Clone, Copy, Default)]
struct OpenRadioMacIrqClassificationSnapshot {
    spurious_entries: u32,
    rx_only_entries: u32,
    rx_mixed_entries: u32,
    tx_only_entries: u32,
    tx_mixed_entries: u32,
    other_only_entries: u32,
    extra_nonzero_snapshots: u32,
    saturated_entries: u32,
}

impl OpenRadioMacIrqClassificationSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            spurious_entries: self
                .spurious_entries
                .wrapping_sub(earlier.spurious_entries),
            rx_only_entries: self
                .rx_only_entries
                .wrapping_sub(earlier.rx_only_entries),
            rx_mixed_entries: self
                .rx_mixed_entries
                .wrapping_sub(earlier.rx_mixed_entries),
            tx_only_entries: self
                .tx_only_entries
                .wrapping_sub(earlier.tx_only_entries),
            tx_mixed_entries: self
                .tx_mixed_entries
                .wrapping_sub(earlier.tx_mixed_entries),
            other_only_entries: self
                .other_only_entries
                .wrapping_sub(earlier.other_only_entries),
            extra_nonzero_snapshots: self
                .extra_nonzero_snapshots
                .wrapping_sub(earlier.extra_nonzero_snapshots),
            saturated_entries: self
                .saturated_entries
                .wrapping_sub(earlier.saturated_entries),
        }
    }
}

struct OpenRadioMacIrqClassificationCounters {
    spurious_entries: AtomicU32,
    rx_only_entries: AtomicU32,
    rx_mixed_entries: AtomicU32,
    tx_only_entries: AtomicU32,
    tx_mixed_entries: AtomicU32,
    other_only_entries: AtomicU32,
    extra_nonzero_snapshots: AtomicU32,
    saturated_entries: AtomicU32,
    auxiliary_status_or: AtomicU32,
    unknown_status_or: AtomicU32,
}

impl OpenRadioMacIrqClassificationCounters {
    const fn new() -> Self {
        Self {
            spurious_entries: AtomicU32::new(0),
            rx_only_entries: AtomicU32::new(0),
            rx_mixed_entries: AtomicU32::new(0),
            tx_only_entries: AtomicU32::new(0),
            tx_mixed_entries: AtomicU32::new(0),
            other_only_entries: AtomicU32::new(0),
            extra_nonzero_snapshots: AtomicU32::new(0),
            saturated_entries: AtomicU32::new(0),
            auxiliary_status_or: AtomicU32::new(0),
            unknown_status_or: AtomicU32::new(0),
        }
    }

    #[inline]
    fn record(&self, first_status: u32, observed_status: u32, nonzero_snapshots: u32) {
        let rx = first_status & MAC_INT_RX_SUCCESS != 0;
        let tx = first_status & OPEN_RADIO_MAC_TX_IRQ_MASK != 0;
        let auxiliary = first_status & MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK != 0;
        let unknown = first_status
            & !(MAC_INT_RX_SUCCESS
                | OPEN_RADIO_MAC_TX_IRQ_MASK
                | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK)
            != 0;
        let counter = if first_status == 0 {
            &self.spurious_entries
        } else if rx && !tx && !unknown {
            // The two acknowledged auxiliary bits do not make this a mixed
            // work entry because they have no independent dispatcher action.
            &self.rx_only_entries
        } else if rx {
            &self.rx_mixed_entries
        } else if tx && !auxiliary && !unknown {
            &self.tx_only_entries
        } else if tx {
            &self.tx_mixed_entries
        } else {
            &self.other_only_entries
        };
        counter.fetch_add(1, Ordering::Relaxed);

        let extra = nonzero_snapshots.saturating_sub(1);
        if extra != 0 {
            self.extra_nonzero_snapshots
                .fetch_add(extra, Ordering::Relaxed);
        }
        if nonzero_snapshots == 32 {
            self.saturated_entries.fetch_add(1, Ordering::Relaxed);
        }
        let auxiliary_status = observed_status & MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK;
        if auxiliary_status != 0 {
            self.auxiliary_status_or
                .fetch_or(auxiliary_status, Ordering::Relaxed);
        }
        let unknown_status = observed_status
            & !(MAC_INT_RX_SUCCESS
                | OPEN_RADIO_MAC_TX_IRQ_MASK
                | MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK);
        if unknown_status != 0 {
            self.unknown_status_or
                .fetch_or(unknown_status, Ordering::Relaxed);
        }
    }

    fn snapshot(&self) -> OpenRadioMacIrqClassificationSnapshot {
        OpenRadioMacIrqClassificationSnapshot {
            spurious_entries: self.spurious_entries.load(Ordering::Relaxed),
            rx_only_entries: self.rx_only_entries.load(Ordering::Relaxed),
            rx_mixed_entries: self.rx_mixed_entries.load(Ordering::Relaxed),
            tx_only_entries: self.tx_only_entries.load(Ordering::Relaxed),
            tx_mixed_entries: self.tx_mixed_entries.load(Ordering::Relaxed),
            other_only_entries: self.other_only_entries.load(Ordering::Relaxed),
            extra_nonzero_snapshots: self.extra_nonzero_snapshots.load(Ordering::Relaxed),
            saturated_entries: self.saturated_entries.load(Ordering::Relaxed),
        }
    }

    fn take_auxiliary_status_or(&self) -> u32 {
        self.auxiliary_status_or.swap(0, Ordering::Relaxed)
    }

    fn take_unknown_status_or(&self) -> u32 {
        self.unknown_status_or.swap(0, Ordering::Relaxed)
    }
}

// These counters are touched for every admitted RX frame or IRQ-side reload
// edge. Keep the diagnostic atomics off PSRAM so HIL observation does not
// become part of the throughput limit it is trying to measure.
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_ENQUEUE_COUNTERS: RxEnqueueCounters = RxEnqueueCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_tx_telemetry")]
static OPEN_RADIO_TX_AGGREGATE_COUNTERS: AggregateTxCounters = AggregateTxCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_RELOAD_DELAYS: AtomicU32 = AtomicU32::new(0);
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_MAC_IRQ_ENTRIES: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_MAC_IRQ_CLASSIFICATION: OpenRadioMacIrqClassificationCounters =
    OpenRadioMacIrqClassificationCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_LAST_UDP_FORMAT: AtomicU32 = AtomicU32::new(u32::MAX);
// Packed last-data-PPDU observation, written once per benchmark UDP frame and
// decoded only after the measured interval. Bits 0..=3 are the BB format,
// 4..=8 the public RX rate, 9..=12 HE-SU MCS, 13..=14 GI/LTF, 15..=16 BW,
// 17 DCM, 18 LDPC, and 31 marks a decoded HE-SU signal.
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_LAST_UDP_PHY: AtomicU32 = AtomicU32::new(u32::MAX);
const OPEN_RADIO_RX_HE_MCS_BUCKETS: usize = 12;

struct OpenRadioRxPhyCounters {
    he_mcs: [AtomicU32; OPEN_RADIO_RX_HE_MCS_BUCKETS],
    other: AtomicU32,
}

impl OpenRadioRxPhyCounters {
    const fn new() -> Self {
        Self {
            he_mcs: [const { AtomicU32::new(0) }; OPEN_RADIO_RX_HE_MCS_BUCKETS],
            other: AtomicU32::new(0),
        }
    }

    fn snapshot(&self) -> ([u32; OPEN_RADIO_RX_HE_MCS_BUCKETS], u32) {
        (
            core::array::from_fn(|index| self.he_mcs[index].load(Ordering::Relaxed)),
            self.other.load(Ordering::Relaxed),
        )
    }
}

#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_PHY_COUNTERS: OpenRadioRxPhyCounters = OpenRadioRxPhyCounters::new();

#[derive(Clone, Copy, Default)]
struct OpenRadioRxOrderSnapshot {
    gap_events: u32,
    forward_missing: u32,
    backward: u32,
    adjacent_duplicates: u32,
    backward_mac_backward: u32,
    backward_mac_same: u32,
    backward_mac_forward: u32,
    backward_mac_other_tid: u32,
    backward_mac_unavailable: u32,
}

impl OpenRadioRxOrderSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            gap_events: self.gap_events.wrapping_sub(earlier.gap_events),
            forward_missing: self.forward_missing.wrapping_sub(earlier.forward_missing),
            backward: self.backward.wrapping_sub(earlier.backward),
            adjacent_duplicates: self
                .adjacent_duplicates
                .wrapping_sub(earlier.adjacent_duplicates),
            backward_mac_backward: self
                .backward_mac_backward
                .wrapping_sub(earlier.backward_mac_backward),
            backward_mac_same: self
                .backward_mac_same
                .wrapping_sub(earlier.backward_mac_same),
            backward_mac_forward: self
                .backward_mac_forward
                .wrapping_sub(earlier.backward_mac_forward),
            backward_mac_other_tid: self
                .backward_mac_other_tid
                .wrapping_sub(earlier.backward_mac_other_tid),
            backward_mac_unavailable: self
                .backward_mac_unavailable
                .wrapping_sub(earlier.backward_mac_unavailable),
        }
    }
}

struct OpenRadioRxOrderCounters {
    gap_events: AtomicU32,
    forward_missing: AtomicU32,
    backward: AtomicU32,
    adjacent_duplicates: AtomicU32,
    backward_mac_backward: AtomicU32,
    backward_mac_same: AtomicU32,
    backward_mac_forward: AtomicU32,
    backward_mac_other_tid: AtomicU32,
    backward_mac_unavailable: AtomicU32,
}

impl OpenRadioRxOrderCounters {
    const fn new() -> Self {
        Self {
            gap_events: AtomicU32::new(0),
            forward_missing: AtomicU32::new(0),
            backward: AtomicU32::new(0),
            adjacent_duplicates: AtomicU32::new(0),
            backward_mac_backward: AtomicU32::new(0),
            backward_mac_same: AtomicU32::new(0),
            backward_mac_forward: AtomicU32::new(0),
            backward_mac_other_tid: AtomicU32::new(0),
            backward_mac_unavailable: AtomicU32::new(0),
        }
    }

    fn snapshot(&self) -> OpenRadioRxOrderSnapshot {
        OpenRadioRxOrderSnapshot {
            gap_events: self.gap_events.load(Ordering::Relaxed),
            forward_missing: self.forward_missing.load(Ordering::Relaxed),
            backward: self.backward.load(Ordering::Relaxed),
            adjacent_duplicates: self.adjacent_duplicates.load(Ordering::Relaxed),
            backward_mac_backward: self.backward_mac_backward.load(Ordering::Relaxed),
            backward_mac_same: self.backward_mac_same.load(Ordering::Relaxed),
            backward_mac_forward: self.backward_mac_forward.load(Ordering::Relaxed),
            backward_mac_other_tid: self.backward_mac_other_tid.load(Ordering::Relaxed),
            backward_mac_unavailable: self.backward_mac_unavailable.load(Ordering::Relaxed),
        }
    }
}

#[unsafe(link_section = ".critical.bss.open_radio_rx_order_telemetry")]
static OPEN_RADIO_RX_ORDER_COUNTERS: OpenRadioRxOrderCounters =
    OpenRadioRxOrderCounters::new();
// Unlike the zero-initialized counters above, this object owns a nonzero
// platform clock function pointer and therefore must be copied as initialized
// critical data. Placing it in NOLOAD `.critical.bss` would erase the callback
// and trap on the first connected RX observation.
#[unsafe(link_section = ".critical.data.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_PIPELINE_COUNTERS: RxPipelineCounters =
    RxPipelineCounters::new(open_radio_rx_telemetry_now_micros);
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_IRQ_RUNTIME: EmbassyMacIrqRuntime<CriticalSectionRawMutex> =
    EmbassyMacIrqRuntime::new();
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_POWER_IRQ_RUNTIME: EmbassyPowerIrqRuntime<CriticalSectionRawMutex> =
    EmbassyPowerIrqRuntime::new();
// One connected-epoch cancellation edge shared only by the production radio
// and staged-protocol tasks. This is HIL executor composition, not driver
// state; the reusable protocol owner accepts any caller-supplied stop future.
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_CONNECTED_PROTOCOL_STOP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_CONNECTED_PROTOCOL_STOPPED: Signal<
    CriticalSectionRawMutex,
    ConnectedRxProtocolStopped<'static>,
> = Signal::new();
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_CONNECTED_BENCHMARK_STOP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_CONNECTED_BENCHMARK_STOPPED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static OPEN_RADIO_MAC_INTERRUPT_REGISTERS: StaticCell<MacInterruptRegisters> = StaticCell::new();
// Storage addresses remain stable across connected epochs. ACTIVE_PTR is
// cleared before task-side code moves the values out; STORAGE_PTR retains the
// exact location that must be reinitialized before the next ISR route opens.
static OPEN_RADIO_MAC_INTERRUPT_STORAGE_PTR: AtomicPtr<MacInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());
static OPEN_RADIO_MAC_INTERRUPT_PTR: AtomicPtr<MacInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());
static OPEN_RADIO_POWER_INTERRUPT_REGISTERS: StaticCell<MacPowerInterruptRegisters> =
    StaticCell::new();
static OPEN_RADIO_POWER_INTERRUPT_STORAGE_PTR: AtomicPtr<MacPowerInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());
static OPEN_RADIO_POWER_INTERRUPT_PTR: AtomicPtr<MacPowerInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());

/// RX descriptor authority carried through pre-connected STA phases.
///
/// `Initial` is the one cold handoff from scan. Every later start must consume
/// a hardware-confirmed `Halted` owner or retry an already published
/// `Prepared` owner. `Vacant` exists only while a transition has moved the
/// non-Copy authority out of this enum; it is never an externally observable
/// station state.
enum RadioHilJoinRx<'storage> {
    Initial,
    Halted(RxRingHalted<'storage, RX_DESCRIPTOR_COUNT>),
    Prepared(RxRingStopped<'storage, RX_DESCRIPTOR_COUNT>),
    Live(RxRingLive<'storage, RX_DESCRIPTOR_COUNT>),
    Vacant,
}

impl<'storage> RadioHilJoinRx<'storage> {
    async fn start<M: RxDma>(
        &mut self,
        mmio: &mut M,
        rx_storage: &'storage RxStorage,
        descriptor_base: u32,
        buffer_addresses: &'storage [u32; RX_DESCRIPTOR_COUNT],
    ) -> Result<(), RadioHilStaJoinError> {
        let state = core::mem::replace(self, Self::Vacant);
        let prepared = match state {
            Self::Initial => match RxRingStopped::prepare(
                mmio,
                rx_storage.descriptors(),
                descriptor_base,
                buffer_addresses,
                RX_BUFFER_SIZE as u32,
                |index| {
                    // SAFETY: the prepare transaction first confirms that the
                    // walker is stopped, then transfers this buffer index to
                    // its caller before the recycle guards are restored.
                    unsafe { rx_storage.buffers()[index].prepare_for_recycle() }
                },
            ) {
                Ok(prepared) => prepared,
                Err(error) => {
                    *self = Self::Initial;
                    return Err(error.into());
                }
            },
            Self::Halted(halted) => match halted.prepare(
                mmio,
                RX_BUFFER_SIZE as u32,
                |index| {
                    // SAFETY: `RxRingHalted` proves that DMA released the
                    // matching buffer before invoking this closure.
                    unsafe { rx_storage.buffers()[index].prepare_for_recycle() }
                },
            ) {
                Ok(prepared) => prepared,
                Err((halted, error)) => {
                    *self = Self::Halted(halted);
                    return Err(error.into());
                }
            },
            Self::Prepared(prepared) => prepared,
            live @ Self::Live(_) => {
                *self = live;
                return Err(RadioHilStaJoinError::ReceiveAlreadyStarted);
            }
            Self::Vacant => {
                *self = Self::Vacant;
                return Err(RadioHilStaJoinError::ReceiveNotStarted);
            }
        };
        Timer::after_micros(5).await;
        match prepared.try_start(mmio) {
            Ok(live) => {
                *self = Self::Live(live);
                Ok(())
            }
            Err((prepared, error)) => {
                *self = Self::Prepared(prepared);
                Err(error.into())
            }
        }
    }

    fn stop<M: RxDma>(&mut self, mmio: &mut M) -> Result<(), RadioHilStaJoinError> {
        let state = core::mem::replace(self, Self::Vacant);
        let Self::Live(live) = state else {
            *self = state;
            return Err(RadioHilStaJoinError::ReceiveNotStarted);
        };
        match live.try_stop(mmio) {
            Ok(halted) => {
                *self = Self::Halted(halted);
                Ok(())
            }
            Err((live, error)) => {
                *self = Self::Live(live);
                Err(error.into())
            }
        }
    }

    fn live_mut(
        &mut self,
    ) -> Result<&mut RxRingLive<'storage, RX_DESCRIPTOR_COUNT>, RadioHilStaJoinError> {
        match self {
            Self::Live(ring) => Ok(ring),
            _ => Err(RadioHilStaJoinError::ReceiveNotStarted),
        }
    }

    fn take_live(
        &mut self,
    ) -> Result<RxRingLive<'storage, RX_DESCRIPTOR_COUNT>, RadioHilStaJoinError> {
        let state = core::mem::replace(self, Self::Vacant);
        match state {
            Self::Live(ring) => Ok(ring),
            state => {
                *self = state;
                Err(RadioHilStaJoinError::ReceiveNotStarted)
            }
        }
    }
}

/// HIL fixture which supplies the production STA join runner with the current
/// S31 PAC/DMA owners. Protocol retry/deadline state lives in `StaJoinRunner`;
/// this adapter performs only finite hardware operations and frame extraction.
struct RadioHilStaJoinBackend<'hardware, 'storage, 'scratch, H> {
    mmio: &'hardware mut H,
    rx_storage: &'storage RxStorage,
    tx_storage: &'hardware mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'storage [u32; RX_DESCRIPTOR_COUNT],
    frame: &'scratch mut [u8],
    station_address: [u8; 6],
    access_point: ScanRecord,
    rx: RadioHilJoinRx<'storage>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilStaJoinError {
    ReceiveAlreadyStarted,
    ReceiveNotStarted,
    Rx(RxRingError),
    Tx(ActiveScanTxError),
}

impl From<RxRingError> for RadioHilStaJoinError {
    fn from(error: RxRingError) -> Self {
        Self::Rx(error)
    }
}

impl From<ActiveScanTxError> for RadioHilStaJoinError {
    fn from(error: ActiveScanTxError) -> Self {
        Self::Tx(error)
    }
}

impl<'hardware, 'storage, 'scratch, H>
    RadioHilStaJoinBackend<'hardware, 'storage, 'scratch, H>
{
    fn into_rx(self) -> RadioHilJoinRx<'storage> {
        self.rx
    }
}

impl<H> StaJoinBackend for RadioHilStaJoinBackend<'_, '_, '_, H>
where
    H: Mmio + RxDma + TxHardware,
{
    type Error = RadioHilStaJoinError;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            self.rx
                .start(
                    self.mmio,
                    self.rx_storage,
                    self.descriptor_base,
                    self.buffer_addresses,
                )
                .await
        }
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move { self.rx.stop(self.mmio) }
    }

    fn transmit_open_authentication(
        &mut self,
        attempt: StaAuthenticationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            transmit_open_authentication(
                self.mmio,
                self.tx_storage,
                self.station_address,
                self.access_point.bssid,
                attempt.sequence_number,
            )
            .await?;
            Ok(())
        }
    }

    fn transmit_association(
        &mut self,
        attempt: StaAssociationAttempt,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            transmit_association_request(
                self.mmio,
                self.tx_storage,
                self.station_address,
                &self.access_point,
                attempt.sequence_number,
            )
            .await?;
            Ok(())
        }
    }

    fn service_receive<'a, O>(
        &'a mut self,
        observer: &'a mut O,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a
    where
        O: StaJoinRxObserver + 'a,
    {
        async move {
            let ring = self.rx.live_mut()?;
            for index in 0..RX_DESCRIPTOR_COUNT {
                let Some(completed) = ring.take_completed(index) else {
                    continue;
                };
                let segment = RxSegment {
                    descriptor_address: completed.descriptor_address(),
                    descriptor_word0: completed.word0(),
                    buffer: unsafe {
                        // The live ring transferred the completed descriptor
                        // and matching buffer to this unique backend.
                        self.rx_storage.buffers()[index].completed()
                    },
                    next_descriptor_address: completed.next_descriptor_address(),
                };
                let management = extract_management(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    self.frame,
                )
                .ok();
                let management = management.map(|frame| &self.frame[..frame.length]);
                if observer.observe_completed(management) == StaJoinRxDirective::Stop {
                    return Ok(());
                }
            }

            ring.recycle_completed_half(self.mmio, |index| {
                // The live ring invokes this only for a detached completed
                // half immediately before republishing it to DMA.
                unsafe { self.rx_storage.buffers()[index].prepare_for_recycle() }
            })?;
            if ring.all_observed() {
                return Err(RadioHilStaJoinError::Rx(RxRingError::Corrupt));
            }
            Ok(())
        }
    }
}

/// HIL PAC/DMA fixture for the production WPA2 response runner. The adapter
/// copies one complete EAPOL packet before returning and never awaits while a
/// descriptor or mutable PAC transaction is borrowed.
struct RadioHilWpa2Backend<'hardware, 'storage, 'scratch, H> {
    mmio: &'hardware mut H,
    rx_storage: &'storage RxStorage,
    tx_storage: &'hardware mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'storage [u32; RX_DESCRIPTOR_COUNT],
    frame: &'scratch mut [u8],
    station_address: [u8; 6],
    bssid: [u8; 6],
    rx: RadioHilJoinRx<'storage>,
    message2_transmissions: u16,
}

impl<'storage, H> RadioHilWpa2Backend<'_, 'storage, '_, H> {
    fn into_rx(self) -> RadioHilJoinRx<'storage> {
        self.rx
    }
}

impl<H> Wpa2HandshakeBackend for RadioHilWpa2Backend<'_, '_, '_, H>
where
    H: Mmio + RxDma + TxHardware,
{
    type Error = RadioHilStaJoinError;

    fn service_receive(
        &mut self,
    ) -> impl Future<Output = Result<Wpa2RxProgress, Self::Error>> + '_ {
        async move {
            let ring = self.rx.live_mut()?;
            let mut completed_frames = 0_u32;
            for index in 0..RX_DESCRIPTOR_COUNT {
                let Some(completed) = ring.take_completed(index) else {
                    continue;
                };
                completed_frames = completed_frames.saturating_add(1);
                let segment = RxSegment {
                    descriptor_address: completed.descriptor_address(),
                    descriptor_word0: completed.word0(),
                    buffer: unsafe {
                        // The live ring transferred this completed descriptor
                        // and matching buffer to the unique WPA2 backend.
                        self.rx_storage.buffers()[index].completed()
                    },
                    next_descriptor_address: completed.next_descriptor_address(),
                };
                let Ok(data) = extract_data(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    self.frame,
                ) else {
                    continue;
                };
                if data.mpdu.length < 24
                    || self.frame[4..10] != self.station_address
                    || self.frame[10..16] != self.bssid
                {
                    continue;
                }
                let Some(eapol_offset) = data.payload_offset.checked_add(LLC_SNAP_EAPOL.len())
                else {
                    continue;
                };
                if self.frame.get(data.payload_offset..eapol_offset) != Some(&LLC_SNAP_EAPOL) {
                    continue;
                }
                let Some(eapol) = self.frame.get(eapol_offset..data.mpdu.length) else {
                    continue;
                };
                let Ok(owned) =
                    OwnedEapolFrame::try_copy(Wpa2Interface::Station, self.bssid, eapol)
                else {
                    continue;
                };
                return Ok(Wpa2RxProgress::eapol(completed_frames, owned));
            }

            ring.recycle_completed_half(self.mmio, |index| {
                // The ring invokes this only for a detached completed half
                // immediately before republishing it to hardware.
                unsafe { self.rx_storage.buffers()[index].prepare_for_recycle() }
            })?;
            if ring.all_observed() {
                return Err(RadioHilStaJoinError::Rx(RxRingError::Corrupt));
            }
            Ok(Wpa2RxProgress::drained(completed_frames))
        }
    }

    fn restart_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            self.rx.stop(self.mmio)?;
            self.rx
                .start(
                    self.mmio,
                    self.rx_storage,
                    self.descriptor_base,
                    self.buffer_addresses,
                )
                .await
        }
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move { self.rx.stop(self.mmio) }
    }

    fn transmit_message2<'a>(
        &'a mut self,
        frame: &'a Wpa2TxFrame<512>,
        sequence_number: u16,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            transmit_unprotected_eapol(
                self.mmio,
                self.tx_storage,
                self.station_address,
                self.bssid,
                frame.as_bytes(),
                sequence_number,
            )
            .await?;
            self.message2_transmissions = self.message2_transmissions.saturating_add(1);
            Ok(())
        }
    }
}

struct RadioHilInstalledWpa2Keys {
    pairwise: StaPairwiseCcmpSlot,
    group: StaGroupCcmpSlot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilWpa2KeyError {
    InvalidRequest,
    Install(CryptoKeyError),
    Transmit(ActiveScanTxError),
    TxStatus(u8),
}

struct RadioHilWpa2KeyBackend<'hardware, 'sequence, H> {
    mmio: &'hardware mut H,
    tx_storage: &'hardware mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    peer_qos: bool,
    sequences: &'sequence mut StaTxSequenceCounters,
    completion: Option<TxCompletion>,
}

impl<H> Wpa2KeyInstallBackend for RadioHilWpa2KeyBackend<'_, '_, H>
where
    H: Mmio + CcmpKeyHardware + TxHardware,
{
    type Error = RadioHilWpa2KeyError;
    type InstalledKeys = RadioHilInstalledWpa2Keys;

    fn install_keys(
        &mut self,
        request: &open_esp_radio::wifi::wpa2::supplicant::Wpa2StaKeyInstallRequest,
    ) -> Result<Self::InstalledKeys, Self::Error> {
        let pairwise = request.pairwise();
        let group = request.group();
        let Wpa2KeyKind::Group { key_id, .. } = group.kind() else {
            return Err(RadioHilWpa2KeyError::InvalidRequest);
        };
        let pairwise =
            install_sta_pairwise_ccmp(self.mmio, *pairwise.peer(), pairwise.key().as_bytes())
                .map_err(RadioHilWpa2KeyError::Install)?;
        let group = match install_sta_group_ccmp(self.mmio, key_id, group.key().as_bytes()) {
            Ok(group) => group,
            Err(error) => {
                pairwise.clear(self.mmio);
                return Err(RadioHilWpa2KeyError::Install(error));
            }
        };
        Ok(RadioHilInstalledWpa2Keys { pairwise, group })
    }

    fn rollback_keys(&mut self, keys: Self::InstalledKeys) -> Result<(), Self::Error> {
        keys.group.clear(self.mmio);
        keys.pairwise.clear(self.mmio);
        Ok(())
    }

    fn transmit_message4<'a>(
        &'a mut self,
        frame: &'a Wpa2TxFrame<512>,
        keys: &'a mut Self::InstalledKeys,
    ) -> impl Future<Output = Result<(), Self::Error>> + 'a {
        async move {
            let completion = if WPA2_MESSAGE_4_HARDWARE_PROTECTED {
                transmit_eapol_message_4(
                    self.mmio,
                    self.tx_storage,
                    self.station_address,
                    self.bssid,
                    frame,
                    &mut keys.pairwise,
                    self.sequences
                        .take_data(self.peer_qos.then_some(0))
                        .expect("selected EAPOL sequence-number owner exists"),
                    self.peer_qos,
                )
                .await
            } else {
                transmit_unprotected_eapol(
                    self.mmio,
                    self.tx_storage,
                    self.station_address,
                    self.bssid,
                    frame.as_bytes(),
                    self.sequences.take_non_qos(),
                )
                .await
            }
            .map_err(RadioHilWpa2KeyError::Transmit)?;
            self.completion = Some(completion);
            if completion.status == 0 {
                Ok(())
            } else {
                Err(RadioHilWpa2KeyError::TxStatus(completion.status))
            }
        }
    }
}

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn open_radio_mac_interrupt() {
    OPEN_RADIO_MAC_IRQ_ENTRIES.fetch_add(1, Ordering::Relaxed);
    let registers = OPEN_RADIO_MAC_INTERRUPT_PTR.load(Ordering::Acquire);
    if registers.is_null() {
        return;
    }
    // SAFETY: the parent STA owner publishes this epoch's split capability
    // before binding this handler. The S31 masks the active interrupt while
    // its handler runs, and task-side code moves the capability only after
    // disabling the CPU route, so calls cannot overlap.
    let interrupt = unsafe { &mut *registers };
    let mut first_status = 0;
    let mut observed_status = 0;
    let mut nonzero_snapshots = 0;
    for _ in 0..32 {
        let (_, snapshot) = handle_mac_irq(&mut *interrupt, &OpenRadioMacIrqSink);
        if snapshot.status == 0 {
            break;
        }
        if nonzero_snapshots == 0 {
            first_status = snapshot.status;
        }
        observed_status |= snapshot.status;
        nonzero_snapshots += 1;
    }
    OPEN_RADIO_MAC_IRQ_CLASSIFICATION.record(
        first_status,
        observed_status,
        nonzero_snapshots,
    );
}

struct OpenRadioMacIrqSink;

impl IrqSink for OpenRadioMacIrqSink {
    #[inline]
    fn post(&self, mac_pending: u32) {
        if mac_pending & MAC_INT_RX_SUCCESS != 0 {
            // Record the epoch before publishing the cross-core Embassy wake.
            // Otherwise the radio task can begin service on core 1 before the
            // core-0 ISR stores its diagnostic timestamp.
            if !OPEN_RADIO_IRQ_RUNTIME.rx_signaled() {
                OPEN_RADIO_RX_PIPELINE_COUNTERS.record_rx_irq_epoch();
            }
        }
        OPEN_RADIO_IRQ_RUNTIME.publish(mac_pending);
    }

    #[inline]
    fn record_unhandled(&self, bits: u32) {
        IrqSink::record_unhandled(&OPEN_RADIO_IRQ_RUNTIME, bits);
    }
}

/// Publish one task-owned interrupt setup into the stable ISR storage.
///
/// The first epoch initializes the `StaticCell`s. Later epochs reuse their
/// addresses after [`deactivate_open_radio_interrupts`] has moved the previous
/// values out and cleared the active pointers. No ISR route is opened until
/// both banks are fully initialized and published.
fn activate_open_radio_interrupts(
    platform: &EspHalRadioPeripheral,
    setup: MacInterruptSetup,
    event_mask: u32,
) {
    let (mac, power) = setup.activate(event_mask);
    let mac_storage = OPEN_RADIO_MAC_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire);
    let power_storage = OPEN_RADIO_POWER_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire);
    let (mac_storage, power_storage) = if mac_storage.is_null() && power_storage.is_null() {
        let mac_storage = OPEN_RADIO_MAC_INTERRUPT_REGISTERS.init(mac) as *mut _;
        let power_storage = OPEN_RADIO_POWER_INTERRUPT_REGISTERS.init(power) as *mut _;
        OPEN_RADIO_MAC_INTERRUPT_STORAGE_PTR.store(mac_storage, Ordering::Release);
        OPEN_RADIO_POWER_INTERRUPT_STORAGE_PTR.store(power_storage, Ordering::Release);
        (mac_storage, power_storage)
    } else {
        assert!(!mac_storage.is_null() && !power_storage.is_null());
        assert!(OPEN_RADIO_MAC_INTERRUPT_PTR.load(Ordering::Acquire).is_null());
        assert!(
            OPEN_RADIO_POWER_INTERRUPT_PTR
                .load(Ordering::Acquire)
                .is_null()
        );
        // SAFETY: the active pointers are null and the CPU routes remain
        // disabled after the preceding deactivation. That transition moved
        // both old values out of these exact cells, so each location is
        // uninitialized and uniquely owned until publication below.
        unsafe {
            mac_storage.write(mac);
            power_storage.write(power);
        }
        (mac_storage, power_storage)
    };
    OPEN_RADIO_MAC_INTERRUPT_PTR.store(mac_storage, Ordering::Release);
    OPEN_RADIO_POWER_INTERRUPT_PTR.store(power_storage, Ordering::Release);
    platform.bind_interrupts(open_radio_mac_interrupt, open_radio_power_interrupt);
}

/// Close the active CPU/peripheral interrupt epoch and recover setup ownership.
fn deactivate_open_radio_interrupts(
    platform: &EspHalRadioPeripheral,
) -> MacInterruptSetup {
    // The handlers are bound on the parent STA task's core. Disabling both CPU
    // routes there proves that neither handler can begin while the task moves
    // the PAC values out of their stable storage. A handler already executing
    // on this same core must have returned before this task can run.
    platform.disable_interrupts();
    let mac = OPEN_RADIO_MAC_INTERRUPT_PTR.swap(core::ptr::null_mut(), Ordering::AcqRel);
    let power = OPEN_RADIO_POWER_INTERRUPT_PTR.swap(core::ptr::null_mut(), Ordering::AcqRel);
    assert!(!mac.is_null() && !power.is_null());
    assert_eq!(
        mac,
        OPEN_RADIO_MAC_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire)
    );
    assert_eq!(
        power,
        OPEN_RADIO_POWER_INTERRUPT_STORAGE_PTR.load(Ordering::Acquire)
    );
    // SAFETY: the CPU routes are disabled on their binding core, both active
    // pointers are null, and these stable locations contain the unique values
    // installed by `activate_open_radio_interrupts`. Reading moves each value
    // out exactly once; the next activation writes them back before routing.
    let mac = unsafe { mac.read() };
    let power = unsafe { power.read() };
    mac.deactivate(power)
}

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn open_radio_power_interrupt() {
    let registers = OPEN_RADIO_POWER_INTERRUPT_PTR.load(Ordering::Acquire);
    if registers.is_null() {
        return;
    }
    // SAFETY: both ISR capabilities are published before either interrupt is
    // routed. The platform masks this active interrupt while the handler runs,
    // and task-side code cannot access the disjoint power STATUS/CLEAR bank.
    let interrupt = unsafe { &mut *registers };
    for _ in 0..32 {
        let (_, snapshot) = handle_power_irq(interrupt, &OPEN_RADIO_POWER_IRQ_RUNTIME);
        if snapshot.status == 0 {
            break;
        }
    }
}

fn read_diagnostic_mmio(address: usize) -> u32 {
    // SAFETY: diagnostic-only reads in this isolated HIL image. Production
    // radio operations use typed PAC identities; keeping snapshots here raw
    // avoids exporting ownership-free aliases solely for logging.
    unsafe { (address as *const u32).read_volatile() }
}

struct EmbassyPhyDelay;

impl PhyAsyncDelay for EmbassyPhyDelay {
    fn after_micros(micros: u64) -> impl core::future::Future<Output = ()> {
        Timer::after_micros(micros)
    }
}

struct HilPhyObserver;

impl PhyTargetObserver for HilPhyObserver {
    fn operation_started(&mut self) {
        DIAGNOSTIC_ACTION_ORDINAL.fetch_add(1, Ordering::AcqRel);
        set_diagnostic_stage(110);
        set_diagnostic_stage(120);
    }

    fn operation_completed(&mut self) {
        set_diagnostic_stage(130);
    }

    fn channel_frequency_ready_timed_out(&mut self, samples: u32) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL channel=frequency-ready-timeout samples={samples}"
        ));
    }

    fn channel_completed(
        &mut self,
        outcome: open_esp_radio::esp32s31::phy::phy_channel::PhyChipChannelOutcome,
        operations: u32,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=post-init-channel channel={} \
             frequency={} operations={operations}",
            outcome.channel, outcome.frequency_mhz,
        ));
    }

    fn channel_failed(
        &mut self,
        failure: open_esp_radio::esp32s31::phy::phy_channel::PhyChipChannelFailure,
        operations: u32,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=post-init-channel \
             failure={failure:?} operations={operations}"
        ));
    }

    fn mac_channel_restarted(&mut self, channel_or_frequency: u16, cbw: u8, link: u8) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=mac-channel-restart \
             channel_or_frequency={channel_or_frequency} cbw={cbw} \
             control={:#010x} regdma_link={link}",
            read_diagnostic_mmio(0x2010_4cac),
        ));
    }

    fn tx_dc_entry(&mut self) {
        log_open_txdc_entry_mmio();
    }

    fn tx_dc_comparator(&mut self, gain_index: u8, iteration: u8, comparator_high: [bool; 2]) {
        if gain_index == 0 && iteration == 0 {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL probe=txdc-first-environment \
                 bb_init={:#010x} pbus={:#010x}/{:#010x}/{:#010x} \
                 tone={:#010x}/{:#010x}/{:#010x}/{:#010x} control={:#010x}",
                read_diagnostic_mmio(0x2010_0800),
                read_diagnostic_mmio(0x2010_0884),
                read_diagnostic_mmio(0x2010_088c),
                read_diagnostic_mmio(0x2010_0890),
                read_diagnostic_mmio(0x2010_040c),
                read_diagnostic_mmio(0x2010_041c),
                read_diagnostic_mmio(0x2010_0420),
                read_diagnostic_mmio(0x2010_0428),
                read_diagnostic_mmio(0x2010_0418),
            ));
        }
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=txdc-comparator gain={} iteration={} \
             comparator={:?} control={:#010x}",
            gain_index,
            iteration,
            comparator_high,
            read_diagnostic_mmio(0x2010_0418),
        ));
    }

    fn power_detector_sample(
        &mut self,
        measurement_index: u8,
        sample_index: u8,
        sample_value: u16,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=pwdet-sample measurement={} sample={} \
             value={} tone={:#010x}/{:#010x}/{:#010x} \
             sar={:#010x}/{:#010x} reference={:#010x}",
            measurement_index,
            sample_index,
            sample_value,
            read_diagnostic_mmio(0x2010_040c),
            read_diagnostic_mmio(0x2010_041c),
            read_diagnostic_mmio(0x2010_0420),
            read_diagnostic_mmio(0x2010_0808),
            read_diagnostic_mmio(0x2010_080c),
            read_diagnostic_mmio(0x2010_0818),
        ));
    }

    fn rf_boundary(&mut self, boundary: PhyRfBoundary) {
        let source = match boundary {
            PhyRfBoundary::BeforeRfInit => "open-before-rf-init",
            PhyRfBoundary::AfterPbusClear => "open-after-pbus-clear",
            PhyRfBoundary::BeforeI2cMasterRegisterInit => "open-before-i2cmst-reg-init",
            PhyRfBoundary::BeforePowerDetectorRegisterInit => "open-before-pwdet-reg-init",
            PhyRfBoundary::BeforeFrontEndRegisterInit => "open-before-fe-reg-init",
            PhyRfBoundary::BeforeTemperatureSensorReadInit => "open-before-tsens-read-init",
            PhyRfBoundary::BeforeTxPowerControlBackgroundInit => "open-before-tx-pwctrl-bg-init",
            PhyRfBoundary::BeforeChannelFrequencyInit => "open-before-chan-freq-init",
        };
        log_open_rf_boundary_mmio(source);
    }
}

async fn select_channel(
    state: &mut PhyColdState,
    channel_or_frequency: u16,
    cbw: u8,
    platform: &mut EspHalRadioPeripheral,
    registers: &mut RadioRegisters,
) -> Result<(), PhyTargetPortError> {
    let mut observer = HilPhyObserver;
    select_phy_channel::<EmbassyPhyDelay, _, _>(
        state,
        channel_or_frequency,
        cbw,
        platform,
        registers,
        &mut observer,
    )
    .await
}

async fn switch_channel_with_mac_restart(
    state: &mut PhyColdState,
    channel_or_frequency: u16,
    cbw: u8,
    platform: &mut EspHalRadioPeripheral,
    registers: &mut RadioRegisters,
) -> Result<(), PhyTargetPortError> {
    let mut observer = HilPhyObserver;
    switch_phy_channel_with_mac_restart::<EmbassyPhyDelay, _, _>(
        state,
        channel_or_frequency,
        cbw,
        platform,
        registers,
        &mut observer,
    )
    .await
}

fn log_open_txdc_entry_mmio() {
    const ADDRESSES: [usize; 18] = [
        0x2010_001c,
        0x2010_0028,
        0x2010_040c,
        0x2010_0418,
        0x2010_041c,
        0x2010_0420,
        0x2010_0428,
        0x2010_0800,
        0x2010_081c,
        0x2010_0820,
        0x2010_0830,
        0x2010_0848,
        0x2010_084c,
        0x2010_0850,
        0x2010_0870,
        0x2010_0884,
        0x2010_088c,
        0x2010_0890,
    ];
    // SOURCE: keep this list and the page hash geometry identical to
    // `open_radio_vendor_oracle_hil::__wrap_phy_txdc_cal_init`. That linker
    // wrapper records the hardware immediately before the blob call proved by
    // `_oracles/libphy.a[phy_tx_cal.o]`; this function records the matching
    // open state before its first `ConfigurePbusDebugMode` action.
    let values: [u32; ADDRESSES.len()] =
        core::array::from_fn(|index| read_diagnostic_mmio(ADDRESSES[index]));
    emergency_log(format_args!(
        "OPEN_RADIO_TXDC_ENTRY source=open-before-txdc addresses={ADDRESSES:08x?}"
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_TXDC_ENTRY source=open-before-txdc values={values:08x?}"
    ));

    const PAGE_OFFSETS: [u16; 28] = [
        0x0000, 0x0400, 0x0800, 0x0c00, 0x4000, 0x4100, 0x4200, 0x4300, 0x4400, 0x4800, 0x4c00,
        0x4d00, 0x5100, 0x5500, 0x5700, 0x7000, 0x7100, 0x7400, 0x7800, 0x7900, 0x7a00, 0x7c00,
        0x7d00, 0x8000, 0x9c00, 0xd800, 0xf000, 0xf800,
    ];
    for offset in PAGE_OFFSETS {
        let base = 0x2010_0000_usize + usize::from(offset);
        let mut hash = 0x811c_9dc5_u32;
        let mut word = 0_usize;
        while word != 64 {
            hash ^= read_diagnostic_mmio(base + word * 4);
            hash = hash.wrapping_mul(0x0100_0193);
            word += 1;
        }
        emergency_log(format_args!(
            "OPEN_RADIO_MMIO_PAGE source=open-before-txdc \
             offset={offset:#06x} hash={hash:#010x}"
        ));
    }

    // SOURCE: these are precisely the hash-mismatching pages from the
    // 2026-07-29 vendor/open cold-entry comparison above. Keep the output
    // geometry identical to the vendor oracle wrapper so it can be diffed
    // mechanically by address.
    const DIFFERING_PAGE_OFFSETS: [u16; 8] = [
        0x0400, 0x0800, 0x0c00, 0x4400, 0x5500, 0x7000, 0x7c00, 0xd800,
    ];
    for page in DIFFERING_PAGE_OFFSETS {
        let base = 0x2010_0000_usize + usize::from(page);
        for chunk in 0..4_u16 {
            let offset = page + chunk * 0x40;
            let values: [u32; 16] = core::array::from_fn(|word| {
                read_diagnostic_mmio(base + usize::from(chunk) * 0x40 + word * 4)
            });
            emergency_log(format_args!(
                "OPEN_RADIO_TXDC_WORDS source=open-before-txdc \
                 offset={offset:#06x} values={values:08x?}"
            ));
        }
    }
}

fn log_open_rf_boundary_mmio(source: &str) {
    emergency_log(format_args!(
        "OPEN_RADIO_RF_BOUNDARY source={source} \
         pbus={:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x} \
         dac_scale={:#010x}",
        read_diagnostic_mmio(0x2010_0884),
        read_diagnostic_mmio(0x2010_088c),
        read_diagnostic_mmio(0x2010_0890),
        read_diagnostic_mmio(0x2010_0898),
        read_diagnostic_mmio(0x2010_089c),
        read_diagnostic_mmio(0x2010_08a0),
        read_diagnostic_mmio(0x2010_08a4),
        read_diagnostic_mmio(0x2010_0c04),
    ));
}

fn observe_scan_descriptors<M: Mmio>(
    mmio: &mut M,
    storage: &RxStorage,
    descriptor_base: u32,
    scan_table: &mut ScanTable,
    scan_frame: &mut [u8; RX_STAGE_CAPACITY],
    station_address: [u8; 6],
    channel: u8,
    observed_mask: &mut u64,
    raw_frames: &mut u32,
    probe_responses: &mut u32,
) {
    for (index, descriptor) in storage.descriptors().iter().enumerate() {
        let word0 = descriptor.word0();
        let bit = 1_u64 << index;
        if !rx_done(word0) || *observed_mask & bit != 0 {
            continue;
        }
        *observed_mask |= bit;
        *raw_frames = raw_frames.saturating_add(1);
        if *raw_frames == 1 {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=rx-frame \
                 channel={channel} descriptor={index} word0={word0:#010x} \
                 length={} control={:#010x} next={:#010x} last={:#010x}",
                descriptor_length(word0),
                mmio.read32(RX_CONTROL),
                mmio.read32(RX_NEXT_DESCRIPTOR),
                mmio.read32(RX_LAST_DESCRIPTOR),
            ));
        }

        let segment = RxSegment {
            descriptor_address: descriptor_base + index as u32 * DESCRIPTOR_BYTES,
            descriptor_word0: word0,
            buffer: unsafe {
                // The completed descriptor has returned this buffer to the
                // sole radio task for the duration of parsing.
                storage.buffers()[index].completed()
            },
            next_descriptor_address: descriptor.next_address(),
        };
        match extract_management(
            core::slice::from_ref(&segment),
            RxIngressConfig {
                ring_entry_limit: 1,
                csi_config: 0,
                flags: 0,
            },
            scan_frame,
        ) {
            Ok(frame) => {
                let rssi = unsafe { storage.buffers()[index].read_byte(0) as i8 };
                if frame.length >= 10
                    && scan_frame[0] & 0xfc == 0x50
                    && scan_frame[4..10] == station_address
                {
                    *probe_responses = probe_responses.saturating_add(1);
                    if *probe_responses <= 3 {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL probe=addressed-probe-response \
                             channel={channel} count={} da={:02x?} sa={:02x?}",
                            *probe_responses,
                            &scan_frame[4..10],
                            &scan_frame[10..16],
                        ));
                    }
                }
                let observation =
                    scan_table.observe_management(&scan_frame[..frame.length], channel, rssi);
                if matches!(
                    observation,
                    ScanObservation::Inserted { .. } | ScanObservation::Updated { .. }
                ) {
                    let record_index = match observation {
                        ScanObservation::Inserted { index }
                        | ScanObservation::Updated { index } => index,
                        _ => unreachable!(),
                    };
                    let record = &scan_table.records()[record_index];
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=scan-record index={record_index} \
                         ssid={:?} bssid={:02x?} channel={} rssi={} \
                         privacy={} rsn={} ht={} ht40={:?} he={} truncated={}",
                        record.ssid_bytes(),
                        record.bssid,
                        record.channel,
                        record.rssi,
                        record.privacy,
                        record.rsn,
                        record.ht_capability_ie_present,
                        record.ht40_secondary_channel(),
                        !record.he_capability_ie_bytes().is_empty(),
                        record.information_elements_truncated,
                    ));
                }
            }
            Err(error) if *raw_frames <= 2 => {
                let boundary: [u32; 12] = core::array::from_fn(|word| unsafe {
                    storage.buffers()[index].read_word(0x28 + word * 4)
                });
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL probe=rx-extract descriptor={index} \
                     error={error:?} boundary={boundary:08x?}"
                ));
            }
            Err(_) => {}
        }
    }
}

async fn transmit_probe_request(
    mmio: &mut RadioRegisters,
    storage: &mut TxStorage,
    source: [u8; 6],
    sequence_number: u16,
) -> Result<TxCompletion, ActiveScanTxError> {
    // The cold MAC transaction has already programmed the interface
    // addresses, HE broadcast-RU policy, TX/RX timing and response-rate
    // tables from complete blob call graphs. Do not replay the old
    // HIL_VENDOR_STA_START_DIFF register snapshot here:
    //
    // - 0x2010_4c54/58 are WDEVDELAY1/WDEVDELAY and complete
    //   `hal_he_set_mac_delay` derives their high fields from
    //   `_random() % 11`; the former fixed images merely froze slot nine.
    // - 0x2010_448c is canonically PHY_I2C.I2C_TX_RATE_CONTROL according to
    //   complete rev0 ROM PHY initialization, not a MAC state-clear word.
    // - 0x2010_4c30 is the read-only WDEV_INT1_RAW diagnostic word named by
    //   complete `libpp.a[hal_debug.o]::print_isr_regs`.
    //
    // SOURCE: `_oracles/libpp.a[hal_mac.o]::mac_txrx_init`,
    // `_oracles/libpp.a[hal_mac_ctl.o]::hal_he_set_mac_delay`,
    // `_oracles/libpp.a[hal_debug.o]::print_isr_regs`, and complete rev0 ROM
    // `phy_i2c_txrate_init`. Cold MAC initialization already owns interface
    // address publication. The only live STA-start operation here is the
    // generated-PAC transaction for complete `hal_set_sta_tsf(0)` followed by
    // complete `hal_enable_sta_tsf`.
    mmio.start_station_tsf(0);
    storage
        .control_mut()
        .transmit_probe_request(
            mmio,
            ProbeRequest {
                source,
                sequence_number,
                ssid: b"",
                supported_rates: &PROBE_REQUEST_RATES,
            },
            Some(sequence_number as u8),
            Some(PROBE_TX_DESCRIPTOR_CAPACITY as u32),
        )
        .await
        .map_err(Into::into)
}

async fn transmit_open_authentication<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    source: [u8; 6],
    bssid: [u8; 6],
    sequence_number: u16,
) -> Result<TxCompletion, ActiveScanTxError> {
    // A live vendor STA capture for this exact 30-byte open-authentication
    // request publishes a 40-byte source allocation and the legacy vector
    // PLCP1=0x00b6. Our direct descriptor additionally publishes the
    // hardware-appended four-byte FCS in its length field, so its bounded DMA
    // capacity must cover metadata + MPDU + FCS: align4(8 + 30 + 4) = 44.
    // Keeping the vendor source capacity of 40 here formerly produced the
    // contradictory descriptor length=42 > capacity=40; the pinned owner now
    // rejects that geometry before the MAC can observe it.
    //
    // The complete blob call graph separately proves scheduler priority 1
    // and packet PTI 1; a post-completion register snapshot that read zero
    // was therefore not a valid source for the submitted PTI.
    // The direct legacy queue consumes MPDU+FCS length in PLCP1. The earlier
    // `0x00b6` vendor-context snapshot was not portable to this raw q0 path;
    // deriving `30 + 4 = 0x22` produces a valid over-air authentication frame.
    let completion = storage
        .control_mut()
        .transmit_open_authentication(
            mmio,
            OpenAuthenticationRequest {
                source,
                bssid,
                sequence_number,
            },
        )
        .await
        .map_err(Into::into);
    if AUTH_REGISTER_SNAPSHOT_CAPTURED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        log_open_auth_register_snapshot();
    }
    completion
}

/// Capture the first open Authentication TX state without changing it.
///
/// The order is intentionally fixed and shared with the address-by-address
/// vendor/open comparison. Unknown words stay address-labelled in this
/// diagnostic instead of receiving speculative names; any stable difference
/// promoted into the driver must first be tied to blob/ROM control flow and
/// documented in the SVD.
fn log_open_auth_register_snapshot() {
    // These raw addresses are deliberately confined to the HIL diagnostic
    // boundary above. A word is promoted to the PAC only after its identity
    // and fields are supported by the blob/ROM evidence recorded in the SVD.
    const ADDRESSES: [usize; 73] = [
        0x2010_4000,
        0x2010_4004,
        0x2010_4038,
        0x2010_403c,
        0x2010_42f4,
        0x2010_4300,
        0x2010_430c,
        0x2010_4310,
        0x2010_4314,
        0x2010_4318,
        0x2010_432c,
        0x2010_4330,
        0x2010_4334,
        0x2010_434c,
        0x2010_4350,
        0x2010_435c,
        0x2010_4360,
        0x2010_4364,
        0x2010_4370,
        0x2010_4388,
        0x2010_438c,
        0x2010_43b4,
        0x2010_43b8,
        0x2010_43bc,
        0x2010_4400,
        0x2010_4404,
        0x2010_443c,
        0x2010_4440,
        0x2010_4444,
        0x2010_4448,
        0x2010_444c,
        0x2010_4450,
        0x2010_4458,
        0x2010_445c,
        0x2010_448c,
        0x2010_4830,
        0x2010_4c04,
        0x2010_4c30,
        0x2010_4c54,
        0x2010_4c58,
        0x2010_4c60,
        0x2010_4c7c,
        0x2010_4c80,
        0x2010_4c8c,
        0x2010_4cac,
        0x2010_4dd4,
        0x2010_4dd8,
        0x2010_4ddc,
        0x2010_4e10,
        0x2010_4e24,
        0x2010_4e2c,
        0x2010_4e30,
        0x2010_4e34,
        0x2010_4e38,
        0x2010_4e44,
        0x2010_4e48,
        0x2010_4e4c,
        0x2010_4e58,
        0x2010_4e5c,
        0x2010_4e60,
        0x2010_54d8,
        0x2010_54dc,
        0x2010_54e0,
        0x2010_54e4,
        0x2010_54e8,
        0x2010_5500,
        0x2010_5504,
        0x2010_550c,
        0x2010_5510,
        0x2010_d814,
        0x2010_d818,
        0x2010_d81c,
        0x2010_d83c,
    ];
    let values: [u32; 73] = core::array::from_fn(|index| read_diagnostic_mmio(ADDRESSES[index]));
    for (chunk, words) in values.chunks(16).enumerate() {
        emergency_log(format_args!(
            "OPEN_AUTH_REGISTER_SNAPSHOT chunk={chunk} values={words:08x?}"
        ));
    }
}

async fn transmit_association_request<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    source: [u8; 6],
    access_point: &ScanRecord,
    sequence_number: u16,
) -> Result<TxCompletion, ActiveScanTxError> {
    let phy = select_sta_association(access_point, STA_ASSOCIATION_PREFERENCE).phy;
    let (power_capability, he_ul_mu_power) = if phy == StaAssociationPhy::He20 {
        let profile = storage.control_mut().power_profile();
        let rate_power = core::array::from_fn(|offset| profile.pair(16 + offset as u8).primary);
        // SOURCE: complete `_oracles/libpp.a[hal_mac_ctl.o]::hal_he_init`
        // installs -11 through `hal_set_tx_min_pwr`; complete
        // `_oracles/libnet80211.a[ieee80211_he.o]::
        // ieee80211_add_power_cap` pairs it with `hal_get_tx_pwr(16, 1)`.
        let power_capability = StaPowerCapability::new(-11, rate_power[0])
            .map_err(ActiveScanTxError::PowerCapability)?;
        let capability = HeUlMuPowerCapability::from_rate_power_indices(rate_power)
            .map_err(ActiveScanTxError::HeUlMuPower)?;
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=sta-he-ul-mu-power \
             minimum_dbm={} maximum_dbm={} rate_16_through_25={rate_power:?} \
             relative_to_rate_16={:?}",
            power_capability.minimum_dbm(),
            power_capability.maximum_dbm(),
            capability.relative_to_rate_16(),
        ));
        (Some(power_capability), Some(capability))
    } else {
        (None, None)
    };
    // `transmit_encoded_management` publishes four additional bytes in the
    // descriptor length for the hardware-appended FCS. Keep the allocation
    // capacity large enough for that hardware-visible length before rounding
    // it to the recovered four-byte DMA granularity.
    storage
        .control_mut()
        .transmit_association(
            mmio,
            AssociationRequest {
                source,
                access_point,
                sequence_number,
                // SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: successful vendor
                // association frame 7624 uses listen interval three.
                listen_interval: 3,
                phy,
                power_capability,
                he_ul_mu_power,
            },
        )
        .await
        .map_err(Into::into)
}

const STA_ASSOCIATION_PREFERENCE: StaAssociationPreference =
    if option_env!("OPEN_RADIO_FORCE_HE20").is_some() || OPEN_RADIO_HE_TB_HIL {
        StaAssociationPreference::PreferHe20
    } else if option_env!("OPEN_RADIO_FORCE_HT20").is_some() {
        StaAssociationPreference::ForceHt20
    } else {
        StaAssociationPreference::Automatic
    };

const fn configured_sta_tx_rate_policy(
    association_phy: StaAssociationPhy,
    peer_qos: bool,
    peer_supports_one_ltf_800ns_gi: bool,
    peer_supports_ldpc: bool,
    peer_dcm_receive: HeDcmConstellation,
) -> StaTxRatePolicy {
    StaTxRatePolicy {
        association_phy,
        high_throughput_enabled: option_env!("OPEN_RADIO_FORCE_LEGACY_TX").is_none() && peer_qos,
        fallback_legacy_rate: OPEN_RADIO_DATA_RATE,
        fallback_ht_mcs: OPEN_RADIO_HT_MCS,
        fallback_ht_guard_interval: OPEN_RADIO_HT_GI,
        ht_mcs_override: if option_env!("OPEN_RADIO_HT_MCS").is_some() {
            Some(OPEN_RADIO_HT_MCS)
        } else {
            None
        },
        ht_guard_interval_override: if option_env!("OPEN_RADIO_HT_SGI").is_some() {
            Some(HtGuardInterval::Short400Ns)
        } else {
            None
        },
        he_mcs_override: if option_env!("OPEN_RADIO_HE_MCS").is_some() {
            Some(OPEN_RADIO_HE_MCS)
        } else {
            None
        },
        he_guard_interval_and_ltf_override: if option_env!("OPEN_RADIO_HE_GI_LTF").is_some() {
            Some(OPEN_RADIO_HE_GI_LTF)
        } else {
            None
        },
        he_dcm_override: OPEN_RADIO_HE_DCM_RATE,
        he_800ns_gi_ltf: if peer_supports_one_ltf_800ns_gi {
            HeGuardIntervalAndLtf::OneLtf800Ns
        } else {
            HeGuardIntervalAndLtf::TwoLtf800Ns
        },
        peer_supports_ldpc,
        peer_dcm_receive,
    }
}

const fn selected_data_tx_rate(association_phy: StaAssociationPhy, peer_qos: bool) -> TxPhyRate {
    configured_sta_tx_rate_policy(
        association_phy,
        peer_qos,
        false,
        false,
        HeDcmConstellation::NotSupported,
    )
    .fallback_rate()
}

fn open_radio_tx_entropy() -> u32 {
    Rng::new().random()
}

async fn transmit_unprotected_eapol<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    eapol: &[u8],
    sequence_number: u16,
) -> Result<TxCompletion, ActiveScanTxError> {
    storage
        .control_mut()
        .transmit_unprotected_data(
            mmio,
            StaDataFrame {
                source: station_address,
                bssid,
                destination: bssid,
                sequence_number,
                ether_type: 0x888e,
                payload: eapol,
            },
        )
        .await
        .map_err(Into::into)
}

async fn transmit_eapol_message_4<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    message: &Wpa2TxFrame,
    key_slot: &mut StaPairwiseCcmpSlot,
    sequence_number: u16,
    peer_qos: bool,
) -> Result<TxCompletion, ActiveScanTxError> {
    let ccmp_header = key_slot.next_tx_ccmp_header();
    storage
        .control_mut()
        .transmit_protected_data(
            mmio,
            StaProtectedDataFrame {
                source: station_address,
                bssid,
                destination: bssid,
                sequence_number,
                user_priority: 7,
                peer_qos,
                ccmp_header,
                ether_type: 0x888e,
                payload: message.as_bytes(),
            },
            LegacyTxQueue::Voice,
            TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
            key_slot.hardware_index(),
        )
        .await
        .map_err(Into::into)
}

fn arp_probe_payload(station_address: [u8; 6]) -> [u8; 28] {
    let mut payload = [0_u8; 28];
    payload[0..2].copy_from_slice(&1_u16.to_be_bytes());
    payload[2..4].copy_from_slice(&0x0800_u16.to_be_bytes());
    payload[4] = 6;
    payload[5] = 4;
    payload[6..8].copy_from_slice(&1_u16.to_be_bytes());
    payload[8..14].copy_from_slice(&station_address);
    // The ordinary-router path has not acquired its DHCP lease yet, so its
    // sender protocol address intentionally remains 0.0.0.0 (an ARP probe).
    // The controlled throughput profile already owns STA_HIL_IPV4 through
    // `NetworkConfig::ipv4_static`; Linux does not reply deterministically to
    // an RFC 5227-style probe for its own gateway address, so publish the
    // station's actual address and send an ordinary ARP request there.
    if PERF_AP_PROFILE {
        payload[14..18].copy_from_slice(&STA_HIL_IPV4);
    }
    payload[24..28].copy_from_slice(&STA_ARP_TARGET_IPV4);
    payload
}

fn queue_arp_probe(
    device: &mut NetworkDevice,
    runner: &NetworkRunner,
    station_address: [u8; 6],
) -> Option<NetworkTxFrame> {
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    device.transmit(&mut context)?.consume(14 + 28, |ethernet| {
        ethernet[..6].fill(0xff);
        ethernet[6..12].copy_from_slice(&station_address);
        ethernet[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
        ethernet[14..].copy_from_slice(&arp_probe_payload(station_address));
    });
    runner.try_receive_tx()
}

async fn transmit_protected_ethernet_frame<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    bssid: [u8; 6],
    key_slot: &mut StaPairwiseCcmpSlot,
    sequence_number: u16,
    peer_qos: bool,
    data_rate: TxPhyRate,
    ethernet: &[u8],
) -> Result<TxCompletion, ActiveScanTxError> {
    if ethernet.len() < 14 {
        return Err(ControlTxError::Tx(TxError::Invalid).into());
    }
    let destination = ethernet[..6]
        .try_into()
        .map_err(|_| ActiveScanTxError::from(ControlTxError::Tx(TxError::Invalid)))?;
    let source = ethernet[6..12]
        .try_into()
        .map_err(|_| ActiveScanTxError::from(ControlTxError::Tx(TxError::Invalid)))?;
    let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
    let ccmp_header = key_slot.next_tx_ccmp_header();
    storage
        .control_mut()
        .transmit_protected_data(
            mmio,
            StaProtectedDataFrame {
                source,
                bssid,
                destination,
                sequence_number,
                user_priority: 0,
                peer_qos,
                ccmp_header,
                ether_type,
                payload: &ethernet[14..],
            },
            LegacyTxQueue::BestEffort,
            data_rate,
            key_slot.hardware_index(),
        )
        .await
        .map_err(Into::into)
}

async fn await_protected_arp_response<M: Mmio + RxDma>(
    mmio: &mut M,
    rx_storage: &RxStorage,
    frame: &mut [u8],
    ethernet: &mut [u8],
    network_device: &mut NetworkDevice,
    network_runner: &NetworkRunner,
    station_address: [u8; 6],
    bssid: [u8; 6],
    rx: &mut RadioHilJoinRx<'_>,
) -> bool {
    let rx_ring = match rx.live_mut() {
        Ok(ring) => ring,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-protected-rx \
                 error={error:?}"
            ));
            return false;
        }
    };
    let mut received_frames = 0_u32;
    let mut protected_frames = 0_u32;
    let mut mic_failures = 0_u32;
    let mut addressed_protected = 0_u32;
    for _ in 0..WPA2_PROTECTED_ARP_TIMEOUT_MS {
        for index in 0..RX_DESCRIPTOR_COUNT {
            let Some(completed) = rx_ring.take_completed(index) else {
                continue;
            };
            received_frames = received_frames.saturating_add(1);
            let segment = RxSegment {
                descriptor_address: completed.descriptor_address(),
                descriptor_word0: completed.word0(),
                buffer: unsafe { rx_storage.buffers()[index].completed() },
                next_descriptor_address: completed.next_descriptor_address(),
            };
            let raw = segment.buffer;
            let raw_fc = u16::from_le_bytes([raw[PUBLIC_HEADER_SIZE], raw[PUBLIC_HEADER_SIZE + 1]]);
            let raw_addressed_protected = raw_fc & 0x400c == 0x4008
                && raw[PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10] == station_address;
            if raw_addressed_protected {
                addressed_protected = addressed_protected.saturating_add(1);
            }
            let data = match extract_ccmp_data(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                frame,
            ) {
                Ok(data) => data,
                Err(RxError::MicFailure) => {
                    mic_failures = mic_failures.saturating_add(1);
                    if raw_addressed_protected {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=wpa2-protected-rx-addressed \
                             result=mic-failure fc={raw_fc:#06x} state={:#04x} \
                             internal={:#04x}",
                            raw[PUBLIC_HEADER_SIZE - 4],
                            raw[PUBLIC_HEADER_SIZE - 3],
                        ));
                    }
                    continue;
                }
                Err(error) => {
                    if raw_addressed_protected {
                        let layout = first_segment_layout(
                            &segment,
                            RxIngressConfig {
                                ring_entry_limit: 1,
                                csi_config: 0,
                                flags: 0,
                            },
                        );
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=wpa2-protected-rx-addressed \
                             result=reject error={error:?} fc={raw_fc:#06x} state={:#04x} \
                             internal={:#04x} layout={layout:?}",
                            raw[PUBLIC_HEADER_SIZE - 4],
                            raw[PUBLIC_HEADER_SIZE - 3],
                        ));
                    }
                    continue;
                }
            };
            protected_frames = protected_frames.saturating_add(1);
            if data.mpdu.length < 24 || frame[4..10] != station_address || frame[10..16] != bssid {
                continue;
            }
            if addressed_protected <= 8 {
                let prefix_end = data.payload_offset.saturating_add(16).min(data.mic_offset);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=wpa2-protected-rx-addressed \
                     result=crypto-ok fc={:#06x} pn={:02x?} mpdu={} payload={} prefix={:02x?}",
                    u16::from_le_bytes([frame[0], frame[1]]),
                    &frame[data.ccmp_header_offset..data.payload_offset],
                    data.mpdu.length,
                    data.payload_length,
                    &frame[data.payload_offset..prefix_end],
                ));
            }
            let ethernet_plan = match decapsulate_data(
                DataInterfaceRole::Station,
                &frame[..data.mpdu.length],
                data.payload_offset,
                data.payload_length,
                ethernet,
            ) {
                Ok(plan) => plan,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=wpa2-protected-rx-addressed \
                         result=decap-reject error={error:?}"
                    ));
                    continue;
                }
            };
            if ethernet_plan.destination != station_address || ethernet_plan.ether_type != 0x0806 {
                continue;
            }
            if let Err(error) =
                network_runner.try_send_rx(&ethernet[..ethernet_plan.ethernet_length])
            {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=wpa2-protected-rx-addressed \
                     result=network-rx-enqueue-reject error={error:?}"
                ));
                continue;
            }
            let waker = Waker::noop();
            let mut context = Context::from_waker(waker);
            let Some((rx_token, reply_token)) = network_device.receive(&mut context) else {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=wpa2-protected-rx-addressed \
                     result=network-rx-token-missing"
                ));
                continue;
            };
            drop(reply_token);
            let Some(arp_source) = rx_token.consume(|network_frame| {
                if network_frame.len() < 14 + 28
                    || network_frame[..6] != station_address
                    || network_frame[12..14] != 0x0806_u16.to_be_bytes()
                {
                    return None;
                }
                let arp = &network_frame[14..];
                if arp[6..8] != 2_u16.to_be_bytes()
                    || arp[14..18] != STA_ARP_TARGET_IPV4
                    || arp[18..24] != station_address
                {
                    return None;
                }
                let mut source = [0_u8; 6];
                source.copy_from_slice(&arp[8..14]);
                Some(source)
            }) else {
                continue;
            };
            let pn = &frame[data.ccmp_header_offset..data.payload_offset];
            let _ = rx.stop(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-protected-rx \
                 protocol=arp source={:02x?} target={}.{}.{}.{} pn={pn:02x?} \
                 mpdu={} payload={} ethernet={} mic_in_dma={} owned_rx=true \
                 frames={received_frames} protected={protected_frames} mic_failures={mic_failures}",
                arp_source,
                STA_ARP_TARGET_IPV4[0],
                STA_ARP_TARGET_IPV4[1],
                STA_ARP_TARGET_IPV4[2],
                STA_ARP_TARGET_IPV4[3],
                data.mpdu.length,
                data.payload_length,
                ethernet_plan.ethernet_length,
                data.mic_present_in_dma,
            ));
            return true;
        }
        if let Err(error) = rx_ring.recycle_completed_half(mmio, |index| {
            // SAFETY: RxRingLive invokes this only for a fully completed,
            // detached half before republishing it to hardware.
            unsafe { rx_storage.buffers()[index].prepare_for_recycle() }
        }) {
            let _ = rx.stop(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-protected-rx-recycle \
                 error={error:?}"
            ));
            return false;
        }
        if rx_ring.all_observed() {
            let _ = rx.stop(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-protected-rx-recycle \
                 error=terminal-before-recycle"
            ));
            return false;
        }
        Timer::after_millis(1).await;
    }
    let _ = rx.stop(mmio);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-protected-rx error=timeout \
         frames={received_frames} protected={protected_frames} \
         addressed_protected={addressed_protected} mic_failures={mic_failures}"
    ));
    false
}

async fn report_network_configuration(stack: Stack<'_>) -> ! {
    for elapsed_ms in 0..15_000_u32 {
        if let Some(config) = stack.config_v4() {
            let local_ipv4 = config.address.address().octets();
            crate::console::publish_event(
                0,
                0,
                HilEvent::NetworkReady(NetworkInfo {
                    address: local_ipv4,
                    prefix_length: config.address.prefix_len(),
                    gateway: config.gateway.map(|address| address.octets()),
                }),
            );
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-dhcp \
                 address={} gateway={:?} dns={:?} elapsed_ms={elapsed_ms}",
                config.address, config.gateway, config.dns_servers,
            ));

            // Keep probe generation at the network/application boundary.
            // Sending one ordinary UDP datagram makes embassy-net resolve
            // the peer through ARP; the HIL RX observer below only records
            // that the matching reply crossed the production driver.
            OPEN_RADIO_LOCAL_IPV4.store(u32::from_be_bytes(local_ipv4), Ordering::Release);
            let mut probe_rx_metadata = [PacketMetadata::EMPTY; 1];
            let mut probe_rx_buffer = [0_u8; 1];
            let mut probe_tx_metadata = [PacketMetadata::EMPTY; 1];
            let mut probe_tx_buffer = [0_u8; 1];
            let mut probe_socket = UdpSocket::new(
                stack,
                &mut probe_rx_metadata,
                &mut probe_rx_buffer,
                &mut probe_tx_metadata,
                &mut probe_tx_buffer,
            );
            if let Err(error) = probe_socket.bind(4_325) {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=embassy-net-external-probe error=bind-{error:?}"
                ));
            } else if let Err(error) = probe_socket
                .send_to(&[0], (Ipv4Address::from_octets(LAN_PROBE_IPV4), 9))
                .await
            {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=embassy-net-external-probe error=send-{error:?}"
                ));
            }
            for _ in 0..5_000 {
                if OPEN_RADIO_LAN_PROBE_RESPONSE.load(Ordering::Acquire) {
                    // Give the network runner one scheduling interval to
                    // install the observed neighbor before reporting ready.
                    Timer::after_millis(10).await;
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS \
                         stage=embassy-net-external-probe-ready address={}",
                        config.address,
                    ));
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
                Timer::after_millis(1).await;
            }
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=embassy-net-external-probe-ready error=arp-prime-timeout \
                 address={}",
                config.address,
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
        Timer::after_millis(1).await;
    }

    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-dhcp error=timeout"
    ));
    loop {
        Timer::after_secs(60).await;
    }
}

// Keep one ordinary-code symbol alive so the host HIL can prove the runtime
// memory profile from periodic UART evidence. In the required
// psram-code-psram-data image its address is in 0x5000_0000..0x5100_0000; a
// directly linked app/Flash-XIP image reports 0x4000_0000..0x5000_0000.
#[inline(never)]
fn open_radio_runtime_code_marker() {}

#[unsafe(link_section = ".rwtext.open_radio_rx_hot")]
fn open_radio_rx_telemetry_now_micros() -> u64 {
    Instant::now().as_micros()
}

struct OpenRadioRxReloadDelay;

impl RxReloadDelay for OpenRadioRxReloadDelay {
    fn after_micros(&mut self, micros: u32) -> impl core::future::Future<Output = ()> + '_ {
        OPEN_RADIO_RX_RELOAD_DELAYS.fetch_add(1, Ordering::Relaxed);
        Timer::after_micros(u64::from(micros))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OpenRadioMacOrder {
    Backward,
    SameMpdu,
    Forward,
}

#[derive(Default)]
struct OpenRadioRxOrderTracker {
    udp_expected: Option<u32>,
    mac_expected: [Option<u16>; 16],
    last_mac: Option<(u8, u16)>,
}

impl OpenRadioRxOrderTracker {
    fn reset(&mut self) {
        self.udp_expected = None;
        self.mac_expected.fill(None);
        self.last_mac = None;
    }

    fn observe(&mut self, udp_sequence: i32, mac: Option<(u8, u16)>) {
        if udp_sequence < 0 {
            self.reset();
            return;
        }
        let udp_sequence = udp_sequence as u32;
        let mac_order = mac.map(|(tid, sequence)| self.observe_mac(tid, sequence));
        let previous_mac = self.last_mac;
        self.last_mac = mac;

        let Some(expected) = self.udp_expected else {
            self.udp_expected = Some(udp_sequence.saturating_add(1));
            return;
        };
        if udp_sequence == expected {
            self.udp_expected = Some(udp_sequence.saturating_add(1));
        } else if udp_sequence > expected {
            OPEN_RADIO_RX_ORDER_COUNTERS
                .gap_events
                .fetch_add(1, Ordering::Relaxed);
            OPEN_RADIO_RX_ORDER_COUNTERS
                .forward_missing
                .fetch_add(udp_sequence - expected, Ordering::Relaxed);
            self.udp_expected = Some(udp_sequence.saturating_add(1));
        } else if udp_sequence.saturating_add(1) == expected {
            OPEN_RADIO_RX_ORDER_COUNTERS
                .adjacent_duplicates
                .fetch_add(1, Ordering::Relaxed);
        } else {
            OPEN_RADIO_RX_ORDER_COUNTERS
                .backward
                .fetch_add(1, Ordering::Relaxed);
            match (previous_mac, mac, mac_order) {
                (Some((previous_tid, _)), Some((tid, _)), _) if previous_tid != tid => {
                    OPEN_RADIO_RX_ORDER_COUNTERS
                        .backward_mac_other_tid
                        .fetch_add(1, Ordering::Relaxed);
                }
                (_, Some(_), Some(OpenRadioMacOrder::Backward)) => {
                    OPEN_RADIO_RX_ORDER_COUNTERS
                        .backward_mac_backward
                        .fetch_add(1, Ordering::Relaxed);
                }
                (_, Some(_), Some(OpenRadioMacOrder::SameMpdu)) => {
                    OPEN_RADIO_RX_ORDER_COUNTERS
                        .backward_mac_same
                        .fetch_add(1, Ordering::Relaxed);
                }
                (_, Some(_), Some(OpenRadioMacOrder::Forward)) => {
                    OPEN_RADIO_RX_ORDER_COUNTERS
                        .backward_mac_forward
                        .fetch_add(1, Ordering::Relaxed);
                }
                _ => {
                    OPEN_RADIO_RX_ORDER_COUNTERS
                        .backward_mac_unavailable
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    fn observe_mac(&mut self, tid: u8, sequence: u16) -> OpenRadioMacOrder {
        if self.last_mac == Some((tid, sequence)) {
            return OpenRadioMacOrder::SameMpdu;
        }
        let expected = &mut self.mac_expected[usize::from(tid)];
        let Some(frontier) = *expected else {
            *expected = Some(sequence.wrapping_add(1) & 0x0fff);
            return OpenRadioMacOrder::Forward;
        };
        let distance = sequence.wrapping_sub(frontier) & 0x0fff;
        if distance < 0x0800 {
            *expected = Some(sequence.wrapping_add(1) & 0x0fff);
            OpenRadioMacOrder::Forward
        } else {
            OpenRadioMacOrder::Backward
        }
    }
}

/// HIL-only observer layered outside the production RX/backend boundary.
///
/// The driver still publishes ordinary Ethernet and owned control events
/// without knowing about diagnostics. This observer only records that the
/// application-level LAN probe's ARP reply crossed the RX handoff, then
/// forwards the same event to the production control mailbox.
struct HilConnectedRxObserver<S> {
    control: S,
    station_address: [u8; 6],
    phy_sample_cursor: u8,
    order: OpenRadioRxOrderTracker,
}

impl<S: ConnectedRxSink> ConnectedRxSink for HilConnectedRxObserver<S> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { frame, raw, .. } = event {
            if OPEN_RADIO_RX_ORDER_TELEMETRY
                && let Some(sequence) = ipv4_udp_sequence(frame, OPEN_RADIO_UDP_RX_PORT)
            {
                self.order.observe(sequence, public_qos_sequence(raw));
            }
            let local_ipv4 = OPEN_RADIO_LOCAL_IPV4.load(Ordering::Acquire).to_be_bytes();
            let is_probe_reply = frame.destination == self.station_address
                && frame.ether_type == 0x0806
                && frame.payload.len() >= 28
                && frame.payload[6..8] == 2_u16.to_be_bytes()
                && frame.payload[14..18] == LAN_PROBE_IPV4
                && frame.payload[18..24] == self.station_address
                && frame.payload[24..28] == local_ipv4;
            if is_probe_reply {
                OPEN_RADIO_LAN_PROBE_RESPONSE.store(true, Ordering::Release);
            }
            let sample_phy = self.phy_sample_cursor == 0;
            self.phy_sample_cursor = self.phy_sample_cursor.wrapping_add(1) & 63;
            if sample_phy
                && ipv4_udp_destination_port(frame) == Some(OPEN_RADIO_UDP_RX_PORT)
                && let Some(phy) = decode_rx_phy_info(raw)
            {
                OPEN_RADIO_RX_LAST_UDP_FORMAT
                    .store(u32::from(phy.baseband_format().raw()), Ordering::Relaxed);
                let mut packed = u32::from(phy.baseband_format().raw())
                    | (u32::from(phy.rate) << 4);
                if let Some(signal) = phy.he_su_signal() {
                    let bandwidth = match signal.bandwidth.mhz() {
                        20 => 0,
                        40 => 1,
                        80 => 2,
                        _ => 3,
                    };
                    packed |= (1 << 31)
                        | (u32::from(signal.mcs) << 9)
                        | (u32::from(signal.guard_interval_and_ltf.encoding()) << 13)
                        | (bandwidth << 15)
                        | (u32::from(signal.dcm) << 17)
                        | (u32::from(signal.ldpc) << 18);
                    if let Some(counter) = OPEN_RADIO_RX_PHY_COUNTERS
                        .he_mcs
                        .get(usize::from(signal.mcs))
                    {
                        counter.fetch_add(1, Ordering::Relaxed);
                    } else {
                        OPEN_RADIO_RX_PHY_COUNTERS
                            .other
                            .fetch_add(1, Ordering::Relaxed);
                    }
                } else {
                    OPEN_RADIO_RX_PHY_COUNTERS
                        .other
                        .fetch_add(1, Ordering::Relaxed);
                }
                OPEN_RADIO_RX_LAST_UDP_PHY.store(packed, Ordering::Relaxed);
            }
        }
        // Never print from this synchronous callback. In particular, the
        // former BlockAck observation spent roughly 15.5 ms in the UART
        // logger while the staging lease remained owned, manufacturing the
        // apparent long protocol-dispatch tail reported by HIL.
        self.control.publish(event);
    }
}

fn ipv4_udp_destination_port(
    frame: open_esp_radio::wifi::ieee80211::data::EthernetFrameParts<'_>,
) -> Option<u16> {
    if frame.ether_type != 0x0800 {
        return None;
    }
    let version_and_ihl = *frame.payload.first()?;
    if version_and_ihl >> 4 != 4 || *frame.payload.get(9)? != 17 {
        return None;
    }
    let header_length = usize::from(version_and_ihl & 0x0f).checked_mul(4)?;
    if header_length < 20 {
        return None;
    }
    Some(u16::from_be_bytes([
        *frame.payload.get(header_length + 2)?,
        *frame.payload.get(header_length + 3)?,
    ]))
}

fn ipv4_udp_sequence(
    frame: open_esp_radio::wifi::ieee80211::data::EthernetFrameParts<'_>,
    destination_port: u16,
) -> Option<i32> {
    if ipv4_udp_destination_port(frame) != Some(destination_port) {
        return None;
    }
    let header_length = usize::from(*frame.payload.first()? & 0x0f).checked_mul(4)?;
    let sequence_offset = header_length.checked_add(8)?;
    let encoded: [u8; 4] = frame
        .payload
        .get(sequence_offset..sequence_offset + 4)?
        .try_into()
        .ok()?;
    Some(i32::from_be_bytes(encoded))
}

fn public_qos_sequence(raw: &[u8]) -> Option<(u8, u16)> {
    const DATA_TYPE: u16 = 0x0008;
    const DATA_TYPE_MASK: u16 = 0x000c;
    const QOS_SUBTYPE: u16 = 0x0080;
    const TO_FROM_DS: u16 = 0x0300;

    let frame_offset = PUBLIC_HEADER_SIZE;
    let frame_control = u16::from_le_bytes([
        *raw.get(frame_offset)?,
        *raw.get(frame_offset + 1)?,
    ]);
    if frame_control & (DATA_TYPE_MASK | QOS_SUBTYPE) != DATA_TYPE | QOS_SUBTYPE {
        return None;
    }
    let sequence_control = u16::from_le_bytes([
        *raw.get(frame_offset + 22)?,
        *raw.get(frame_offset + 23)?,
    ]);
    let qos_offset = frame_offset + 24 + usize::from(frame_control & TO_FROM_DS == TO_FROM_DS) * 6;
    let tid = *raw.get(qos_offset)? & 0x0f;
    Some((tid, sequence_control >> 4))
}

fn iperf2_udp_sequence(packet: &[u8]) -> Option<i32> {
    let encoded: [u8; 4] = packet.get(..4)?.try_into().ok()?;
    Some(i32::from_be_bytes(encoded))
}

#[derive(Default)]
struct OpenRadioUdpSequenceEvidence {
    first: Option<u32>,
    highest: u32,
    expected: u32,
    gap_events: u32,
    forward_missing: u32,
    maximum_gap: u32,
    maximum_gap_at: Option<u32>,
    first_gap_at: Option<u32>,
    last_gap_at: Option<u32>,
    backward: u32,
    adjacent_duplicates: u32,
    unsequenced: u32,
    maximum_interarrival_micros: u32,
    maximum_interarrival_at: Option<u32>,
}

impl OpenRadioUdpSequenceEvidence {
    fn observe(&mut self, sequence: Option<i32>) {
        let Some(sequence) = sequence.filter(|sequence| *sequence >= 0).map(|value| value as u32)
        else {
            self.unsequenced = self.unsequenced.saturating_add(1);
            return;
        };
        let Some(_) = self.first else {
            self.first = Some(sequence);
            self.highest = sequence;
            self.expected = sequence.saturating_add(1);
            return;
        };
        if sequence == self.expected {
            self.highest = sequence;
            self.expected = sequence.saturating_add(1);
        } else if sequence > self.expected {
            let gap = sequence - self.expected;
            self.gap_events = self.gap_events.saturating_add(1);
            self.forward_missing = self.forward_missing.saturating_add(gap);
            if gap > self.maximum_gap {
                self.maximum_gap = gap;
                self.maximum_gap_at = Some(sequence);
            }
            self.first_gap_at.get_or_insert(sequence);
            self.last_gap_at = Some(sequence);
            self.highest = sequence;
            self.expected = sequence.saturating_add(1);
        } else if sequence.saturating_add(1) == self.expected {
            self.adjacent_duplicates = self.adjacent_duplicates.saturating_add(1);
        } else {
            self.backward = self.backward.saturating_add(1);
        }
    }

    fn observe_interarrival(&mut self, sequence: Option<i32>, elapsed_micros: u64) {
        let Some(sequence) = sequence.filter(|sequence| *sequence >= 0).map(|value| value as u32)
        else {
            return;
        };
        let elapsed_micros = elapsed_micros.min(u64::from(u32::MAX)) as u32;
        if elapsed_micros > self.maximum_interarrival_micros {
            self.maximum_interarrival_micros = elapsed_micros;
            self.maximum_interarrival_at = Some(sequence);
        }
    }
}

async fn run_open_radio_udp_benchmark(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
) -> ! {
    if OPEN_RADIO_TCP_RX_BENCH {
        run_open_radio_tcp_rx_benchmark(stack, registers).await
    } else if OPEN_RADIO_RAW_MAC_BENCH {
        loop {
            Timer::after_secs(60).await;
        }
    } else if OPEN_RADIO_BIDIRECTIONAL_BENCH {
        match select(
            run_open_radio_bidirectional_session_coordinator(),
            select(
                run_open_radio_udp_tx_benchmark(stack, association_phy, data_tx_rate),
                run_open_radio_bidirectional_rx_benchmark(
                    stack,
                    association_phy,
                    data_tx_rate,
                    registers,
                ),
            ),
        )
        .await {}
    } else if option_env!("OPEN_RADIO_TX_BENCH").is_some() {
        run_open_radio_udp_tx_benchmark(stack, association_phy, data_tx_rate).await
    } else {
        run_open_radio_udp_rx_benchmark(stack, association_phy, data_tx_rate, registers).await
    }
}

async fn run_open_radio_tcp_rx_benchmark(
    stack: Stack<'static>,
    registers: &RefCell<&mut RadioRegisters>,
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

    let rx_buffer = OPEN_RADIO_TCP_RX_BUFFER.init([0; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY]);
    let tx_buffer = OPEN_RADIO_TCP_TX_BUFFER.init([0; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY]);
    let read_buffer = OPEN_RADIO_TCP_READ_BUFFER.init([0; OPEN_RADIO_TCP_READ_CAPACITY]);
    let mut socket = TcpSocket::new(stack, rx_buffer, tx_buffer);
    crate::console::publish_event(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            transport: HilTransport::Tcp,
            direction: HilDirection::Rx,
            local_port: OPEN_RADIO_TCP_RX_PORT,
            maximum_payload_bytes: OPEN_RADIO_TCP_CHUNK_CAPACITY as u16,
        }),
    );
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-rx-ready port={OPEN_RADIO_TCP_RX_PORT} \
         receive_buffer={OPEN_RADIO_TCP_RX_BUFFER_CAPACITY} \
         read_capacity={OPEN_RADIO_TCP_READ_CAPACITY} runtime_session=1"
    ));

    loop {
        let session = crate::console::receive_session_start().await;
        let flow = session
            .config
            .target_rx
            .expect("validated TCP RX session carries a target RX flow");
        let duration_millis = match session.config.completion {
            HilCompletion::DurationMillis(duration) => duration,
            HilCompletion::TransferBytes(_) | HilCompletion::HostStop => {
                unreachable!("protocol owner accepts only duration-completed sessions")
            }
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-rx-session-start session={} \
             chunk={} duration_ms={} offered_bps={:?}",
            session.session_id,
            flow.payload_bytes,
            duration_millis,
            flow.offered_rate_bps,
        ));

        let hardware_start = registers.borrow().rx_statistics_snapshot().primary;
        let enqueue_start = OPEN_RADIO_RX_ENQUEUE_COUNTERS.snapshot();
        let accept_timeout =
            Duration::from_millis(u64::from(duration_millis)) + Duration::from_secs(5);
        let mut bytes = 0_u64;
        let mut read_errors = 0_u32;
        let mut eof = false;
        let accepted = matches!(
            with_timeout(accept_timeout, socket.accept(OPEN_RADIO_TCP_RX_PORT)).await,
            Ok(Ok(()))
        );
        let started = Instant::now();
        if accepted {
            loop {
                match with_timeout(OPEN_RADIO_TCP_IDLE_TIMEOUT, socket.read(read_buffer)).await {
                    Ok(Ok(0)) => {
                        eof = true;
                        break;
                    }
                    Ok(Ok(length)) => bytes = bytes.saturating_add(length as u64),
                    Ok(Err(_)) | Err(_) => {
                        read_errors = read_errors.saturating_add(1);
                        break;
                    }
                }
            }
        } else {
            read_errors = read_errors.saturating_add(1);
        }
        let elapsed_us = started.elapsed().as_micros().max(1);
        socket.abort();

        let hardware_delta = registers
            .borrow()
            .rx_statistics_snapshot()
            .primary
            .wrapping_delta_since(hardware_start);
        let enqueue_end = OPEN_RADIO_RX_ENQUEUE_COUNTERS.snapshot();
        let enqueued = enqueue_end.enqueued.wrapping_sub(enqueue_start.enqueued);
        let queue_dropped = enqueue_end.dropped.wrapping_sub(enqueue_start.dropped);
        let health_errors = u32::from(hardware_delta.buffer_full)
            .saturating_add(u32::from(hardware_delta.fifo_overflow))
            .saturating_add(queue_dropped);
        let transport_errors = read_errors.saturating_add(health_errors);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        emergency_log(format_args!(
            "OTCPRX b={bytes} s={} u={elapsed_us} k={throughput_kbps} e={transport_errors} \
             bf={} fo={} enq={enqueued} drop={queue_dropped} eof={}",
            u8::from(accepted),
            hardware_delta.buffer_full,
            hardware_delta.fifo_overflow,
            u8::from(eof),
        ));
        let passed = accepted && eof && bytes != 0 && transport_errors == 0;
        crate::console::complete_session(
            session.session_id,
            TransportEvidence {
                rx_bytes: bytes,
                tx_bytes: 0,
                rx_units: u64::from(accepted && eof),
                tx_units: 0,
                elapsed_micros: elapsed_us,
                transport_errors,
            },
            passed,
        )
        .await;
    }
}

async fn run_open_radio_bidirectional_session_coordinator() -> ! {
    loop {
        let session = crate::console::receive_session_start().await;
        OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS.send(session).await;
        OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS.send(session).await;

        let first = OPEN_RADIO_BIDIRECTIONAL_RESULTS.receive().await;
        let second = OPEN_RADIO_BIDIRECTIONAL_RESULTS.receive().await;
        let valid_pair = first.session_id == session.session_id
            && second.session_id == session.session_id
            && first.direction != second.direction;
        let evidence = TransportEvidence {
            rx_bytes: first.evidence.rx_bytes.saturating_add(second.evidence.rx_bytes),
            tx_bytes: first.evidence.tx_bytes.saturating_add(second.evidence.tx_bytes),
            rx_units: first.evidence.rx_units.saturating_add(second.evidence.rx_units),
            tx_units: first.evidence.tx_units.saturating_add(second.evidence.tx_units),
            elapsed_micros: first
                .evidence
                .elapsed_micros
                .max(second.evidence.elapsed_micros),
            transport_errors: first
                .evidence
                .transport_errors
                .saturating_add(second.evidence.transport_errors)
                .saturating_add(u32::from(!valid_pair)),
        };
        crate::console::complete_session(
            session.session_id,
            evidence,
            valid_pair && first.passed && second.passed,
        )
        .await;
    }
}

async fn complete_open_radio_bidirectional_direction(
    session_id: u64,
    direction: OpenRadioBidirectionalDirection,
    evidence: TransportEvidence,
    passed: bool,
) {
    OPEN_RADIO_BIDIRECTIONAL_RESULTS
        .send(OpenRadioBidirectionalResult {
            session_id,
            direction,
            evidence,
            passed,
        })
        .await;
}

/// Device-to-host UDP load through Embassy and the open TX scheduler.
async fn run_open_radio_udp_tx_benchmark(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

    let rx_metadata =
        OPEN_RADIO_UDP_RX_METADATA.init([PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]);
    let rx_buffer = OPEN_RADIO_UDP_RX_BUFFER
        .init([0; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
    let tx_metadata =
        OPEN_RADIO_UDP_TX_METADATA.init([PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]);
    let tx_buffer = OPEN_RADIO_UDP_TX_BUFFER
        .init([0; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
    let packet = OPEN_RADIO_UDP_PACKET.init([0x5a; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
    let mut socket = UdpSocket::new(stack, rx_metadata, rx_buffer, tx_metadata, tx_buffer);
    if let Err(error) = socket.bind(4_324) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=udp-tx-bind error={error:?}"
        ));
        loop {
            Timer::after_secs(60).await;
        }
    }
    // Complete the connected-data-path settle before advertising readiness.
    // `Start` must mean the benchmark task can consume its session without a
    // hidden post-acceptance delay.
    Timer::after_secs(1).await;
    crate::console::publish_event(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            transport: HilTransport::Udp,
            direction: HilDirection::Tx,
            local_port: 4_324,
            maximum_payload_bytes: OPEN_RADIO_UDP_PAYLOAD_CAPACITY as u16,
        }),
    );
    if OPEN_RADIO_RUNTIME_TX_SESSIONS || OPEN_RADIO_RUNTIME_BIDIRECTIONAL_SESSIONS {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
             source_port=4324 queue={OPEN_RADIO_UDP_TX_QUEUE_DEPTH} \
             payload_capacity={OPEN_RADIO_UDP_PAYLOAD_CAPACITY} \
             tx_mode=ampdu runtime_session=1 \
             rate_code={:#04x} rate_kbps={}",
            data_tx_rate.code(),
            data_tx_rate.nominal_kbps(),
        ));
    } else {
        let server = Ipv4Address::from_octets(OPEN_RADIO_TX_BENCH_TARGET_IPV4);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
             target={server}:{OPEN_RADIO_UDP_TX_BENCH_PORT} \
             queue={OPEN_RADIO_UDP_TX_QUEUE_DEPTH} payload={OPEN_RADIO_UDP_PAYLOAD_CAPACITY} \
             tx_mode=ampdu \
             offered_tx_kbps={OPEN_RADIO_TX_BENCH_RATE_KBPS:?} \
             rate_code={:#04x} rate_kbps={}",
            data_tx_rate.code(),
            data_tx_rate.nominal_kbps(),
        ));
    }
    loop {
        let session = if OPEN_RADIO_RUNTIME_TX_SESSIONS {
            Some(crate::console::receive_session_start().await)
        } else if OPEN_RADIO_RUNTIME_BIDIRECTIONAL_SESSIONS {
            Some(OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS.receive().await)
        } else {
            None
        };
        let (server, server_port, payload_bytes, duration, offered_rate_bps) =
            if let Some(session) = session {
                let peer = session
                    .config
                    .peer
                    .expect("validated TX session carries a peer");
                let flow = session
                    .config
                    .target_tx
                    .expect("validated TX session carries a target TX flow");
                let duration_millis = match session.config.completion {
                    HilCompletion::DurationMillis(duration) => duration,
                    HilCompletion::TransferBytes(_) | HilCompletion::HostStop => {
                        unreachable!("protocol owner accepts only duration-completed sessions")
                    }
                };
                (
                    Ipv4Address::from_octets(peer.address),
                    peer.port,
                    usize::from(flow.payload_bytes),
                    Duration::from_millis(u64::from(duration_millis)),
                    flow.offered_rate_bps,
                )
            } else {
                (
                    Ipv4Address::from_octets(OPEN_RADIO_TX_BENCH_TARGET_IPV4),
                    OPEN_RADIO_UDP_TX_BENCH_PORT,
                    OPEN_RADIO_UDP_PAYLOAD_CAPACITY,
                    OPEN_RADIO_UDP_TX_BENCH_DURATION,
                    OPEN_RADIO_TX_BENCH_RATE_KBPS.map(|rate| rate.saturating_mul(1_000)),
                )
            };
        if let Some(session) = session {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-session-start \
                 session={} target={server}:{server_port} payload={payload_bytes} \
                 duration_ms={} offered_bps={offered_rate_bps:?}",
                session.session_id,
                duration.as_millis(),
            ));
        }
        let started = Instant::now();
        let aggregate_start = (!OPEN_RADIO_BIDIRECTIONAL_BENCH)
            .then(|| OPEN_RADIO_TX_AGGREGATE_COUNTERS.snapshot());
        let mut next_send = started;
        let mut bytes = 0_u64;
        let mut datagrams = 0_u64;
        let mut send_errors = 0_u32;
        while started.elapsed() < duration {
            packet[..4].copy_from_slice(&(datagrams as u32).to_be_bytes());
            match socket
                .send_to(&packet[..payload_bytes], (server, server_port))
                .await
            {
                Ok(()) => {
                    bytes = bytes.saturating_add(payload_bytes as u64);
                    datagrams = datagrams.saturating_add(1);
                }
                Err(_) => send_errors = send_errors.saturating_add(1),
            }
            if let Some(rate_bps) = offered_rate_bps {
                // Pace absolute microsecond deadlines so a temporarily
                // blocking network queue does not produce a compensating
                // burst after it becomes writable.
                let interval_us = (payload_bytes as u64)
                    .saturating_mul(8_000_000)
                    .saturating_add(rate_bps - 1)
                    / rate_bps;
                next_send += Duration::from_micros(interval_us);
                let now = Instant::now();
                if now < next_send {
                    Timer::at(next_send).await;
                } else {
                    next_send = now;
                }
            }
        }
        let elapsed_us = started.elapsed().as_micros().max(1);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        // UDP enqueue completion precedes MAC acknowledgement. Keep draining
        // outside the measured interval so the structured result cannot race
        // the final network queue and A-MPDU exchange.
        if session.is_some() {
            Timer::after(OPEN_RADIO_UDP_TX_DRAIN).await;
        }
        // Publish one compact bounded record synchronously per five-second
        // interval. The asynchronous logger repeatedly truncated this record
        // after `OTX b=` under sustained TX, whereas the same emergency path
        // already carries the compact ORX/ORXP records without starving RX.
        // This is HIL evidence, not a per-packet data-path log.
        emergency_log(format_args!(
            "OTX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} \
             e={send_errors} p={} w={} r={} g={} x={} l={} a={}",
            offered_rate_bps.unwrap_or(0) / 1_000,
            association_phy.bandwidth_mhz(),
            data_tx_rate.nominal_kbps(),
            match data_tx_rate {
                TxPhyRate::He(rate) => rate.guard_interval_and_ltf().encoding(),
                TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
            },
            match data_tx_rate {
                TxPhyRate::He(rate) => rate.is_dcm() as u8,
                TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
            },
            match data_tx_rate {
                TxPhyRate::He(rate) => rate.is_ldpc() as u8,
                TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
            },
            open_radio_runtime_code_marker as *const () as usize,
        ));
        if let Some(aggregate_start) = aggregate_start {
            log_open_radio_ampdu_interval(aggregate_start);
        }
        if let Some(session) = session {
            let evidence = TransportEvidence {
                rx_bytes: 0,
                tx_bytes: bytes,
                rx_units: 0,
                tx_units: datagrams,
                elapsed_micros: elapsed_us,
                transport_errors: send_errors,
            };
            if OPEN_RADIO_RUNTIME_BIDIRECTIONAL_SESSIONS {
                complete_open_radio_bidirectional_direction(
                    session.session_id,
                    OpenRadioBidirectionalDirection::Tx,
                    evidence,
                    send_errors == 0,
                )
                .await;
            } else {
                crate::console::complete_session(
                    session.session_id,
                    evidence,
                    send_errors == 0,
                )
                .await;
            }
        } else {
            Timer::after_secs(2).await;
        }
    }
}

fn log_open_radio_ampdu_interval(earlier: AggregateTxCounterSnapshot) {
    let aggregate = OPEN_RADIO_TX_AGGREGATE_COUNTERS
        .snapshot()
        .wrapping_delta_since(earlier);
    let aggregate_min = aggregate.minimum_prepared_subframes().unwrap_or(0);
    let aggregate_max = aggregate.maximum_prepared_subframes().unwrap_or(0);
    emergency_log(format_args!(
        "OAMP aggregates={} publications={} completed={} subframes={} \
         acknowledged={} single={} single_rate={} single_ba={} single_pair={} \
         single_capacity={} single_capacity_max_len={} individual_retry={} timeout={} collision={} \
         min={} max={} stop_frame={} stop_capacity={} stop_empty={}",
        aggregate.aggregates_prepared,
        aggregate.aggregate_publications,
        aggregate.aggregates_completed,
        aggregate.prepared_subframe_total(),
        aggregate.subframes_acknowledged,
        aggregate.network_single_mpdu_started,
        aggregate.network_single_legacy_rate,
        aggregate.network_single_block_ack_unavailable,
        aggregate.network_single_ht_needs_pair,
        aggregate.network_single_fresh_aggregate_capacity,
        aggregate.network_single_fresh_capacity_lifetime_max_ethernet_length,
        aggregate.individual_retries,
        aggregate.hardware_timeouts,
        aggregate.collisions,
        aggregate_min,
        aggregate_max,
        aggregate.stopped_at_frame_limit,
        aggregate.stopped_at_capacity_limit,
        aggregate.stopped_on_empty_queue,
    ));
    emergency_log(format_args!(
        "OAMPH one={} two_three={} four_seven={} eight_fifteen={} \
         sixteen_twentythree={} twentyfour_thirty={} thirtyone={} full32={}",
        aggregate.prepared_in_range(1, 1),
        aggregate.prepared_in_range(2, 3),
        aggregate.prepared_in_range(4, 7),
        aggregate.prepared_in_range(8, 15),
        aggregate.prepared_in_range(16, 23),
        aggregate.prepared_in_range(24, 30),
        aggregate.prepared_in_range(31, 31),
        aggregate.prepared_in_range(32, 32),
    ));
    emergency_log(format_args!(
        "OAMPT preparation_us={} preparation_max_us={} publication_us={} \
         publication_max_us={} exchange_us={} exchange_max_us={}",
        aggregate.preparation_micros,
        aggregate.preparation_lifetime_max_micros,
        aggregate.publication_program_micros,
        aggregate.publication_program_lifetime_max_micros,
        aggregate.exchange_micros,
        aggregate.exchange_lifetime_max_micros,
    ));
}

/// Host-to-device UDP throughput baseline for the fully open data path.
///
/// A sample starts on the first payload datagram and closes after a quiet
/// interval. The first four bytes are interpreted only to discard iperf2's
/// negative terminal/report datagrams; ordinary UDP payloads remain valid.
async fn run_open_radio_udp_rx_benchmark(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

    let rx_metadata =
        OPEN_RADIO_UDP_RX_METADATA.init([PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]);
    let rx_buffer = OPEN_RADIO_UDP_RX_BUFFER
        .init([0; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
    let tx_metadata =
        OPEN_RADIO_UDP_TX_METADATA.init([PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]);
    let tx_buffer = OPEN_RADIO_UDP_TX_BUFFER
        .init([0; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
    run_open_radio_udp_rx_benchmark_with_buffers(
        stack,
        association_phy,
        data_tx_rate,
        registers,
        rx_metadata,
        rx_buffer,
        tx_metadata,
        tx_buffer,
        OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH,
    )
    .await
}

async fn run_open_radio_bidirectional_rx_benchmark(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
) -> ! {
    let rx_metadata = OPEN_RADIO_BIDIRECTIONAL_RX_METADATA
        .init([PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH]);
    let rx_buffer = OPEN_RADIO_BIDIRECTIONAL_RX_BUFFER
        .init([0; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
    let tx_metadata = OPEN_RADIO_BIDIRECTIONAL_TX_METADATA
        .init([PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH]);
    let tx_buffer = OPEN_RADIO_BIDIRECTIONAL_TX_BUFFER
        .init([0; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
    run_open_radio_udp_rx_benchmark_with_buffers(
        stack,
        association_phy,
        data_tx_rate,
        registers,
        rx_metadata,
        rx_buffer,
        tx_metadata,
        tx_buffer,
        OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH,
    )
    .await
}

async fn run_open_radio_udp_rx_benchmark_with_buffers(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
    rx_metadata: &'static mut [PacketMetadata],
    rx_buffer: &'static mut [u8],
    tx_metadata: &'static mut [PacketMetadata],
    tx_buffer: &'static mut [u8],
    rx_queue_depth: usize,
) -> ! {
    let mut socket = UdpSocket::new(stack, rx_metadata, rx_buffer, tx_metadata, tx_buffer);
    if let Err(error) = socket.bind(OPEN_RADIO_UDP_RX_PORT) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=udp-rx-bind \
             port={OPEN_RADIO_UDP_RX_PORT} error={error:?}"
        ));
        loop {
            Timer::after_secs(60).await;
        }
    }
    crate::console::publish_event(
        0,
        0,
        HilEvent::ServiceReady(ServiceInfo {
            transport: HilTransport::Udp,
            direction: HilDirection::Rx,
            local_port: OPEN_RADIO_UDP_RX_PORT,
            maximum_payload_bytes: OPEN_RADIO_UDP_PAYLOAD_CAPACITY as u16,
        }),
    );
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=udp-rx-ready \
         port={OPEN_RADIO_UDP_RX_PORT} queue={rx_queue_depth} \
         payload_capacity={OPEN_RADIO_UDP_PAYLOAD_CAPACITY} \
         bandwidth_mhz={} phy={} rate_code={:#04x} rate_kbps={}",
        association_phy.bandwidth_mhz(),
        association_phy.name(),
        data_tx_rate.code(),
        data_tx_rate.nominal_kbps(),
    ));

    let mut last_radio_handoff = Instant::now();
    loop {
        let session = if OPEN_RADIO_RUNTIME_RX_SESSIONS {
            Some(crate::console::receive_session_start().await)
        } else if OPEN_RADIO_RUNTIME_BIDIRECTIONAL_SESSIONS {
            Some(OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS.receive().await)
        } else {
            None
        };
        // Close the previous benchmark poll before taking interval baselines.
        // In particular, synchronous UART evidence from a readiness probe
        // must not be charged to the following sustained traffic interval.
        yield_now().await;
        OPEN_RADIO_RX_LAST_UDP_FORMAT.store(u32::MAX, Ordering::Relaxed);
        OPEN_RADIO_RX_LAST_UDP_PHY.store(u32::MAX, Ordering::Relaxed);
        let hardware_start = registers.borrow().rx_statistics_snapshot().primary;
        let phy_start = OPEN_RADIO_RX_PHY_COUNTERS.snapshot();
        let order_start = OPEN_RADIO_RX_ORDER_COUNTERS.snapshot();
        let pipeline_start = OPEN_RADIO_RX_PIPELINE_COUNTERS.snapshot();
        let task_poll_start = OPEN_RADIO_TASK_POLLS.snapshot();
        let enqueue_start = OPEN_RADIO_RX_ENQUEUE_COUNTERS.snapshot();
        let reload_delay_start = OPEN_RADIO_RX_RELOAD_DELAYS.load(Ordering::Relaxed);
        let irq_start = OPEN_RADIO_IRQ_RUNTIME.rx_post_count();
        let irq_entry_start = OPEN_RADIO_MAC_IRQ_ENTRIES.load(Ordering::Relaxed);
        let irq_classification_start = OPEN_RADIO_MAC_IRQ_CLASSIFICATION.snapshot();
        let _ = OPEN_RADIO_MAC_IRQ_CLASSIFICATION.take_auxiliary_status_or();
        let _ = OPEN_RADIO_MAC_IRQ_CLASSIFICATION.take_unknown_status_or();
        let (first_length, first_sequence) = loop {
            // The benchmark only needs the datagram length and four-byte
            // sequence. Consuming in the UDP ring avoids copying every full
            // payload into a second PSRAM buffer merely to inspect those
            // fields.
            let received = socket
                .recv_from_with(|packet, _| (packet.len(), iperf2_udp_sequence(packet)))
                .await;
            yield_to_pending_radio_rx(&mut last_radio_handoff).await;
            let (length, sequence) = received;
            if sequence.is_some_and(|sequence| sequence < 0) {
                continue;
            }
            break (length, sequence);
        };
        let aggregate_start = OPEN_RADIO_BIDIRECTIONAL_BENCH
            .then(|| OPEN_RADIO_TX_AGGREGATE_COUNTERS.snapshot());
        let started = Instant::now();
        let mut last_packet = started;
        let mut bytes = first_length as u64;
        let mut datagrams = 1_u64;
        let expected_payload_bytes = session.map(|session| {
            usize::from(
                session
                    .config
                    .target_rx
                    .expect("validated RX session carries a target RX flow")
                    .payload_bytes,
            )
        });
        let mut receive_errors = u32::from(
            expected_payload_bytes.is_some_and(|expected| first_length != expected),
        );
        let mut terminal_seen = false;
        let mut sequence_evidence = OpenRadioUdpSequenceEvidence::default();
        sequence_evidence.observe(first_sequence);

        loop {
            let received = with_timeout(
                OPEN_RADIO_UDP_RX_IDLE,
                socket.recv_from_with(|packet, _| (packet.len(), iperf2_udp_sequence(packet))),
            )
            .await;
            yield_to_pending_radio_rx(&mut last_radio_handoff).await;
            match received {
                Ok((length, sequence)) => {
                    if sequence.is_some_and(|sequence| sequence < 0) {
                        terminal_seen = true;
                        break;
                    }
                    receive_errors = receive_errors.saturating_add(u32::from(
                        expected_payload_bytes.is_some_and(|expected| length != expected),
                    ));
                    let received_at = Instant::now();
                    sequence_evidence.observe(sequence);
                    sequence_evidence.observe_interarrival(
                        sequence,
                        received_at.duration_since(last_packet).as_micros(),
                    );
                    bytes = bytes.saturating_add(length as u64);
                    datagrams = datagrams.saturating_add(1);
                    last_packet = received_at;
                }
                Err(_) => break,
            }
        }

        let elapsed_us = last_packet.duration_since(started).as_micros().max(1);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        let hardware_delta = registers
            .borrow()
            .rx_statistics_snapshot()
            .primary
            .wrapping_delta_since(hardware_start);
        let enqueue_end = OPEN_RADIO_RX_ENQUEUE_COUNTERS.snapshot();
        let enqueued = enqueue_end.enqueued.wrapping_sub(enqueue_start.enqueued);
        let queue_dropped = enqueue_end.dropped.wrapping_sub(enqueue_start.dropped);
        let reload_delays = OPEN_RADIO_RX_RELOAD_DELAYS
            .load(Ordering::Relaxed)
            .wrapping_sub(reload_delay_start);
        let rx_irqs = OPEN_RADIO_IRQ_RUNTIME
            .rx_post_count()
            .wrapping_sub(irq_start);
        let irq_entries = OPEN_RADIO_MAC_IRQ_ENTRIES
            .load(Ordering::Relaxed)
            .wrapping_sub(irq_entry_start);
        let irq_classification = OPEN_RADIO_MAC_IRQ_CLASSIFICATION
            .snapshot()
            .wrapping_delta_since(irq_classification_start);
        let irq_auxiliary_status_or =
            OPEN_RADIO_MAC_IRQ_CLASSIFICATION.take_auxiliary_status_or();
        let irq_unknown_status_or = OPEN_RADIO_MAC_IRQ_CLASSIFICATION.take_unknown_status_or();
        let rx_format = OPEN_RADIO_RX_LAST_UDP_FORMAT.load(Ordering::Relaxed);
        let rx_phy = OPEN_RADIO_RX_LAST_UDP_PHY.load(Ordering::Relaxed);
        let rx_he_valid = rx_phy >> 31;
        let rx_rate = (rx_phy >> 4) & 0x1f;
        let rx_mcs = (rx_phy >> 9) & 0x0f;
        let rx_gi_ltf = (rx_phy >> 13) & 0x03;
        let rx_bandwidth_mhz = 20_u32 << ((rx_phy >> 15) & 0x03);
        let rx_dcm = (rx_phy >> 17) & 1;
        let rx_ldpc = (rx_phy >> 18) & 1;
        let phy_end = OPEN_RADIO_RX_PHY_COUNTERS.snapshot();
        let rx_mcs_histogram = core::array::from_fn::<_, OPEN_RADIO_RX_HE_MCS_BUCKETS, _>(|index| {
            phy_end.0[index].wrapping_sub(phy_start.0[index])
        });
        let rx_other_phy = phy_end.1.wrapping_sub(phy_start.1);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx \
             bytes={bytes} datagrams={datagrams} elapsed_us={elapsed_us} \
             throughput_kbps={throughput_kbps} receive_errors={receive_errors} \
             terminal={} bandwidth_mhz={} phy={} \
             rate_code={:#04x} rate_kbps={} code_address={}",
            u8::from(terminal_seen),
            association_phy.bandwidth_mhz(),
            association_phy.name(),
            data_tx_rate.code(),
            data_tx_rate.nominal_kbps(),
            open_radio_runtime_code_marker as *const () as usize,
        ));
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-path \
             mpdu={} data_success={} fcs_error={} buffer_full={} fifo_overflow={} \
             enqueued={enqueued} queue_dropped={queue_dropped} rx_irqs={rx_irqs} \
             reload_delays={reload_delays} rx_format={rx_format} rx_rate={rx_rate} \
             rx_he_valid={rx_he_valid} rx_mcs={rx_mcs} rx_gi_ltf={rx_gi_ltf} \
             rx_bandwidth_mhz={rx_bandwidth_mhz} rx_dcm={rx_dcm} rx_ldpc={rx_ldpc}",
            hardware_delta.mpdu_count,
            hardware_delta.data_success,
            hardware_delta.fcs_error,
            hardware_delta.buffer_full,
            hardware_delta.fifo_overflow,
        ));
        emergency_log(format_args!(
            "ORXQ first={} highest={} next={} gap_events={} forward_missing={} \
             maximum_gap={} maximum_gap_at={} first_gap_at={} last_gap_at={} backward={} \
             adjacent_duplicates={} unsequenced={} maximum_interarrival_us={} \
             maximum_interarrival_at={}",
            sequence_evidence.first.unwrap_or(u32::MAX),
            sequence_evidence.first.map(|_| sequence_evidence.highest).unwrap_or(u32::MAX),
            sequence_evidence.first.map(|_| sequence_evidence.expected).unwrap_or(u32::MAX),
            sequence_evidence.gap_events,
            sequence_evidence.forward_missing,
            sequence_evidence.maximum_gap,
            sequence_evidence.maximum_gap_at.unwrap_or(u32::MAX),
            sequence_evidence.first_gap_at.unwrap_or(u32::MAX),
            sequence_evidence.last_gap_at.unwrap_or(u32::MAX),
            sequence_evidence.backward,
            sequence_evidence.adjacent_duplicates,
            sequence_evidence.unsequenced,
            sequence_evidence.maximum_interarrival_micros,
            sequence_evidence
                .maximum_interarrival_at
                .unwrap_or(u32::MAX),
        ));
        if OPEN_RADIO_RX_ORDER_TELEMETRY {
            let order = OPEN_RADIO_RX_ORDER_COUNTERS
                .snapshot()
                .wrapping_delta_since(order_start);
            emergency_log(format_args!(
                "ORXO gap_events={} forward_missing={} backward={} adjacent_duplicates={} \
                 backward_mac_backward={} backward_mac_same={} backward_mac_forward={} \
                 backward_mac_other_tid={} backward_mac_unavailable={}",
                order.gap_events,
                order.forward_missing,
                order.backward,
                order.adjacent_duplicates,
                order.backward_mac_backward,
                order.backward_mac_same,
                order.backward_mac_forward,
                order.backward_mac_other_tid,
                order.backward_mac_unavailable,
            ));
        }
        emergency_log(format_args!(
            "ORXM m0={} m1={} m2={} m3={} m4={} m5={} m6={} m7={} m8={} \
             m9={} m10={} m11={} other={rx_other_phy}",
            rx_mcs_histogram[0],
            rx_mcs_histogram[1],
            rx_mcs_histogram[2],
            rx_mcs_histogram[3],
            rx_mcs_histogram[4],
            rx_mcs_histogram[5],
            rx_mcs_histogram[6],
            rx_mcs_histogram[7],
            rx_mcs_histogram[8],
            rx_mcs_histogram[9],
            rx_mcs_histogram[10],
            rx_mcs_histogram[11],
        ));
        log_open_radio_rx_pipeline_interval(
            pipeline_start,
            rx_irqs,
            irq_entries,
            irq_classification,
            irq_auxiliary_status_or,
            irq_unknown_status_or,
        );
        log_open_radio_task_poll_interval(task_poll_start);
        if let Some(aggregate_start) = aggregate_start {
            log_open_radio_ampdu_interval(aggregate_start);
        }
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-rx-interval-complete \
             datagrams={datagrams} terminal={}",
            u8::from(terminal_seen),
        ));
        if let Some(session) = session {
            let evidence = TransportEvidence {
                rx_bytes: bytes,
                tx_bytes: 0,
                rx_units: datagrams,
                tx_units: 0,
                elapsed_micros: elapsed_us,
                transport_errors: receive_errors,
            };
            if OPEN_RADIO_RUNTIME_BIDIRECTIONAL_SESSIONS {
                complete_open_radio_bidirectional_direction(
                    session.session_id,
                    OpenRadioBidirectionalDirection::Rx,
                    evidence,
                    terminal_seen && receive_errors == 0,
                )
                .await;
            } else {
                crate::console::complete_session(
                    session.session_id,
                    evidence,
                    terminal_seen && receive_errors == 0,
                )
                .await;
            }
        }
    }
}

/// Cooperatively preempt an application-side ready socket only when the ISR
/// has already published fresh descriptor work. This is event-driven rather
/// than a frame batch: an idle radio adds no scheduler edge, while a sustained
/// RX stream cannot let a full UDP socket queue monopolize one executor poll.
async fn yield_to_pending_radio_rx(last_handoff: &mut Instant) {
    if OPEN_RADIO_IRQ_RUNTIME.rx_signaled()
        && last_handoff.elapsed() >= OPEN_RADIO_RX_APPLICATION_HANDOFF_BUDGET
    {
        yield_now().await;
        *last_handoff = Instant::now();
    }
}

fn log_open_radio_rx_pipeline_interval(
    earlier: RxPipelineCounterSnapshot,
    rx_irq_posts: u32,
    mac_irq_entries: u32,
    irq_classification: OpenRadioMacIrqClassificationSnapshot,
    irq_auxiliary_status_or: u32,
    irq_unknown_status_or: u32,
) {
    let pipeline = OPEN_RADIO_RX_PIPELINE_COUNTERS
        .snapshot()
        .wrapping_delta_since(earlier);
    emergency_log(format_args!(
        "ORXS calls={} frontier={} admitted={} bytes={} discard_empty={} discard_long={} \
         back={} pool={} queue={} deferred_max={} pool_min={} queue_min={} \
         fmax={} amax={} service_us={} service_boot_max_us={}",
        pipeline.service_calls,
        pipeline.completion_frontier_frames,
        pipeline.admitted_frames,
        pipeline.staged_bytes,
        pipeline.stage_empty_discards,
        pipeline.stage_too_long_discards,
        pipeline.backpressured_services,
        pipeline.pool_credit_limited_services,
        pipeline.queue_credit_limited_services,
        pipeline.maximum_deferred_frames,
        pipeline.minimum_backpressured_pool_credits,
        pipeline.minimum_backpressured_queue_credits,
        pipeline.maximum_frontier,
        pipeline.maximum_admitted,
        pipeline.service_micros,
        pipeline.service_lifetime_max_micros,
    ));
    emergency_log(format_args!(
        "ORXB increments={} samples={} last_service={} last_counter={} \
         last_frontier={} last_admitted={} last_pool={} last_queue={} last_service_us={}",
        pipeline.dma_buffer_full_increments,
        pipeline.dma_buffer_full_service_samples,
        pipeline.dma_buffer_full_last_service,
        pipeline.dma_buffer_full_last_counter,
        pipeline.dma_buffer_full_last_frontier,
        pipeline.dma_buffer_full_last_admitted,
        pipeline.dma_buffer_full_last_pool_credits,
        pipeline.dma_buffer_full_last_queue_credits,
        pipeline.dma_buffer_full_last_service_micros,
    ));
    emergency_log(format_args!(
        "ORXD frames={} data={} amsdu={} amsdu_subframes={} unit_le1700={} \
         unit_1701_3400={} unit_over3400={} unit_boot_max_bytes={} \
         waits={} wait_us={} wait_boot_max_us={} dispatch_us={} \
         dispatch_boot_max_us={} publications={} bytes={} publish_us={} publish_boot_max_us={}",
        pipeline.protocol_frames,
        pipeline.protocol_data_frames,
        pipeline.protocol_amsdu_mpdus,
        pipeline.protocol_amsdu_subframes,
        pipeline.protocol_units_le_1700,
        pipeline.protocol_units_1701_3400,
        pipeline.protocol_units_over_3400,
        pipeline.protocol_unit_lifetime_max_bytes,
        pipeline.network_ready_waits,
        pipeline.network_ready_wait_micros,
        pipeline.network_ready_wait_lifetime_max_micros,
        pipeline.dispatch_micros,
        pipeline.dispatch_lifetime_max_micros,
        pipeline.network_publications,
        pipeline.network_published_bytes,
        pipeline.network_publish_micros,
        pipeline.network_publish_lifetime_max_micros,
    ));
    emergency_log(format_args!(
        "ORXR starts={} stops={} start_tid={} start_seq={} window={} first_samples={} \
         first_tid={} first_start={} first_seq={} first_distance={} buffered={} released={} \
         missing={} stale={} expiries={} occupied={} occupied_max={}",
        pipeline.reorder_starts,
        pipeline.reorder_stops,
        pipeline.reorder_last_start >> 26 & 0x07,
        pipeline.reorder_last_start & 0x0fff,
        pipeline.reorder_last_start >> 16 & 0x03ff,
        pipeline.reorder_first_samples,
        pipeline.reorder_last_first >> 24 & 0x0f,
        pipeline.reorder_last_first >> 12 & 0x0fff,
        pipeline.reorder_last_first & 0x0fff,
        pipeline.reorder_last_first_distance,
        pipeline.reorder_buffered,
        pipeline.reorder_released,
        pipeline.reorder_missing,
        pipeline.reorder_stale,
        pipeline.reorder_gap_expiries,
        pipeline.reorder_current_occupied,
        pipeline.reorder_maximum_occupied,
    ));
    emergency_log(format_args!(
        "ORXF zero={} one={} two_three={} four_seven={} eight_fifteen={} \
         sixteen_thirty_one={} thirty_two_plus={} irq_posts={} irq_epochs={} \
         irq_entries={} irq_coalesced={} irq_samples={} irq_skew={} \
         irq_service_us={} irq_service_boot_max_us={}",
        pipeline.frontier_zero_services,
        pipeline.frontier_one_services,
        pipeline.frontier_two_three_services,
        pipeline.frontier_four_seven_services,
        pipeline.frontier_eight_fifteen_services,
        pipeline.frontier_sixteen_thirty_one_services,
        pipeline.frontier_thirty_two_plus_services,
        rx_irq_posts,
        pipeline.rx_irq_epochs,
        mac_irq_entries,
        rx_irq_posts.saturating_sub(pipeline.rx_irq_epochs),
        pipeline.rx_irq_service_samples,
        pipeline.rx_irq_clock_skew_samples,
        pipeline.rx_irq_to_service_micros,
        pipeline.rx_irq_to_service_lifetime_max_micros,
    ));
    emergency_log(format_args!(
        "ORXI spurious={} rx_only={} rx_mixed={} tx_only={} tx_mixed={} \
         other_only={} extra={} saturated={} aux_or={} unknown_or={}",
        irq_classification.spurious_entries,
        irq_classification.rx_only_entries,
        irq_classification.rx_mixed_entries,
        irq_classification.tx_only_entries,
        irq_classification.tx_mixed_entries,
        irq_classification.other_only_entries,
        irq_classification.extra_nonzero_snapshots,
        irq_classification.saturated_entries,
        irq_auxiliary_status_or,
        irq_unknown_status_or,
    ));
}

fn log_open_radio_task_poll_interval(earlier: OpenRadioTaskPollSetSnapshot) {
    if !OPEN_RADIO_TASK_POLL_TELEMETRY {
        return;
    }
    let current = OPEN_RADIO_TASK_POLLS.snapshot();
    log_open_radio_task_poll(
        "network",
        current.network.wrapping_delta_since(earlier.network),
    );
    log_open_radio_task_poll(
        "protocol",
        current.protocol.wrapping_delta_since(earlier.protocol),
    );
    log_open_radio_task_poll(
        "radio",
        current.radio.wrapping_delta_since(earlier.radio),
    );
    log_open_radio_task_poll(
        "benchmark",
        current.benchmark.wrapping_delta_since(earlier.benchmark),
    );
}

fn log_open_radio_task_poll(task: &str, poll: OpenRadioTaskPollSnapshot) {
    emergency_log(format_args!(
        "ORTP task={task} polls={} poll_us={} poll_boot_max_us={} \
         over_100us={} over_500us={} over_1000us={} over_5000us={}",
        poll.polls,
        poll.poll_micros,
        poll.lifetime_max_micros,
        poll.over_100_micros,
        poll.over_500_micros,
        poll.over_1_000_micros,
        poll.over_5_000_micros,
    ));
}

/// Observe continuous executor residence without changing the wrapped
/// future's wake or pending semantics. Wall time includes interrupt
/// preemption, which is intentional: a long task poll that blocks sibling
/// Embassy work is harmful regardless of whether its body or an ISR consumed
/// the interval.
async fn observe_open_radio_task_polls<F: Future>(
    future: F,
    counters: &'static OpenRadioTaskPollCounters,
) -> F::Output {
    if !OPEN_RADIO_TASK_POLL_TELEMETRY {
        return future.await;
    }
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|context| {
        let started = Instant::now();
        let result = future.as_mut().poll(context);
        counters.record(started.elapsed().as_micros());
        result
    })
    .await
}

// These concrete wrappers belong to the HIL composition root. The reusable
// driver crates expose owned runners but do not choose an executor, task
// storage or benchmark policy. Keeping each long-running future in its own
// Embassy task gives it an independent waker and removes the fixed outer poll
// order that previously coupled stack, protocol and PAC progress.
#[embassy_executor::task]
async fn connected_network_stack_task(mut runner: ConnectedNetworkStackRunner) {
    observe_open_radio_task_polls(runner.run(), &OPEN_RADIO_TASK_POLLS.network).await
}

#[embassy_executor::task(pool_size = 2)]
async fn connected_rx_protocol_task(protocol: ConnectedRxProtocol) {
    let stopped = observe_open_radio_task_polls(
        protocol.run_until_stopped(OPEN_RADIO_CONNECTED_PROTOCOL_STOP.wait()),
        &OPEN_RADIO_TASK_POLLS.protocol,
    )
    .await;
    let shutdown = stopped.shutdown();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-protocol-stop \
         queued_frames={} retained_frames={} reorder_commands={} active_reorders={}",
        shutdown.queued_frames,
        shutdown.retained_frames,
        shutdown.reorder_commands,
        shutdown.active_reorders,
    ));
    OPEN_RADIO_CONNECTED_PROTOCOL_STOPPED.signal(stopped);
}

#[embassy_executor::task]
async fn connected_network_report_task(stack: Stack<'static>) {
    report_network_configuration(stack).await
}

#[embassy_executor::task(pool_size = 2)]
async fn connected_benchmark_task(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &'static RefCell<&'static mut RadioRegisters>,
) {
    let _ = select(
        OPEN_RADIO_CONNECTED_BENCHMARK_STOP.wait(),
        observe_open_radio_task_polls(
            run_open_radio_udp_benchmark(stack, association_phy, data_tx_rate, registers),
            &OPEN_RADIO_TASK_POLLS.benchmark,
        ),
    )
    .await;
    OPEN_RADIO_CONNECTED_BENCHMARK_STOPPED.signal(());
}

async fn run_connected_network<'fixture, 'rate, 'security>(
    fixture: RadioHilConnectedTaskFixture<'fixture>,
    epoch_resources: RadioHilConnectedEpochResources,
    session: StaConnectedSession<'rate, 'security>,
    pairwise_slot: StaPairwiseCcmpSlot,
    group_slot: StaGroupCcmpSlot,
) -> RadioHilConnectedEpochReturn<'fixture, 'security> {
    let RadioHilConnectedTaskFixture {
        spawner,
        protocol_spawner,
        platform,
        interrupt_setup,
        rx_storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        frame,
        ethernet,
    } = fixture;
    let StaConnectedSession {
        link,
        network,
        rate_control,
        pmk,
        supplicant_nonce,
        sequences,
    } = session;
    let StaConnectedLink {
        station_address,
        bssid,
        association_id,
        beacon_interval_tu,
        peer_qos,
        association_phy,
        peer_supports_one_ltf_800ns_gi,
        peer_supports_ldpc,
        peer_dcm_receive,
    } = link;
    // The polling-only scan/auth path kept every MAC interrupt masked. Consume
    // the last task-side enable/clear capability immediately before the
    // connected path enables the ISR-owned RX/TX Signal sink.
    // After `activate`, ordinary `RadioRegisters` cannot touch those
    // registers.
    let interrupt_epoch = interrupt_setup
        .take()
        .expect("MAC interrupt setup has no concurrent active epoch");
    activate_open_radio_interrupts(platform, interrupt_epoch, MAC_COLD_RX_INTERRUPT_MASK);

    let (stack, network_runner, stack_runner) = match network {
        RadioHilStaNetwork::Unstarted { device, runner } => {
            let stack_resources = OPEN_RADIO_STACK_RESOURCES.init(StackResources::new());
            let mut seed = [0_u8; 8];
            seed[..6].copy_from_slice(&station_address);
            seed[6..].copy_from_slice(&0x31a5_u16.to_le_bytes());
            // Keep the controlled local throughput setup independent of DHCP
            // while preserving DHCP as an end-to-end router test.
            let network_config = if PERF_AP_PROFILE {
                NetworkConfig::ipv4_static(StaticConfigV4 {
                    address: Ipv4Cidr::new(Ipv4Address::from_octets(STA_HIL_IPV4), 24),
                    gateway: Some(Ipv4Address::from_octets(STA_ARP_TARGET_IPV4)),
                    dns_servers: Default::default(),
                })
            } else {
                NetworkConfig::dhcpv4(Default::default())
            };
            let (stack, stack_runner) = embassy_net::new(
                device,
                network_config,
                stack_resources,
                u64::from_le_bytes(seed),
            );
            (stack, runner, Some(stack_runner))
        }
        RadioHilStaNetwork::Running(network) => (network.stack, network.runner, None),
    };
    network_runner.set_link_state(LinkState::Up);
    let data_tx_rate = selected_data_tx_rate(association_phy, peer_qos);
    let benchmark_tx_rate = rate_control.ampdu_tx_rate(configured_sta_tx_rate_policy(
        association_phy,
        peer_qos,
        peer_supports_one_ltf_800ns_gi,
        peer_supports_ldpc,
        peer_dcm_receive,
    ));
    let peer_ampdu_limit = tx_storage
        .control
        .as_ref()
        .expect("control TX owner is present before connected handoff")
        .policy()
        .ht_ampdu()
        .maximum_aggregate_bytes();
    let rate_ampdu_limit = match benchmark_tx_rate {
        TxPhyRate::Legacy(_) => 0,
        TxPhyRate::Ht(rate) => u32::from(rate.vendor_ampdu_byte_limit().unwrap_or(u16::MAX)),
        TxPhyRate::He(rate) => rate.maximum_apep_bytes(HeEdcaTxopLimit::DEFAULT),
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-start \
         frame_capacity={NETWORK_FRAME_CAPACITY} \
         rx_queue_depth={NETWORK_RX_QUEUE_DEPTH} tx_queue_depth={NETWORK_TX_QUEUE_DEPTH} \
         rx_stage_slots={RX_STAGE_SLOT_COUNT} rx_stage_capacity={RX_STAGE_CAPACITY} \
         rx_ba_window={RX_BLOCK_ACK_SOFTWARE_WINDOW} \
         bandwidth_mhz={} phy={} data_rate_code={:#04x} data_rate_kbps={} \
         ampdu_rate_code={:#04x} ampdu_rate_kbps={} peer_ampdu_limit={} rate_ampdu_limit={}",
        association_phy.bandwidth_mhz(),
        association_phy.name(),
        data_tx_rate.code(),
        data_tx_rate.nominal_kbps(),
        benchmark_tx_rate.code(),
        benchmark_tx_rate.nominal_kbps(),
        peer_ampdu_limit,
        rate_ampdu_limit,
    ));

    let (staged_rx_sender, staged_rx_receiver) = OPEN_RADIO_STAGED_RX_QUEUE.split();
    let (hardware, rx, tx_ampdu_storage, control_resources) = match epoch_resources {
        RadioHilConnectedEpochResources::Initial { registers, mut rx } => {
            if let Err(error) = rx
                .start(registers, rx_storage, descriptor_base, buffer_addresses)
                .await
            {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=production-runner-rx-arm epoch=initial error={error:?}"
                        ));
                let _owner = rx;
                        loop {
                            Timer::after_secs(60).await;
                        }
                    }
            let rx_ring = match rx.take_live() {
                Ok(ring) => ring,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch=initial \
                         transition=take-live error={error:?}"
                    ));
                    let _owner = rx;
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
            };
            let rx = Esp32s31ConnectedRx::new(
                rx_ring,
                rx_storage.buffers(),
                &OPEN_RADIO_RX_STAGE_POOL,
                OpenRadioRxReloadDelay,
                staged_rx_sender,
            )
            .with_pipeline_counters(&OPEN_RADIO_RX_PIPELINE_COUNTERS);
            // The production aggregate owner is descriptor-only
            // (`BUFFER_SIZE == 0`), so constructing it in the static cell does
            // not materialize the former 55-KiB payload arena on this task's
            // stack. This edge belongs exclusively to the first epoch.
            let ampdu = HtAmpduTxStorage::pin_static(
                OPEN_RADIO_TX_AMPDU_STORAGE.init_with(HtAmpduTxStorage::new),
            );
            let control_resources = OPEN_RADIO_CONTROL_RESOURCES.init(ControlResources::new());
            let registers = OPEN_RADIO_REGISTER_CELL.init(RefCell::new(registers));
            (
                CooperativeTxHardware::new(registers),
                rx,
                ampdu,
                &*control_resources,
            )
        }
        RadioHilConnectedEpochResources::Reconnected {
            mut hardware,
            mut rx,
            rx_resources,
            ampdu,
            control_resources,
        } => {
            if let Err(error) = rx
                .start(
                    &mut hardware,
                    rx_storage,
                    descriptor_base,
                    buffer_addresses,
                )
                .await
            {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch=reconnected \
                         transition=start error={error:?}"
                    ));
                    let _owners = (
                        hardware,
                        rx,
                        rx_resources,
                        ampdu,
                        control_resources,
                        network_runner,
                    );
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
            let rx_ring = match rx.take_live() {
                Ok(ring) => ring,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch=reconnected \
                         transition=take-live error={error:?}"
                    ));
                    let _owners = (
                        hardware,
                        rx,
                        rx_resources,
                        ampdu,
                        control_resources,
                        network_runner,
                    );
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
            };
            let rx = rx_resources.with_live_ring(rx_ring);
            (hardware, rx, ampdu, control_resources)
        }
    };
    let network_rx = network_runner.rx_publisher();
    let (control_publisher, control_receiver) = control_resources.split();
    let rx_sink = EmbassyNetConnectedRxSink::new(
        network_rx,
        HilConnectedRxObserver {
            control: control_publisher,
            station_address,
            phy_sample_cursor: 0,
            order: OpenRadioRxOrderTracker::default(),
        },
    )
    .with_counters(&OPEN_RADIO_RX_ENQUEUE_COUNTERS)
    .with_pipeline_counters(&OPEN_RADIO_RX_PIPELINE_COUNTERS);
    let (rx_reorder_sender, rx_reorder_receiver) = OPEN_RADIO_RX_REORDER_COMMANDS.split();
    let dispatcher = ConnectedRxDispatcher::new(ConnectedRxConfig {
        station_address,
        bssid,
        association_id,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
    });
    let rx_protocol = Esp32s31ConnectedRxProtocol::new(
        staged_rx_receiver,
        &OPEN_RADIO_IRQ_RUNTIME,
        dispatcher,
        rx_sink,
        frame,
        ethernet,
    )
    .with_rx_reorder_commands(rx_reorder_receiver)
    .with_rx_reorder_storage(&OPEN_RADIO_RX_REORDER_STORAGE)
    .with_pipeline_counters(&OPEN_RADIO_RX_PIPELINE_COUNTERS);

    let tx_sequences = core::mem::replace(sequences, StaTxSequenceCounters::new(0));
    let control_tx = tx_storage
        .control
        .take()
        .expect("control TX owner moves into the connected runner exactly once");
    let ordinary_tx = match control_tx.try_into_connected(ConnectedTxHandoff {
        key: pairwise_slot,
        sequences: tx_sequences,
        config: SingleMpduTxConfig {
            station_address,
            bssid,
            peer_qos,
            rate: data_tx_rate,
            attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
            completion_timeout_us: TX_COMPLETION_DEADLINE_MS * 1_000,
        },
    }) {
        Ok(tx) => tx,
        Err((_control, _handoff)) => {
            panic!("connected handoff requires an idle control TX owner")
        }
    };
    let tx = Esp32s31ConnectedTx::new(
        ordinary_tx,
        tx_ampdu_storage,
        AggregateTxConfig {
            rate: benchmark_tx_rate,
            frame_limit: TX_AMPDU_FRAME_COUNT as u8,
            attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
            completion_timeout_us: TX_COMPLETION_DEADLINE_MS * 1_000,
            he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        },
    )
    .expect("fixed connected aggregate TX configuration")
    .with_counters(&OPEN_RADIO_TX_AGGREGATE_COUNTERS);
    let tx_block_ack = StaTxBlockAckSessions::new(
        TX_BLOCK_ACK_WINDOW as u16,
        500_000,
        OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_NETWORK_AMSDU_BENCH,
    )
    .expect("fixed three-TID TX BlockAck configuration");
    let mut control = Esp32s31ConnectedControl::new(
        control_receiver,
        bssid,
        association_phy == StaAssociationPhy::He20,
        tx_block_ack,
    )
    .with_rx_block_ack_maximum_window(RX_BLOCK_ACK_SOFTWARE_WINDOW as u16)
    .expect("staging-derived RX BlockAck window is valid")
    .with_rx_reorder_commands(rx_reorder_sender);
    control.enable_beacon_loss(
        StaBeaconLossConfig::new(beacon_interval_tu, CONNECTED_BEACON_MISS_LIMIT)
            .expect("scan admitted a nonzero connected beacon interval"),
    );
    if peer_qos && matches!(benchmark_tx_rate, TxPhyRate::Ht(_) | TxPhyRate::He(_)) {
        control.queue_initial_tx_block_ack();
    }

    let registers = hardware.register_cell();
    let backend = Esp32s31WifiBackend::with_control(hardware, rx, tx, control);
    let mut radio_runner = WifiRunner::new(&OPEN_RADIO_IRQ_RUNTIME, network_runner, backend);

    let network_started = stack_runner.is_some();
    if let Some(stack_runner) = stack_runner {
        let stack_task = connected_network_stack_task(stack_runner)
            .unwrap_or_else(|_| panic!("connected network task allocation failed"));
        spawner.spawn(stack_task);
        let report_task = connected_network_report_task(stack)
            .unwrap_or_else(|_| panic!("connected network report task allocation failed"));
        spawner.spawn(report_task);
    }
    let protocol_task = connected_rx_protocol_task(rx_protocol)
        .unwrap_or_else(|_| panic!("connected RX protocol task allocation failed"));
    // embassy-net intentionally stores its Stack/Runner state behind a
    // RefCell and is therefore !Send. Keep that owner, the PAC runner and the
    // MMIO-backed tasks on Core 0. The staged protocol owns only cross-core
    // CriticalSectionRawMutex queues and is compiler-proven Send, so moving it
    // to Core 1 removes one long cooperative poll interval without inventing
    // a fixed per-wake frame ceiling.
    protocol_spawner.spawn(protocol_task);
    let benchmark_task = connected_benchmark_task(
        stack,
        association_phy,
        benchmark_tx_rate,
        registers,
    )
    .unwrap_or_else(|_| panic!("connected benchmark task allocation failed"));
    spawner.spawn(benchmark_task);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-task-topology \
         network=core0 rx_protocol=core1 radio=sta-parent-core0 \
         report=core0 benchmark=core0 network_started={}"
        , u8::from(network_started)
    ));

    // The radio loop intentionally remains in this parent STA future. Other
    // long-running owners still have independent executor tasks/wakers, while
    // disconnect returns RX/TX/control ownership into the same scope that
    // retains the GTK and platform token. A spawned task could only report
    // the edge and would strand those values in its private task storage.
    let runner_exit = match observe_open_radio_task_polls(
        radio_runner.run_until(crate::console::receive_station_epoch_cycle()),
        &OPEN_RADIO_TASK_POLLS.radio,
    )
    .await
    {
        Ok(WifiRunnerExit::Disconnected) => {
            let control = radio_runner.backend().control();
            let beacon_monitor = control.beacon_monitor();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner \
                 error=disconnected beacon_lost={} beacons_observed={} \
                 beacon_deadline_us={:?} last_control_event={:?} last_tx_failure={:?}",
                u8::from(control.beacon_lost()),
                beacon_monitor.map_or(0, |monitor| monitor.observed()),
                beacon_monitor.and_then(|monitor| monitor.deadline_micros()),
                control.last_event(),
                control.last_tx_failure(),
            ));
            RadioHilConnectedExit::Disconnected
        }
        Ok(WifiRunnerExit::Stopped) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-runner-stop \
                 source=host-station-epoch-cycle"
            ));
            RadioHilConnectedExit::Stopped
        }
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner error={error:?}"
            ));
            RadioHilConnectedExit::HardwareFailure
        }
    };
    // Close hardware publication before stopping the protocol consumer. The
    // radio runner no longer schedules RX/control; masking both CPU and
    // peripheral routes now makes the command/frame drain finite and prevents
    // a stale wake from leaking into the next connected epoch.
    *interrupt_setup = Some(deactivate_open_radio_interrupts(platform));
    let irq_drain = OPEN_RADIO_IRQ_RUNTIME.drain_pending();
    let power_irq_drain = OPEN_RADIO_POWER_IRQ_RUNTIME.try_take().unwrap_or(0);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-interrupt-stop \
         rx_wake={} rx_capacity_wake={} tx_events={:#010x} power_events={:#010x}",
        u8::from(irq_drain.rx),
        u8::from(irq_drain.rx_capacity),
        irq_drain.tx_events,
        power_irq_drain,
    ));
    // No spawned task may retain a PAC borrow when this epoch returns. The
    // benchmark is the only task besides the radio runner that receives the
    // register cell; stop it before waiting for protocol ownership release.
    OPEN_RADIO_CONNECTED_BENCHMARK_STOP.signal(());
    OPEN_RADIO_CONNECTED_PROTOCOL_STOP.signal(());
    OPEN_RADIO_CONNECTED_BENCHMARK_STOPPED.wait().await;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-benchmark-stopped"
    ));
    let stopped_protocol = OPEN_RADIO_CONNECTED_PROTOCOL_STOPPED.wait().await;
    let protocol_shutdown = stopped_protocol.shutdown();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-protocol-stopped \
         queued_frames={} retained_frames={} reorder_commands={} active_reorders={}",
        protocol_shutdown.queued_frames,
        protocol_shutdown.retained_frames,
        protocol_shutdown.reorder_commands,
        protocol_shutdown.active_reorders,
    ));
    let (frame, ethernet) = stopped_protocol.into_scratch();
    let (network, backend) = radio_runner.into_parts();
    let (mut hardware, rx, mut tx, mut control) = backend.into_parts();
    match control.shutdown(&mut hardware, &mut tx) {
        Ok(shutdown) => emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=production-control-stop \
             rx_ba={} tx_ba={} discarded_events={} in_flight={:?}",
            shutdown.rx_block_ack_agreements,
            shutdown.tx_block_ack_sessions,
            shutdown.discarded_events,
            shutdown.in_flight,
        )),
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-control-stop error={error:?}"
            ));
            let _owners = (network, hardware, rx, tx, control, group_slot);
            loop {
                Timer::after_secs(60).await;
            }
        }
    }
    match rx.try_stop(&mut hardware) {
        Ok(stopped_rx) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-dma-stop \
                 descriptor_base={:#010x} queued_frames={}",
                stopped_rx.ring().descriptor_base(),
                stopped_rx.queued_frames(),
            ));
            match tx.try_into_teardown_parts() {
                Ok((resources, handoff, ampdu)) => {
                    let ConnectedTxHandoff {
                        key,
                        sequences: connected_sequences,
                        config: _,
                    } = handoff;
                    let pairwise_hardware_index = key.hardware_index();
                    let group_hardware_index = group_slot.hardware_index();
                    group_slot.clear(&mut hardware);
                    key.clear(&mut hardware);
                    let key_bitmap = hardware.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP);
                    let keys_cleared = key_bitmap
                        & ((1 << pairwise_hardware_index) | (1 << group_hardware_index))
                        == 0;
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result={} stage=production-connected-key-clear \
                         pairwise_slot={} group_slot={} valid_bitmap={key_bitmap:#010x}",
                        if keys_cleared { "PASS" } else { "FAIL" },
                        pairwise_hardware_index,
                        group_hardware_index,
                    ));
                    *sequences = connected_sequences;
                    tx_storage.control = Some(Esp32s31ControlTx::new(
                        resources,
                        ControlTxConfig {
                            unicast_attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
                            completion_timeout_us: TX_COMPLETION_DEADLINE_MS * 1_000,
                            poll_interval_us: 1,
                        },
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS \
                         stage=production-connected-tx-return"
                    ));
                    drop(control);
                    return RadioHilConnectedEpochReturn {
                        fixture: RadioHilConnectedTaskFixture {
                            spawner,
                            protocol_spawner,
                            platform,
                            interrupt_setup,
                            rx_storage,
                            tx_storage,
                            descriptor_base,
                            buffer_addresses,
                            frame,
                            ethernet,
                        },
                        disconnected: RadioHilDisconnectedEpoch {
                            network: RadioHilRunningNetwork {
                                stack,
                                runner: network,
                            },
                            hardware,
                            rx: stopped_rx,
                            ampdu,
                            control_resources,
                        },
                        security: StaAssociationSecurity {
                            pmk,
                            supplicant_nonce,
                            sequences,
                        },
                        exit: runner_exit,
                    };
                }
                Err(tx) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-connected-tx-return error=aggregate-active"
                    ));
                    let _owners = (network, hardware, stopped_rx, tx, control, group_slot);
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
            }
        }
        Err((live_rx, error)) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-rx-dma-stop error={error:?}"
            ));
            let _owners = (network, hardware, live_rx, tx, control, group_slot);
            loop {
                Timer::after_secs(60).await;
            }
        }
    }
}

/// Prove that one disconnected owner can create and close a second RX epoch
/// without reinitializing any static storage.
async fn qualify_disconnected_rx_restart(
    epoch: RadioHilDisconnectedEpoch,
) -> RadioHilDisconnectedEpoch {
    let RadioHilDisconnectedEpoch {
        network,
        mut hardware,
        rx,
        ampdu,
        control_resources,
    } = epoch;
    let prepared = match rx.prepare(&mut hardware) {
        Ok(prepared) => prepared,
        Err((rx, error)) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-rx-restart-prepare \
                 error={error:?}"
            ));
            let _owners = (network, hardware, rx, ampdu, control_resources);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let live = match prepared.start(&mut hardware).await {
        Ok(live) => live,
        Err((prepared, error)) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-rx-restart-enable \
                 error={error:?}"
            ));
            let _owners = (network, hardware, prepared, ampdu, control_resources);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let rx = match live.try_stop(&mut hardware) {
        Ok(rx) => rx,
        Err((live, error)) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-rx-restart-stop \
                 error={error:?}"
            ));
            let _owners = (network, hardware, live, ampdu, control_resources);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-restart \
         descriptor_base={:#010x} queued_frames={}",
        rx.ring().descriptor_base(),
        rx.queued_frames(),
    ));
    RadioHilDisconnectedEpoch {
        network,
        hardware,
        rx,
        ampdu,
        control_resources,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilStaLifecycleFailure {
    Authentication,
    InitialJoin {
        associated: bool,
        message1: bool,
        message3: bool,
    },
    PeerScanPolicy,
    ScanWmmPolicy,
    InvalidEpochOwner,
    Association,
    SecuritySelection,
    PeerPlan,
    AssociationWmmPolicy,
    HePeerProgramming,
    RateControlProgramming,
    Wpa2Handshake,
    Wpa2KeyInstall,
    ConnectedHardware,
}

fn failed_reconnect<'fixture, 'security>(
    ready: RadioHilReconnectReady<'fixture, 'security>,
    stage: StaLifecycleStage,
    disposition: StaFailureDisposition,
    error: RadioHilStaLifecycleFailure,
) -> StaAttemptOutcome<RadioHilReconnectReady<'fixture, 'security>, RadioHilStaLifecycleFailure> {
    StaAttemptOutcome::Failed {
        owner: ready,
        failure: StaAttemptFailure::new(stage, disposition, error),
    }
}

fn initial_join_outcome<'fixture, 'security>(
    outcome: RadioHilJoinOutcome<'fixture, 'security>,
) -> StaAttemptOutcome<
    RadioHilStaLifecycleOwner<'fixture, 'security>,
    RadioHilStaLifecycleFailure,
> {
    match outcome {
        RadioHilJoinOutcome::Failed(failure) => {
            let (associated, message1, message3) = failure.progress();
            let stage = if associated {
                StaLifecycleStage::Security
            } else {
                StaLifecycleStage::Association
            };
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::Join(failure.retry),
                failure: StaAttemptFailure::new(
                    stage,
                    StaFailureDisposition::RetryCurrentCandidate,
                    RadioHilStaLifecycleFailure::InitialJoin {
                        associated,
                        message1,
                        message3,
                    },
                ),
            }
        }
        RadioHilJoinOutcome::Connected { ready, exit } => match exit {
            RadioHilConnectedExit::HardwareFailure => StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::Reconnect(ready),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::ConnectedHardware,
                ),
            },
            RadioHilConnectedExit::Disconnected | RadioHilConnectedExit::Stopped => {
                // The HIL's first controlled runner stop is an explicit
                // request to cross the reconnect boundary. Production callers
                // preserve the distinction in `RadioHilConnectedExit`.
                StaAttemptOutcome::Disconnected {
                    owner: RadioHilStaLifecycleOwner::Reconnect(ready),
                    next_candidate: StaNextCandidate::Reuse,
                }
            }
        },
    }
}

struct RadioHilStaLifecycleBackend<O> {
    _owner: PhantomData<fn() -> O>,
}

impl<O> RadioHilStaLifecycleBackend<O> {
    const fn new() -> Self {
        Self {
            _owner: PhantomData,
        }
    }
}

impl<'fixture, 'security> StaLifecycleBackend
    for RadioHilStaLifecycleBackend<RadioHilStaLifecycleOwner<'fixture, 'security>>
{
    type Owner = RadioHilStaLifecycleOwner<'fixture, 'security>;
    type Error = RadioHilStaLifecycleFailure;

    fn run_attempt(
        &mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + '_ {
        async move {
            let phase = match &owner {
                RadioHilStaLifecycleOwner::Authenticate(_) => "authentication",
                RadioHilStaLifecycleOwner::Join(_) => "join",
                RadioHilStaLifecycleOwner::Reconnect(_) => "reconnect",
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE \
                 stage=production-sta-lifecycle-attempt generation={} attempt={} \
                 refresh_candidate={} phase={phase}",
                context.generation,
                context.attempt,
                u8::from(context.refresh_candidate),
            ));
            match owner {
                RadioHilStaLifecycleOwner::Authenticate(ready) => {
                    let RadioHilAuthenticationReady {
                        mut fixture,
                        target,
                        rx,
                        network,
                        security,
                    } = ready;
                    let authentication_started = Instant::now();
                    let (authenticated, rx) = authenticate_target(
                        &mut fixture,
                        target,
                        rx,
                        security.sequences.non_qos_mut(),
                    )
                    .await;
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=OBSERVE \
                         stage=sta-auth-timing passed={} elapsed_ms={}",
                        authenticated,
                        authentication_started.elapsed().as_millis(),
                    ));
                    if authenticated {
                        initial_join_outcome(
                            associate_target(
                                fixture.into_connected(),
                                target,
                                rx,
                                network,
                                security,
                            )
                            .await,
                        )
                    } else {
                        StaAttemptOutcome::Failed {
                            owner: RadioHilStaLifecycleOwner::Authenticate(
                                RadioHilAuthenticationReady {
                                    fixture,
                                    target,
                                    rx,
                                    network,
                                    security,
                                },
                            ),
                            failure: StaAttemptFailure::new(
                                StaLifecycleStage::Authentication,
                                StaFailureDisposition::RetryCurrentCandidate,
                                RadioHilStaLifecycleFailure::Authentication,
                            ),
                        }
                    }
                }
                RadioHilStaLifecycleOwner::Join(retry) => {
                    let RadioHilJoinRetry {
                        fixture,
                        target,
                        rx,
                        network,
                        security,
                    } = retry;
                    initial_join_outcome(
                        associate_target(fixture, target, rx, network, security).await,
                    )
                }
                RadioHilStaLifecycleOwner::Reconnect(ready) => {
                    match qualify_reconnected_epoch(ready).await {
                        StaAttemptOutcome::Disconnected {
                            owner,
                            next_candidate,
                        } => StaAttemptOutcome::Disconnected {
                            owner: RadioHilStaLifecycleOwner::Reconnect(owner),
                            next_candidate,
                        },
                        StaAttemptOutcome::Stopped { owner } => StaAttemptOutcome::Stopped {
                            owner: RadioHilStaLifecycleOwner::Reconnect(owner),
                        },
                        StaAttemptOutcome::Failed { owner, failure } => {
                            StaAttemptOutcome::Failed {
                                owner: RadioHilStaLifecycleOwner::Reconnect(owner),
                                failure,
                            }
                        }
                    }
                }
            }
        }
    }

    fn wait_backoff(
        &mut self,
        owner: Self::Owner,
        delay_millis: u32,
        reason: StaBackoffReason,
    ) -> impl Future<Output = StaBackoffOutcome<Self::Owner>> + '_ {
        async move {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE \
                 stage=production-sta-lifecycle-backoff delay_ms={delay_millis} \
                 reason={reason:?}"
            ));
            Timer::after_millis(u64::from(delay_millis)).await;
            StaBackoffOutcome::Elapsed { owner }
        }
    }
}

/// Execute a complete Association/WPA2/connected epoch on the exact owners
/// returned by the preceding connected epoch.
///
/// This is not a parallel station implementation: all three finite protocol
/// transitions and the connected runner are the same production owners used
/// by the initial path. The only new composition is that their hardware
/// capability is `CooperativeTxHardware`, their network stack is already
/// running with link-down, and their RX frontier came from connected teardown.
async fn qualify_reconnected_epoch<'fixture, 'security>(
    mut ready: RadioHilReconnectReady<'fixture, 'security>,
) -> StaAttemptOutcome<RadioHilReconnectReady<'fixture, 'security>, RadioHilStaLifecycleFailure> {
    let StaJoinTarget {
        station_address,
        access_point,
    } = ready.target;
    let peer_scan_policy = match StaPeerScanPolicy::new(&access_point) {
        Ok(policy) => policy,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-peer-scan-policy error={error:?}"
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::CandidateSelection,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::PeerScanPolicy,
            );
        }
    };
    ready
        .fixture
        .tx_storage
        .install_ht_ampdu_policy(peer_scan_policy.ht_ampdu);
    ready
        .fixture
        .tx_storage
        .install_he_bss_color(peer_scan_policy.he_bss_color);
    if let Some(parameters) = peer_scan_policy.wmm.parameters() {
        if let Err(error) = ready.fixture.tx_storage.install_wmm_edca(parameters) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-wmm-edca source=scan error={error:?}"
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::CandidateSelection,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::ScanWmmPolicy,
            );
        }
    }

    let RadioHilConnectedEpochResources::Reconnected {
        hardware,
        rx,
        rx_resources: _,
        ampdu: _,
        control_resources: _,
    } = &mut ready.epoch
    else {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL \
             stage=production-reconnect-association error=initial-epoch-owner"
        ));
        return failed_reconnect(
            ready,
            StaLifecycleStage::Hardware,
            StaFailureDisposition::Terminal,
            RadioHilStaLifecycleFailure::InvalidEpochOwner,
        );
    };
    let join_rx = core::mem::replace(rx, RadioHilJoinRx::Vacant);
    let backend = RadioHilStaJoinBackend {
        mmio: hardware,
        rx_storage: ready.fixture.rx_storage,
        tx_storage: &mut *ready.fixture.tx_storage,
        descriptor_base: ready.fixture.descriptor_base,
        buffer_addresses: ready.fixture.buffer_addresses,
        frame: &mut *ready.fixture.frame,
        station_address,
        access_point,
        rx: join_rx,
    };
    let mut runner = StaJoinRunner::new(backend, EmbassyStaJoinTimer);
    let association_started = Instant::now();
    let result = runner
        .associate(
            station_address,
            access_point.bssid,
            ready.security.sequences.non_qos_mut(),
        )
        .await;
    let (backend, _) = runner.into_parts();
    *rx = backend.into_rx();
    let success = match result {
        Ok(success) => success,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-association error={error:?} \
                 elapsed_ms={} bssid={:02x?}",
                association_started.elapsed().as_millis(),
                access_point.bssid,
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::Association,
                StaFailureDisposition::RetryCurrentCandidate,
                RadioHilStaLifecycleFailure::Association,
            );
        }
    };
    let response = success.response;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-reconnect-association status={} aid={} \
         frames={} elapsed_ms={} bssid={:02x?}",
        response.status_code,
        response.association_id,
        success.total_received_frames,
        association_started.elapsed().as_millis(),
        access_point.bssid,
    ));

    let selected_rsn = match select_wpa2_psk_rsn(&access_point) {
        Ok(rsn) => rsn,
        Err(error) => {
            let _ = rx.stop(hardware);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-rsn-select error={error:?}"
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::Security,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::SecuritySelection,
            );
        }
    };
    let association_phy = select_sta_association(&access_point, STA_ASSOCIATION_PREFERENCE).phy;
    let noise_floor_dbm = hardware.read_noise_floor_dbm();
    let mut peer_plan = match peer_scan_policy.complete(
        &access_point,
        &response,
        association_phy,
        noise_floor_dbm,
    ) {
        Ok(plan) => plan,
        Err(error) => {
            let _ = rx.stop(hardware);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-peer-plan error={error:?}"
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::Association,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::PeerPlan,
            );
        }
    };
    ready
        .fixture
        .tx_storage
        .install_ht_ampdu_policy(peer_plan.ht_ampdu);
    ready
        .fixture
        .tx_storage
        .install_he_bss_color(peer_plan.he_bss_color);
    if peer_plan.wmm.source() == StaWmmSource::AssociationResponse {
        let Some(parameters) = peer_plan.wmm.parameters() else {
            let _ = rx.stop(hardware);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-wmm-edca \
                 source=association-response error=missing-parameters"
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::Association,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::AssociationWmmPolicy,
            );
        };
        if let Err(error) = ready.fixture.tx_storage.install_wmm_edca(parameters) {
            let _ = rx.stop(hardware);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-wmm-edca \
                 source=association-response error={error:?}"
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::Association,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::AssociationWmmPolicy,
            );
        }
    }
    if let Some(state) = peer_plan.he_peer_state
        && let Err(error) =
            program_he20_peer_state(hardware, state, response.association_id, 0, 0)
    {
        let _ = rx.stop(hardware);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL \
             stage=production-reconnect-he20-peer error={error:?}"
        ));
        return failed_reconnect(
            ready,
            StaLifecycleStage::Hardware,
            StaFailureDisposition::Terminal,
            RadioHilStaLifecycleFailure::HePeerProgramming,
        );
    }
    if let Err(error) = peer_plan.rate_control.program_hardware(hardware) {
        let _ = rx.stop(hardware);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL \
             stage=production-reconnect-rate-control error={error:?}"
        ));
        return failed_reconnect(
            ready,
            StaLifecycleStage::Hardware,
            StaFailureDisposition::Terminal,
            RadioHilStaLifecycleFailure::RateControlProgramming,
        );
    }
    let peer_he_capabilities = peer_plan.he_capabilities;
    let link = StaConnectedLink {
        station_address,
        bssid: access_point.bssid,
        association_id: response.association_id,
        beacon_interval_tu: access_point.beacon_interval_tu,
        peer_qos: peer_plan.peer_qos,
        association_phy,
        peer_supports_one_ltf_800ns_gi: peer_he_capabilities
            .is_some_and(|capability| capability.supports_one_ltf_800ns_gi()),
        peer_supports_ldpc: peer_he_capabilities
            .is_some_and(|capability| capability.supports_ldpc_coding_in_payload()),
        peer_dcm_receive: peer_he_capabilities.map_or(
            HeDcmConstellation::NotSupported,
            |capability| capability.dcm_receive_constellation(),
        ),
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-reconnect-peer-programmed phy={association_phy:?} \
         qos={} noise_floor_dbm={} metric={} \
         bf_mode={} bf_rate={}",
        u8::from(link.peer_qos),
        noise_floor_dbm,
        peer_plan.link_metric.value(),
        peer_plan.rate_control.beamforming_report().signal_mode(),
        peer_plan.rate_control.beamforming_report().rate_code(),
    ));

    let handshake = Wpa2HandshakeConfig {
        local: station_address,
        authenticator: access_point.bssid,
        supplicant_nonce: ready.security.supplicant_nonce,
        association_security_ies: selected_rsn.as_bytes(),
        authenticator_rsn_ie: access_point.rsn_ie_bytes(),
        authenticator_rsnxe: access_point.rsnxe_bytes(),
        pmk: ready.security.pmk,
    };
    let join_rx = core::mem::replace(rx, RadioHilJoinRx::Vacant);
    let backend = RadioHilWpa2Backend {
        mmio: hardware,
        rx_storage: ready.fixture.rx_storage,
        tx_storage: &mut *ready.fixture.tx_storage,
        descriptor_base: ready.fixture.descriptor_base,
        buffer_addresses: ready.fixture.buffer_addresses,
        frame: &mut *ready.fixture.frame,
        station_address,
        bssid: access_point.bssid,
        rx: join_rx,
        message2_transmissions: 0,
    };
    let mut runner =
        Wpa2HandshakeRunner::new(backend, EmbassyWpa2HandshakeTimer, Wpa2SoftwareAes::new());
    let handshake_started = Instant::now();
    let pending = match runner
        .run(handshake, ready.security.sequences.non_qos_mut())
        .await
    {
        Ok(pending) => pending,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-wpa2-handshake error={error:?} \
                 elapsed_ms={}",
                handshake_started.elapsed().as_millis(),
            ));
            let (backend, _, _) = runner.into_parts();
            *rx = backend.into_rx();
            return failed_reconnect(
                ready,
                StaLifecycleStage::Security,
                StaFailureDisposition::RetryCurrentCandidate,
                RadioHilStaLifecycleFailure::Wpa2Handshake,
            );
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-reconnect-wpa2-message3 frames={} \
         message2_transmissions={} replay={} elapsed_ms={}",
        pending.completed_frames(),
        pending.message2_transmissions(),
        pending.request().replay_counter(),
        handshake_started.elapsed().as_millis(),
    ));
    let (backend, _, _) = runner.into_parts();
    *rx = backend.into_rx();

    let backend = RadioHilWpa2KeyBackend {
        mmio: hardware,
        tx_storage: &mut *ready.fixture.tx_storage,
        station_address,
        bssid: access_point.bssid,
        peer_qos: link.peer_qos,
        sequences: ready.security.sequences,
        completion: None,
    };
    let mut runner = Wpa2KeyInstallRunner::new(backend);
    let established = match runner.run(pending).await {
        Ok(established) => established,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-wpa2-key-install error={error:?}"
            ));
            return failed_reconnect(
                ready,
                StaLifecycleStage::Security,
                StaFailureDisposition::RetryCurrentCandidate,
                RadioHilStaLifecycleFailure::Wpa2KeyInstall,
            );
        }
    };
    let metadata = established.metadata();
    let backend = runner.into_backend();
    let completion = backend
        .completion
        .expect("successful reconnect WPA2 key runner retains Message 4 completion");
    let RadioHilInstalledWpa2Keys { pairwise, group } = established.into_keys();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-reconnect-wpa2-complete replay={} \
         message4_status={} message4_primary={:#010x} \
         pairwise_slot={} group_slot={}",
        metadata.replay_counter,
        completion.status,
        completion.primary_word,
        pairwise.hardware_index(),
        group.hardware_index(),
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-reconnect-protected-data-deferred \
         reason=persistent-stack"
    ));

    let RadioHilReconnectReady {
        fixture,
        target,
        network,
        epoch,
        security,
    } = ready;
    let StaAssociationSecurity {
        pmk,
        supplicant_nonce,
        sequences,
    } = security;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-reconnect-connected-enter"
    ));
    let returned = run_connected_network(
        fixture,
        epoch,
        StaConnectedSession {
            link,
            network,
            rate_control: &mut peer_plan.rate_control,
            pmk,
            supplicant_nonce,
            sequences,
        },
        pairwise,
        group,
    )
    .await;
    let RadioHilConnectedEpochReturn {
        fixture,
        disconnected,
        security,
        exit,
    } = returned;
    let disconnected = qualify_disconnected_rx_restart(disconnected).await;
    let (network, epoch) = disconnected.into_reconnected_resources();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS \
         stage=production-reconnect-connected-returned"
    ));
    let owner = RadioHilReconnectReady {
        fixture,
        target,
        network: RadioHilStaNetwork::Running(network),
        epoch,
        security,
    };
    match exit {
        RadioHilConnectedExit::Disconnected => StaAttemptOutcome::Disconnected {
            owner,
            // This adapter currently owns only a proven same-peer frontier.
            // A future cold candidate variant must perform scan/auth before
            // it may return `Refresh` here.
            next_candidate: StaNextCandidate::Reuse,
        },
        RadioHilConnectedExit::Stopped => StaAttemptOutcome::Stopped { owner },
        RadioHilConnectedExit::HardwareFailure => failed_reconnect(
            owner,
            StaLifecycleStage::Hardware,
            StaFailureDisposition::Terminal,
            RadioHilStaLifecycleFailure::ConnectedHardware,
        ),
    }
}

async fn authenticate_target(
    fixture: &mut RadioHilJoinFixture<'_>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    sequence: &mut StaSequenceCounter,
) -> (bool, RadioHilJoinRx<'static>) {
    let StaJoinTarget {
        station_address,
        access_point,
    } = target;
    let state = &mut *fixture.state;
    let radio = &mut fixture.radio;
    let platform = &mut *radio.platform;
    let mmio = &mut *radio.mmio;
    let rx_storage = radio.rx_storage;
    let tx_storage = &mut *radio.tx_storage;
    let descriptor_base = radio.descriptor_base;
    let buffer_addresses = radio.buffer_addresses;
    let frame = &mut *radio.frame;
    let selection = select_sta_association(&access_point, STA_ASSOCIATION_PREFERENCE);
    let association_phy = selection.phy;
    let channel_or_frequency = selection.channel_or_frequency;
    let cbw = selection.cbw;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=sta-channel-select primary={} \
         channel_or_frequency={channel_or_frequency} cbw={cbw} phy={association_phy:?} \
         ht_capability={:02x?} ht_operation={:02x?} \
         he_capability={:02x?} he_operation={:02x?}",
        access_point.channel,
        access_point.ht_capability_ie_bytes(),
        access_point.ht_operation_ie_bytes(),
        access_point.he_capability_ie_bytes(),
        access_point.he_operation_ie_bytes(),
    ));
    if let Err(error) =
        switch_channel_with_mac_restart(state, channel_or_frequency, cbw, platform, mmio).await
    {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-channel \
             channel={} error={error:?}",
            access_point.channel,
        ));
        return (false, rx);
    }
    // The vendor HE-node lifecycle remains deferred until Association.
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=sta-he-bsr deferred=post-association"
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=sta-he-operation deferred=post-association"
    ));
    configure_sta_link_receive_policy(mmio, access_point.bssid);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=sta-link-rx-policy \
         frame_policy={:#010x} address_policy={:#010x} \
         sniffer_policy={:#010x} misc_policy={:#010x}",
        mmio.read32(mac_registers::RX_FILTER[0]),
        mmio.read32(mac_registers::BSSID_HIGH[0]),
        read_diagnostic_mmio(0x2010_40e4),
        read_diagnostic_mmio(0x2010_40f4),
    ));

    let backend = RadioHilStaJoinBackend {
        mmio,
        rx_storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        frame,
        station_address,
        access_point,
        rx,
    };
    let mut runner = StaJoinRunner::new(backend, EmbassyStaJoinTimer);
    let result = runner
        .authenticate(station_address, access_point.bssid, sequence)
        .await;
    let (backend, _) = runner.into_parts();
    let rx = backend.into_rx();
    match result {
        Ok(success) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=sta-auth-response \\
                 attempt={} frames={} bssid={:02x?}",
                success.attempt, success.total_received_frames, access_point.bssid,
            ));
            (true, rx)
        }
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-runner \\
                 error={error:?} bssid={:02x?}",
                access_point.bssid,
            ));
            (false, rx)
        }
    }
}

/// Allocate the station/network ownership graph exactly once.
///
/// Keep both 32-frame queues out of the task stack. Passing
/// `NetworkResources::new()` to `StaticCell::init` materializes a temporary of
/// more than 100 KiB and previously corrupted the saved channel waker. A
/// reconnect never calls this function: it receives `RadioHilStaNetwork::Running`
/// from the completed connected epoch.
fn initialize_sta_network(station_address: [u8; 6]) -> RadioHilStaNetwork {
    let resources = NetworkResources::init_in_place(OPEN_RADIO_NETWORK_RESOURCES.uninit());
    let tx_pool = NetworkTxPool::pin_static(NetworkTxPool::init_in_place(
        OPEN_RADIO_NETWORK_TX_POOL.uninit(),
    ));
    let (device, runner) = resources.split(tx_pool, station_address);
    RadioHilStaNetwork::Unstarted { device, runner }
}

async fn associate_target<'fixture, 'security>(
    fixture: RadioHilConnectedFixture<'fixture>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    network: RadioHilStaNetwork,
    security: StaAssociationSecurity<'security>,
) -> RadioHilJoinOutcome<'fixture, 'security> {
    let StaJoinTarget {
        station_address,
        access_point,
    } = target;
    let association_phy = select_sta_association(&access_point, STA_ASSOCIATION_PREFERENCE).phy;
    let peer_scan_policy = match StaPeerScanPolicy::new(&access_point) {
        Ok(policy) => policy,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-peer-scan-policy error={error:?}"
            ));
            return failed_join(fixture, target, rx, network, security, false).into();
        }
    };
    fixture
        .tx_storage
        .install_ht_ampdu_policy(peer_scan_policy.ht_ampdu);
    fixture
        .tx_storage
        .install_he_bss_color(peer_scan_policy.he_bss_color);
    if let Some(parameters) = peer_scan_policy.wmm.parameters() {
        if let Err(error) = fixture.tx_storage.install_wmm_edca(parameters) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-wmm-edca \
                 source=scan error={error:?}"
            ));
            return failed_join(fixture, target, rx, network, security, false).into();
        }
    }
    let backend = RadioHilStaJoinBackend {
        mmio: &mut *fixture.mmio,
        rx_storage: fixture.rx_storage,
        tx_storage: &mut *fixture.tx_storage,
        descriptor_base: fixture.descriptor_base,
        buffer_addresses: fixture.buffer_addresses,
        frame: &mut *fixture.frame,
        station_address,
        access_point,
        rx,
    };
    let mut runner = StaJoinRunner::new(backend, EmbassyStaJoinTimer);
    let association_started = Instant::now();
    let result = runner
        .associate(
            station_address,
            access_point.bssid,
            security.sequences.non_qos_mut(),
        )
        .await;
    let (backend, _) = runner.into_parts();
    let rx = backend.into_rx();
    let success = match result {
        Ok(success) => success,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-runner \\
                 error={error:?} bssid={:02x?}",
                access_point.bssid,
            ));
            return failed_join(fixture, target, rx, network, security, false).into();
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=sta-assoc-timing elapsed_ms={}",
        association_started.elapsed().as_millis(),
    ));
    let mut rx = rx;
    let response = success.response;
    let received_frames = success.total_received_frames;
    {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=sta-assoc-response \
                     status={} aid={} ht={} he_cap={} he_op={} wmm={} \
                     frames={received_frames} bssid={:02x?}",
            response.status_code,
            response.association_id,
            response.ht_capability,
            response.he_capability,
            response.he_operation,
            response.wmm,
            access_point.bssid,
        ));
        let selected_rsn = match select_wpa2_psk_rsn(&access_point) {
            Ok(rsn) => rsn,
            Err(error) => {
                let _ = rx.stop(fixture.mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-rsn-select error={error:?}"
                ));
                return failed_join(fixture, target, rx, network, security, true).into();
            }
        };
        let noise_floor_dbm = fixture.mmio.read_noise_floor_dbm();
        let mut peer_plan = match peer_scan_policy.complete(
            &access_point,
            &response,
            association_phy,
            noise_floor_dbm,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let _ = rx.stop(fixture.mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=sta-peer-association-plan error={error:?}"
                ));
                return failed_join(fixture, target, rx, network, security, true).into();
            }
        };
        fixture
            .tx_storage
            .install_ht_ampdu_policy(peer_plan.ht_ampdu);
        fixture
            .tx_storage
            .install_he_bss_color(peer_plan.he_bss_color);
        if peer_plan.wmm.source() == StaWmmSource::AssociationResponse {
            let Some(parameters) = peer_plan.wmm.parameters() else {
                let _ = rx.stop(fixture.mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-wmm-edca \
                             source=association-response error=missing-parameters"
                ));
                return failed_join(fixture, target, rx, network, security, true).into();
            };
            if let Err(error) = fixture.tx_storage.install_wmm_edca(parameters) {
                let _ = rx.stop(fixture.mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-wmm-edca \
                             source=association-response error={error:?}"
                ));
                return failed_join(fixture, target, rx, network, security, true).into();
            }
        }
        if let Some(state) = peer_plan.he_peer_state {
            if let Err(error) =
                program_he20_peer_state(fixture.mmio, state, response.association_id, 0, 0)
            {
                let _ = rx.stop(fixture.mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-he20-peer \
                             error={error:?}"
                ));
                return failed_join(fixture, target, rx, network, security, true).into();
            }
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=sta-he20-peer \
                         max_rate_code={} padding_8us={} operation={:#08x} \
                         color={:#04x} basic_mcs={:#06x} \
                         ersu_disabled={} ersu_permitted={}",
                state.max_rate_code,
                state.packet_padding_eight_us,
                state.operation_parameters,
                state.bss_color_information,
                state.basic_mcs_nss_map,
                state.extended_range_single_user_disabled,
                state.extended_range_single_user_permitted(),
            ));
        }
        let peer_he_capabilities = peer_plan.he_capabilities;
        if let Some(capability) = peer_he_capabilities {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=he20-peer-capabilities \
                         stbc_tx_under_80={} stbc_rx_under_80={} \
                         one_ltf_800ns_gi={} ldpc={} \
                         dcm_tx={:?} dcm_rx={:?} trig_su_feedback={} \
                         trig_mu_feedback={} trig_cqi={} non_trig_cqi={}",
                capability.stbc_transmit_under_80_mhz,
                capability.stbc_receive_under_80_mhz,
                capability.supports_one_ltf_800ns_gi(),
                capability.ldpc_coding_in_payload,
                capability.dcm_transmit,
                capability.dcm_receive,
                capability.triggered_su_beamforming_feedback,
                capability.triggered_mu_beamforming_partial_bandwidth_feedback,
                capability.triggered_cqi_feedback,
                capability.non_triggered_cqi_feedback,
            ));
        }
        let peer_qos = peer_plan.peer_qos;
        let peer_supports_short_guard_interval =
            peer_he_capabilities.is_some_and(|capability| capability.supports_one_ltf_800ns_gi());
        let peer_supports_ldpc = peer_he_capabilities
            .is_some_and(|capability| capability.supports_ldpc_coding_in_payload());
        let peer_dcm_constellation = peer_he_capabilities
            .map_or(HeDcmConstellation::NotSupported, |capability| {
                capability.dcm_receive_constellation()
            });
        let link_metric = peer_plan.link_metric;
        let rate_control = &mut peer_plan.rate_control;
        if let Err(error) = rate_control.program_hardware(fixture.mmio) {
            let _ = rx.stop(fixture.mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-rate-control \
                         error={error:?}"
            ));
            return failed_join(fixture, target, rx, network, security, true).into();
        }
        let rate_schedule = schedule_state(rate_control.current_schedule());
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=sta-rate-control \
                     rssi_dbm={} noise_floor_dbm={} metric={} \
                     schedule={:?}/{} rate={:#04x} max_schedule={} count={} \
                     ampdu_limit_rate={:?} bf_mode={} bf_rate={}",
            access_point.rssi,
            noise_floor_dbm,
            link_metric.value(),
            rate_control.current_schedule().kind,
            rate_control.current_schedule().index,
            rate_schedule.rate,
            rate_control.maximum_schedule_index(),
            rate_control.schedule_count(),
            rate_control.ampdu_limit_rate(),
            rate_control.beamforming_report().signal_mode(),
            rate_control.beamforming_report().rate_code(),
        ));
        let link = StaConnectedLink {
            station_address,
            bssid: access_point.bssid,
            association_id: response.association_id,
            beacon_interval_tu: access_point.beacon_interval_tu,
            peer_qos,
            association_phy,
            peer_supports_one_ltf_800ns_gi: peer_supports_short_guard_interval,
            peer_supports_ldpc,
            peer_dcm_receive: peer_dcm_constellation,
        };
        let StaAssociationSecurity {
            pmk,
            supplicant_nonce,
            sequences,
        } = security;
        let handshake = Wpa2HandshakeConfig {
            local: station_address,
            authenticator: access_point.bssid,
            supplicant_nonce,
            association_security_ies: selected_rsn.as_bytes(),
            authenticator_rsn_ie: access_point.rsn_ie_bytes(),
            authenticator_rsnxe: access_point.rsnxe_bytes(),
            pmk,
        };
        let session = StaConnectedSession {
            link,
            network,
            rate_control,
            pmk,
            supplicant_nonce,
            sequences,
        };
        return await_wpa2_message_1(fixture, target, rx, handshake, session).await;
    }
}

async fn await_wpa2_message_1<'fixture, 'rate, 'security>(
    fixture: RadioHilConnectedFixture<'fixture>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    handshake: Wpa2HandshakeConfig<'_>,
    session: StaConnectedSession<'rate, 'security>,
) -> RadioHilJoinOutcome<'fixture, 'security> {
    let link = session.link;
    let backend = RadioHilWpa2Backend {
        mmio: &mut *fixture.mmio,
        rx_storage: fixture.rx_storage,
        tx_storage: &mut *fixture.tx_storage,
        descriptor_base: fixture.descriptor_base,
        buffer_addresses: fixture.buffer_addresses,
        frame: &mut *fixture.frame,
        station_address: link.station_address,
        bssid: link.bssid,
        rx,
        message2_transmissions: 0,
    };
    let mut runner =
        Wpa2HandshakeRunner::new(backend, EmbassyWpa2HandshakeTimer, Wpa2SoftwareAes::new());
    let handshake_started = Instant::now();
    let pending = match runner.run(handshake, session.sequences.non_qos_mut()).await {
        Ok(pending) => pending,
        Err(error) => {
            let message1_complete = runner.backend().message2_transmissions != 0;
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-handshake-runner \
                 message1_complete={message1_complete} error={error:?} bssid={:02x?}",
                link.bssid,
            ));
            let (backend, _, _) = runner.into_parts();
            let rx = backend.into_rx();
            return failed_join_from_session(
                fixture,
                target,
                rx,
                session,
                message1_complete,
            )
            .into();
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-3-key-request \
         frames={} message2_transmissions={} replay={} elapsed_ms={}",
        pending.completed_frames(),
        pending.message2_transmissions(),
        pending.request().replay_counter(),
        handshake_started.elapsed().as_millis(),
    ));
    let (backend, _, _) = runner.into_parts();
    let rx = backend.into_rx();

    complete_wpa2_key_install_and_connect(fixture, target, pending, session, rx).await
}

async fn complete_wpa2_key_install_and_connect<'fixture, 'rate, 'security>(
    fixture: RadioHilConnectedFixture<'fixture>,
    target: StaJoinTarget,
    pending: Wpa2PendingKeyInstall,
    session: StaConnectedSession<'rate, 'security>,
    mut rx: RadioHilJoinRx<'static>,
) -> RadioHilJoinOutcome<'fixture, 'security> {
    let RadioHilConnectedFixture {
        spawner,
        protocol_spawner,
        platform,
        mmio,
        interrupt_setup,
        rx_storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        frame,
        ethernet,
    } = fixture;
    let StaConnectedSession {
        link,
        mut network,
        rate_control,
        pmk,
        supplicant_nonce,
        sequences,
    } = session;
    let StaConnectedLink {
        station_address,
        bssid,
        association_id: _,
        beacon_interval_tu: _,
        peer_qos,
        association_phy,
        peer_supports_one_ltf_800ns_gi: _,
        peer_supports_ldpc: _,
        peer_dcm_receive: _,
    } = link;
    let backend = RadioHilWpa2KeyBackend {
        mmio,
        tx_storage,
        station_address,
        bssid,
        peer_qos,
        sequences,
        completion: None,
    };
    let mut runner = Wpa2KeyInstallRunner::new(backend);
    let result = runner.run(pending).await;
    let backend = runner.into_backend();
    let RadioHilWpa2KeyBackend {
        mmio,
        tx_storage,
        station_address: _,
        bssid: _,
        peer_qos: _,
        sequences,
        completion,
    } = backend;
    let established = match result {
        Ok(established) => established,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-key-install-runner \
                 error={error:?} bssid={bssid:02x?}"
            ));
            return RadioHilJoinFailure::new(
                RadioHilJoinRetry {
                    fixture: RadioHilConnectedFixture {
                        spawner,
                        protocol_spawner,
                        platform,
                        mmio,
                        interrupt_setup,
                        rx_storage,
                        tx_storage,
                        descriptor_base,
                        buffer_addresses,
                        frame,
                        ethernet,
                    },
                    target,
                    rx,
                    network,
                    security: StaAssociationSecurity {
                        pmk,
                        supplicant_nonce,
                        sequences,
                    },
                },
                true,
                true,
                false,
            )
            .into();
        }
    };
    let metadata = established.metadata();
    let completion = completion
        .expect("successful WPA2 key runner retains Message 4 completion");
    let RadioHilInstalledWpa2Keys {
        pairwise: mut key_slot,
        group: group_slot,
    } = established.into_keys();
    let replay_counter = metadata.replay_counter;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-3-key-data \
         encrypted={} plain={} gtk_id={} gtk_tx={}",
        metadata.encrypted_key_data,
        metadata.plain_key_data_len,
        metadata.group_key_id,
        metadata.group_transmit,
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-pairwise-key-install \
         slot={} valid={} peer_control={:#010x} crypto_control={:#010x} \
         crypto_policy={:#010x}",
        key_slot.hardware_index(),
        mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP) & (1 << key_slot.hardware_index()) != 0,
        mmio.read32(
            mac_pac::crypto_key_entry_word(key_slot.hardware_index(), 1)
                .expect("fixed pairwise slot metadata word"),
        ),
        mmio.read32(mac_pac::CRYPTO_INTERFACE_CONTROL[0]),
        mmio.read32(mac_pac::CRYPTO_POLICY_CONTROL),
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-group-key-install \
         slot={} gtk_id={} valid={} control={:#010x}",
        group_slot.hardware_index(),
        group_slot.key_id(),
        mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP) & (1 << group_slot.hardware_index()) != 0,
        mmio.read32(
            mac_pac::crypto_key_entry_word(group_slot.hardware_index(), 1)
                .expect("fixed group slot metadata word"),
        ),
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-3 \
         replay={} mic=true encrypted_key_data={} bssid={bssid:02x?}",
        replay_counter, metadata.encrypted_key_data,
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-4-build \
         protocol_version=1 replay={} bytes={}",
        replay_counter, metadata.message4_len,
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-4-tx \
         protected={} replay={} status={} primary={:#010x}",
        WPA2_MESSAGE_4_HARDWARE_PROTECTED,
        replay_counter,
        completion.status,
        completion.primary_word,
    ));
    let hardware_index = key_slot.hardware_index();
    let message4_valid = true;
    let message4_sent = true;
    let protected_arp_pass = if message4_sent {
        // M4 TX completion only proves that the AP acknowledged
        // the EAPOL MPDU. The vendor STA path reports the connected
        // event separately, after its EAPOL callback has completed,
        // so do not queue ordinary protected traffic on the same
        // scheduling edge.
        //
        // SOURCE: promoted migration STA connected/EAPOL callback
        // split; 2026-07-29 open-TX/hostapd HIL, where hostapd
        // completed the four-way handshake but four immediate ARP
        // MAC retries all returned status 5.
        Timer::after_millis(WPA2_CONTROLLED_PORT_SETTLE_MS).await;
        match &mut network {
            RadioHilStaNetwork::Running(_) => {
                // On reassociation the persistent stack already owns the
                // network device. Its ordinary connected traffic is the
                // protected-data assertion; manufacturing a second direct
                // device alias solely for this HIL probe would violate that
                // ownership boundary.
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS \
                     stage=wpa2-protected-arp-deferred reason=persistent-stack"
                ));
                true
            }
            RadioHilStaNetwork::Unstarted {
                device: network_device,
                runner: network_runner,
            } => {
                network_runner.set_link_state(LinkState::Up);
                let mut passed = false;
                for attempt in 1..=WPA2_PROTECTED_ARP_ATTEMPTS {
                    if let Err(error) = rx
                        .start(mmio, rx_storage, descriptor_base, buffer_addresses)
                        .await
                    {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                                     stage=wpa2-protected-rx-arm attempt={attempt} \
                                     error={error:?}"
                        ));
                        break;
                    }
            let Some(queued_arp) = queue_arp_probe(network_device, network_runner, station_address)
            else {
                let _ = rx.stop(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                                 stage=wpa2-protected-arp-tx attempt={attempt} \
                                 error=network-tx-token"
                ));
                break;
            };
            match transmit_protected_ethernet_frame(
                mmio,
                tx_storage,
                bssid,
                &mut key_slot,
                sequences
                    .take_data(peer_qos.then_some(0))
                    .expect("selected data sequence-number owner exists"),
                peer_qos,
                selected_data_tx_rate(association_phy, peer_qos),
                queued_arp.as_slice(),
            )
            .await
            {
                Ok(completion) => {
                    let transmitted = completion.status == 0;
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result={} \
                                     stage=wpa2-protected-arp-tx attempt={attempt} \
                                     status={} primary={:#010x} owned_tx=true",
                        if transmitted { "PASS" } else { "FAIL" },
                        completion.status,
                        completion.primary_word,
                    ));
                    if transmitted
                        && await_protected_arp_response(
                            mmio,
                            rx_storage,
                            frame,
                            ethernet,
                            network_device,
                            network_runner,
                            station_address,
                            bssid,
                            &mut rx,
                        )
                        .await
                    {
                        passed = true;
                        break;
                    }
                    if !transmitted {
                        let _ = rx.stop(mmio);
                    }
                }
                Err(error) => {
                    let _ = rx.stop(mmio);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                                     stage=wpa2-protected-arp-tx attempt={attempt} \
                                     error={error:?}"
                    ));
                }
            }
            if attempt < WPA2_PROTECTED_ARP_ATTEMPTS {
                // An ARP reply is ordinary data, not part of the
                // four-way handshake. Losing it must allocate a new
                // 802.11 sequence number and CCMP PN; it must not
                // roll the WPA state machine back to M2.
                //
                // SOURCE: IEEE 802.11 CCMP packet-number uniqueness;
                // 2026-07-29 HIL run 4, where hostapd remained
                // authorized after the first ARP response was lost.
                Timer::after_millis(WPA2_PROTECTED_ARP_RETRY_DELAY_MS).await;
            }
        }
                passed
            }
        }
    } else {
        false
    };
    if message4_valid && message4_sent && protected_arp_pass {
        let (connected_fixture, registers) = RadioHilConnectedFixture {
            spawner,
            protocol_spawner,
            platform,
            mmio,
            interrupt_setup,
            rx_storage,
            tx_storage,
            descriptor_base,
            buffer_addresses,
            frame,
            ethernet,
        }
        .into_task_fixture();
        let returned = run_connected_network(
            connected_fixture,
            RadioHilConnectedEpochResources::Initial {
                registers,
                rx,
            },
            StaConnectedSession {
                link,
                network,
                rate_control,
                pmk,
                supplicant_nonce,
                sequences,
            },
            key_slot,
            group_slot,
        )
        .await;
        let disconnected = qualify_disconnected_rx_restart(returned.disconnected).await;
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=production-connected-epoch-returned \
             descriptor_base={:#010x} queued_frames={}",
            disconnected.rx.ring().descriptor_base(),
            disconnected.rx.queued_frames(),
        ));
        let (network, reconnect_resources) = disconnected.into_reconnected_resources();
        let ready = RadioHilReconnectReady {
            fixture: returned.fixture,
            target,
            network: RadioHilStaNetwork::Running(network),
            epoch: reconnect_resources,
            security: returned.security,
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=production-reconnect-owner-ready"
        ));
        return RadioHilJoinOutcome::Connected {
            ready,
            exit: returned.exit,
        };
    }
    let group_hardware_index = group_slot.hardware_index();
    group_slot.clear(mmio);
    let group_key_cleared =
        mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP) & (1 << group_hardware_index) == 0;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result={} stage=wpa2-group-key-clear \
                     slot={group_hardware_index}",
        if group_key_cleared { "PASS" } else { "FAIL" },
    ));
    key_slot.clear(mmio);
    let key_cleared = mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP) & (1 << hardware_index) == 0;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result={} stage=wpa2-pairwise-key-clear slot={hardware_index}",
        if key_cleared { "PASS" } else { "FAIL" },
    ));
    let _cleanup_complete = message4_valid
        && message4_sent
        && group_key_cleared
        && key_cleared;
    RadioHilJoinFailure::new(
        RadioHilJoinRetry {
            fixture: RadioHilConnectedFixture {
                spawner,
                protocol_spawner,
                platform,
                mmio,
                interrupt_setup,
                rx_storage,
                tx_storage,
                descriptor_base,
                buffer_addresses,
                frame,
                ethernet,
            },
            target,
            rx,
            network,
            security: StaAssociationSecurity {
                pmk,
                supplicant_nonce,
                sequences,
            },
        },
        true,
        true,
        false,
    )
    .into()
}

/// Borrowed owner for the currently qualified cold scan transaction.
///
/// This is deliberately not the future running-rescan owner. It retains the
/// cold PAC view and the staged raw RX ring used before the one-way
/// `ColdRadioRegisters::into_running` transition.
struct RadioHilColdScanOwner<'hardware, 'ssid> {
    state: &'hardware mut PhyColdState,
    platform: &'hardware mut EspHalRadioPeripheral,
    mmio: ColdRadioRegisters,
    storage: &'static RxStorage,
    tx_storage: &'static mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    scan_table: &'static mut ScanTable,
    scan_frame: &'static mut [u8; RX_STAGE_CAPACITY],
    station_address: [u8; 6],
    target_ssid: &'ssid [u8],
    raw_frames: u32,
    probe_responses: u32,
    tx_completions: u32,
    tx_failures: u32,
    active_tx_available: bool,
    ring_epochs: u32,
    observed_mask: u64,
    channel_records_before: usize,
    channel_frames_before: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilColdScanPortError {
    ChannelSwitch,
    ReceiveStart,
    ReceiveStop,
    RingRebuild,
    RingRestart,
}

impl Esp32s31StaScanPort for RadioHilColdScanOwner<'_, '_> {
    type Channel = u8;
    type Candidate = ScanRecord;
    type Error = RadioHilColdScanPortError;

    fn begin_scan(
        &mut self,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            self.scan_table.clear();
            self.raw_frames = 0;
            self.probe_responses = 0;
            self.tx_completions = 0;
            self.tx_failures = 0;
            self.active_tx_available = true;
            self.ring_epochs = 0;
            self.observed_mask = 0;
            self.channel_records_before = 0;
            self.channel_frames_before = 0;
            Ok(())
        }
    }

    fn switch_channel(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            let channel = context.channel;
            if let Err(error) = switch_channel_with_mac_restart(
                self.state,
                u16::from(channel),
                0,
                self.platform,
                &mut self.mmio,
            )
            .await
            {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=scan-channel \
                     channel={channel} error={error:?}"
                ));
                return Err(RadioHilColdScanPortError::ChannelSwitch);
            }
            Ok(())
        }
    }

    fn start_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            let channel = context.channel;
            self.observed_mask = 0;
            self.channel_records_before = self.scan_table.summary().records;
            self.channel_frames_before = self.raw_frames;
            let rx_start = if context.index == 0 {
                enable_receive(&mut self.mmio)
            } else {
                publish_cold_ring(&mut self.mmio, self.descriptor_base, true)
            };
            if let Err(error) = rx_start {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-start \
                     channel={channel} error={error:?}"
                ));
                return Err(RadioHilColdScanPortError::ReceiveStart);
            }
            Ok(())
        }
    }

    fn transmit_active_probe(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> impl Future<Output = Esp32s31ActiveProbeOutcome> + '_ {
        async move {
            let channel = context.channel;
            if self.active_tx_available {
                self.mmio.clear_mac_interrupts(u32::MAX);
                match transmit_probe_request(
                    &mut self.mmio,
                    self.tx_storage,
                    self.station_address,
                    u16::from(channel),
                )
                .await
                {
                    Ok(completion) => {
                        self.tx_completions = self.tx_completions.saturating_add(1);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=probe-request-tx channel={channel} \
                             status={} alternate={} trigger={} primary={:#010x} alternate_word={:#010x}",
                            completion.status,
                            completion.used_alternate,
                            completion.trigger_flow,
                            completion.primary_word,
                            completion.alternate_word,
                        ));
                        if completion.status != 0 {
                            self.tx_failures = self.tx_failures.saturating_add(1);
                            self.active_tx_available = false;
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL stage=passive-fallback \
                                 channel={channel} tx_status={}",
                                 completion.status,
                            ));
                            return Esp32s31ActiveProbeOutcome::PassiveFallback;
                        }
                    }
                    Err(error) => {
                        self.tx_failures = self.tx_failures.saturating_add(1);
                        self.active_tx_available = false;
                        let control = self.mmio.read32(TX_Q_CONTROL[0]);
                        self
                            .mmio
                            .write32(TX_Q_CONTROL[0], control & !TX_Q_ENABLE_VALID);
                        Mmio::fence(&mut self.mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=passive-fallback \
                             channel={channel} tx_error={error:?}"
                        ));
                        return Esp32s31ActiveProbeOutcome::PassiveFallback;
                    }
                }
                Esp32s31ActiveProbeOutcome::Transmitted
            } else {
                Esp32s31ActiveProbeOutcome::PassiveFallback
            }
        }
    }

    fn observe_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        let channel = context.channel;
        observe_scan_descriptors(
            &mut self.mmio,
            self.storage,
            self.descriptor_base,
            self.scan_table,
            self.scan_frame,
            self.station_address,
            channel,
            &mut self.observed_mask,
            &mut self.raw_frames,
            &mut self.probe_responses,
        );
        if self.observed_mask != RX_DESCRIPTOR_COMPLETE_MASK {
            return Ok(());
        }

        if let Err(error) = disable_receive(&mut self.mmio) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-disable \
                 channel={channel} error={error:?}"
            ));
            return Err(RadioHilColdScanPortError::ReceiveStop);
        }
        if let Err(error) = build_cold_ring(
            self.storage.descriptors(),
            self.descriptor_base,
            self.buffer_addresses,
            RX_BUFFER_SIZE as u32,
        ) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-rebuild \
                 channel={channel} error={error:?}"
            ));
            return Err(RadioHilColdScanPortError::RingRebuild);
        }
        if let Err(error) = publish_cold_ring(&mut self.mmio, self.descriptor_base, true) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-restart \
                 channel={channel} error={error:?}"
            ));
            return Err(RadioHilColdScanPortError::RingRestart);
        }
        self.observed_mask = 0;
        self.ring_epochs = self.ring_epochs.saturating_add(1);
        Ok(())
    }

    fn wait_dwell_tick(
        &mut self,
    ) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async {
            Timer::after_millis(1).await;
            Ok(())
        }
    }

    fn stop_receive(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        let channel = context.channel;
        if let Err(error) = disable_receive(&mut self.mmio) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-disable \
                 channel={channel} error={error:?}"
            ));
            return Err(RadioHilColdScanPortError::ReceiveStop);
        }
        observe_scan_descriptors(
            &mut self.mmio,
            self.storage,
            self.descriptor_base,
            self.scan_table,
            self.scan_frame,
            self.station_address,
            channel,
            &mut self.observed_mask,
            &mut self.raw_frames,
            &mut self.probe_responses,
        );
        let channel_summary = self.scan_table.summary();
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=scan-channel-complete channel={channel} \
             raw_frames={} new_records={} mask={:#010x}",
            self.raw_frames - self.channel_frames_before,
            channel_summary.records - self.channel_records_before,
            self.observed_mask,
        ));
        Ok(())
    }

    fn prepare_next_ring(
        &mut self,
        context: StaScanChannelContext<Self::Channel>,
    ) -> Result<(), Self::Error> {
        let channel = context.channel;
        build_cold_ring(
            self.storage.descriptors(),
            self.descriptor_base,
            self.buffer_addresses,
            RX_BUFFER_SIZE as u32,
        )
        .map_err(|error| {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-rebuild \
                 channel={channel} error={error:?}"
            ));
            RadioHilColdScanPortError::RingRebuild
        })
    }

    fn select_candidate(&mut self) -> Result<Option<Self::Candidate>, Self::Error> {
        Ok(best_matching_ssid(self.scan_table.records(), self.target_ssid).copied())
    }
}

async fn run_promiscuous_rx_hil(
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    state: &mut PhyColdState,
    mut platform: EspHalRadioPeripheral,
    mut cold_mmio: ColdRadioRegisters,
    trng: &Trng,
    network_credentials: &mut NetworkCredentials,
) -> bool {
    let platform = &mut platform;
    let mmio = &mut cold_mmio;
    let storage = RxStorage::init_in_place(OPEN_RADIO_RX_DMA_STORAGE.uninit());
    let tx_slot = TxSlot::pin_static(TxSlot::init_in_place(OPEN_RADIO_TX_DMA_STORAGE.uninit()));
    let tx_storage = OPEN_RADIO_TX_STATE.init(TxStorage::new(
        tx_slot,
        state
            .tx_target_power_profile()
            .with_maximum_quarter_dbm(OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM),
    ));
    let descriptor_base = storage.descriptors().as_ptr().addr() as u32;
    let buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT] =
        OPEN_RADIO_RX_BUFFER_ADDRESSES.init(core::array::from_fn(|index| {
            storage.buffers()[index].dma_address().unwrap()
        }));

    if let Err(error) = build_cold_ring(
        storage.descriptors(),
        descriptor_base,
        buffer_addresses,
        RX_BUFFER_SIZE as u32,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-build error={error:?} \
             descriptor_base={descriptor_base:#010x}"
        ));
        return false;
    }

    let mut station_address = [0_u8; 6];
    station_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::Station).as_bytes());
    let mut access_point_address = [0_u8; 6];
    access_point_address
        .copy_from_slice(efuse::interface_mac_address(InterfaceMacAddress::AccessPoint).as_bytes());
    let cold = match initialize_promiscuous_receive(
        platform,
        mmio,
        MAC_HANDSHAKE_SAMPLE_LIMIT,
        station_address,
        access_point_address,
    ) {
        Ok(outcome) => outcome,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=mac-cold-start error={error:?}"
            ));
            return false;
        }
    };
    // Cold `hal_init` publishes its production interrupt mask, but this
    // bounded scan/auth path has no task-side IRQ consumer yet and polls RX/TX
    // ownership directly. Keep the CPU line quiescent until the connected
    // path enables the ISR-owned RX/TX Signal sink.
    let cold_interrupt_mask = mmio.mac_interrupt_enable();
    mmio.mask_and_clear_mac_interrupts(u32::MAX);
    // Match the vendor cold path rather than collapsing two independently
    // recovered hardware edges. `wDev_AppendRxBlocks` publishes the base first;
    // the later `chip_enable` path opens RX only after the remaining MAC/channel
    // setup. The first-boot failure was localized to this exact boundary:
    // rewriting the same base followed by a fresh enable edge restored RX
    // without resetting either PHY or MAC.
    if let Err(error) = publish_cold_ring(mmio, descriptor_base, false) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-stage error={error:?}"
        ));
        return false;
    }
    let scan_table = SCAN_TABLE.init(ScanTable::new());
    scan_table.clear();
    let scan_frame = SCAN_FRAME.init([0; RX_STAGE_CAPACITY]);
    let ethernet_frame = ETHERNET_FRAME.init([0; RX_STAGE_CAPACITY]);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=rx-active descriptor_base={descriptor_base:#010x} \
         buffer0={:#010x} handshake_samples={} handshake_value={:#010x} \
         control={:#010x} base={:#010x} next={:#010x} last={:#010x} high={:#010x} \
         int_raw={:#010x} int_status={:#010x} \
         cold_int_mask={cold_interrupt_mask:#010x} \
         he={:#010x}/{:#010x}/{:#010x} filter={:#010x} low_rate={:#010x}",
        buffer_addresses[0],
        cold.handshake_samples,
        cold.handshake_value,
        mmio.read32(RX_CONTROL),
        mmio.read32(RX_DESCRIPTOR_BASE),
        mmio.read32(RX_NEXT_DESCRIPTOR),
        mmio.read32(RX_LAST_DESCRIPTOR),
        mmio.read32(RX_LAST_DESCRIPTOR_HIGH),
        mmio.read32(MAC_INT_RAW),
        mmio.read32(MAC_INT_STATUS),
        read_diagnostic_mmio(0x2010_4c80),
        read_diagnostic_mmio(0x2010_4c88),
        read_diagnostic_mmio(0x2010_4cc0),
        read_diagnostic_mmio(0x2010_4020),
        mmio.read32(mac_registers::R_8060),
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL probe=mac-rx-diff \
         gate407c={:#010x} sniffer={:#010x}/{:#010x} \
         policy={:#010x}/{:#010x}/{:#010x}/{:#010x} \
         subtype={:#010x} timing={:#010x} he_flags={:#010x} \
         mac_control={:#010x} regdma={:#010x}",
        mmio.read32(mac_registers::R_407C),
        read_diagnostic_mmio(0x2010_40e4),
        read_diagnostic_mmio(0x2010_40f4),
        mmio.read32(mac_registers::RX_QUEUE_DEFAULT[0]),
        mmio.read32(mac_registers::RX_QUEUE_DEFAULT[1]),
        mmio.read32(mac_registers::RX_QUEUE_DEFAULT[2]),
        mmio.read32(mac_registers::RX_QUEUE_DEFAULT[3]),
        mmio.read32(mac_registers::R_4114),
        mmio.read32(mac_registers::R_4C20),
        mmio.read32(mac_registers::R_4C98),
        mmio.read32(mac_registers::CONTROL),
        mmio.read32(mac_registers::R_D83C),
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL probe=phy-rx-diff \
         frequency={:#010x}/{:#010x}/{:#010x} \
         clocks={:#010x}/{:#010x} agc={:#010x}/{:#010x} \
         rx={:#010x}/{:#010x}/{:#010x}/{:#010x}/{:#010x}",
        read_diagnostic_mmio(0x2010_001c),
        read_diagnostic_mmio(0x2010_0024),
        read_diagnostic_mmio(0x2010_0028),
        read_diagnostic_mmio(0x2010_0400),
        read_diagnostic_mmio(0x2010_0408),
        read_diagnostic_mmio(0x2010_705c),
        read_diagnostic_mmio(0x2010_7064),
        read_diagnostic_mmio(0x2010_70a0),
        read_diagnostic_mmio(0x2010_7104),
        read_diagnostic_mmio(0x2010_7114),
        read_diagnostic_mmio(0x2010_7848),
        read_diagnostic_mmio(0x2010_78c8),
    ));

    let scan_started = Instant::now();
    let scan_owner = RadioHilColdScanOwner {
        state,
        platform,
        mmio: cold_mmio,
        storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        scan_frame,
        station_address,
        target_ssid: network_credentials.ssid(),
        raw_frames: 0,
        probe_responses: 0,
        tx_completions: 0,
        tx_failures: 0,
        active_tx_available: true,
        ring_epochs: 0,
        observed_mask: 0,
        channel_records_before: 0,
        channel_frames_before: 0,
    };
    let scan_config = Esp32s31StaScanConfig::new(SCAN_DWELL_MS)
        .expect("fixed HIL scan dwell policy is nonzero");
    let scan_backend = Esp32s31StaScanBackend::new(scan_config);
    let mut scan_service = StaCandidateScanService::new(scan_backend);
    let (scan_owner, primary_target) = match scan_service
        .run(scan_owner, &STA_SCAN_CHANNELS)
        .await
    {
        StaCandidateScanExit::Selected {
            owner, candidate, ..
        } => (owner, Some(candidate)),
        StaCandidateScanExit::NoCandidate { owner, .. } => (owner, None),
        StaCandidateScanExit::Stopped { owner, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=cold-scan-service-stop \
                 channels_completed={}",
                progress.channels_completed,
            ));
            let _owner = owner;
            return false;
        }
        StaCandidateScanExit::Failed {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=cold-scan-service \
                 channels_completed={} error={error:?}",
                progress.channels_completed,
            ));
            let _owner = owner;
            return false;
        }
        StaCandidateScanExit::InvalidPlan {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=cold-scan-plan \
                 channels_planned={} error={error:?}",
                progress.channels_planned,
            ));
            let _owner = owner;
            return false;
        }
    };
    let RadioHilColdScanOwner {
        state,
        platform,
        mmio: mut cold_mmio,
        storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        scan_frame,
        station_address,
        target_ssid: _,
        mut raw_frames,
        mut probe_responses,
        mut tx_completions,
        mut tx_failures,
        active_tx_available: _,
        ring_epochs,
        observed_mask: _,
        channel_records_before: _,
        channel_frames_before: _,
    } = scan_owner;
    let mmio = &mut cold_mmio;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=active-scan-timing channels={} elapsed_ms={}",
        STA_SCAN_CHANNEL_COUNT,
        scan_started.elapsed().as_millis(),
    ));

    // First isolate the RX descriptor publication edge. This does not reset or
    // reconfigure either PHY or MAC; it only returns ownership of the stopped
    // walker to Rust, rebuilds the same ring, and republishes it.
    if raw_frames == 0 {
        let channel = sta_scan_channel(0);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=rx-dma-rearm-start channel={channel} \
             control={:#010x} base={:#010x} next={:#010x} last={:#010x} \
             int_raw={:#010x}",
            mmio.read32(RX_CONTROL),
            mmio.read32(RX_DESCRIPTOR_BASE),
            mmio.read32(RX_NEXT_DESCRIPTOR),
            mmio.read32(RX_LAST_DESCRIPTOR),
            mmio.read32(MAC_INT_RAW),
        ));
        if disable_receive(mmio).is_err()
            || build_cold_ring(
                storage.descriptors(),
                descriptor_base,
                buffer_addresses,
                RX_BUFFER_SIZE as u32,
            )
            .is_err()
            || publish_cold_ring(mmio, descriptor_base, true).is_err()
        {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-dma-rearm-ring"
            ));
            return false;
        }

        mmio.clear_mac_interrupts(u32::MAX);
        match transmit_probe_request(mmio, tx_storage, station_address, u16::from(channel)).await {
            Ok(completion) => {
                tx_completions = tx_completions.saturating_add(1);
                if completion.status != 0 {
                    tx_failures = tx_failures.saturating_add(1);
                }
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=rx-dma-rearm-probe channel={channel} \
                     status={}",
                    completion.status,
                ));
            }
            Err(error) => {
                tx_failures = tx_failures.saturating_add(1);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-dma-rearm-probe error={error:?}"
                ));
            }
        }

        let records_before = scan_table.summary().records;
        let mut observed_mask = 0_u64;
        for _ in 0..SCAN_DWELL_MS {
            observe_scan_descriptors(
                mmio,
                storage,
                descriptor_base,
                scan_table,
                scan_frame,
                station_address,
                channel,
                &mut observed_mask,
                &mut raw_frames,
                &mut probe_responses,
            );
            Timer::after_millis(1).await;
        }
        let _ = disable_receive(mmio);
        observe_scan_descriptors(
            mmio,
            storage,
            descriptor_base,
            scan_table,
            scan_frame,
            station_address,
            channel,
            &mut observed_mask,
            &mut raw_frames,
            &mut probe_responses,
        );
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result={} stage=rx-dma-rearm \
             raw_frames={raw_frames} new_records={} mask={observed_mask:#010x}",
            if raw_frames == 0 { "FAIL" } else { "PASS" },
            scan_table.summary().records - records_before,
        ));
    }

    // A first boot immediately following OTA selection has intermittently
    // completed PHY calibration and TX while producing no RX descriptors.
    // If republishing the DMA edge alone did not recover it, reset only
    // WIFIMAC, repeat MAC initialization, and retain the calibrated PHY.
    if raw_frames == 0 {
        let channel = sta_scan_channel(0);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=mac-rx-recovery-start channel={channel} \
             control={:#010x} base={:#010x} next={:#010x} last={:#010x} \
             int_raw={:#010x}",
            mmio.read32(RX_CONTROL),
            mmio.read32(RX_DESCRIPTOR_BASE),
            mmio.read32(RX_NEXT_DESCRIPTOR),
            mmio.read32(RX_LAST_DESCRIPTOR),
            mmio.read32(MAC_INT_RAW),
        ));

        if disable_receive(mmio).is_err()
            || build_cold_ring(
                storage.descriptors(),
                descriptor_base,
                buffer_addresses,
                RX_BUFFER_SIZE as u32,
            )
            .is_err()
        {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=mac-rx-recovery-ring"
            ));
            return false;
        }
        let recovery = match initialize_promiscuous_receive(
            platform,
            mmio,
            MAC_HANDSHAKE_SAMPLE_LIMIT,
            station_address,
            access_point_address,
        ) {
            Ok(outcome) => outcome,
            Err(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=mac-rx-recovery-init error={error:?}"
                ));
                return false;
            }
        };
        if let Err(error) =
            switch_channel_with_mac_restart(state, u16::from(channel), 0, platform, mmio).await
        {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=mac-rx-recovery-channel error={error:?}"
            ));
            return false;
        }
        if let Err(error) = publish_cold_ring(mmio, descriptor_base, true) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=mac-rx-recovery-publish error={error:?}"
            ));
            return false;
        }

        mmio.clear_mac_interrupts(u32::MAX);
        match transmit_probe_request(mmio, tx_storage, station_address, u16::from(channel)).await {
            Ok(completion) => {
                tx_completions = tx_completions.saturating_add(1);
                if completion.status != 0 {
                    tx_failures = tx_failures.saturating_add(1);
                }
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=mac-rx-recovery-probe channel={channel} \
                     status={} handshake_samples={}",
                    completion.status, recovery.handshake_samples,
                ));
            }
            Err(error) => {
                tx_failures = tx_failures.saturating_add(1);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=mac-rx-recovery-probe error={error:?}"
                ));
            }
        }

        let records_before = scan_table.summary().records;
        let mut observed_mask = 0_u64;
        for _ in 0..SCAN_DWELL_MS {
            observe_scan_descriptors(
                mmio,
                storage,
                descriptor_base,
                scan_table,
                scan_frame,
                station_address,
                channel,
                &mut observed_mask,
                &mut raw_frames,
                &mut probe_responses,
            );
            Timer::after_millis(1).await;
        }
        let _ = disable_receive(mmio);
        observe_scan_descriptors(
            mmio,
            storage,
            descriptor_base,
            scan_table,
            scan_frame,
            station_address,
            channel,
            &mut observed_mask,
            &mut raw_frames,
            &mut probe_responses,
        );
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result={} stage=mac-rx-recovery \
             raw_frames={raw_frames} new_records={} mask={observed_mask:#010x}",
            if raw_frames == 0 { "FAIL" } else { "PASS" },
            scan_table.summary().records - records_before,
        ));
    }

    let summary = scan_table.summary();
    let rx_dma_pass = summary.records != 0 && raw_frames != 0;
    let active_scan_pass =
        tx_completions >= STA_SCAN_CHANNEL_COUNT as u32 && probe_responses != 0 && tx_failures == 0;
    let target = primary_target.or_else(|| {
        best_matching_ssid(scan_table.records(), network_credentials.ssid()).copied()
    });
    // No cold MAC operation is permitted beyond this point. Consume the cold
    // owner before authentication and retain the inactive interrupt setup
    // token until WPA2 has opened the controlled port.
    let (running_mmio, interrupt_setup) = cold_mmio.into_running();
    let mmio: &'static mut RadioRegisters = OPEN_RADIO_RUNNING_REGISTERS.init(running_mmio);
    let mut interrupt_setup = Some(interrupt_setup);
    let (sta_auth_pass, sta_assoc_pass, wpa2_message_1_pass, wpa2_message_3_pass) = match target {
        Some(access_point) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=sta-target ssid={:?} bssid={:02x?} \
                 channel={} rssi={} rsn={}",
                access_point.ssid_bytes(),
                access_point.bssid,
                access_point.channel,
                access_point.rssi,
                access_point.rsn,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=wpa2-pmk-derive start iterations=4096"
            ));
            let pmk_started = Instant::now();
            let pmk_result =
                Pmk::derive(network_credentials.passphrase(), network_credentials.ssid());
            network_credentials.clear_passphrase();
            let pmk = match pmk_result {
                Ok(pmk) => pmk,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-pmk-derive error={error:?}"
                    ));
                    return false;
                }
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-pmk-derive elapsed_ms={}",
                pmk_started.elapsed().as_millis(),
            ));
            let mut supplicant_nonce = [0; 32];
            for word in supplicant_nonce.chunks_exact_mut(4) {
                word.copy_from_slice(&trng.random().to_le_bytes());
            }
            // Management/non-QoS traffic and each QoS TID own independent
            // twelve-bit sequence spaces. Seed every independent owner from
            // the entropy-qualified TRNG so a software reset does not replay
            // the previous peer epoch's initial values.
            //
            // SOURCE: complete `_oracles/libnet80211.a[ieee80211_ht.o]::
            // ieee80211_ampdu_request` instructions 0x9a..0xa2 read the AddBA
            // SSN from a TID-indexed node halfword. The 2026-07-30 air capture
            // proved that the former global counter let Action frames shift
            // the advertised TID0 SSN. The seed is visible on air and is not
            // cryptographic key material.
            let sequence_seed = (trng.random() & 0x0fff) as u16;
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=sta-sequence-session seed={sequence_seed}"
            ));
            let mut sequences = StaTxSequenceCounters::new(sequence_seed);
            let target = StaJoinTarget {
                station_address,
                access_point,
            };
            let join_fixture = RadioHilJoinFixture {
                state,
                radio: RadioHilConnectedFixture {
                    spawner,
                    protocol_spawner,
                    platform,
                    mmio,
                    interrupt_setup: &mut interrupt_setup,
                    rx_storage: storage,
                    tx_storage,
                    descriptor_base,
                    buffer_addresses,
                    frame: scan_frame,
                    ethernet: ethernet_frame,
                },
            };
            let owner = RadioHilStaLifecycleOwner::Authenticate(RadioHilAuthenticationReady {
                fixture: join_fixture,
                target,
                rx: RadioHilJoinRx::Initial,
                network: initialize_sta_network(station_address),
                security: StaAssociationSecurity {
                    pmk: &pmk,
                    supplicant_nonce,
                    sequences: &mut sequences,
                },
            });
            let policy = StaReconnectPolicy::new(3, 100, 1_000, 100)
                .expect("fixed HIL station reconnect policy is valid");
            let backend = RadioHilStaLifecycleBackend::new();
            let mut lifecycle = StaLifecycleService::new(backend, policy);
            let progress = match lifecycle
                .run_with_candidate(owner, StaNextCandidate::Reuse)
                .await
            {
                    StaLifecycleExit::Stopped { owner, progress } => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=PASS \
                             stage=production-sta-lifecycle-stop \
                             connected_epochs={} attempts={}",
                            progress.connected_epochs, progress.attempts_started,
                        ));
                        let completed_join =
                            matches!(&owner, RadioHilStaLifecycleOwner::Reconnect(_));
                        let _owner = owner;
                        (
                            completed_join,
                            completed_join,
                            completed_join,
                            completed_join,
                        )
                    }
                    StaLifecycleExit::Exhausted {
                        owner,
                        progress,
                        failure,
                    } => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=production-sta-lifecycle-exhausted \
                             connected_epochs={} attempts={} failure={failure:?}",
                            progress.connected_epochs, progress.attempts_started,
                        ));
                        let result = match failure.error {
                            RadioHilStaLifecycleFailure::Authentication => {
                                (false, false, false, false)
                            }
                            RadioHilStaLifecycleFailure::InitialJoin {
                                associated,
                                message1,
                                message3,
                            } => (true, associated, message1, message3),
                            _ => (true, true, true, true),
                        };
                        let _owner = owner;
                        result
                    }
                    StaLifecycleExit::Terminal {
                        owner,
                        progress,
                        failure,
                    } => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=production-sta-lifecycle-terminal \
                             connected_epochs={} attempts={} failure={failure:?}",
                            progress.connected_epochs, progress.attempts_started,
                        ));
                        let completed_join =
                            matches!(&owner, RadioHilStaLifecycleOwner::Reconnect(_));
                        let _owner = owner;
                        (
                            completed_join,
                            completed_join,
                            completed_join,
                            completed_join,
                        )
                    }
            };
            supplicant_nonce.fill(0);
            progress
        }
        None => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-target ssid={:?}",
                network_credentials.ssid(),
            ));
            (false, false, false, false)
        }
    };
    if rx_dma_pass && active_scan_pass {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=active-scan channels={} \
             tx_completions={tx_completions} tx_failures={tx_failures} \
             probe_responses={probe_responses} \
             records={} observed_frames={} raw_frames={} dropped={} ring_epochs={ring_epochs}",
            STA_SCAN_CHANNEL_COUNT,
            summary.records,
            summary.observed_frames,
            raw_frames,
            summary.dropped_unique_bss,
        ));
    } else if rx_dma_pass {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=passive-scan channels={} \
             dma=sram tx_completions={tx_completions} tx_failures={tx_failures} \
             probe_responses={probe_responses} records={} observed_frames={} \
             raw_frames={} dropped={} ring_epochs={ring_epochs}",
            STA_SCAN_CHANNEL_COUNT,
            summary.records,
            summary.observed_frames,
            raw_frames,
            summary.dropped_unique_bss,
        ));
    } else {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-dma channels={} \
             tx_completions={tx_completions} tx_failures={tx_failures} \
             probe_responses={probe_responses} \
             records={} observed_frames={} raw_frames={} dropped={} ring_epochs={ring_epochs}",
            STA_SCAN_CHANNEL_COUNT,
            summary.records,
            summary.observed_frames,
            raw_frames,
            summary.dropped_unique_bss,
        ));
    }
    rx_dma_pass
        && active_scan_pass
        && sta_auth_pass
        && sta_assoc_pass
        && wpa2_message_1_pass
        && wpa2_message_3_pass
}

/// Runs the complete open PHY, MAC and DMA scan qualification.
///
/// The caller owns system/RTOS initialization and transfers the unique Wi-Fi
/// peripheral token into this function. Keeping the workload outside the
/// standalone binary lets the same source run inside the relocated
/// PSRAM/PSRAM runtime image.
pub async fn run(
    spawner: Spawner,
    protocol_spawner: SendSpawner,
    platform: EspHalRadioPeripheral,
    trng: Trng,
) {
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL stage=network-config-waiting source=hil-protocol"
    ));
    let startup_configuration = crate::console::receive_startup_configuration().await;
    let mut network_credentials = startup_configuration.network_credentials;
    let calibration_record = startup_configuration
        .phy_calibration_record
        .map(PhyCalibrationRecord::from_bytes);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=network-config-received ssid_length={}",
        network_credentials.ssid().len(),
    ));
    set_diagnostic_stage(10);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL schema=8 writes=full-phy+mac-rx+mac-tx+active-scan mac=open \
         channel={LISTEN_CHANNEL}"
    ));

    // SAFETY: this isolated image owns the sole `WIFI` singleton and its
    // audited dependency graph excludes every vendor radio package.
    set_diagnostic_stage(20);
    let owned = unsafe { Radio::claim(platform) };
    set_diagnostic_stage(30);
    let mut powered = match owned.power_up() {
        Ok(powered) => powered,
        Err(failure) => {
            let error = failure.error();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_PRELUDE_HIL result=FAIL stage=power \
                 checkpoint={:?} expected={} observed={}",
                error.checkpoint, error.expected, error.observed
            ));
            halt();
        }
    };
    set_diagnostic_stage(40);
    // `register_chipv7_phy` always finishes `phy_bb_init` on channel 11.
    // Selecting the requested listen channel is a separate post-init call,
    // matching the vendor call graph instead of folding it into cold init.
    let efuse = esp_hal::peripherals::EFUSE::regs();
    let calibration_identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        mac_sys0: efuse.rd_mac_sys0().read().bits(),
        mac_sys1: efuse.rd_mac_sys1().read().bits(),
    };
    let mut transition = PhyRegisterTransition::with_default_profile_and_calibration(
        calibration_identity,
        calibration_record,
    );
    let mut port =
        TargetPhyRegisterPort::<_, EmbassyPhyDelay, _>::new(&mut powered, HilPhyObserver);
    let phy_started = Instant::now();
    loop {
        set_diagnostic_stage(100);
        let outcome = match run_phy_register(&mut transition, &mut port).await {
            Ok(outcome) => outcome,
            Err(error) => {
                match error {
                    PhyRegisterRunError::Lowering(error) => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=lowering error={error:?}"
                    )),
                    PhyRegisterRunError::Port(error) => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=port error={error:?} \
                         rf_operations={} baseband_operations={}",
                        port.counters().rf_operations,
                        port.counters().baseband_operations,
                    )),
                    PhyRegisterRunError::Transition(error) => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=transition error={error:?}"
                    )),
                    PhyRegisterRunError::Radio(error) => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=radio error={error:?}"
                    )),
                }
                break;
            }
        };
        set_diagnostic_stage(200);
        let phy_elapsed = phy_started.elapsed();
        let calibration_record = transition.take_calibration_record();
        let mut state = match transition.into_state() {
            Ok(state) => state,
            Err(_) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=take-state"
                ));
                break;
            }
        };
        let counters = port.counters();
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration={} calibration_path={:?} \
                     mmio={} delays={} reset_samples={} rf_operations={} \
                     baseband_operations={} elapsed_ms={}",
            outcome.full_calibration_performed,
            outcome.calibration_path,
            counters.mmio,
            counters.delays,
            counters.reset_samples,
            counters.rf_operations,
            counters.baseband_operations,
            phy_elapsed.as_millis(),
        ));
        // `TargetPhyRegisterPort` borrowed the complete radio while the PHY
        // transition was active.  The transition is now finished, so
        // release that borrow before lending the owned register block
        // to the MAC/RX HIL.
        drop(port);
        if let Some(record) = calibration_record.as_ref() {
            let disposition = match outcome.calibration_path {
                PhyCalibrationPath::FullUncached | PhyCalibrationPath::FullForRecord => {
                    StartupArtifactDisposition::Created
                }
                PhyCalibrationPath::FullAfterRejectedRecord => StartupArtifactDisposition::Replaced,
                PhyCalibrationPath::PartialRestored => StartupArtifactDisposition::Restored,
            };
            crate::console::publish_startup_artifact(
                disposition,
                phy_elapsed.as_micros(),
                record.bytes(),
            )
            .await;
        }
        // Match Espressif's `enable_phy_with_wifi_rx` lifecycle
        // wrapper.  PHY registration may leave WIFI_ENABLE cleared;
        // the powered radio owner must make RX/baseband live before
        // channel selection and MAC startup.
        powered.enable_wifi_rx();
        let (platform, registers) = powered.parts_mut();
        set_diagnostic_stage(210);
        if let Err(error) = select_channel(&mut state, LISTEN_CHANNEL, 0, platform, registers).await
        {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=post-init-channel \
                         error={error:?}"
            ));
            break;
        }
        let parameter = state.parameter_image();
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=tx-calibration-parameter \
                     references={:?} flags={:?} txdc={:?} txiq={:?} tail={:?}",
            &parameter[0x018..0x01e],
            &parameter[0x0a4..0x0a8],
            &parameter[0x0a8..0x0c8],
            &parameter[0x0d0..0x0e8],
            &parameter[0x18e..0x1a8],
        ));
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=tx-calibration-mmio \
                     detector={:#010x}/{:#010x}/{:#010x} \
                     iq={:#010x}/{:#010x}/{:#010x} \
                     tone={:#010x} correction={:#010x}",
            read_diagnostic_mmio(0x2010_081c),
            read_diagnostic_mmio(0x2010_0820),
            read_diagnostic_mmio(0x2010_0830),
            read_diagnostic_mmio(0x2010_0848),
            read_diagnostic_mmio(0x2010_084c),
            read_diagnostic_mmio(0x2010_0850),
            read_diagnostic_mmio(0x2010_0870),
            read_diagnostic_mmio(0x2010_0890),
        ));
        set_diagnostic_stage(220);
        let tx_power_profile = state
            .tx_target_power_profile()
            .with_maximum_quarter_dbm(OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM);
        let legacy_power =
            core::array::from_fn::<_, 4, _>(|rate| tx_power_profile.pair(rate as u8));
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=open-tx-power rates0_3={legacy_power:?}"
        ));
        powered
            .parts_mut()
            .0
            .install_phy_tx_power_profile(tx_power_profile);
        set_diagnostic_stage(230);
        let (platform, registers) = powered.into_parts();
        let _ = run_promiscuous_rx_hil(
            spawner,
            protocol_spawner,
            &mut state,
            platform,
            registers,
            &trng,
            &mut network_credentials,
        )
        .await;
        break;
    }
    set_diagnostic_stage(250);
    halt()
}

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
