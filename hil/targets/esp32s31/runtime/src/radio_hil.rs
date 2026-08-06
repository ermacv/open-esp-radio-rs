use core::{
    cell::RefCell,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
};

use crate::console::emergency_log;
use embassy_executor::{SendSpawner, Spawner};
use embassy_futures::select::select;
use embassy_net::{Stack, StackResources, udp::PacketMetadata};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::efuse::{self, InterfaceMacAddress};
use esp_hal::rng::{Rng, Trng};
use open_esp_radio::esp32s31::phy::PhyTxTargetPowerProfile;
use open_esp_radio::esp32s31::wifi::dma::{
    tx_ampdu_storage::AmpduDmaStorage, tx_storage::TxDmaStorage,
};
use open_esp_radio::esp32s31::wifi::sta::channel::Esp32s31ScanPhy;
use open_esp_radio::esp32s31::wifi::sta::cold_start::{
    Esp32s31ColdStartConfig, Esp32s31ColdStartFailure, start_esp32s31_station_radio,
};
use open_esp_radio::esp32s31::wifi::sta::scan::{
    Esp32s31StaScanBackend, Esp32s31StaScanConfig, Esp32s31StaScanError,
};
use open_esp_radio::esp32s31::wifi::sta::tx::ControlTxConfig;
use open_esp_radio::esp32s31::wifi::sta::{
    attempt::{
        Esp32s31StaAttempt, Esp32s31StaAttemptOutcome, Esp32s31StaAttemptSecurity,
        Esp32s31StaAttemptStage, Esp32s31StaAttemptStation,
    },
    tx_epoch::Esp32s31StaTxEpoch,
    wpa2::Esp32s31Wpa2Message4Protection,
};
use open_esp_radio::{
    adapters::{
        esp32s31::wifi_embassy::{
            connected_rx_protocol::{
                ConnectedRxProtocolStopped, Esp32s31ConnectedRxProtocol, Esp32s31StagedRxQueue,
            },
            connected_sta_port::{Esp32s31ConnectedStaConfig, Esp32s31ConnectedStaRateConfig},
            control_mailbox::{ConnectedControlPublisher, ConnectedControlResources},
            control_tx::{ControlTxError, Esp32s31ControlTx},
            cooperative_hardware::CooperativeRadioHardware,
            embassy_irq::{
                EmbassyMacIrqRuntime, EmbassyPowerIrqRuntime, Esp32s31MacInterruptEpoch,
            },
            embassy_rx::RxReloadDelay,
            network_rx::EmbassyNetConnectedRxSink,
            phy_delay::EmbassyEsp32s31PhyDelay as EmbassyPhyDelay,
            preconnected_rx::{EmbassyEsp32s31PreconnectedRxDelay, Esp32s31PreconnectedRx},
            rx_dma_service::{
                ESP32S31_RX_BUFFER_SIZE, Esp32s31RxDmaStorage, Esp32s31RxEpochResources,
                Esp32s31StoppedRx,
            },
            rx_reorder::{
                RX_REORDER_BACKING_SLOT_COUNT, RxReorderCommandResources, RxReorderFrameStorage,
            },
            scan_port::{
                EmbassyEsp32s31ScanTimer, Esp32s31ScanPort, Esp32s31ScanPortError,
                Esp32s31ScanPortParts, Esp32s31ScanRadio, Esp32s31ScanStation, Esp32s31ScanStorage,
            },
            scan_rx::{
                Esp32s31RunningScanRx, Esp32s31ScanFrameObserver, Esp32s31ScanRx,
                Esp32s31ScanRxError,
            },
            scan_target::Esp32s31ColdScanTx,
            scan_tx::Esp32s31RunningScanTx,
            sta_attempt_target::{
                Esp32s31StaAttemptRadio, Esp32s31StaAttemptStorage, Esp32s31StaAttemptTargetOwner,
                Esp32s31StaAttemptTargetPort,
            },
            sta_tx_epoch::Esp32s31StaTxEpochExt,
            station::{
                Esp32s31Station, Esp32s31StationCommandReceiver, Esp32s31StationConfig,
                Esp32s31StationControlResources, Esp32s31StationController, Esp32s31StationExit,
                Esp32s31StationResources,
            },
            station_epoch::{Esp32s31DisconnectedStaEpoch, Esp32s31ReconnectedStaEpoch},
            tx_time::EmbassyWifiTxTimer,
        },
        network::embassy_net::{
            PinnedTxPool as OpenRadioNetworkTxPool, SplitPinnedDevice as OpenRadioNetworkDevice,
            SplitPinnedRadioRunner as OpenRadioNetworkRunner,
            SplitPinnedResources as OpenRadioNetworkResources,
        },
    },
    esp32s31::{
        hal::{ColdRadioRegisters, Radio, RadioRegisters},
        phy::{
            PhyCalibrationIdentity, PhyCalibrationPath, PhyRegisterRunError,
            phy_cold::{PhyCalibrationRecord, PhyColdState},
            phy_rfpll::phy_get_rf_cal_version,
            target_executor::PhyTargetPortError,
        },
        wifi::lmac::{
            crypto::CcmpKeyHardware,
            he::He20PeerHardware,
            init::{
                StaLinkRxPolicyHardware, StaNoiseFloorHardware, initialize_promiscuous_receive,
            },
            irq::{IrqSink, MAC_INT_RX_SUCCESS},
            rate_control::BeamformingReportHardware,
            rx::{HeGuardIntervalAndLtf, RxDma, RxIngressConfig},
            rx_pool::RxStagePool,
            scan::{ScanObservation, ScanTable},
            tx::{
                HeBccDcmMcs, HeDcmRate, HeEdcaTxopLimit, HeLdpcDcmMcs, HeMcs, HtGuardInterval,
                HtMcs, LegacyRate, TxHardware, TxPhyRate, TxSlot,
            },
            tx_ampdu::{HtAmpduTxResources, HtAmpduTxStorage},
        },
    },
    wifi::ieee80211::{
        scan::best_matching_ssid,
        station::{
            STA_PROTECTED_QOS_ETHERNET_HEADROOM, StaAssociationPhy, StaAssociationPreference,
            StaSequenceCounter, StaTxSequenceCounters,
        },
    },
    wifi::sta::scan::{StaCandidateScanExit, StaCandidateScanService},
    wifi::sta::station::{
        StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaLifecycleStage,
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
use open_esp_radio_hil_esp32s31_telemetry::aggregate_tx::AggregateTxCounters;
use open_esp_radio_hil_esp32s31_telemetry::mac_irq::MacIrqClassificationCounters;
use open_esp_radio_hil_esp32s31_telemetry::rx_evidence::{
    RxAmpduCounters, RxPhyCounters, RxSmpduCounters,
};
use open_esp_radio_hil_esp32s31_telemetry::rx_order::RxOrderCounters;
use open_esp_radio_hil_esp32s31_telemetry::rx_pipeline::RxPipelineCounters;
use open_esp_radio_hil_esp32s31_telemetry::task_poll::TaskPollSet;
use open_esp_radio_hil_protocol::{
    Capabilities, FeatureCapabilities, MAX_WIRE_FRAME_BYTES, NetworkCredentials,
    StartupArtifactDisposition, StationLifecycleEvent,
};

mod phy_diagnostics;
use phy_diagnostics::*;
mod connected_traffic;
use connected_traffic::{
    BidirectionalResultChannel, BidirectionalSessionChannel, TcpRxBenchmarkConfig,
    UdpRxBenchmarkConfig, UdpRxSessionSource, UdpRxTelemetry, UdpSocketBuffers,
    UdpTxBenchmarkConfig, UdpTxSessionSource, observe_open_radio_task_polls,
    run_open_radio_bidirectional_session_coordinator, run_open_radio_tcp_rx_benchmark,
    run_open_radio_udp_rx_benchmark, run_open_radio_udp_tx_benchmark,
};
mod station;
use station::{
    HilConnectedRxObserver, RadioHilAuthenticationReady, RadioHilConnectedEpochResources,
    RadioHilConnectedEpochReturn, RadioHilConnectedExit, RadioHilConnectedFixture,
    RadioHilConnectedRxBindings, RadioHilConnectedTaskBindings, RadioHilConnectedTaskFixture,
    RadioHilConnectedTaskGroup, RadioHilNetworkReportBindings, RadioHilReconnectReady,
    RadioHilRunningNetwork, RadioHilRunningScanContext, RadioHilRunningScanFailure,
    RadioHilRunningScanReady, RadioHilStaJoinObserver, RadioHilStaLifecycleBackend,
    RadioHilStaLifecycleFailure, RadioHilStaLifecycleOwner, RadioHilStaNetwork,
    RadioHilStationEpochCoordinator, RadioHilStationEpochProgress,
    RadioHilStationEpochProgressChannel, RadioHilStationEpochReporter, StaAssociationSecurity,
    StaConnectedSession, StaJoinTarget, connected_network_report_task,
    connected_network_stack_task, connected_rx_protocol_task, injected_tx_source_requires_reset,
    protocol_station_failure_reason, protocol_station_failure_stage,
    qualify_disconnected_running_scan, run_connected_network, station_control_task,
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
    "legacy raw/A-MPDU/A-MSDU HIL profiles are not wired to the production ConnectedRunner"
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
static OPEN_RADIO_TX_DMA_STORAGE: StaticCell<TxDmaStorage<TX_BUFFER_SIZE>> = StaticCell::new();
static OPEN_RADIO_TX_SLOT_STORAGE: StaticCell<TxSlot<TX_BUFFER_SIZE>> = StaticCell::new();
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
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_tx_ampdu_descriptors")]
static OPEN_RADIO_TX_AMPDU_DMA_STORAGE: StaticCell<AmpduDmaStorage<TX_AMPDU_FRAME_COUNT, 0>> =
    StaticCell::new();
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
type ConnectedHardware = CooperativeRadioHardware<'static, 'static>;

fn assert_join_hardware_capabilities<
    H: RxDma
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
    Esp32s31ScanPortError<PhyTargetPortError, Esp32s31ScanRxError, ControlTxError>;
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
    HtAmpduTxResources<'static, TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>;
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
static OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS: BidirectionalSessionChannel = Channel::new();
#[unsafe(link_section = ".critical.data.open_radio_bidirectional_session")]
static OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS: BidirectionalSessionChannel = Channel::new();
#[unsafe(link_section = ".critical.data.open_radio_bidirectional_session")]
static OPEN_RADIO_BIDIRECTIONAL_RESULTS: BidirectionalResultChannel = Channel::new();
static OPEN_RADIO_LOCAL_IPV4: AtomicU32 = AtomicU32::new(0);
static OPEN_RADIO_LAN_PROBE_RESPONSE: AtomicBool = AtomicBool::new(false);
// 0/1 preserve the hardware-observed IEEE S-MPDU flag; u32::MAX means that
// the ARP reply has not carried usable physical provenance. S-MPDU is a
// specific VHT/HE single-MPDU A-MPDU form, not an ordinary MPDU synonym.
static OPEN_RADIO_LAN_PROBE_RX_S_MPDU: AtomicU32 = AtomicU32::new(u32::MAX);

// Poll telemetry is HIL-only and deliberately sits in internal SRAM. Reading
// these counters once per completed traffic interval must not add PSRAM
// traffic to the executor hot path being diagnosed.
#[unsafe(link_section = ".critical.bss.open_radio_task_poll_telemetry")]
static OPEN_RADIO_TASK_POLLS: TaskPollSet = TaskPollSet::new();

// Keep per-exchange and IRQ-edge diagnostic atomics off PSRAM so HIL
// observation does not become part of the throughput limit it is measuring.
#[unsafe(link_section = ".critical.bss.open_radio_tx_telemetry")]
static OPEN_RADIO_TX_AGGREGATE_COUNTERS: AggregateTxCounters = AggregateTxCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_RELOAD_DELAYS: AtomicU32 = AtomicU32::new(0);
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_MAC_IRQ_ENTRIES: AtomicU32 = AtomicU32::new(0);
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_MAC_IRQ_CLASSIFICATION: MacIrqClassificationCounters =
    MacIrqClassificationCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_LAST_UDP_FORMAT: AtomicU32 = AtomicU32::new(u32::MAX);
// Packed last-data-PPDU observation, written once per benchmark UDP frame and
// decoded only after the measured interval. Bits 0..=3 are the BB format,
// 4..=8 the public RX rate, 9..=12 HE-SU MCS, 13..=14 GI/LTF, 15..=16 BW,
// 17 DCM, 18 LDPC, and 31 marks a decoded HE-SU signal.
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_LAST_UDP_PHY: AtomicU32 = AtomicU32::new(u32::MAX);

#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_PHY_COUNTERS: RxPhyCounters = RxPhyCounters::new();

#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_S_MPDU_COUNTERS: RxSmpduCounters = RxSmpduCounters::new();
#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS: RxSmpduCounters = RxSmpduCounters::new();

#[unsafe(link_section = ".critical.bss.open_radio_rx_telemetry")]
static OPEN_RADIO_RX_A_MPDU_COUNTERS: RxAmpduCounters = RxAmpduCounters::new();

#[unsafe(link_section = ".critical.bss.open_radio_rx_order_telemetry")]
static OPEN_RADIO_RX_ORDER_COUNTERS: RxOrderCounters = RxOrderCounters::new();
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
static OPEN_RADIO_CONNECTED_TRAFFIC_STOP: Signal<CriticalSectionRawMutex, ()> = Signal::new();
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_CONNECTED_TRAFFIC_STOPPED: Signal<CriticalSectionRawMutex, ()> = Signal::new();
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_CONNECTED_TRAFFIC_START: Channel<
    CriticalSectionRawMutex,
    RadioHilConnectedTrafficConfig,
    1,
> = Channel::new();
#[unsafe(link_section = ".critical.bss.open_radio_station_epoch")]
static OPEN_RADIO_STATION_EPOCH_ACTIVE: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.bss.open_radio_station_epoch")]
static OPEN_RADIO_STATION_EPOCH_PROGRESS: RadioHilStationEpochProgressChannel = Channel::new();

fn station_epoch_reporter() -> RadioHilStationEpochReporter {
    RadioHilStationEpochReporter::new(
        &OPEN_RADIO_STATION_EPOCH_ACTIVE,
        &OPEN_RADIO_STATION_EPOCH_PROGRESS,
    )
}

fn station_epoch_coordinator() -> RadioHilStationEpochCoordinator {
    RadioHilStationEpochCoordinator::new(
        &OPEN_RADIO_STATION_EPOCH_ACTIVE,
        &OPEN_RADIO_STATION_EPOCH_PROGRESS,
    )
}

fn network_report_bindings() -> RadioHilNetworkReportBindings {
    RadioHilNetworkReportBindings::new(
        &OPEN_RADIO_LOCAL_IPV4,
        &OPEN_RADIO_LAN_PROBE_RESPONSE,
        &OPEN_RADIO_LAN_PROBE_RX_S_MPDU,
        LAN_PROBE_IPV4,
    )
}

fn connected_rx_bindings() -> RadioHilConnectedRxBindings {
    RadioHilConnectedRxBindings {
        local_ipv4: &OPEN_RADIO_LOCAL_IPV4,
        lan_probe_response: &OPEN_RADIO_LAN_PROBE_RESPONSE,
        lan_probe_rx_s_mpdu: &OPEN_RADIO_LAN_PROBE_RX_S_MPDU,
        lan_probe_ipv4: LAN_PROBE_IPV4,
        udp_port: OPEN_RADIO_UDP_RX_PORT,
        order_telemetry: OPEN_RADIO_RX_ORDER_TELEMETRY,
        beacon_s_mpdu: &OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS,
        order: &OPEN_RADIO_RX_ORDER_COUNTERS,
        s_mpdu: &OPEN_RADIO_RX_S_MPDU_COUNTERS,
        ampdu: &OPEN_RADIO_RX_A_MPDU_COUNTERS,
        last_format: &OPEN_RADIO_RX_LAST_UDP_FORMAT,
        last_phy: &OPEN_RADIO_RX_LAST_UDP_PHY,
        phy: &OPEN_RADIO_RX_PHY_COUNTERS,
    }
}

fn connected_task_bindings() -> RadioHilConnectedTaskBindings {
    RadioHilConnectedTaskBindings::new(
        &OPEN_RADIO_TASK_POLLS,
        OPEN_RADIO_TASK_POLL_TELEMETRY,
        &OPEN_RADIO_CONNECTED_PROTOCOL_STOP,
        &OPEN_RADIO_CONNECTED_PROTOCOL_STOPPED,
        &OPEN_RADIO_CONNECTED_TRAFFIC_STOP,
        &OPEN_RADIO_CONNECTED_TRAFFIC_STOPPED,
    )
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

// Keep one ordinary-code symbol alive so the host HIL can prove the runtime
// memory profile from periodic UART evidence. In the required
// psram-code-psram-data image its address is in 0x5000_0000..0x5100_0000; a
// directly linked app/Flash-XIP image reports 0x4000_0000..0x5000_0000.
#[inline(never)]
fn open_radio_runtime_code_marker() {}

fn open_radio_udp_tx_benchmark_config(session_source: UdpTxSessionSource) -> UdpTxBenchmarkConfig {
    UdpTxBenchmarkConfig {
        source_port: 4_324,
        queue_depth: OPEN_RADIO_UDP_TX_QUEUE_DEPTH,
        payload_capacity: OPEN_RADIO_UDP_PAYLOAD_CAPACITY,
        default_target: OPEN_RADIO_TX_BENCH_TARGET_IPV4,
        default_port: OPEN_RADIO_UDP_TX_BENCH_PORT,
        default_duration: OPEN_RADIO_UDP_TX_BENCH_DURATION,
        default_offered_rate_bps: OPEN_RADIO_TX_BENCH_RATE_KBPS
            .map(|rate| rate.saturating_mul(1_000)),
        drain: OPEN_RADIO_UDP_TX_DRAIN,
        code_address: open_radio_runtime_code_marker as *const () as usize,
        session_source,
    }
}

fn open_radio_udp_rx_benchmark_config(
    queue_depth: usize,
    session_source: UdpRxSessionSource,
) -> UdpRxBenchmarkConfig {
    UdpRxBenchmarkConfig {
        local_port: OPEN_RADIO_UDP_RX_PORT,
        queue_depth,
        payload_capacity: OPEN_RADIO_UDP_PAYLOAD_CAPACITY,
        idle_timeout: OPEN_RADIO_UDP_RX_IDLE,
        application_handoff_budget: OPEN_RADIO_RX_APPLICATION_HANDOFF_BUDGET,
        task_poll_telemetry: OPEN_RADIO_TASK_POLL_TELEMETRY,
        rx_order_telemetry: OPEN_RADIO_RX_ORDER_TELEMETRY,
        code_address: open_radio_runtime_code_marker as *const () as usize,
        session_source,
    }
}

fn open_radio_udp_rx_telemetry() -> UdpRxTelemetry {
    UdpRxTelemetry {
        last_format: &OPEN_RADIO_RX_LAST_UDP_FORMAT,
        last_phy: &OPEN_RADIO_RX_LAST_UDP_PHY,
        phy: &OPEN_RADIO_RX_PHY_COUNTERS,
        s_mpdu: &OPEN_RADIO_RX_S_MPDU_COUNTERS,
        beacon_s_mpdu: &OPEN_RADIO_RX_BEACON_S_MPDU_COUNTERS,
        ampdu: &OPEN_RADIO_RX_A_MPDU_COUNTERS,
        order: &OPEN_RADIO_RX_ORDER_COUNTERS,
        pipeline: &OPEN_RADIO_RX_PIPELINE_COUNTERS,
        task_polls: &OPEN_RADIO_TASK_POLLS,
        reload_delays: &OPEN_RADIO_RX_RELOAD_DELAYS,
        irq_runtime: &OPEN_RADIO_IRQ_RUNTIME,
        irq_entries: &OPEN_RADIO_MAC_IRQ_ENTRIES,
        irq_classification: &OPEN_RADIO_MAC_IRQ_CLASSIFICATION,
        aggregate_tx: &OPEN_RADIO_TX_AGGREGATE_COUNTERS,
    }
}

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

async fn run_connected_traffic_workload(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &RefCell<&mut RadioRegisters>,
    buffers: &mut RadioHilConnectedTrafficBuffers,
) -> ! {
    match buffers {
        RadioHilConnectedTrafficBuffers::Raw => loop {
            Timer::after_secs(60).await;
        },
        RadioHilConnectedTrafficBuffers::Tcp { rx, tx, read } => {
            run_open_radio_tcp_rx_benchmark(
                stack,
                registers,
                &mut **rx,
                &mut **tx,
                &mut **read,
                TcpRxBenchmarkConfig {
                    local_port: OPEN_RADIO_TCP_RX_PORT,
                    maximum_payload_bytes: OPEN_RADIO_TCP_CHUNK_CAPACITY as u16,
                    receive_buffer_capacity: OPEN_RADIO_TCP_RX_BUFFER_CAPACITY,
                    read_capacity: OPEN_RADIO_TCP_READ_CAPACITY,
                    idle_timeout: OPEN_RADIO_TCP_IDLE_TIMEOUT,
                },
                &OPEN_RADIO_RX_PIPELINE_COUNTERS,
            )
            .await
        }
        RadioHilConnectedTrafficBuffers::UdpRx {
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
                UdpSocketBuffers::new(&mut **rx_metadata, &mut **rx, &mut **tx_metadata, &mut **tx),
                open_radio_udp_rx_benchmark_config(
                    OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH,
                    if OPEN_RADIO_RUNTIME_RX_SESSIONS {
                        UdpRxSessionSource::Console
                    } else {
                        UdpRxSessionSource::Standalone
                    },
                ),
                open_radio_udp_rx_telemetry(),
            )
            .await
        }
        RadioHilConnectedTrafficBuffers::UdpTx {
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
                UdpSocketBuffers::new(&mut **rx_metadata, &mut **rx, &mut **tx_metadata, &mut **tx),
                &mut **packet,
                open_radio_udp_tx_benchmark_config(if OPEN_RADIO_RUNTIME_TX_SESSIONS {
                    UdpTxSessionSource::Console
                } else {
                    UdpTxSessionSource::Standalone
                }),
                &OPEN_RADIO_TX_AGGREGATE_COUNTERS,
            )
            .await
        }
        RadioHilConnectedTrafficBuffers::Bidirectional {
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
            run_open_radio_bidirectional_session_coordinator(
                &OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS,
                &OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS,
                &OPEN_RADIO_BIDIRECTIONAL_RESULTS,
            ),
            select(
                run_open_radio_udp_tx_benchmark(
                    stack,
                    association_phy,
                    data_tx_rate,
                    UdpSocketBuffers::new(
                        &mut **tx_rx_metadata,
                        &mut **tx_rx,
                        &mut **tx_tx_metadata,
                        &mut **tx_tx,
                    ),
                    &mut **packet,
                    open_radio_udp_tx_benchmark_config(UdpTxSessionSource::Bidirectional {
                        sessions: &OPEN_RADIO_BIDIRECTIONAL_TX_SESSIONS,
                        results: &OPEN_RADIO_BIDIRECTIONAL_RESULTS,
                    }),
                    &OPEN_RADIO_TX_AGGREGATE_COUNTERS,
                ),
                run_open_radio_udp_rx_benchmark(
                    stack,
                    association_phy,
                    data_tx_rate,
                    registers,
                    UdpSocketBuffers::new(
                        &mut **rx_rx_metadata,
                        &mut **rx_rx,
                        &mut **rx_tx_metadata,
                        &mut **rx_tx,
                    ),
                    open_radio_udp_rx_benchmark_config(
                        OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH,
                        UdpRxSessionSource::Bidirectional {
                            sessions: &OPEN_RADIO_BIDIRECTIONAL_RX_SESSIONS,
                            results: &OPEN_RADIO_BIDIRECTIONAL_RESULTS,
                        },
                    ),
                    open_radio_udp_rx_telemetry(),
                ),
            ),
        )
        .await {},
    }
}

// These concrete wrappers belong to the HIL composition root. The reusable
// driver crates expose owned runners but do not choose an executor, task
// storage or benchmark policy. Keeping each long-running future in its own
// Embassy task gives it an independent waker and removes the fixed outer poll
// order that previously coupled stack, protocol and PAC progress.
#[derive(Clone, Copy)]
struct RadioHilConnectedTrafficConfig {
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
}

enum RadioHilConnectedTrafficBuffers {
    Raw,
    Tcp {
        rx: &'static mut [u8; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY],
        tx: &'static mut [u8; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY],
        read: &'static mut [u8; OPEN_RADIO_TCP_READ_CAPACITY],
    },
    UdpRx {
        rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        rx: &'static mut [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx: &'static mut [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
    UdpTx {
        rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        rx: &'static mut [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx: &'static mut [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        packet: &'static mut [u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
    Bidirectional {
        tx_rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH],
        tx_rx:
            &'static mut [u8; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        tx_tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH],
        tx_tx:
            &'static mut [u8; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        packet: &'static mut [u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        rx_rx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH],
        rx_rx: &'static mut [u8; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH
                         * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
        rx_tx_metadata: &'static mut [PacketMetadata; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH],
        rx_tx: &'static mut [u8; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH
                         * OPEN_RADIO_UDP_PAYLOAD_CAPACITY],
    },
}

impl RadioHilConnectedTrafficBuffers {
    fn init() -> Self {
        if OPEN_RADIO_TCP_RX_BENCH {
            Self::Tcp {
                rx: OPEN_RADIO_TCP_RX_BUFFER.init_with(|| [0; OPEN_RADIO_TCP_RX_BUFFER_CAPACITY]),
                tx: OPEN_RADIO_TCP_TX_BUFFER.init_with(|| [0; OPEN_RADIO_TCP_TX_BUFFER_CAPACITY]),
                read: OPEN_RADIO_TCP_READ_BUFFER.init_with(|| [0; OPEN_RADIO_TCP_READ_CAPACITY]),
            }
        } else if OPEN_RADIO_RAW_MAC_BENCH {
            Self::Raw
        } else if OPEN_RADIO_BIDIRECTIONAL_BENCH {
            Self::Bidirectional {
                tx_rx_metadata: OPEN_RADIO_UDP_RX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH]),
                tx_rx: OPEN_RADIO_UDP_RX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_SOCKET_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                tx_tx_metadata: OPEN_RADIO_UDP_TX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH]),
                tx_tx: OPEN_RADIO_UDP_TX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_SOCKET_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                packet: OPEN_RADIO_UDP_PACKET.init_with(|| [0x5a; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]),
                rx_rx_metadata: OPEN_RADIO_BIDIRECTIONAL_RX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH]),
                rx_rx: OPEN_RADIO_BIDIRECTIONAL_RX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_BIDIRECTIONAL_RX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
                }),
                rx_tx_metadata: OPEN_RADIO_BIDIRECTIONAL_TX_METADATA
                    .init_with(|| [PacketMetadata::EMPTY; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH]),
                rx_tx: OPEN_RADIO_BIDIRECTIONAL_TX_BUFFER.init_with(|| {
                    [0; OPEN_RADIO_BIDIRECTIONAL_TX_QUEUE_DEPTH * OPEN_RADIO_UDP_PAYLOAD_CAPACITY]
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
async fn connected_traffic_task(
    stack: Stack<'static>,
    registers: &'static RefCell<&'static mut RadioRegisters>,
) {
    let mut buffers = RadioHilConnectedTrafficBuffers::init();
    loop {
        let config = OPEN_RADIO_CONNECTED_TRAFFIC_START.receive().await;
        let _ = select(
            OPEN_RADIO_CONNECTED_TRAFFIC_STOP.wait(),
            observe_open_radio_task_polls(
                run_connected_traffic_workload(
                    stack,
                    config.association_phy,
                    config.data_tx_rate,
                    registers,
                    &mut buffers,
                ),
                OPEN_RADIO_TASK_POLLS.benchmark(),
                OPEN_RADIO_TASK_POLL_TELEMETRY,
            ),
        )
        .await;
        OPEN_RADIO_CONNECTED_TRAFFIC_STOPPED.signal(());
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
        RadioHilConnectedExit::Disconnected { .. } | RadioHilConnectedExit::ReconnectRequested => {
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
        RadioHilConnectedExit::InjectedTxFault { .. } | RadioHilConnectedExit::HardwareFailure => {
            StaAttemptOutcome::Failed {
                owner: RadioHilStaLifecycleOwner::RunningScan(owner),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    RadioHilStaLifecycleFailure::ConnectedHardware,
                ),
            }
        }
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
        station_epoch_reporter(),
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
            station_epoch_reporter().report(RadioHilStationEpochProgress::JoinCompleted);
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
/// `StaticCell::init_with` constructs the resources directly in their final
/// allocation and avoids a temporary of more than 100 KiB. A
/// reconnect never calls this function: it receives `RadioHilStaNetwork::Running`
/// from the completed connected epoch.
fn initialize_sta_network(station_address: [u8; 6]) -> RadioHilStaNetwork {
    let resources = OPEN_RADIO_NETWORK_RESOURCES.init_with(NetworkResources::new);
    let tx_pool =
        NetworkTxPool::pin_static(OPEN_RADIO_NETWORK_TX_POOL.init_with(NetworkTxPool::new));
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
    let storage = OPEN_RADIO_RX_DMA_STORAGE.init_with(RxStorage::new);
    let tx_dma = TxDmaStorage::pin_static(OPEN_RADIO_TX_DMA_STORAGE.init_with(TxDmaStorage::new))
        .expect("TX DMA storage must be addressable by ESP32-S31");
    let tx_slot = Pin::static_mut(OPEN_RADIO_TX_SLOT_STORAGE.init(TxSlot::from_dma(tx_dma)));
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
    let buffer_addresses = OPEN_RADIO_RX_BUFFER_ADDRESSES.init([0; RX_DESCRIPTOR_COUNT]);
    let descriptor_base = storage
        .dma_layout(buffer_addresses)
        .expect("RX DMA storage must be addressable by ESP32-S31");
    let buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT] = buffer_addresses;

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
         cold_int_mask={cold_interrupt_mask:#010x}",
        buffer_addresses[0], cold.handshake_samples, cold.handshake_value,
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
                connected_tasks: connected_task_bindings(),
                connected_rx: connected_rx_bindings(),
                network_report: network_report_bindings(),
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
                station_control_task(controller, station_epoch_coordinator())
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

    set_diagnostic_stage(20);
    let owned = match Radio::claim(platform) {
        Ok(owned) => owned,
        Err(_) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=radio-already-claimed"
            ));
            halt();
        }
    };
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
