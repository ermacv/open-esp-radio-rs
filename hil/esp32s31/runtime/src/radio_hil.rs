use core::{
    cell::RefCell,
    future::Future,
    marker::PhantomData,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use crate::console::emergency_log;
use embassy_executor::{SendSpawner, Spawner};
use embassy_futures::{
    select::{Either, select},
    yield_now,
};
use embassy_net::{
    Config as NetworkConfig, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_net_driver::LinkState;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::efuse::{self, InterfaceMacAddress};
use esp_hal::rng::{Rng, Trng};
use open_esp_radio::esp32s31::phy::PhyTxTargetPowerProfile;
use open_esp_radio::{
    esp32s31::{
        hal::{ColdRadioRegisters, Radio, RadioRegisters},
        pac::{
            MacInterruptSetup,
            mac::{self as mac_pac, init as mac_registers},
        },
        phy::{
            PhyCalibrationIdentity, PhyCalibrationPath, PhyRegisterRunError, PhyRfBoundary,
            PhyTargetObserver,
            phy_cold::{PhyCalibrationRecord, PhyColdState},
            phy_rfpll::phy_get_rf_cal_version,
            target_executor::PhyTargetPortError,
        },
        wifi::mac::{
            connected_rx::{ConnectedRxEvent, ConnectedRxSink},
            crypto::{CcmpKeyHardware, StaGroupCcmpSlot, StaPairwiseCcmpSlot},
            he::He20PeerHardware,
            init::{
                MAC_COLD_RX_INTERRUPT_MASK, StaLinkRxPolicyHardware, StaNoiseFloorHardware,
                initialize_promiscuous_receive,
            },
            irq::{
                IrqSink, MAC_INT_COLLISION, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
                MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT,
            },
            rate_control::BeamformingReportHardware,
            registers::{
                MAC_INT_RAW, MAC_INT_STATUS, Mmio, RX_CONTROL, RX_DESCRIPTOR_BASE,
                RX_LAST_DESCRIPTOR, RX_LAST_DESCRIPTOR_HIGH, RX_NEXT_DESCRIPTOR,
            },
            rx::{
                HeGuardIntervalAndLtf, PUBLIC_HEADER_SIZE, RxDma, RxIngressConfig,
                decode_rx_phy_info,
            },
            rx_pool::RxStagePool,
            scan::{ScanObservation, ScanRecord, ScanTable},
            tx::{
                HeBccDcmMcs, HeDcmRate, HeEdcaTxopLimit, HeLdpcDcmMcs, HeMcs, HtGuardInterval,
                HtMcs, LegacyRate, TxCompletion, TxHardware, TxPhyRate, TxSlot,
            },
            tx_ampdu::HtAmpduTxStorage,
        },
    },
    integration::{
        esp32s31::wifi_embassy::{
            aggregate_tx::{
                AggregateTxCounterSnapshot, AggregateTxCounters, AggregateTxError,
                AggregateTxResetReason,
            },
            backend::Esp32s31WifiBackendError,
            connected_sta_port::{
                Esp32s31ConnectedStaConfig, Esp32s31ConnectedStaControlResources,
                Esp32s31ConnectedStaDriverParts, Esp32s31ConnectedStaNetworkTxDomain,
                Esp32s31ConnectedStaPort, Esp32s31ConnectedStaRateConfig,
                Esp32s31ConnectedStaRxProtocolResources, Esp32s31ConnectedStaTxResources,
            },
            connected_sta_teardown::{
                Esp32s31ConnectedStaTeardownFailure, Esp32s31ConnectedStaTeardownPort,
            },
            cold_start::{
                Esp32s31ColdStartConfig, Esp32s31ColdStartFailure,
                start_esp32s31_station_radio,
            },
            control_tx::{ControlTxConfig, ControlTxError, Esp32s31ControlTx},
            cooperative_tx::CooperativeTxHardware,
            embassy_irq::{
                EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch,
            },
            embassy_rx::RxReloadDelay,
            phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
            preconnected_rx::{EmbassyEsp32s31PreconnectedRxDelay, Esp32s31PreconnectedRx},
            runner::WifiRunner,
            running_scan::{
                EmbassyEsp32s31RunningScanTimer, Esp32s31RunningScanParts, Esp32s31RunningScanPort,
                Esp32s31RunningScanPortError, Esp32s31RunningScanRadio, Esp32s31RunningScanStation,
                Esp32s31RunningScanStorage,
            },
            rx_backend::{
                ConnectedControlPublisher, ConnectedControlResources, ESP32S31_RX_BUFFER_SIZE,
                EmbassyNetConnectedRxSink, Esp32s31RxDmaStorage, Esp32s31RxEpochResources,
                Esp32s31StoppedRx, RxEnqueueCounters,
            },
            rx_reorder::{
                RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandResources, RxReorderFrameStorage,
            },
            rx_telemetry::{RxPipelineCounterSnapshot, RxPipelineCounters},
            scan_port::{
                EmbassyEsp32s31ScanTimer, Esp32s31ScanPort, Esp32s31ScanPortParts,
                Esp32s31ScanRadio, Esp32s31ScanStation, Esp32s31ScanStorage,
            },
            single_mpdu_tx::{EmbassyWifiTxTimer, SingleMpduTxError, TxResetReason},
            sta_attempt::{Esp32s31StaAttempt, Esp32s31StaAttemptOutcome, Esp32s31StaAttemptStage},
            sta_attempt_target::{
                Esp32s31StaAttemptRadio, Esp32s31StaAttemptSecurity, Esp32s31StaAttemptStation,
                Esp32s31StaAttemptStorage, Esp32s31StaAttemptTargetOwner,
                Esp32s31StaAttemptTargetPort,
            },
            sta_join_port::{Esp32s31StaAssociationProfile, Esp32s31StaJoinObserver},
            sta_peer_port::{Esp32s31ConnectedStaPeer, Esp32s31StaConnectedLink},
            sta_scan::{
                Esp32s31RunningScanRx, Esp32s31RunningScanTx, Esp32s31ScanFrameObserver,
                Esp32s31ScanRx, Esp32s31ScanRxError, Esp32s31StaScanBackend, Esp32s31StaScanConfig,
                Esp32s31StaScanError,
            },
            sta_scan_target::{Esp32s31ColdScanTx, Esp32s31ScanPhy},
            sta_tx_epoch::Esp32s31StaTxEpoch,
            staged_rx::{
                ConnectedRxProtocolStopped, Esp32s31ConnectedRxProtocol, Esp32s31StagedRxQueue,
            },
            station::{
                Esp32s31ConnectedStationExit, Esp32s31ConnectedTaskGroup,
                Esp32s31ConnectedTaskStopOutcome, Esp32s31Station, Esp32s31StationCommand,
                Esp32s31StationCommandReceiver, Esp32s31StationConfig,
                Esp32s31StationControlResources, Esp32s31StationController, Esp32s31StationExit,
                Esp32s31StationReconnectSource, Esp32s31StationResources,
                run_esp32s31_connected_station_epoch, stop_esp32s31_connected_task_group,
            },
            station_epoch::{
                Esp32s31DisconnectedStaEpoch, Esp32s31ReconnectedStaEpoch,
                Esp32s31ReconnectedStaEpochParts, Esp32s31RunningScanEpochParts,
            },
            wpa2_port::Esp32s31Wpa2Message4Protection,
        },
        network::embassy_net::{
            PinnedTxPool as OpenRadioNetworkTxPool, SplitPinnedDevice as OpenRadioNetworkDevice,
            SplitPinnedRadioRunner as OpenRadioNetworkRunner,
            SplitPinnedResources as OpenRadioNetworkResources,
        },
    },
    wifi::ieee80211::{
        mac_service::MacRxEvidence,
        scan::best_matching_ssid,
        station::{
            STA_PROTECTED_QOS_ETHERNET_HEADROOM, StaAssociationPhy, StaAssociationPreference,
            StaSequenceCounter, StaTxSequenceCounters,
        },
    },
    wifi::lifecycle::scan::{StaCandidateScanExit, StaCandidateScanService, StaScanPlanError},
    wifi::lifecycle::station::{
        StaAttemptContext, StaAttemptFailure, StaAttemptOutcome, StaBackoffOutcome,
        StaBackoffReason, StaFailureDisposition, StaLifecycleBackend, StaLifecycleStage,
        StaNextCandidate, StaReconnectPolicy,
    },
    wifi::wpa2::Pmk,
};
use open_esp_radio_esp32s31_wifi_esp_hal::{
    EspHalRadioPeripheral,
    mac_interrupt_epoch::{
        EspHalMacInterruptRoute, service_mac_interrupt, service_power_interrupt,
    },
};
use open_esp_radio_hil_protocol::{
    Capabilities, Completion as HilCompletion, Direction as HilDirection, Event as HilEvent,
    FeatureCapabilities, MAX_WIRE_FRAME_BYTES, NetworkCredentials, NetworkInfo, ServiceInfo,
    StartupArtifactDisposition, StationAttemptFailureReason, StationDisconnectReason,
    StationEpochEvidence, StationFailureStage, StationFaultClassification, StationFaultEvidence,
    StationLifecycleEvent, Transport as HilTransport, TransportEvidence,
};

use crate::radio_fault::{
    ArmedStationFault, FaultInjectingBackendError, FaultInjectingWifiBackend,
    STATION_FAULT_CONTROL,
};

mod phy_diagnostics;
use phy_diagnostics::*;

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
const OPEN_RADIO_CONNECTED_TASK_STOP_TIMEOUT: Duration = Duration::from_secs(2);
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
            station_lifecycle_events: true,
            station_fault_injection: true,
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
const OPEN_RADIO_RUNTIME_SESSIONS: bool = OPEN_RADIO_RUNTIME_RX_SESSIONS
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
// Migration installs both keys before queueing M4, but keeps STA EAPOL on its
// measured plaintext layout until the M4 TX-done edge opens the controlled
// port. Protected M4 remains a useful explicit negative control experiment.
const WPA2_MESSAGE_4_HARDWARE_PROTECTED: bool = false;

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

type RxStorage = Esp32s31RxDmaStorage<RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
type ScanRx = Esp32s31ScanRx<'static, RX_DESCRIPTOR_COUNT, RX_BUFFER_SIZE, RX_BUFFER_STORAGE_SIZE>;
const _: () = assert!(RX_BUFFER_SIZE <= ESP32S31_RX_BUFFER_SIZE);

type ControlTx = Esp32s31ControlTx<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
type ScanTx = Esp32s31ColdScanTx<
    'static,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
type TxStorage = Esp32s31StaTxEpoch<ControlTx>;
type RadioHilMacInterruptEpoch =
    Esp32s31MacInterruptEpoch<'static, EspHalMacInterruptRoute, CriticalSectionRawMutex>;

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
static OPEN_RADIO_RX_BUFFER_ADDRESSES: StaticCell<[u32; RX_DESCRIPTOR_COUNT]> = StaticCell::new();
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
static OPEN_RADIO_RX_STAGE_POOL: RxStagePool<RX_STAGE_SLOT_COUNT, RX_STAGE_CAPACITY> =
    RxStagePool::new();
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
type NetworkTxPool = OpenRadioNetworkTxPool<
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
>;
type ControlResources =
    ConnectedControlResources<CriticalSectionRawMutex, CONNECTED_CONTROL_QUEUE_DEPTH>;
type ControlPublisher =
    ConnectedControlPublisher<'static, CriticalSectionRawMutex, CONNECTED_CONTROL_QUEUE_DEPTH>;
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
type RunningScanRx = Esp32s31RunningScanRx<
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
type RunningScanTx<'interrupt> = Esp32s31RunningScanTx<
    'static,
    'interrupt,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    EmbassyWifiTxTimer,
    TX_BUFFER_SIZE,
>;
type RadioHilJoinRx<'storage> = Esp32s31PreconnectedRx<
    'storage,
    EmbassyEsp32s31PreconnectedRxDelay,
    RX_DESCRIPTOR_COUNT,
    RX_BUFFER_SIZE,
>;
type RadioHilStaAttemptChannel<'state> =
    Esp32s31ScanPhy<'state, EspHalRadioPeripheral, HilPhyObserver, EmbassyPhyDelay>;
type RadioHilStaAttemptOwner<'hardware, 'transmit, 'state, 'scratch, 'security, H> =
    Esp32s31StaAttemptTargetOwner<
        'hardware,
        'transmit,
        'static,
        'scratch,
        'security,
        H,
        RadioHilStaAttemptChannel<'state>,
        EmbassyEsp32s31PreconnectedRxDelay,
        ControlTx,
        RadioHilStaJoinObserver,
        RX_DESCRIPTOR_COUNT,
        RX_BUFFER_SIZE,
        RX_BUFFER_STORAGE_SIZE,
    >;
type RadioHilRunningScanPortError =
    Esp32s31RunningScanPortError<PhyTargetPortError, Esp32s31ScanRxError, ControlTxError>;
type RadioHilStationController<'resources> =
    Esp32s31StationController<'resources, CriticalSectionRawMutex>;
type RadioHilStationCommandReceiver<'resources> =
    Esp32s31StationCommandReceiver<'resources, CriticalSectionRawMutex>;
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
type ConnectedAmpduStorage =
    Pin<&'static mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>;
type RadioHilReconnectedEpoch = Esp32s31ReconnectedStaEpoch<
    ConnectedHardware,
    RadioHilJoinRx<'static>,
    ConnectedRxEpochResources,
    ConnectedAmpduStorage,
    &'static ControlResources,
>;
type RadioHilDisconnectedEpoch = Esp32s31DisconnectedStaEpoch<
    RadioHilRunningNetwork,
    ConnectedHardware,
    ConnectedStoppedRx,
    ConnectedAmpduStorage,
    &'static ControlResources,
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
    Reconnected(RadioHilReconnectedEpoch),
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
    Disconnected { beacon_lost: bool },
    ReconnectRequested,
    StationStopped(Esp32s31StationCommand),
    InjectedTxFault {
        fault: ArmedStationFault,
        reset_required: bool,
    },
    HardwareFailure,
}

fn injected_tx_source_requires_reset<R, C>(
    source: &Esp32s31WifiBackendError<R, C, AggregateTxError>,
) -> bool {
    let expected_events = MAC_INT_TX_COMPLETE | MAC_INT_TX_TIMEOUT;
    matches!(
        source,
        Esp32s31WifiBackendError::Tx(AggregateTxError::RadioResetRequired(
            AggregateTxResetReason::ConflictingInterruptEvents(events),
        )) if *events == expected_events
    ) || matches!(
        source,
        Esp32s31WifiBackendError::Tx(AggregateTxError::Ordinary(
            SingleMpduTxError::RadioResetRequired(TxResetReason::ConflictingInterruptEvents(
                events,
            )),
        )) if *events == expected_events
    )
}

/// Complete input frontier for the next Authentication/Association/WPA2
/// epoch.
struct RadioHilReconnectReady<'fixture, 'security> {
    fixture: RadioHilConnectedTaskFixture<'fixture>,
    target: StaJoinTarget,
    network: RadioHilStaNetwork,
    epoch: RadioHilConnectedEpochResources,
    security: StaAssociationSecurity<'security>,
}

/// Disconnected hardware frontier which must refresh its candidate before the
/// next Authentication/Association/WPA2 epoch.
///
/// Unlike `RadioHilReconnectReady`, this owner still carries the connected
/// teardown bundle. Only the `RunningScan` lifecycle branch may split it into
/// polling RX/TX owners and produce a refreshed reconnect target.
struct RadioHilRunningScanReady<'fixture, 'security> {
    fixture: RadioHilConnectedTaskFixture<'fixture>,
    previous_target: StaJoinTarget,
    disconnected: RadioHilDisconnectedEpoch,
    security: StaAssociationSecurity<'security>,
}

struct StaConnectedSession<'security> {
    generation: u32,
    peer: Esp32s31ConnectedStaPeer,
    network: RadioHilStaNetwork,
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

/// The initial join and later reconnect frontiers intentionally remain
/// different Rust types. This enum is only the outer lifecycle's sum type; it
/// does not erase either phase into a mutable vendor-style context.
enum RadioHilStaLifecycleOwner<'fixture, 'security> {
    Authenticate(RadioHilAuthenticationReady<'fixture, 'security>),
    RunningScan(RadioHilRunningScanReady<'fixture, 'security>),
    Reconnect(RadioHilReconnectReady<'fixture, 'security>),
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
    state: &'a mut PhyColdState,
    platform: &'a mut EspHalRadioPeripheral,
    mmio: &'static mut RadioRegisters,
    interrupt_epoch: &'a mut RadioHilMacInterruptEpoch,
    rx_storage: &'static RxStorage,
    tx_storage: &'static mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    scan_table: &'static mut ScanTable,
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
    state: &'a mut PhyColdState,
    platform: &'a mut EspHalRadioPeripheral,
    interrupt_epoch: &'a mut RadioHilMacInterruptEpoch,
    rx_storage: &'static RxStorage,
    tx_storage: &'static mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    scan_table: &'static mut ScanTable,
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
                state: self.state,
                platform: self.platform,
                interrupt_epoch: self.interrupt_epoch,
                rx_storage: self.rx_storage,
                tx_storage: self.tx_storage,
                descriptor_base: self.descriptor_base,
                buffer_addresses: self.buffer_addresses,
                scan_table: self.scan_table,
                frame: self.frame,
                ethernet: self.ethernet,
            },
            self.mmio,
        )
    }
}

/// Complete same-candidate frontier before Open Authentication.
///
/// The PHY channel owner remains part of the connected fixture after this
/// phase so a later running rescan can retune without reconstructing hidden
/// state. A failed finite authentication returns the complete value so the
/// outer lifecycle can wait and retry without recreating DMA or security
/// state.
struct RadioHilAuthenticationReady<'fixture, 'security> {
    fixture: RadioHilConnectedFixture<'fixture>,
    target: StaJoinTarget,
    rx: RadioHilJoinRx<'static>,
    network: RadioHilStaNetwork,
    security: StaAssociationSecurity<'security>,
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
static OPEN_RADIO_STATION_CONTROL_RESOURCES: StaticCell<
    Esp32s31StationControlResources<CriticalSectionRawMutex>,
> = StaticCell::new();
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
// 0/1 preserve the hardware-observed IEEE S-MPDU flag; u32::MAX means that
// the ARP reply has not carried usable physical provenance. S-MPDU is a
// specific VHT/HE single-MPDU A-MPDU form, not an ordinary MPDU synonym.
static OPEN_RADIO_LAN_PROBE_RX_S_MPDU: AtomicU32 = AtomicU32::new(u32::MAX);

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
        match maximum.compare_exchange_weak(observed, value, Ordering::Relaxed, Ordering::Relaxed) {
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
            spurious_entries: self.spurious_entries.wrapping_sub(earlier.spurious_entries),
            rx_only_entries: self.rx_only_entries.wrapping_sub(earlier.rx_only_entries),
            rx_mixed_entries: self.rx_mixed_entries.wrapping_sub(earlier.rx_mixed_entries),
            tx_only_entries: self.tx_only_entries.wrapping_sub(earlier.tx_only_entries),
            tx_mixed_entries: self.tx_mixed_entries.wrapping_sub(earlier.tx_mixed_entries),
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
struct OpenRadioRxSmpduSnapshot {
    s_mpdu_frames: u32,
    not_s_mpdu_frames: u32,
    unavailable_frames: u32,
}

impl OpenRadioRxSmpduSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            s_mpdu_frames: self.s_mpdu_frames.wrapping_sub(earlier.s_mpdu_frames),
            not_s_mpdu_frames: self
                .not_s_mpdu_frames
                .wrapping_sub(earlier.not_s_mpdu_frames),
            unavailable_frames: self
                .unavailable_frames
                .wrapping_sub(earlier.unavailable_frames),
        }
    }
}

struct OpenRadioRxSmpduCounters {
    s_mpdu_frames: AtomicU32,
    not_s_mpdu_frames: AtomicU32,
    unavailable_frames: AtomicU32,
}

impl OpenRadioRxSmpduCounters {
    const fn new() -> Self {
        Self {
            s_mpdu_frames: AtomicU32::new(0),
            not_s_mpdu_frames: AtomicU32::new(0),
            unavailable_frames: AtomicU32::new(0),
        }
    }

    fn observe(&self, evidence: MacRxEvidence<bool>) {
        let counter = match evidence {
            MacRxEvidence::HardwareObserved(true) => &self.s_mpdu_frames,
            MacRxEvidence::HardwareObserved(false) => &self.not_s_mpdu_frames,
            MacRxEvidence::ProtocolValidated(_) | MacRxEvidence::Unavailable => {
                &self.unavailable_frames
            }
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> OpenRadioRxSmpduSnapshot {
        OpenRadioRxSmpduSnapshot {
            s_mpdu_frames: self.s_mpdu_frames.load(Ordering::Relaxed),
            not_s_mpdu_frames: self.not_s_mpdu_frames.load(Ordering::Relaxed),
            unavailable_frames: self.unavailable_frames.load(Ordering::Relaxed),
        }
    }
}

#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_S_MPDU_COUNTERS: OpenRadioRxSmpduCounters = OpenRadioRxSmpduCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS: OpenRadioRxSmpduCounters =
    OpenRadioRxSmpduCounters::new();

#[derive(Clone, Copy, Default)]
struct OpenRadioRxAmpduSnapshot {
    ampdu_frames: u32,
    not_ampdu_frames: u32,
    hardware_ampdu_frames: u32,
    hardware_not_ampdu_frames: u32,
    protocol_ampdu_frames: u32,
    protocol_not_ampdu_frames: u32,
    unavailable_frames: u32,
}

impl OpenRadioRxAmpduSnapshot {
    fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            ampdu_frames: self.ampdu_frames.wrapping_sub(earlier.ampdu_frames),
            not_ampdu_frames: self
                .not_ampdu_frames
                .wrapping_sub(earlier.not_ampdu_frames),
            hardware_ampdu_frames: self
                .hardware_ampdu_frames
                .wrapping_sub(earlier.hardware_ampdu_frames),
            hardware_not_ampdu_frames: self
                .hardware_not_ampdu_frames
                .wrapping_sub(earlier.hardware_not_ampdu_frames),
            protocol_ampdu_frames: self
                .protocol_ampdu_frames
                .wrapping_sub(earlier.protocol_ampdu_frames),
            protocol_not_ampdu_frames: self
                .protocol_not_ampdu_frames
                .wrapping_sub(earlier.protocol_not_ampdu_frames),
            unavailable_frames: self
                .unavailable_frames
                .wrapping_sub(earlier.unavailable_frames),
        }
    }
}

struct OpenRadioRxAmpduCounters {
    ampdu_frames: AtomicU32,
    not_ampdu_frames: AtomicU32,
    hardware_ampdu_frames: AtomicU32,
    hardware_not_ampdu_frames: AtomicU32,
    protocol_ampdu_frames: AtomicU32,
    protocol_not_ampdu_frames: AtomicU32,
    unavailable_frames: AtomicU32,
}

impl OpenRadioRxAmpduCounters {
    const fn new() -> Self {
        Self {
            ampdu_frames: AtomicU32::new(0),
            not_ampdu_frames: AtomicU32::new(0),
            hardware_ampdu_frames: AtomicU32::new(0),
            hardware_not_ampdu_frames: AtomicU32::new(0),
            protocol_ampdu_frames: AtomicU32::new(0),
            protocol_not_ampdu_frames: AtomicU32::new(0),
            unavailable_frames: AtomicU32::new(0),
        }
    }

    fn observe(&self, evidence: MacRxEvidence<bool>) {
        let (total, provenance) = match evidence {
            MacRxEvidence::HardwareObserved(true) => {
                (&self.ampdu_frames, &self.hardware_ampdu_frames)
            }
            MacRxEvidence::HardwareObserved(false) => {
                (&self.not_ampdu_frames, &self.hardware_not_ampdu_frames)
            }
            MacRxEvidence::ProtocolValidated(true) => {
                (&self.ampdu_frames, &self.protocol_ampdu_frames)
            }
            MacRxEvidence::ProtocolValidated(false) => {
                (&self.not_ampdu_frames, &self.protocol_not_ampdu_frames)
            }
            MacRxEvidence::Unavailable => {
                self.unavailable_frames.fetch_add(1, Ordering::Relaxed);
                return;
            }
        };
        total.fetch_add(1, Ordering::Relaxed);
        provenance.fetch_add(1, Ordering::Relaxed);
    }

    fn snapshot(&self) -> OpenRadioRxAmpduSnapshot {
        OpenRadioRxAmpduSnapshot {
            ampdu_frames: self.ampdu_frames.load(Ordering::Relaxed),
            not_ampdu_frames: self.not_ampdu_frames.load(Ordering::Relaxed),
            hardware_ampdu_frames: self.hardware_ampdu_frames.load(Ordering::Relaxed),
            hardware_not_ampdu_frames: self.hardware_not_ampdu_frames.load(Ordering::Relaxed),
            protocol_ampdu_frames: self.protocol_ampdu_frames.load(Ordering::Relaxed),
            protocol_not_ampdu_frames: self.protocol_not_ampdu_frames.load(Ordering::Relaxed),
            unavailable_frames: self.unavailable_frames.load(Ordering::Relaxed),
        }
    }
}

#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_A_MPDU_COUNTERS: OpenRadioRxAmpduCounters =
    OpenRadioRxAmpduCounters::new();

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
static OPEN_RADIO_RX_ORDER_COUNTERS: OpenRadioRxOrderCounters = OpenRadioRxOrderCounters::new();
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
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_CONNECTED_BENCHMARK_START: Channel<
    CriticalSectionRawMutex,
    RadioHilConnectedBenchmarkConfig,
    1,
> = Channel::new();
#[unsafe(link_section = ".critical.bss.open_radio_station_epoch")]
static OPEN_RADIO_STATION_EPOCH_ACTIVE: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.bss.open_radio_station_epoch")]
static OPEN_RADIO_STATION_EPOCH_PROGRESS: Channel<
    CriticalSectionRawMutex,
    RadioHilStationEpochProgress,
    4,
> = Channel::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilStationEpochProgress {
    RunnerStopped,
    ScanOwnersReturned,
    JoinCompleted,
    ConnectedRunnerStarted,
}

fn report_station_epoch_progress(progress: RadioHilStationEpochProgress) {
    if OPEN_RADIO_STATION_EPOCH_ACTIVE.load(Ordering::Acquire)
        && OPEN_RADIO_STATION_EPOCH_PROGRESS
            .try_send(progress)
            .is_err()
    {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=production-station-epoch-evidence \
             error=progress-queue-full progress={progress:?}"
        ));
    }
}
/// HIL diagnostics attached to the production join port. These callbacks do
/// not select policy, access DMA ownership or wrap a driver transaction.
#[derive(Clone, Copy, Debug, Default)]
struct RadioHilStaJoinObserver;

impl Esp32s31StaJoinObserver for RadioHilStaJoinObserver {
    fn authentication_transmitted(&mut self, _completion: TxCompletion) {
        if AUTH_REGISTER_SNAPSHOT_CAPTURED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            log_open_auth_register_snapshot();
        }
    }

    fn association_profile_selected(&mut self, profile: Esp32s31StaAssociationProfile) {
        let (Some(power), Some(capability), Some(rate_power)) = (
            profile.power_capability,
            profile.he_ul_mu_power,
            profile.rate_16_through_25,
        ) else {
            return;
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=sta-he-ul-mu-power \
             minimum_dbm={} maximum_dbm={} rate_16_through_25={rate_power:?} \
             relative_to_rate_16={:?}",
            power.minimum_dbm(),
            power.maximum_dbm(),
            capability.relative_to_rate_16(),
        ));
    }
}

const fn radio_hil_message4_protection() -> Esp32s31Wpa2Message4Protection {
    if WPA2_MESSAGE_4_HARDWARE_PROTECTED {
        Esp32s31Wpa2Message4Protection::PairwiseCcmp
    } else {
        Esp32s31Wpa2Message4Protection::Unprotected
    }
}

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn open_radio_mac_interrupt() {
    OPEN_RADIO_MAC_IRQ_ENTRIES.fetch_add(1, Ordering::Relaxed);
    let report = service_mac_interrupt(&OpenRadioMacIrqSink);
    OPEN_RADIO_MAC_IRQ_CLASSIFICATION.record(
        report.first_status,
        report.observed_status,
        u32::from(report.nonzero_snapshots),
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

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn open_radio_power_interrupt() {
    let _report = service_power_interrupt(&OPEN_RADIO_POWER_IRQ_RUNTIME);
}

fn read_diagnostic_mmio(address: usize) -> u32 {
    // SAFETY: diagnostic-only reads in this isolated HIL image. Production
    // radio operations use typed PAC identities; keeping snapshots here raw
    // avoids exporting ownership-free aliases solely for logging.
    unsafe { (address as *const u32).read_volatile() }
}

const STA_ASSOCIATION_PREFERENCE: StaAssociationPreference =
    if option_env!("OPEN_RADIO_FORCE_HE20").is_some() || OPEN_RADIO_HE_TB_HIL {
        StaAssociationPreference::PreferHe20
    } else if option_env!("OPEN_RADIO_FORCE_HT20").is_some() {
        StaAssociationPreference::ForceHt20
    } else {
        StaAssociationPreference::Automatic
    };

const fn radio_hil_connected_sta_config() -> Esp32s31ConnectedStaConfig {
    Esp32s31ConnectedStaConfig {
        rate: Esp32s31ConnectedStaRateConfig {
            high_throughput_enabled: option_env!("OPEN_RADIO_FORCE_LEGACY_TX").is_none(),
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
        },
        rx_ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        unicast_attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
        completion_timeout_us: TX_COMPLETION_DEADLINE_MS * 1_000,
        aggregate_frame_limit: TX_AMPDU_FRAME_COUNT as u8,
        aggregate_he_txop_limit: HeEdcaTxopLimit::DEFAULT,
        tx_block_ack_window: TX_BLOCK_ACK_WINDOW as u16,
        tx_block_ack_negotiation_timeout_us: 500_000,
        tid0_amsdu: OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_NETWORK_AMSDU_BENCH,
        rx_block_ack_maximum_window: RX_BLOCK_ACK_SOFTWARE_WINDOW as u16,
        beacon_miss_limit: CONNECTED_BEACON_MISS_LIMIT,
        request_initial_tx_block_ack: true,
    }
}

fn open_radio_tx_entropy() -> u32 {
    Rng::new().random()
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
                         stage=embassy-net-external-probe-ready address={} rx_s_mpdu={}",
                        config.address,
                        OPEN_RADIO_LAN_PROBE_RX_S_MPDU.load(Ordering::Relaxed),
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
        if let ConnectedRxEvent::Beacon { metadata, .. } = event {
            OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS.observe(metadata.s_mpdu);
        }
        if let ConnectedRxEvent::Ethernet {
            frame,
            raw,
            metadata,
            ..
        } = event
        {
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
                let s_mpdu = match metadata.s_mpdu {
                    MacRxEvidence::HardwareObserved(s_mpdu) => u32::from(s_mpdu),
                    MacRxEvidence::ProtocolValidated(_) | MacRxEvidence::Unavailable => {
                        u32::MAX
                    }
                };
                OPEN_RADIO_LAN_PROBE_RX_S_MPDU.store(s_mpdu, Ordering::Relaxed);
                OPEN_RADIO_LAN_PROBE_RESPONSE.store(true, Ordering::Release);
            }
            let benchmark_udp =
                ipv4_udp_destination_port(frame) == Some(OPEN_RADIO_UDP_RX_PORT);
            if benchmark_udp {
                OPEN_RADIO_RX_S_MPDU_COUNTERS.observe(metadata.s_mpdu);
                OPEN_RADIO_RX_A_MPDU_COUNTERS.observe(metadata.ampdu);
                let sample_phy = self.phy_sample_cursor == 0;
                self.phy_sample_cursor = self.phy_sample_cursor.wrapping_add(1) & 63;
                if sample_phy && let Some(phy) = decode_rx_phy_info(raw) {
                    OPEN_RADIO_RX_LAST_UDP_FORMAT
                        .store(u32::from(phy.baseband_format().raw()), Ordering::Relaxed);
                    let mut packed =
                        u32::from(phy.baseband_format().raw()) | (u32::from(phy.rate) << 4);
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
    let frame_control = u16::from_le_bytes([*raw.get(frame_offset)?, *raw.get(frame_offset + 1)?]);
    if frame_control & (DATA_TYPE_MASK | QOS_SUBTYPE) != DATA_TYPE | QOS_SUBTYPE {
        return None;
    }
    let sequence_control =
        u16::from_le_bytes([*raw.get(frame_offset + 22)?, *raw.get(frame_offset + 23)?]);
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
        let Some(sequence) = sequence
            .filter(|sequence| *sequence >= 0)
            .map(|value| value as u32)
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
        let Some(sequence) = sequence
            .filter(|sequence| *sequence >= 0)
            .map(|value| value as u32)
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
    buffers: &mut RadioHilBenchmarkBuffers,
) -> ! {
    match buffers {
        RadioHilBenchmarkBuffers::Raw => loop {
            Timer::after_secs(60).await;
        },
        RadioHilBenchmarkBuffers::Tcp { rx, tx, read } => {
            run_open_radio_tcp_rx_benchmark(
                stack,
                registers,
                &mut **rx,
                &mut **tx,
                &mut **read,
            )
            .await
        }
        RadioHilBenchmarkBuffers::UdpRx {
            rx_metadata,
            rx,
            tx_metadata,
            tx,
        } => {
            run_open_radio_udp_rx_benchmark(
                stack,
                association_phy,
                data_tx_rate,
                registers,
                &mut **rx_metadata,
                &mut **rx,
                &mut **tx_metadata,
                &mut **tx,
            )
            .await
        }
        RadioHilBenchmarkBuffers::UdpTx {
            rx_metadata,
            rx,
            tx_metadata,
            tx,
            packet,
        } => {
            run_open_radio_udp_tx_benchmark(
                stack,
                association_phy,
                data_tx_rate,
                &mut **rx_metadata,
                &mut **rx,
                &mut **tx_metadata,
                &mut **tx,
                &mut **packet,
            )
            .await
        }
        RadioHilBenchmarkBuffers::Bidirectional {
            tx_rx_metadata,
            tx_rx,
            tx_tx_metadata,
            tx_tx,
            packet,
            rx_rx_metadata,
            rx_rx,
            rx_tx_metadata,
            rx_tx,
        } => match select(
            run_open_radio_bidirectional_session_coordinator(),
            select(
                run_open_radio_udp_tx_benchmark(
                    stack,
                    association_phy,
                    data_tx_rate,
                    &mut **tx_rx_metadata,
                    &mut **tx_rx,
                    &mut **tx_tx_metadata,
                    &mut **tx_tx,
                    &mut **packet,
                ),
                run_open_radio_bidirectional_rx_benchmark(
                    stack,
                    association_phy,
                    data_tx_rate,
                    registers,
                    &mut **rx_rx_metadata,
                    &mut **rx_rx,
                    &mut **rx_tx_metadata,
                    &mut **rx_tx,
                ),
            ),
        )
        .await {},
    }
}

async fn run_open_radio_tcp_rx_benchmark<'a>(
    stack: Stack<'a>,
    registers: &RefCell<&mut RadioRegisters>,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    read_buffer: &mut [u8],
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

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
            session.session_id, flow.payload_bytes, duration_millis, flow.offered_rate_bps,
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
            rx_bytes: first
                .evidence
                .rx_bytes
                .saturating_add(second.evidence.rx_bytes),
            tx_bytes: first
                .evidence
                .tx_bytes
                .saturating_add(second.evidence.tx_bytes),
            rx_units: first
                .evidence
                .rx_units
                .saturating_add(second.evidence.rx_units),
            tx_units: first
                .evidence
                .tx_units
                .saturating_add(second.evidence.tx_units),
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
async fn run_open_radio_udp_tx_benchmark<'a>(
    stack: Stack<'a>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    rx_metadata: &'a mut [PacketMetadata],
    rx_buffer: &'a mut [u8],
    tx_metadata: &'a mut [PacketMetadata],
    tx_buffer: &'a mut [u8],
    packet: &mut [u8],
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

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
        let aggregate_start =
            (!OPEN_RADIO_BIDIRECTIONAL_BENCH).then(|| OPEN_RADIO_TX_AGGREGATE_COUNTERS.snapshot());
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
                crate::console::complete_session(session.session_id, evidence, send_errors == 0)
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
async fn run_open_radio_udp_rx_benchmark<'a>(
    stack: Stack<'a>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
    rx_metadata: &'a mut [PacketMetadata],
    rx_buffer: &'a mut [u8],
    tx_metadata: &'a mut [PacketMetadata],
    tx_buffer: &'a mut [u8],
) -> ! {
    stack.wait_config_up().await;
    while stack.config_v4().is_none() {
        Timer::after_millis(100).await;
    }

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

async fn run_open_radio_bidirectional_rx_benchmark<'a>(
    stack: Stack<'a>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
    rx_metadata: &'a mut [PacketMetadata],
    rx_buffer: &'a mut [u8],
    tx_metadata: &'a mut [PacketMetadata],
    tx_buffer: &'a mut [u8],
) -> ! {
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

async fn run_open_radio_udp_rx_benchmark_with_buffers<'a>(
    stack: Stack<'a>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
    rx_metadata: &'a mut [PacketMetadata],
    rx_buffer: &'a mut [u8],
    tx_metadata: &'a mut [PacketMetadata],
    tx_buffer: &'a mut [u8],
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
        let s_mpdu_start = OPEN_RADIO_RX_S_MPDU_COUNTERS.snapshot();
        let beacon_s_mpdu_start = OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS.snapshot();
        let ampdu_start = OPEN_RADIO_RX_A_MPDU_COUNTERS.snapshot();
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
        let aggregate_start =
            OPEN_RADIO_BIDIRECTIONAL_BENCH.then(|| OPEN_RADIO_TX_AGGREGATE_COUNTERS.snapshot());
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
        let mut receive_errors =
            u32::from(expected_payload_bytes.is_some_and(|expected| first_length != expected));
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
        let irq_auxiliary_status_or = OPEN_RADIO_MAC_IRQ_CLASSIFICATION.take_auxiliary_status_or();
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
        let rx_mcs_histogram =
            core::array::from_fn::<_, OPEN_RADIO_RX_HE_MCS_BUCKETS, _>(|index| {
                phy_end.0[index].wrapping_sub(phy_start.0[index])
            });
        let rx_other_phy = phy_end.1.wrapping_sub(phy_start.1);
        let rx_s_mpdu = OPEN_RADIO_RX_S_MPDU_COUNTERS
            .snapshot()
            .wrapping_delta_since(s_mpdu_start);
        let beacon_s_mpdu = OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS
            .snapshot()
            .wrapping_delta_since(beacon_s_mpdu_start);
        let rx_ampdu = OPEN_RADIO_RX_A_MPDU_COUNTERS
            .snapshot()
            .wrapping_delta_since(ampdu_start);
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
            sequence_evidence
                .first
                .map(|_| sequence_evidence.highest)
                .unwrap_or(u32::MAX),
            sequence_evidence
                .first
                .map(|_| sequence_evidence.expected)
                .unwrap_or(u32::MAX),
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
            "ORXSM s_mpdu={} not_s_mpdu={} unavailable={} \
             beacon_s_mpdu={} beacon_not_s_mpdu={} beacon_unavailable={}",
            rx_s_mpdu.s_mpdu_frames,
            rx_s_mpdu.not_s_mpdu_frames,
            rx_s_mpdu.unavailable_frames,
            beacon_s_mpdu.s_mpdu_frames,
            beacon_s_mpdu.not_s_mpdu_frames,
            beacon_s_mpdu.unavailable_frames,
        ));
        emergency_log(format_args!(
            "ORXAG ampdu={} not_ampdu={} hardware_ampdu={} hardware_not_ampdu={} \
             protocol_ampdu={} protocol_not_ampdu={} unavailable={}",
            rx_ampdu.ampdu_frames,
            rx_ampdu.not_ampdu_frames,
            rx_ampdu.hardware_ampdu_frames,
            rx_ampdu.hardware_not_ampdu_frames,
            rx_ampdu.protocol_ampdu_frames,
            rx_ampdu.protocol_not_ampdu_frames,
            rx_ampdu.unavailable_frames,
        ));
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
    log_open_radio_task_poll("radio", current.radio.wrapping_delta_since(earlier.radio));
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
async fn station_control_task(controller: RadioHilStationController<'static>) {
    loop {
        let request_id = crate::console::receive_station_epoch_cycle().await;
        let was_active = OPEN_RADIO_STATION_EPOCH_ACTIVE.swap(true, Ordering::AcqRel);
        if was_active {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-station-epoch-evidence error=overlapping-request"
            ));
        }
        let queued = controller.request_reconnect();
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=OBSERVE \
             stage=production-station-controller command=reconnect queued={}",
            u8::from(queued),
        ));
        let mut evidence = StationEpochEvidence {
            runner_stopped: false,
            scan_owners_returned: false,
            join_completed: false,
            connected_runner_started: false,
        };
        loop {
            match OPEN_RADIO_STATION_EPOCH_PROGRESS.receive().await {
                RadioHilStationEpochProgress::RunnerStopped => evidence.runner_stopped = true,
                RadioHilStationEpochProgress::ScanOwnersReturned => {
                    evidence.scan_owners_returned = true;
                }
                RadioHilStationEpochProgress::JoinCompleted => evidence.join_completed = true,
                RadioHilStationEpochProgress::ConnectedRunnerStarted => {
                    evidence.connected_runner_started = true;
                    OPEN_RADIO_STATION_EPOCH_ACTIVE.store(false, Ordering::Release);
                    crate::console::complete_station_epoch_cycle(request_id, evidence).await;
                    break;
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn connected_network_stack_task(mut runner: ConnectedNetworkStackRunner) {
    observe_open_radio_task_polls(runner.run(), &OPEN_RADIO_TASK_POLLS.network).await
}

#[embassy_executor::task]
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

/// HIL composition of the executor tasks which borrow one connected epoch.
///
/// The production station boundary owns the finite stop/deadline semantics;
/// this adapter only maps the fixture's concrete Embassy signals and returns
/// the staged protocol scratch owner needed by teardown.
struct RadioHilConnectedTaskGroup;

impl Esp32s31ConnectedTaskGroup for RadioHilConnectedTaskGroup {
    type Stopped = ConnectedRxProtocolStopped<'static>;

    fn request_stop(&mut self) {
        OPEN_RADIO_CONNECTED_BENCHMARK_STOP.signal(());
        OPEN_RADIO_CONNECTED_PROTOCOL_STOP.signal(());
    }

    fn wait_stopped(&mut self) -> impl Future<Output = Self::Stopped> + '_ {
        async {
            OPEN_RADIO_CONNECTED_BENCHMARK_STOPPED.wait().await;
            OPEN_RADIO_CONNECTED_PROTOCOL_STOPPED.wait().await
        }
    }
}

#[derive(Clone, Copy)]
struct RadioHilConnectedBenchmarkConfig {
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
}

enum RadioHilBenchmarkBuffers {
    Raw,
    Tcp {
        rx: &'static mut [u8; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY],
        tx: &'static mut [u8; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY],
        read: &'static mut [u8; OPEN_RADIO_TCP_READ_CAPACITY],
    },
    UdpRx {
        rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        rx: &'static mut
            [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx: &'static mut
            [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
    UdpTx {
        rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        rx: &'static mut
            [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx: &'static mut
            [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        packet: &'static mut [u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
    Bidirectional {
        tx_rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        tx_rx: &'static mut
            [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx_tx: &'static mut
            [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        packet: &'static mut [u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        rx_rx_metadata:
            &'static mut [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH],
        rx_rx: &'static mut
            [u8; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        rx_tx_metadata:
            &'static mut [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH],
        rx_tx: &'static mut
            [u8; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
}

impl RadioHilBenchmarkBuffers {
    fn init() -> Self {
        if OPEN_RADIO_TCP_RX_BENCH {
            Self::Tcp {
                rx: OPEN_RADIO_TCP_RX_BUFFER
                    .init_with(|| [0; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY]),
                tx: OPEN_RADIO_TCP_TX_BUFFER
                    .init_with(|| [0; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY]),
                read: OPEN_RADIO_TCP_READ_BUFFER
                    .init_with(|| [0; OPEN_RADIO_TCP_READ_CAPACITY]),
            }
        } else if OPEN_RADIO_RAW_MAC_BENCH {
            Self::Raw
        } else if OPEN_RADIO_BIDIRECTIONAL_BENCH {
            Self::Bidirectional {
                tx_rx_metadata: OPEN_RADIO_UDP_RX_METADATA.init_with(|| {
                    [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]
                }),
                tx_rx: OPEN_RADIO_UDP_RX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                tx_tx_metadata: OPEN_RADIO_UDP_TX_METADATA.init_with(|| {
                    [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]
                }),
                tx_tx: OPEN_RADIO_UDP_TX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                packet: OPEN_RADIO_UDP_PACKET
                    .init_with(|| [0x5a; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]),
                rx_rx_metadata: OPEN_RADIO_BIDIRECTIONAL_RX_METADATA.init_with(|| {
                    [PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH]
                }),
                rx_rx: OPEN_RADIO_BIDIRECTIONAL_RX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH
                        * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                rx_tx_metadata: OPEN_RADIO_BIDIRECTIONAL_TX_METADATA.init_with(|| {
                    [PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH]
                }),
                rx_tx: OPEN_RADIO_BIDIRECTIONAL_TX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH
                        * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
            }
        } else {
            let rx_metadata = OPEN_RADIO_UDP_RX_METADATA
                .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]);
            let rx = OPEN_RADIO_UDP_RX_BUFFER.init_with(|| {
                [0; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
            });
            let tx_metadata = OPEN_RADIO_UDP_TX_METADATA
                .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]);
            let tx = OPEN_RADIO_UDP_TX_BUFFER.init_with(|| {
                [0; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
            });
            if option_env!("OPEN_RADIO_TX_BENCH").is_some() {
                Self::UdpTx {
                    rx_metadata,
                    rx,
                    tx_metadata,
                    tx,
                    packet: OPEN_RADIO_UDP_PACKET
                        .init_with(|| [0x5a; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]),
                }
            } else {
                Self::UdpRx {
                    rx_metadata,
                    rx,
                    tx_metadata,
                    tx,
                }
            }
        }
    }
}

#[embassy_executor::task]
async fn connected_benchmark_task(
    stack: Stack<'static>,
    registers: &'static RefCell<&'static mut RadioRegisters>,
) {
    let mut buffers = RadioHilBenchmarkBuffers::init();
    loop {
        let config = OPEN_RADIO_CONNECTED_BENCHMARK_START.receive().await;
        let _ = select(
            OPEN_RADIO_CONNECTED_BENCHMARK_STOP.wait(),
            observe_open_radio_task_polls(
                run_open_radio_udp_benchmark(
                    stack,
                    config.association_phy,
                    config.data_tx_rate,
                    registers,
                    &mut buffers,
                ),
                &OPEN_RADIO_TASK_POLLS.benchmark,
            ),
        )
        .await;
        OPEN_RADIO_CONNECTED_BENCHMARK_STOPPED.signal(());
    }
}

async fn run_connected_network<'fixture, 'security>(
    fixture: RadioHilConnectedTaskFixture<'fixture>,
    epoch_resources: RadioHilConnectedEpochResources,
    session: StaConnectedSession<'security>,
    pairwise_slot: StaPairwiseCcmpSlot,
    group_slot: StaGroupCcmpSlot,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
) -> RadioHilConnectedEpochReturn<'fixture, 'security> {
    let reconnected_epoch = matches!(
        &epoch_resources,
        RadioHilConnectedEpochResources::Reconnected(_)
    );
    let RadioHilConnectedTaskFixture {
        spawner,
        protocol_spawner,
        state,
        platform,
        interrupt_epoch,
        rx_storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        scan_table,
        frame,
        ethernet,
    } = fixture;
    let StaConnectedSession {
        generation,
        peer,
        network,
        pmk,
        supplicant_nonce,
        sequences,
    } = session;
    let connected_plan = Esp32s31ConnectedStaPort::prepare::<TX_AMPDU_FRAME_COUNT>(
        peer,
        radio_hil_connected_sta_config(),
    )
    .unwrap_or_else(|failure| panic!("invalid connected STA policy: {:?}", failure.error));
    let link = connected_plan.link();
    let Esp32s31StaConnectedLink {
        station_address,
        association_phy,
        ..
    } = link;
    // The polling-only scan/auth path kept every MAC interrupt masked. Consume
    // the last task-side enable/clear capability immediately before the
    // connected path enables the ISR-owned RX/TX Signal sink.
    // After `activate`, ordinary `RadioRegisters` cannot touch those
    // registers.
    interrupt_epoch
        .activate(platform, MAC_COLD_RX_INTERRUPT_MASK)
        .unwrap_or_else(|error| panic!("MAC interrupt epoch activation failed: {error:?}"));

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
    let data_tx_rate = connected_plan.data_tx_rate();
    let benchmark_tx_rate = connected_plan.aggregate_tx_rate();
    let peer_ampdu_limit = tx_storage
        .control()
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
        RadioHilConnectedEpochResources::Initial { registers, rx } => {
            let rx_ring = match rx.try_into_live_with_storage(registers, rx_storage).await {
                Ok(ring) => ring,
                Err(failure) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch=initial error={:?}",
                        failure.error,
                    ));
                    let _owner = failure.owner;
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
            };
            let rx = Esp32s31RxEpochResources::new(
                rx_storage.buffers(),
                &OPEN_RADIO_RX_STAGE_POOL,
                staged_rx_sender,
                OpenRadioRxReloadDelay,
            )
            .with_pipeline_counters(&OPEN_RADIO_RX_PIPELINE_COUNTERS)
            .with_live_ring(rx_ring);
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
        RadioHilConnectedEpochResources::Reconnected(epoch) => {
            let Esp32s31ReconnectedStaEpochParts {
                mut hardware,
                rx,
                rx_resources,
                aggregate_tx: ampdu,
                control: control_resources,
            } = epoch.into_parts();
            let rx_ring = match rx
                .try_into_live_with_storage(&mut hardware, rx_storage)
                .await
            {
                Ok(ring) => ring,
                Err(failure) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=production-runner-rx-arm epoch=reconnected \
                         error={:?}",
                        failure.error,
                    ));
                    let _owners = (
                        hardware,
                        failure.owner,
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
    let rx_protocol = Esp32s31ConnectedStaPort::build_rx_protocol(
        &connected_plan,
        Esp32s31ConnectedStaRxProtocolResources {
            frames: staged_rx_receiver,
            irq: &OPEN_RADIO_IRQ_RUNTIME,
            sink: rx_sink,
            mpdu: frame,
            ethernet,
            reorder_commands: rx_reorder_receiver,
            reorder_storage: &OPEN_RADIO_RX_REORDER_STORAGE,
            reorder_scratch: None,
            pipeline_counters: Some(&OPEN_RADIO_RX_PIPELINE_COUNTERS),
        },
    );

    let tx_sequences = core::mem::replace(sequences, StaTxSequenceCounters::new(0));
    let control_tx = tx_storage
        .take_control()
        .expect("control TX owner moves into the connected runner exactly once");
    let tx = Esp32s31ConnectedStaPort::build_tx(
        &connected_plan,
        Esp32s31ConnectedStaTxResources {
            control: control_tx,
            aggregate: tx_ampdu_storage,
            pairwise_key: pairwise_slot,
            sequences: tx_sequences,
            counters: Some(&OPEN_RADIO_TX_AGGREGATE_COUNTERS),
            network_domain: Esp32s31ConnectedStaNetworkTxDomain::new(),
        },
    )
    .unwrap_or_else(|_failure| panic!("connected handoff requires an idle control TX owner"));
    let control = Esp32s31ConnectedStaPort::build_control(
        &connected_plan,
        Esp32s31ConnectedStaControlResources {
            receiver: control_receiver,
            reorder_commands: rx_reorder_sender,
        },
    );

    let registers = hardware.register_cell();
    let drivers = Esp32s31ConnectedStaPort::assemble(
        connected_plan,
        Esp32s31ConnectedStaDriverParts {
            hardware,
            rx,
            tx,
            control,
            protocol: rx_protocol,
        },
    );
    let rx_protocol = drivers.protocol;
    let backend = FaultInjectingWifiBackend::new(drivers.backend, &STATION_FAULT_CONTROL);
    let mut radio_runner =
        WifiRunner::new(&OPEN_RADIO_IRQ_RUNTIME, network_runner, backend);

    let network_started = stack_runner.is_some();
    if let Some(stack_runner) = stack_runner {
        let stack_task = connected_network_stack_task(stack_runner)
            .unwrap_or_else(|_| panic!("connected network task allocation failed"));
        spawner.spawn(stack_task);
        let report_task = connected_network_report_task(stack)
            .unwrap_or_else(|_| panic!("connected network report task allocation failed"));
        spawner.spawn(report_task);
        let benchmark_task = connected_benchmark_task(stack, registers)
            .unwrap_or_else(|_| panic!("connected benchmark task allocation failed"));
        spawner.spawn(benchmark_task);
    }
    // embassy-net intentionally stores its Stack/Runner state behind a
    // RefCell and is therefore !Send. Keep that owner, the PAC runner and the
    // MMIO-backed tasks on Core 0. The staged protocol owns only cross-core
    // CriticalSectionRawMutex queues and is compiler-proven Send, so moving it
    // to Core 1 removes one long cooperative poll interval without inventing
    // a fixed per-wake frame ceiling.
    let protocol_task = connected_rx_protocol_task(rx_protocol)
        .unwrap_or_else(|_| panic!("connected RX protocol task allocation failed"));
    protocol_spawner.spawn(protocol_task);
    OPEN_RADIO_CONNECTED_BENCHMARK_START
        .send(RadioHilConnectedBenchmarkConfig {
            association_phy,
            data_tx_rate: benchmark_tx_rate,
        })
        .await;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-task-topology \
         network=core0 rx_protocol=core1 radio=sta-parent-core0 \
         report=core0 benchmark=core0 network_started={}",
        u8::from(network_started)
    ));
    crate::console::publish_station_lifecycle(StationLifecycleEvent::Connected { generation })
        .await;
    if reconnected_epoch {
        report_station_epoch_progress(RadioHilStationEpochProgress::ConnectedRunnerStarted);
    }

    // The radio loop intentionally remains in this parent STA future. Other
    // long-running owners still have independent executor tasks/wakers, while
    // disconnect returns RX/TX/control ownership into the same scope that
    // retains the GTK and platform token. A spawned task could only report
    // the edge and would strand those values in its private task storage.
    let runner_exit = match observe_open_radio_task_polls(
        run_esp32s31_connected_station_epoch(&mut radio_runner, station_control),
        &OPEN_RADIO_TASK_POLLS.radio,
    )
    .await
    {
        Esp32s31ConnectedStationExit::Disconnected => {
            let control = radio_runner.backend().inner().control();
            let beacon_monitor = control.beacon_monitor();
            let beacon_lost = control.beacon_lost();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-runner \
                 exit=disconnected beacon_lost={} beacons_observed={} \
                 beacon_deadline_us={:?} last_control_event={:?} last_tx_failure={:?}",
                u8::from(beacon_lost),
                beacon_monitor.map_or(0, |monitor| monitor.observed()),
                beacon_monitor.and_then(|monitor| monitor.deadline_micros()),
                control.last_event(),
                control.last_tx_failure(),
            ));
            RadioHilConnectedExit::Disconnected { beacon_lost }
        }
        Esp32s31ConnectedStationExit::ReconnectRequested { source } => {
            let source = match source {
                Esp32s31StationReconnectSource::Controller => "station-controller",
                Esp32s31StationReconnectSource::CoalescedDisconnect => "coalesced-disconnect",
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-runner-stop \
                 source={source} command=Reconnect"
            ));
            RadioHilConnectedExit::ReconnectRequested
        }
        Esp32s31ConnectedStationExit::StationStopped(command) => {
            RadioHilConnectedExit::StationStopped(command)
        }
        Esp32s31ConnectedStationExit::HardwareFailure(error) => {
            match error {
                FaultInjectingBackendError::InjectedTxAfterPublication { fault, source } => {
                    let reset_required = injected_tx_source_requires_reset(&source);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result={} stage=production-runner-fault \
                         injection={:?} request_id={} reset_required={} source={source:?}",
                        if reset_required { "PASS" } else { "FAIL" },
                        fault.injection,
                        fault.request_id,
                        u8::from(reset_required),
                    ));
                    RadioHilConnectedExit::InjectedTxFault {
                        fault,
                        reset_required,
                    }
                }
                FaultInjectingBackendError::Inner(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner error={error:?}"
                    ));
                    RadioHilConnectedExit::HardwareFailure
                }
                FaultInjectingBackendError::InjectionContractViolation { fault, progress } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner-fault \
                         injection={:?} request_id={} contract_progress={progress:?}",
                        fault.injection, fault.request_id,
                    ));
                    RadioHilConnectedExit::HardwareFailure
                }
            }
        }
    };
    // Close hardware publication before stopping the protocol consumer. The
    // radio runner no longer schedules RX/control; masking both CPU and
    // peripheral routes now makes the command/frame drain finite and prevents
    // a stale wake from leaking into the next connected epoch.
    let interrupt_drain = interrupt_epoch
        .quiesce(platform)
        .unwrap_or_else(|error| panic!("MAC interrupt epoch quiescence failed: {error:?}"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-interrupt-stop \
         rx_wake={} rx_capacity_wake={} tx_events={:#010x} power_events={:#010x}",
        u8::from(interrupt_drain.mac.rx),
        u8::from(interrupt_drain.mac.rx_capacity),
        interrupt_drain.mac.tx_events,
        interrupt_drain.power_events,
    ));
    // No spawned task may retain a PAC borrow when this epoch returns. The
    // benchmark is the only task besides the radio runner that receives the
    // register cell; stop it before waiting for protocol ownership release.
    let mut connected_tasks = RadioHilConnectedTaskGroup;
    let stopped_protocol = match stop_esp32s31_connected_task_group(
        &mut connected_tasks,
        OPEN_RADIO_CONNECTED_TASK_STOP_TIMEOUT,
    )
    .await
    {
        Esp32s31ConnectedTaskStopOutcome::Stopped(stopped) => stopped,
        Esp32s31ConnectedTaskStopOutcome::ResetRequired { timeout } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-connected-task-stop error=timeout \
                 timeout_ms={} reset_required=1",
                timeout.as_millis(),
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-benchmark-stopped"
    ));
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
    let backend = backend.into_inner();
    let mut teardown = match Esp32s31ConnectedStaTeardownPort::try_teardown(backend, group_slot) {
        Ok(teardown) => teardown,
        Err(Esp32s31ConnectedStaTeardownFailure::Control {
            error,
            backend,
            group_key,
        }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-control-stop error={error:?}"
            ));
            let _owners = (network, backend, group_key);
            loop {
                Timer::after_secs(60).await;
            }
        }
        Err(Esp32s31ConnectedStaTeardownFailure::Rx {
            error,
            hardware,
            rx,
            tx,
            control,
            group_key,
        }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-rx-dma-stop error={error:?}"
            ));
            let _owners = (network, hardware, rx, tx, control, group_key);
            loop {
                Timer::after_secs(60).await;
            }
        }
        Err(Esp32s31ConnectedStaTeardownFailure::TxActive {
            hardware,
            stopped_rx,
            tx,
            control,
            group_key,
        }) => {
            if let RadioHilConnectedExit::InjectedTxFault {
                fault,
                reset_required,
            } = runner_exit
            {
                let tx_owner_reset_required = tx.is_reset_required();
                let complete = reset_required && tx_owner_reset_required;
                let evidence = StationFaultEvidence {
                    injection: fault.injection,
                    classification: if complete {
                        StationFaultClassification::RadioResetRequired
                    } else {
                        StationFaultClassification::ContractViolation
                    },
                    runner_returned: true,
                    executor_tasks_stopped: true,
                    rx_dma_stopped: true,
                    tx_owner_reset_required,
                };
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result={} stage=production-station-fault-frontier \
                     injection={:?} request_id={} runner_returned=1 tasks_stopped=1 \
                     rx_dma_stopped=1 tx_reset_required={} source_reset_required={}",
                    if evidence.is_complete() { "PASS" } else { "FAIL" },
                    fault.injection,
                    fault.request_id,
                    u8::from(tx_owner_reset_required),
                    u8::from(reset_required),
                ));
                crate::console::publish_station_fault(fault.request_id, evidence).await;
            } else {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                     stage=production-connected-tx-return error=aggregate-active"
                ));
            }
            let _owners = (network, hardware, stopped_rx, tx, control, group_key);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let control_shutdown = teardown.control;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-control-stop \
         rx_ba={} tx_ba={} discarded_events={} in_flight={:?}",
        control_shutdown.rx_block_ack_agreements,
        control_shutdown.tx_block_ack_sessions,
        control_shutdown.discarded_events,
        control_shutdown.in_flight,
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-rx-dma-stop \
         descriptor_base={:#010x} queued_frames={}",
        teardown.stopped_rx.ring().descriptor_base(),
        teardown.stopped_rx.queued_frames(),
    ));
    let key_bitmap = teardown.hardware.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP);
    let keys_cleared = key_bitmap
        & ((1 << teardown.keys.pairwise_hardware_index)
            | (1 << teardown.keys.group_hardware_index))
        == 0;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result={} stage=production-connected-key-clear \
         pairwise_slot={} group_slot={} valid_bitmap={key_bitmap:#010x}",
        if keys_cleared { "PASS" } else { "FAIL" },
        teardown.keys.pairwise_hardware_index,
        teardown.keys.group_hardware_index,
    ));
    *sequences = teardown.sequences;
    tx_storage
        .restore_resources(teardown.tx_resources)
        .unwrap_or_else(|_| panic!("connected TX return found a live owner"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-connected-tx-return"
    ));
    let disconnected = RadioHilDisconnectedEpoch::new(
        RadioHilRunningNetwork {
            stack,
            runner: network,
        },
        teardown.hardware,
        teardown.stopped_rx,
        teardown.aggregate,
        control_resources,
    );
    let lifecycle_event = match runner_exit {
        RadioHilConnectedExit::Disconnected { beacon_lost } => {
            Some(StationLifecycleEvent::Disconnected {
                generation,
                reason: if beacon_lost {
                    StationDisconnectReason::BeaconLoss
                } else {
                    StationDisconnectReason::LinkPolicy
                },
            })
        }
        RadioHilConnectedExit::ReconnectRequested => {
            Some(StationLifecycleEvent::Disconnected {
                generation,
                reason: StationDisconnectReason::ReconnectRequested,
            })
        }
        RadioHilConnectedExit::StationStopped(_)
        | RadioHilConnectedExit::InjectedTxFault { .. }
        | RadioHilConnectedExit::HardwareFailure => None,
    };
    if let Some(event) = lifecycle_event {
        crate::console::publish_station_lifecycle(event).await;
    }
    if matches!(runner_exit, RadioHilConnectedExit::ReconnectRequested) {
        report_station_epoch_progress(RadioHilStationEpochProgress::RunnerStopped);
    }
    RadioHilConnectedEpochReturn {
        fixture: RadioHilConnectedTaskFixture {
            spawner,
            protocol_spawner,
            state,
            platform,
            interrupt_epoch,
            rx_storage,
            tx_storage,
            descriptor_base,
            buffer_addresses,
            scan_table,
            frame,
            ethernet,
        },
        disconnected,
        security: StaAssociationSecurity {
            pmk,
            supplicant_nonce,
            sequences,
        },
        exit: runner_exit,
    }
}

/// Borrowed board context for one running candidate scan.
///
/// Keeping the borrowed capabilities together prevents the scan entry point
/// from regrowing a positional argument list while the disconnected epoch
/// itself remains one separately owned value.
struct RadioHilRunningScanContext<'fixture, 'ssid> {
    state: &'fixture mut PhyColdState,
    platform: &'fixture mut EspHalRadioPeripheral,
    tx_storage: &'fixture mut TxStorage,
    interrupt_setup: &'fixture MacInterruptSetup,
    scan_table: &'fixture mut ScanTable,
    scan_frame: &'fixture mut [u8],
    station_address: [u8; 6],
    target_ssid: &'ssid [u8],
    sequence: &'fixture mut StaSequenceCounter,
}

/// Complete resource and candidate result of one running scan.
struct RadioHilRunningScanReturn {
    disconnected: RadioHilDisconnectedEpoch,
    candidate: ScanRecord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilRunningScanFailure {
    NoCandidate {
        channels_completed: u16,
    },
    Stopped {
        channels_completed: u16,
    },
    Transaction {
        error: Esp32s31StaScanError<RadioHilRunningScanPortError>,
        channels_completed: u16,
    },
    InvalidPlan(StaScanPlanError),
}

struct RadioHilRunningScanRecovery {
    disconnected: RadioHilDisconnectedEpoch,
    failure: RadioHilRunningScanFailure,
}

struct RadioHilRunningScanFrameObserver {
    station_address: [u8; 6],
    probe_responses: u32,
}

impl Esp32s31ScanFrameObserver for RadioHilRunningScanFrameObserver {
    fn observe(&mut self, frame: &[u8], _rssi: i8, table_outcome: ScanObservation) {
        if frame.len() >= 10 && frame[0] & 0xfc == 0x50 && frame[4..10] == self.station_address {
            self.probe_responses = self.probe_responses.saturating_add(1);
            if self.probe_responses <= 3 {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL probe=addressed-probe-response \
                     count={} da={:02x?} sa={:02x?} table={table_outcome:?}",
                    self.probe_responses,
                    &frame[4..10],
                    &frame[10..16],
                ));
            }
        }
    }
}

/// Prove that one disconnected owner can complete a finite multi-channel
/// running scan and return every resource without reinitializing static
/// storage.
async fn qualify_disconnected_running_scan(
    epoch: RadioHilDisconnectedEpoch,
    context: RadioHilRunningScanContext<'_, '_>,
) -> Result<RadioHilRunningScanReturn, RadioHilRunningScanRecovery> {
    let RadioHilRunningScanContext {
        state,
        platform,
        tx_storage,
        interrupt_setup,
        scan_table,
        scan_frame,
        station_address,
        target_ssid,
        sequence,
    } = context;
    let Esp32s31RunningScanEpochParts {
        retained,
        hardware,
        rx,
    } = epoch.into_running_scan_parts();
    let control = tx_storage
        .take_control()
        .expect("connected teardown returned the ordinary TX owner");
    let scan_owner = Esp32s31RunningScanPort::new(
        Esp32s31RunningScanRadio::new(
            Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(state, platform, HilPhyObserver),
            hardware,
            RunningScanRx::from_stopped(rx),
            RunningScanTx::new(control, interrupt_setup),
        ),
        Esp32s31RunningScanStorage::new(
            scan_table,
            scan_frame,
            RadioHilRunningScanFrameObserver {
                station_address,
                probe_responses: 0,
            },
            sequence,
        ),
        Esp32s31RunningScanStation::new(station_address, target_ssid, &PROBE_REQUEST_RATES)
            .with_descriptor_capacity(PROBE_TX_DESCRIPTOR_CAPACITY as u32),
        EmbassyEsp32s31RunningScanTimer,
    );
    let scan_config =
        Esp32s31StaScanConfig::new(SCAN_DWELL_MS).expect("fixed HIL scan dwell policy is nonzero");
    let scan_backend = Esp32s31StaScanBackend::new(scan_config);
    let mut scan_service = StaCandidateScanService::new(scan_backend);
    let scan_started = Instant::now();
    let (scan_owner, outcome) = match scan_service.run(scan_owner, &STA_SCAN_CHANNELS).await {
        StaCandidateScanExit::Selected {
            owner,
            candidate,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=production-running-scan \
                 channels={} elapsed_ms={} candidate_channel={} candidate_rssi={}",
                progress.channels_completed,
                scan_started.elapsed().as_millis(),
                candidate.channel,
                candidate.rssi,
            ));
            (owner, Ok(candidate))
        }
        StaCandidateScanExit::NoCandidate { owner, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-running-scan \
                 channels={} error=no-candidate",
                progress.channels_completed,
            ));
            (
                owner,
                Err(RadioHilRunningScanFailure::NoCandidate {
                    channels_completed: progress.channels_completed,
                }),
            )
        }
        StaCandidateScanExit::Stopped { owner, progress } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan \
                 channels={} error=stopped",
                progress.channels_completed,
            ));
            (
                owner,
                Err(RadioHilRunningScanFailure::Stopped {
                    channels_completed: progress.channels_completed,
                }),
            )
        }
        StaCandidateScanExit::Failed {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan \
                 channels={} error={error:?}",
                progress.channels_completed,
            ));
            (
                owner,
                Err(RadioHilRunningScanFailure::Transaction {
                    error,
                    channels_completed: progress.channels_completed,
                }),
            )
        }
        StaCandidateScanExit::InvalidPlan {
            owner,
            error,
            progress,
        } => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-running-scan-plan \
                 channels={} error={error:?}",
                progress.channels_planned,
            ));
            (owner, Err(RadioHilRunningScanFailure::InvalidPlan(error)))
        }
    };

    let Esp32s31RunningScanParts {
        phy,
        hardware,
        rx,
        tx,
        timer: _,
        observer,
        telemetry,
        ..
    } = scan_owner.into_parts();
    let probe_responses = observer.probe_responses;
    let (_state, _platform, _observer) = phy.into_parts();
    let rx = match rx.into_stopped() {
        Ok(rx) => rx,
        Err(rx) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-running-scan-return phase={:?}",
                rx.phase(),
            ));
            let _owners = (retained, hardware, rx, tx);
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    let (control, tx_summary) = tx.into_parts();
    tx_storage
        .restore_control(control)
        .unwrap_or_else(|_| panic!("running scan returned over a live TX owner"));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-running-scan-owner-return \
         descriptor_base={:#010x} queued_frames={} probe_completions={} probe_failures={} \
         raw_frames={} probe_responses={} ring_epochs={}",
        rx.ring().descriptor_base(),
        rx.queued_frames(),
        tx_summary.completions,
        tx_summary.failures,
        telemetry.raw_frames,
        probe_responses,
        telemetry.ring_epochs,
    ));
    let disconnected = retained.restore(hardware, rx);
    match outcome {
        Ok(candidate) => {
            report_station_epoch_progress(RadioHilStationEpochProgress::ScanOwnersReturned);
            Ok(RadioHilRunningScanReturn {
                disconnected,
                candidate,
            })
        }
        Err(failure) => Err(RadioHilRunningScanRecovery {
            disconnected,
            failure,
        }),
    }
}

async fn run_initial_station_attempt<'fixture, 'security>(
    ready: RadioHilAuthenticationReady<'fixture, 'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilAuthenticationReady {
        fixture,
        target,
        rx,
        network,
        security,
    } = ready;
    let StaAssociationSecurity {
        pmk,
        supplicant_nonce,
        sequences,
    } = security;
    let channel = Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(
        &mut *fixture.state,
        &mut *fixture.platform,
        HilPhyObserver,
    );
    let owner: RadioHilStaAttemptOwner<'_, '_, '_, '_, '_, RadioRegisters> =
        Esp32s31StaAttemptTargetOwner::new(
            Esp32s31StaAttemptRadio::new(
                &mut *fixture.mmio,
                channel,
                rx,
                fixture.rx_storage,
                fixture
                    .tx_storage
                    .control_mut()
                    .expect("initial station attempt owns control TX"),
            ),
            Esp32s31StaAttemptStorage::new(&mut *fixture.frame),
            Esp32s31StaAttemptStation {
                station_address: target.station_address,
                access_point: target.access_point,
                association_preference: STA_ASSOCIATION_PREFERENCE,
            },
            Esp32s31StaAttemptSecurity {
                pmk,
                supplicant_nonce,
                sequences,
                message4_protection: radio_hil_message4_protection(),
            },
        );
    let mut attempt = Esp32s31StaAttempt::new(Esp32s31StaAttemptTargetPort::new());
    match attempt.run(owner).await {
        Esp32s31StaAttemptOutcome::Failed(failure) => {
            let (owner, stage, disposition, error, progress) = failure.into_parts();
            let report = owner.report();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=production-sta-attempt \
                 phase={stage:?} disposition={disposition:?} error={error:?}"
            ));
            let (radio, _storage, _station, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
            let security = StaAssociationSecurity {
                pmk: security.pmk,
                supplicant_nonce: security.supplicant_nonce,
                sequences: security.sequences,
            };
            let associated = progress.completed(Esp32s31StaAttemptStage::Association);
            let message1 = report
                .wpa2_handshake
                .is_some_and(|telemetry| telemetry.message2_transmissions != 0);
            let message3 = progress.completed(Esp32s31StaAttemptStage::Wpa2Handshake);
            let lifecycle_error = if progress.completed(Esp32s31StaAttemptStage::Authentication) {
                RadioHilStaLifecycleFailure::InitialJoin {
                    associated,
                    message1,
                    message3,
                }
            } else {
                RadioHilStaLifecycleFailure::Authentication
            };
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::Authenticate(RadioHilAuthenticationReady {
                    fixture,
                    target,
                    rx: receive,
                    network,
                    security,
                }),
                failure: StaAttemptFailure::new(
                    stage.lifecycle_stage(),
                    disposition,
                    lifecycle_error,
                ),
            }
        }
        Esp32s31StaAttemptOutcome::Connected {
            connected,
            progress: _,
        } => {
            let mut owner = connected.into_owner();
            let report = owner.report();
            let peer = owner
                .take_connected_peer()
                .expect("successful station attempt owns its connected peer");
            let (pairwise, group) = owner
                .take_installed_keys()
                .expect("successful station attempt owns both CCMP slots");
            let (radio, _storage, _station, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
            let security = StaAssociationSecurity {
                pmk: security.pmk,
                supplicant_nonce: security.supplicant_nonce,
                sequences: security.sequences,
            };
            let authentication = report
                .authentication
                .expect("successful station attempt reports Authentication");
            let association = report
                .association
                .expect("successful station attempt reports Association");
            let wpa2 = report
                .wpa2
                .expect("successful station attempt reports WPA2 key install");
            let message4 = report
                .message4
                .expect("successful station attempt reports Message 4 completion");
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=sta-auth-response \
                 attempt={} frames={} bssid={:02x?}",
                authentication.attempt,
                authentication.total_received_frames,
                target.access_point.bssid,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=sta-assoc-response \
                 status={} aid={} frames={} bssid={:02x?}",
                association.response.status_code,
                association.response.association_id,
                association.total_received_frames,
                target.access_point.bssid,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-4-tx \
                 protected={} replay={} status={} primary={:#010x}",
                WPA2_MESSAGE_4_HARDWARE_PROTECTED,
                wpa2.replay_counter,
                message4.status,
                message4.primary_word,
            ));
            let (connected_fixture, registers) = fixture.into_task_fixture();
            let returned = run_connected_network(
                connected_fixture,
                RadioHilConnectedEpochResources::Initial {
                    registers,
                    rx: receive,
                },
                StaConnectedSession {
                    generation,
                    peer,
                    network,
                    pmk: security.pmk,
                    supplicant_nonce: security.supplicant_nonce,
                    sequences: security.sequences,
                },
                pairwise,
                group,
                station_control,
            )
            .await;
            connected_attempt_outcome(returned, target)
        }
    }
}

fn connected_attempt_outcome<'fixture, 'security>(
    returned: RadioHilConnectedEpochReturn<'fixture, 'security>,
    target: StaJoinTarget,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilConnectedEpochReturn {
        fixture,
        disconnected,
        security,
        exit,
    } = returned;
    let owner = RadioHilRunningScanReady {
        fixture,
        previous_target: target,
        disconnected,
        security,
    };
    match exit {
        RadioHilConnectedExit::Disconnected { .. }
        | RadioHilConnectedExit::ReconnectRequested => {
            StaAttemptOutcome::Disconnected {
                owner: RadioHilStaLifecycleOwner::RunningScan(owner),
                next_candidate: StaNextCandidate::Refresh,
            }
        }
        RadioHilConnectedExit::StationStopped(command) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-station-stop command={command:?}"
            ));
            StaAttemptOutcome::Stopped {
                owner: RadioHilStaLifecycleOwner::RunningScan(owner),
            }
        }
        RadioHilConnectedExit::InjectedTxFault { .. }
        | RadioHilConnectedExit::HardwareFailure => StaAttemptOutcome::Failed {
            owner: RadioHilStaLifecycleOwner::RunningScan(owner),
            failure: StaAttemptFailure::new(
                StaLifecycleStage::Hardware,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::ConnectedHardware,
            ),
        },
    }
}

/// Execute the explicit candidate-refresh phase selected by the outer STA
/// lifecycle, then continue with fresh Authentication on the returned
/// cooperative hardware owner.
async fn run_running_scan_attempt<'fixture, 'security>(
    ready: RadioHilRunningScanReady<'fixture, 'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilRunningScanReady {
        fixture,
        previous_target,
        disconnected,
        security,
    } = ready;
    let scan_result = qualify_disconnected_running_scan(
        disconnected,
        RadioHilRunningScanContext {
            state: &mut *fixture.state,
            platform: &mut *fixture.platform,
            tx_storage: &mut *fixture.tx_storage,
            interrupt_setup: fixture
                .interrupt_epoch
                .setup()
                .expect("connected teardown returned the quiesced interrupt owner"),
            scan_table: &mut *fixture.scan_table,
            scan_frame: &mut *fixture.frame,
            station_address: previous_target.station_address,
            target_ssid: previous_target.access_point.ssid_bytes(),
            sequence: security.sequences.non_qos_mut(),
        },
    )
    .await;
    let scan_return = match scan_result {
        Ok(scan_return) => scan_return,
        Err(recovery) => {
            let owner = RadioHilStaLifecycleOwner::RunningScan(RadioHilRunningScanReady {
                fixture,
                previous_target,
                disconnected: recovery.disconnected,
                security,
            });
            let (disposition, error) = match recovery.failure {
                RadioHilRunningScanFailure::NoCandidate { .. } => (
                    StaFailureDisposition::RefreshCandidate,
                    RadioHilStaLifecycleFailure::RunningScanNoCandidate,
                ),
                RadioHilRunningScanFailure::Stopped { .. } => {
                    return StaAttemptOutcome::Stopped { owner };
                }
                RadioHilRunningScanFailure::Transaction { error, .. } => {
                    let disposition = match error {
                        Esp32s31StaScanError::ActiveProbe(
                            RadioHilRunningScanPortError::Transmit(_),
                        )
                        | Esp32s31StaScanError::ReceiveStop(_) => StaFailureDisposition::Terminal,
                        _ => StaFailureDisposition::RefreshCandidate,
                    };
                    (
                        disposition,
                        RadioHilStaLifecycleFailure::RunningScanTransaction(error),
                    )
                }
                RadioHilRunningScanFailure::InvalidPlan(error) => (
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::RunningScanPlan(error),
                ),
            };
            return StaAttemptOutcome::Failed {
                owner,
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    disposition,
                    error,
                ),
            };
        }
    };
    let target = StaJoinTarget {
        station_address: previous_target.station_address,
        access_point: scan_return.candidate,
    };
    assert_join_hardware_capabilities(scan_return.disconnected.hardware());
    let (network, epoch) = scan_return
        .disconnected
        .prepare_reconnect::<EmbassyEsp32s31PreconnectedRxDelay>();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=production-reconnect-owner-ready \
         candidate_channel={} candidate_bssid={:02x?}",
        target.access_point.channel, target.access_point.bssid,
    ));
    run_reconnected_station_attempt(
        RadioHilReconnectReady {
            fixture,
            target,
            network: RadioHilStaNetwork::Running(network),
            epoch: RadioHilConnectedEpochResources::Reconnected(epoch),
            security,
        },
        station_control,
        generation,
    )
    .await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RadioHilStaLifecycleFailure {
    Authentication,
    InitialJoin {
        associated: bool,
        message1: bool,
        message3: bool,
    },
    CandidateRefreshContract,
    RunningScanNoCandidate,
    RunningScanTransaction(Esp32s31StaScanError<RadioHilRunningScanPortError>),
    RunningScanPlan(StaScanPlanError),
    InvalidEpochOwner,
    StationAttempt(Esp32s31StaAttemptStage),
    ConnectedHardware,
}

const fn protocol_station_failure_stage(stage: StaLifecycleStage) -> StationFailureStage {
    match stage {
        StaLifecycleStage::CandidateSelection => StationFailureStage::CandidateSelection,
        StaLifecycleStage::Authentication => StationFailureStage::Authentication,
        StaLifecycleStage::Association => StationFailureStage::Association,
        StaLifecycleStage::Security => StationFailureStage::Security,
        StaLifecycleStage::Connected => StationFailureStage::Connected,
        StaLifecycleStage::Hardware => StationFailureStage::Hardware,
    }
}

const fn protocol_station_failure_reason(
    error: RadioHilStaLifecycleFailure,
) -> StationAttemptFailureReason {
    match error {
        RadioHilStaLifecycleFailure::RunningScanNoCandidate => {
            StationAttemptFailureReason::NoCandidate
        }
        RadioHilStaLifecycleFailure::Authentication
        | RadioHilStaLifecycleFailure::InitialJoin { .. }
        | RadioHilStaLifecycleFailure::StationAttempt(_) => {
            StationAttemptFailureReason::PeerProtocol
        }
        RadioHilStaLifecycleFailure::RunningScanTransaction(_)
        | RadioHilStaLifecycleFailure::ConnectedHardware => {
            StationAttemptFailureReason::Hardware
        }
        RadioHilStaLifecycleFailure::CandidateRefreshContract
        | RadioHilStaLifecycleFailure::RunningScanPlan(_)
        | RadioHilStaLifecycleFailure::InvalidEpochOwner => {
            StationAttemptFailureReason::ContractViolation
        }
    }
}

struct RadioHilStaLifecycleBackend<'control, O> {
    station_control: RadioHilStationCommandReceiver<'control>,
    _owner: PhantomData<fn() -> O>,
}

impl<'control, O> RadioHilStaLifecycleBackend<'control, O> {
    const fn new(station_control: RadioHilStationCommandReceiver<'control>) -> Self {
        Self {
            station_control,
            _owner: PhantomData,
        }
    }
}

impl<'control, 'fixture, 'security> StaLifecycleBackend
    for RadioHilStaLifecycleBackend<'control, RadioHilStaLifecycleOwner<'fixture, 'security>>
{
    type Owner = RadioHilStaLifecycleOwner<'fixture, 'security>;
    type Error = RadioHilStaLifecycleFailure;

    fn run_attempt(
        &mut self,
        owner: Self::Owner,
        context: StaAttemptContext,
    ) -> impl Future<Output = StaAttemptOutcome<Self::Owner, Self::Error>> + '_ {
        async move {
            if let Some(command) = self.station_control.try_take() {
                match command {
                    Esp32s31StationCommand::Reconnect => {
                        let deferred = self.station_control.defer(command);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=OBSERVE \
                             stage=production-station-command command=reconnect \
                             action=deferred deferred={}",
                            u8::from(deferred),
                        ));
                    }
                    Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop => {
                        self.station_control.record_terminal(command);
                        return StaAttemptOutcome::Stopped { owner };
                    }
                }
            }
            let phase = match &owner {
                RadioHilStaLifecycleOwner::Authenticate(_) => "authentication",
                RadioHilStaLifecycleOwner::RunningScan(_) => "running-scan",
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
            let outcome = match owner {
                RadioHilStaLifecycleOwner::Authenticate(ready) => {
                    run_initial_station_attempt(
                        ready,
                        &mut self.station_control,
                        context.generation,
                    )
                    .await
                }
                RadioHilStaLifecycleOwner::RunningScan(ready) => {
                    if context.refresh_candidate {
                        run_running_scan_attempt(
                            ready,
                            &mut self.station_control,
                            context.generation,
                        )
                        .await
                    } else {
                        StaAttemptOutcome::Failed {
                            owner: RadioHilStaLifecycleOwner::RunningScan(ready),
                            failure: StaAttemptFailure::new(
                                StaLifecycleStage::CandidateSelection,
                                StaFailureDisposition::Terminal,
                                RadioHilStaLifecycleFailure::CandidateRefreshContract,
                            ),
                        }
                    }
                }
                RadioHilStaLifecycleOwner::Reconnect(ready) => {
                    run_reconnected_station_attempt(
                        ready,
                        &mut self.station_control,
                        context.generation,
                    )
                    .await
                }
            };
            if let StaAttemptOutcome::Failed { failure, .. } = &outcome {
                crate::console::publish_station_lifecycle(
                    StationLifecycleEvent::AttemptFailed {
                        generation: context.generation,
                        attempt: context.attempt,
                        stage: protocol_station_failure_stage(failure.stage),
                        reason: protocol_station_failure_reason(failure.error),
                    },
                )
                .await;
            }
            outcome
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
            match select(
                Timer::after_millis(u64::from(delay_millis)),
                self.station_control.wait(),
            )
            .await
            {
                Either::First(()) => StaBackoffOutcome::Elapsed { owner },
                Either::Second(command @ Esp32s31StationCommand::Reconnect) => {
                    let _ = self.station_control.defer(command);
                    StaBackoffOutcome::Elapsed { owner }
                }
                Either::Second(
                    command @ (Esp32s31StationCommand::Disconnect | Esp32s31StationCommand::Stop),
                ) => {
                    self.station_control.record_terminal(command);
                    StaBackoffOutcome::Stopped { owner }
                }
            }
        }
    }
}

async fn run_reconnected_station_attempt<'fixture, 'security>(
    ready: RadioHilReconnectReady<'fixture, 'security>,
    station_control: &mut RadioHilStationCommandReceiver<'_>,
    generation: u32,
) -> StaAttemptOutcome<RadioHilStaLifecycleOwner<'fixture, 'security>, RadioHilStaLifecycleFailure>
{
    let RadioHilReconnectReady {
        fixture,
        target,
        network,
        epoch,
        security,
    } = ready;
    let RadioHilConnectedEpochResources::Reconnected(mut epoch) = epoch else {
        return StaAttemptOutcome::Failed {
            owner: RadioHilStaLifecycleOwner::Reconnect(RadioHilReconnectReady {
                fixture,
                target,
                network,
                epoch,
                security,
            }),
            failure: StaAttemptFailure::new(
                StaLifecycleStage::Hardware,
                StaFailureDisposition::Terminal,
                RadioHilStaLifecycleFailure::InvalidEpochOwner,
            ),
        };
    };
    let StaAssociationSecurity {
        pmk,
        supplicant_nonce,
        sequences,
    } = security;
    let (hardware, rx_slot) = epoch.hardware_and_rx_mut();
    let receive = match rx_slot.take() {
        Ok(receive) => receive,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-attempt-rx error={error:?}"
            ));
            return StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::Reconnect(RadioHilReconnectReady {
                    fixture,
                    target,
                    network,
                    epoch: RadioHilConnectedEpochResources::Reconnected(epoch),
                    security: StaAssociationSecurity {
                        pmk,
                        supplicant_nonce,
                        sequences,
                    },
                }),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::InvalidEpochOwner,
                ),
            };
        }
    };
    let channel = Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(
        &mut *fixture.state,
        &mut *fixture.platform,
        HilPhyObserver,
    );
    let owner: RadioHilStaAttemptOwner<'_, '_, '_, '_, '_, ConnectedHardware> =
        Esp32s31StaAttemptTargetOwner::new(
            Esp32s31StaAttemptRadio::new(
                hardware,
                channel,
                receive,
                fixture.rx_storage,
                fixture
                    .tx_storage
                    .control_mut()
                    .expect("reconnected station attempt owns control TX"),
            ),
            Esp32s31StaAttemptStorage::new(&mut *fixture.frame),
            Esp32s31StaAttemptStation {
                station_address: target.station_address,
                access_point: target.access_point,
                association_preference: STA_ASSOCIATION_PREFERENCE,
            },
            Esp32s31StaAttemptSecurity {
                pmk,
                supplicant_nonce,
                sequences,
                message4_protection: radio_hil_message4_protection(),
            },
        );
    let mut attempt = Esp32s31StaAttempt::new(Esp32s31StaAttemptTargetPort::new());
    match attempt.run(owner).await {
        Esp32s31StaAttemptOutcome::Failed(failure) => {
            let (owner, stage, disposition, error, _progress) = failure.into_parts();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=production-reconnect-attempt phase={stage:?} \
                 disposition={disposition:?} error={error:?}"
            ));
            let (radio, _storage, _station, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
            let (_, rx_slot) = epoch.hardware_and_rx_mut();
            *rx_slot = receive;
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::Reconnect(RadioHilReconnectReady {
                    fixture,
                    target,
                    network,
                    epoch: RadioHilConnectedEpochResources::Reconnected(epoch),
                    security: StaAssociationSecurity {
                        pmk: security.pmk,
                        supplicant_nonce: security.supplicant_nonce,
                        sequences: security.sequences,
                    },
                }),
                failure: StaAttemptFailure::new(
                    stage.lifecycle_stage(),
                    disposition,
                    RadioHilStaLifecycleFailure::StationAttempt(stage),
                ),
            }
        }
        Esp32s31StaAttemptOutcome::Connected {
            connected,
            progress: _,
        } => {
            let mut owner = connected.into_owner();
            let report = owner.report();
            let peer = owner
                .take_connected_peer()
                .expect("successful reconnect owns its connected peer");
            let (pairwise, group) = owner
                .take_installed_keys()
                .expect("successful reconnect owns both CCMP slots");
            let (radio, _storage, _station, security) = owner.into_parts();
            let Esp32s31StaAttemptRadio {
                hardware: _,
                channel,
                receive,
                rx_storage: _,
                transmit: _,
            } = radio;
            let _ = channel.into_parts();
            let (_, rx_slot) = epoch.hardware_and_rx_mut();
            *rx_slot = receive;
            let authentication = report
                .authentication
                .expect("successful reconnect reports Authentication");
            let association = report
                .association
                .expect("successful reconnect reports Association");
            let wpa2 = report
                .wpa2
                .expect("successful reconnect reports WPA2 key install");
            let message4 = report
                .message4
                .expect("successful reconnect reports Message 4 completion");
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-authentication attempt={} frames={} bssid={:02x?}",
                authentication.attempt,
                authentication.total_received_frames,
                target.access_point.bssid,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-association status={} aid={} frames={}",
                association.response.status_code,
                association.response.association_id,
                association.total_received_frames,
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-wpa2-complete replay={} \
                 message4_status={} message4_primary={:#010x} \
                 pairwise_slot={} group_slot={}",
                wpa2.replay_counter,
                message4.status,
                message4.primary_word,
                pairwise.hardware_index(),
                group.hardware_index(),
            ));
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS \
                 stage=production-reconnect-connected-enter"
            ));
            report_station_epoch_progress(RadioHilStationEpochProgress::JoinCompleted);
            let returned = run_connected_network(
                fixture,
                RadioHilConnectedEpochResources::Reconnected(epoch),
                StaConnectedSession {
                    generation,
                    peer,
                    network,
                    pmk: security.pmk,
                    supplicant_nonce: security.supplicant_nonce,
                    sequences: security.sequences,
                },
                pairwise,
                group,
                station_control,
            )
            .await;
            connected_attempt_outcome(returned, target)
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

struct RadioHilColdScanFrameObserver<'a> {
    station_address: [u8; 6],
    probe_responses: &'a mut u32,
}

impl Esp32s31ScanFrameObserver for RadioHilColdScanFrameObserver<'_> {
    fn observe(&mut self, frame: &[u8], _rssi: i8, table_outcome: ScanObservation) {
        if frame.len() >= 10 && frame[0] & 0xfc == 0x50 && frame[4..10] == self.station_address {
            *self.probe_responses = self.probe_responses.saturating_add(1);
            if *self.probe_responses <= 3 {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL probe=addressed-probe-response \
                     count={} da={:02x?} sa={:02x?} table={table_outcome:?}",
                    *self.probe_responses,
                    &frame[4..10],
                    &frame[10..16],
                ));
            }
        }
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
    let tx_storage = OPEN_RADIO_TX_STATE.init(TxStorage::from_slot(
        tx_slot,
        state
            .tx_target_power_profile()
            .with_maximum_quarter_dbm(OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM),
        open_radio_tx_entropy as fn() -> u32,
        EmbassyWifiTxTimer,
        ControlTxConfig {
            unicast_attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
            completion_timeout_us: TX_COMPLETION_DEADLINE_MS * 1_000,
            poll_interval_us: 1,
        },
    ));
    let descriptor_base = storage.descriptors().as_ptr().addr() as u32;
    let buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT] = OPEN_RADIO_RX_BUFFER_ADDRESSES
        .init(core::array::from_fn(|index| {
            storage.buffers()[index].dma_address().unwrap()
        }));

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
    // recovered hardware edges. `Esp32s31ScanRx` prepares and publishes the
    // stopped ring here; the scan executor opens it only after each channel
    // switch. The returned type-state owner is later handed directly to
    // Authentication instead of reconstructing descriptor authority.
    let scan_rx = match ScanRx::prepare_initial(mmio, storage, descriptor_base, buffer_addresses) {
        Ok(rx) => rx,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-stage error={error:?}"
            ));
            return false;
        }
    };
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
    let scan_tx = ScanTx::new(
        tx_storage
            .take_control()
            .expect("cold scan owns the initial control TX owner"),
    );
    let mut scan_sequence = StaSequenceCounter::new(1);
    let mut addressed_probe_responses = 0;
    let scan_owner = Esp32s31ScanPort::new(
        Esp32s31ScanRadio::new(
            Esp32s31ScanPhy::<_, _, EmbassyPhyDelay>::new(state, platform, HilPhyObserver),
            cold_mmio,
            scan_rx,
            scan_tx,
        ),
        Esp32s31ScanStorage::new(
            scan_table,
            scan_frame,
            RadioHilColdScanFrameObserver {
                station_address,
                probe_responses: &mut addressed_probe_responses,
            },
            &mut scan_sequence,
        ),
        Esp32s31ScanStation::new(
            station_address,
            network_credentials.ssid(),
            &PROBE_REQUEST_RATES,
        )
        .with_descriptor_capacity(PROBE_TX_DESCRIPTOR_CAPACITY as u32),
        EmbassyEsp32s31ScanTimer,
    );
    let scan_config =
        Esp32s31StaScanConfig::new(SCAN_DWELL_MS).expect("fixed HIL scan dwell policy is nonzero");
    let scan_backend = Esp32s31StaScanBackend::new(scan_config);
    let mut scan_service = StaCandidateScanService::new(scan_backend);
    let (scan_owner, primary_target) = match scan_service.run(scan_owner, &STA_SCAN_CHANNELS).await
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
    let Esp32s31ScanPortParts {
        phy,
        hardware: cold_mmio,
        rx: scan_rx,
        tx: scan_tx,
        observer,
        table: scan_table,
        frame: scan_frame,
        telemetry,
        ..
    } = scan_owner.into_parts();
    let raw_frames = telemetry.raw_frames;
    let ring_epochs = telemetry.ring_epochs;
    let probe_responses = *observer.probe_responses;
    let (state, platform, _observer) = phy.into_parts();
    let (control_tx, tx_summary) = scan_tx.into_parts();
    tx_storage
        .restore_control(control_tx)
        .unwrap_or_else(|_| panic!("cold scan returned over a live TX owner"));
    let tx_completions = tx_summary.completions;
    let tx_failures = tx_summary.failures;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=active-scan-timing channels={} elapsed_ms={}",
        STA_SCAN_CHANNEL_COUNT,
        scan_started.elapsed().as_millis(),
    ));

    let summary = scan_table.summary();
    let rx_dma_pass = summary.records != 0 && raw_frames != 0;
    let active_scan_pass =
        tx_completions >= STA_SCAN_CHANNEL_COUNT as u32 && probe_responses != 0 && tx_failures == 0;
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=OBSERVE stage=production-cold-scan-owner-return \
         records={} raw_frames={} ring_epochs={} probe_completions={} \
         probe_failures={} probe_responses={} rx_pass={} active_pass={}",
        summary.records,
        raw_frames,
        ring_epochs,
        tx_completions,
        tx_failures,
        probe_responses,
        u8::from(rx_dma_pass),
        u8::from(active_scan_pass),
    ));
    let target = primary_target
        .or_else(|| best_matching_ssid(scan_table.records(), network_credentials.ssid()).copied());
    let scan_ring = match scan_rx.into_halted() {
        Ok(ring) => ring,
        Err(rx) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=scan-rx-handoff phase={:?}",
                rx.phase(),
            ));
            return false;
        }
    };
    let descriptor_base = scan_ring.descriptor_base();
    let buffer_addresses = scan_ring.buffer_addresses();
    // No cold MAC operation is permitted beyond this point. Consume the cold
    // owner before authentication and retain the inactive interrupt setup
    // token until WPA2 has opened the controlled port.
    let (running_mmio, interrupt_setup) = cold_mmio.into_running();
    let mmio: &'static mut RadioRegisters = OPEN_RADIO_RUNNING_REGISTERS.init(running_mmio);
    let mut interrupt_epoch = Esp32s31MacInterruptEpoch::new(
        EspHalMacInterruptRoute::new(open_radio_mac_interrupt, open_radio_power_interrupt),
        interrupt_setup,
        &OPEN_RADIO_IRQ_RUNTIME,
        &OPEN_RADIO_POWER_IRQ_RUNTIME,
    );
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
            let fixture = RadioHilConnectedFixture {
                state,
                spawner,
                protocol_spawner,
                platform,
                mmio,
                interrupt_epoch: &mut interrupt_epoch,
                rx_storage: storage,
                tx_storage,
                descriptor_base,
                buffer_addresses,
                scan_table,
                frame: scan_frame,
                ethernet: ethernet_frame,
            };
            let owner = RadioHilStaLifecycleOwner::Authenticate(RadioHilAuthenticationReady {
                fixture,
                target,
                rx: RadioHilJoinRx::from_halted(scan_ring),
                network: initialize_sta_network(station_address),
                security: StaAssociationSecurity {
                    pmk: &pmk,
                    supplicant_nonce,
                    sequences: &mut sequences,
                },
            });
            let policy = StaReconnectPolicy::new(3, 100, 1_000, 100)
                .expect("fixed HIL station reconnect policy is valid");
            let station_control =
                OPEN_RADIO_STATION_CONTROL_RESOURCES.init(Esp32s31StationControlResources::new());
            let (controller, station) = Esp32s31Station::new(
                Esp32s31StationConfig::new(policy).with_initial_candidate(StaNextCandidate::Reuse),
                Esp32s31StationResources::new(owner),
                station_control,
                RadioHilStaLifecycleBackend::new,
            );
            spawner.spawn(
                station_control_task(controller)
                    .unwrap_or_else(|_| panic!("station controller task allocation failed")),
            );
            let progress = match station.run().await {
                Esp32s31StationExit::Stopped {
                    resources,
                    progress,
                    reason,
                } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS \
                             stage=production-sta-lifecycle-stop \
                             connected_epochs={} attempts={} reason={reason:?}",
                        progress.connected_epochs, progress.attempts_started,
                    ));
                    let owner = resources.into_owner();
                    let completed_join = progress.connected_epochs != 0;
                    let _owner = owner;
                    (
                        completed_join,
                        completed_join,
                        completed_join,
                        completed_join,
                    )
                }
                Esp32s31StationExit::RetryExhausted {
                    resources,
                    progress,
                    failure,
                } => {
                    crate::console::publish_station_lifecycle(
                        StationLifecycleEvent::RetryExhausted {
                            generation: progress.connected_epochs,
                            attempts: progress.final_generation_attempt,
                            stage: protocol_station_failure_stage(failure.stage),
                            reason: protocol_station_failure_reason(failure.error),
                        },
                    )
                    .await;
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=OBSERVE \
                             stage=production-sta-lifecycle-exhausted \
                             connected_epochs={} attempts={} failure={failure:?}",
                        progress.connected_epochs, progress.attempts_started,
                    ));
                    let result = match failure.error {
                        RadioHilStaLifecycleFailure::Authentication => (false, false, false, false),
                        RadioHilStaLifecycleFailure::InitialJoin {
                            associated,
                            message1,
                            message3,
                        } => (true, associated, message1, message3),
                        _ => (true, true, true, true),
                    };
                    let owner = resources.into_owner();
                    let _owner = owner;
                    result
                }
                Esp32s31StationExit::Terminal {
                    resources,
                    progress,
                    failure,
                } => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=production-sta-lifecycle-terminal \
                             connected_epochs={} attempts={} failure={failure:?}",
                        progress.connected_epochs, progress.attempts_started,
                    ));
                    let owner = resources.into_owner();
                    let completed_join = progress.connected_epochs != 0;
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
            let _ring = scan_ring;
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
    // `register_chipv7_phy` always finishes `phy_bb_init` on channel 11.
    // Selecting the requested listen channel is a separate post-init call,
    // matching the vendor call graph instead of folding it into cold init.
    let efuse = esp_hal::peripherals::EFUSE::regs();
    let calibration_identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        mac_sys0: efuse.rd_mac_sys0().read().bits(),
        mac_sys1: efuse.rd_mac_sys1().read().bits(),
    };
    let phy_started = Instant::now();
    set_diagnostic_stage(30);
    set_diagnostic_stage(100);
    let cold = match start_esp32s31_station_radio::<_, EmbassyPhyDelay, _>(
        owned,
        Esp32s31ColdStartConfig::new(calibration_identity, LISTEN_CHANNEL)
            .with_maximum_tx_power_quarter_dbm(OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM),
        calibration_record,
        HilPhyObserver,
    )
    .await
    {
        Ok(cold) => cold,
        Err(Esp32s31ColdStartFailure::Power(failure)) => {
            let error = failure.error();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_PRELUDE_HIL result=FAIL stage=power \
                 checkpoint={:?} expected={} observed={}",
                error.checkpoint, error.expected, error.observed
            ));
            halt();
        }
        Err(Esp32s31ColdStartFailure::Registration {
            error,
            port_counters,
            ..
        }) => {
            match error {
                PhyRegisterRunError::Lowering(error) => emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=lowering error={error:?}"
                )),
                PhyRegisterRunError::Port(error) => emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=port error={error:?} \
                     rf_operations={} baseband_operations={}",
                    port_counters.rf_operations, port_counters.baseband_operations,
                )),
                PhyRegisterRunError::Transition(error) => emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=transition error={error:?}"
                )),
                PhyRegisterRunError::Radio(error) => emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=radio error={error:?}"
                )),
            }
            halt();
        }
        Err(Esp32s31ColdStartFailure::MissingPhyOwner { .. }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=take-state"
            ));
            halt();
        }
        Err(Esp32s31ColdStartFailure::InitialChannel { error, .. }) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=post-init-channel error={error:?}"
            ));
            halt();
        }
    };
    set_diagnostic_stage(200);
    let phy_elapsed = phy_started.elapsed();
    let report = cold.report();
    let outcome = report.registration;
    let counters = report.port_counters;
    let (mut powered, mut state, tx_power_profile, calibration_record, _) = cold.into_parts();
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
    set_diagnostic_stage(210);
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
    let legacy_power = core::array::from_fn::<_, 4, _>(|rate| tx_power_profile.pair(rate as u8));
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
    set_diagnostic_stage(250);
    halt()
}

fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
