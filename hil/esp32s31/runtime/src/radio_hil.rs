use core::{
    cell::RefCell,
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
    task::{Context, Waker},
};

use crate::console::emergency_log;
use embassy_executor::Spawner;
use embassy_futures::{select::select, yield_now};
use embassy_net::{
    Config as NetworkConfig, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_net_driver::{Driver, LinkState, RxToken as _, TxToken as _};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
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
            PhyRegisterRunError, PhyRegisterTransition, PhyRfBoundary, PhyTargetObserver,
            TargetPhyRegisterPort,
            phy_cold::PhyColdState,
            run_phy_register, select_phy_channel, switch_phy_channel_with_mac_restart,
            target_executor::{PhyAsyncDelay, PhyTargetPortError},
        },
        wifi::mac::{
            connected_rx::{
                ConnectedRxConfig, ConnectedRxDispatcher, ConnectedRxEvent, ConnectedRxSink,
            },
            crypto::{
                CryptoKeyError, StaGroupCcmpSlot, StaPairwiseCcmpSlot, install_sta_group_ccmp,
                install_sta_pairwise_ccmp,
            },
            descriptor::{DESCRIPTOR_BYTES, length as descriptor_length, rx_done},
            edca::EdcaParametersError,
            he::program_he20_peer_state,
            init::{
                MAC_COLD_RX_INTERRUPT_MASK, StaPeerScanPolicy, StaWmmSource,
                configure_sta_link_receive_policy, initialize_promiscuous_receive,
            },
            irq::{
                IrqSink, MAC_INT_COLLISION, MAC_INT_RX_ASSOCIATED_AUXILIARY_MASK,
                MAC_INT_RX_SUCCESS, MAC_INT_TX_COMPLETE, MAC_INT_TX_TIMEOUT, handle_mac_irq,
                handle_power_irq,
            },
            rate_control::{StaRateControlAssociation, StaTxRatePolicy},
            rate_schedule::schedule_state,
            registers::{
                MAC_INT_RAW, MAC_INT_STATUS, Mmio, RX_CONTROL, RX_DESCRIPTOR_BASE,
                RX_LAST_DESCRIPTOR, RX_LAST_DESCRIPTOR_HIGH, RX_NEXT_DESCRIPTOR, TX_Q_CONTROL,
                TX_Q_ENABLE_VALID,
            },
            rx::{
                HeGuardIntervalAndLtf, PUBLIC_HEADER_SIZE, RxError, RxIngressConfig, RxRingError,
                RxRingLive, RxRingStopped, RxSegment, build_cold_ring, decode_rx_phy_info,
                disable_receive, enable_receive, extract_ccmp_data, extract_data,
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
            runner::WifiRunner,
            rx_backend::{
                ConnectedControlPublisher, ConnectedControlResources, ESP32S31_RX_BUFFER_SIZE,
                EmbassyNetConnectedRxSink, Esp32s31ConnectedRx, Esp32s31RxDmaStorage,
                RxEnqueueCounters,
            },
            rx_telemetry::{RxPipelineCounterSnapshot, RxPipelineCounters},
            single_mpdu_tx::{EmbassyWifiTxTimer, SingleMpduTxConfig},
            sta_join::{
                EmbassyStaJoinTimer, StaJoinBackend, StaJoinRunner, StaJoinRxDirective,
                StaJoinRxObserver,
            },
            staged_rx::{Esp32s31ConnectedRxProtocol, Esp32s31StagedRxQueue},
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
    wifi::wpa2::{
        OwnedEapolFrame, Pmk, Wpa2Interface, aes::Wpa2SoftwareAes, frames::Wpa2TxFrame,
        keys::Wpa2KeyKind,
    },
};
use open_esp_radio_esp32s31_wifi_esp_hal::EspHalRadioPeripheral;

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
// HIL 2026-07-30: increasing this ring to the vendor throughput profile's 48
// buffers did not improve simultaneous RX/TX throughput or eliminate the
// hardware BUFFER_FULL count. Keep the smaller ring until that counter's
// precise contract is recovered instead of permanently spending 74 KiB SRAM.
const RX_DESCRIPTOR_COUNT: usize = 32;
const RX_DESCRIPTOR_COMPLETE_MASK: u64 = (1_u64 << RX_DESCRIPTOR_COUNT) - 1;
// HT Capabilities Info keeps Max A-MSDU Length clear, so the negotiated
// maximum MPDU is the 3,839-byte A-MSDU class plus MAC/CCMP/metadata overhead.
// 1,700 bytes was sufficient for one ordinary Ethernet MSDU, but HT40 HIL
// proved that the AP can use a split RX unit under load. Keeping the complete
// negotiated MPDU in one descriptor avoids a descriptor-frontier stall while
// the upper Rust path decapsulates its bounded A-MSDU subframes.
const RX_BUFFER_SIZE: usize = 4_608;
// Staging owns the complete negotiated MPDU after DMA recycle. Keeping the
// older 1,700-byte vendor singleton capacity here silently discarded valid
// A-MSDU units even though both the DMA owner and connected dispatcher already
// support the negotiated 3,839-byte class.
const RX_STAGE_CAPACITY: usize = RX_BUFFER_SIZE;
// The vendor-equivalent default remains 32. Sustained HE20 HIL, however,
// observed both pool and queue credits at zero while the 32-descriptor DMA
// ring was complete. Retain half of one additional aggregate here so protocol
// publication can overlap the next hardware burst without enlarging the DMA
// ring or imposing a per-poll frame budget. Forty-eight slots crossed the
// linker's protected 64-KiB CPU0-stack frontier. Both 47 (about 66 KiB left)
// and 44 (about 80 KiB left) passed linking but failed the on-device readiness
// path. Forty slots retain roughly 98 KiB and are the largest runtime-stable
// geometry qualified so far. The TX-only image does not carry downlink load;
// retain the vendor 32-slot RX ownership there so its 64-entry TX DMA pool
// remains placeable. Bidirectional and RX-shaped images retain 40.
const RX_STAGE_SLOT_COUNT: usize =
    if OPEN_RADIO_THROUGHPUT_BENCH && !OPEN_RADIO_BIDIRECTIONAL_BENCH {
        32
    } else {
        40
    };
const NETWORK_FRAME_CAPACITY: usize = 1_600;
const CONNECTED_CONTROL_QUEUE_DEPTH: usize = 32;
// Raw A-MSDU/A-MPDU HIL generates TX below the network stack, and its direct
// UDP RX meter consumes the benchmark stream before the Embassy handoff.
// Deep Ethernet queues therefore only waste memory in those diagnostic
// images. Production-shaped RX retains one complete 32-frame hardware burst
// plus eight overlap owners in ordinary/PSRAM storage, matching the qualified
// 40-slot staging profile. A 64-entry experiment removed network-ready waits
// but increased PSRAM/cache cost and did not improve the 80-Mbit/s boundary.
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
const OPEN_RADIO_UDP_RX_IDLE: Duration = Duration::from_millis(750);
const OPEN_RADIO_RX_APPLICATION_HANDOFF_BUDGET: Duration = Duration::from_micros(500);
const OPEN_RADIO_THROUGHPUT_BENCH: bool = option_env!("OPEN_RADIO_TX_BENCH").is_some();
const OPEN_RADIO_BIDIRECTIONAL_BENCH: bool =
    option_env!("OPEN_RADIO_BIDIRECTIONAL_BENCH").is_some();
const OPEN_RADIO_TASK_POLL_TELEMETRY: bool = cfg!(feature = "task-poll-telemetry");
const OPEN_RADIO_STACK_SOCKET_COUNT: usize = if OPEN_RADIO_BIDIRECTIONAL_BENCH { 5 } else { 4 };
// Every HE matrix owns a synthetic A-MPDU traffic source. Requiring a second
// independent build flag previously allowed the matrix selector and its log
// labels to be active while no matrix traffic was generated.
const OPEN_RADIO_RAW_MAC_BENCH: bool = option_env!("OPEN_RADIO_RAW_MAC_BENCH").is_some()
    || option_env!("OPEN_RADIO_HE_MATRIX_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_LDPC_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_DCM_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_TB_HIL").is_some()
    || option_env!("OPEN_RADIO_HE_DELIMITER_HIL").is_some();
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
const SCAN_DWELL_MS: u32 = 200;
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
const STA_TARGET_SSID: &[u8] = if let Some(ssid) = option_env!("OPEN_RADIO_STA_SSID") {
    ssid.as_bytes()
} else if OPEN_RADIO_HE_MATRIX_HIL
    || OPEN_RADIO_HE_DCM_HIL
    || OPEN_RADIO_HE_TB_HIL
    || OPEN_RADIO_HE_DELIMITER_HIL
{
    b"codex_he20_wpa"
} else if PERF_AP_PROFILE {
    b"codex_ht_wpa"
} else {
    b"FRITZ!Box 7530 FN"
};
const WPA2_PROTECTED_ARP_TIMEOUT_MS: u32 = 1_500;
const WPA2_CONTROLLED_PORT_SETTLE_MS: u64 = 10;
const WPA2_PROTECTED_ARP_ATTEMPTS: u8 = 3;
const WPA2_PROTECTED_ARP_RETRY_DELAY_MS: u64 = 20;
// Migration installs both keys before queueing M4, but keeps STA EAPOL on its
// measured plaintext layout until the M4 TX-done edge opens the controlled
// port. Protected M4 remains a useful explicit negative control experiment.
const WPA2_MESSAGE_4_HARDWARE_PROTECTED: bool = false;
const STA_PASSPHRASE: Option<&str> = option_env!("OPEN_RADIO_STA_PASSWORD");
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

type RxStorage = Esp32s31RxDmaStorage<RX_DESCRIPTOR_COUNT>;
const _: () = assert!(RX_BUFFER_SIZE == ESP32S31_RX_BUFFER_SIZE);

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
static SCAN_FRAME: StaticCell<[u8; RX_BUFFER_SIZE]> = StaticCell::new();
static ETHERNET_FRAME: StaticCell<[u8; RX_BUFFER_SIZE]> = StaticCell::new();
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
type ConnectedRxOwner = Esp32s31ConnectedRx<
    'static,
    'static,
    'static,
    OpenRadioRxReloadDelay,
    CriticalSectionRawMutex,
    RX_STAGE_SLOT_COUNT,
    RX_DESCRIPTOR_COUNT,
    RX_STAGE_CAPACITY,
    RX_STAGE_SLOT_COUNT,
>;
type ConnectedTxOwner = Esp32s31ConnectedTx<
    'static,
    'static,
    'static,
    CriticalSectionRawMutex,
    PhyTxTargetPowerProfile,
    fn() -> u32,
    EmbassyWifiTxTimer,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_TX_QUEUE_DEPTH,
    TX_AMPDU_FRAME_COUNT,
    TX_AMPDU_BUFFER_SIZE,
    TX_BUFFER_SIZE,
>;
type ConnectedControlOwner = Esp32s31ConnectedControl<
    'static,
    CriticalSectionRawMutex,
    CONNECTED_CONTROL_QUEUE_DEPTH,
>;
type ConnectedHardware = CooperativeTxHardware<'static, 'static>;
type ConnectedBackend = Esp32s31WifiBackend<
    ConnectedHardware,
    ConnectedRxOwner,
    ConnectedTxOwner,
    ConnectedControlOwner,
>;
type ConnectedWifiRunner = WifiRunner<
    'static,
    'static,
    CriticalSectionRawMutex,
    ConnectedBackend,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_RX_QUEUE_DEPTH,
    NETWORK_TX_QUEUE_DEPTH,
>;
type ConnectedNetworkStackRunner = embassy_net::Runner<'static, NetworkDevice>;

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

struct StaConnectedSession<'a> {
    link: StaConnectedLink,
    network_device: NetworkDevice,
    network_runner: NetworkRunner,
    rate_control: &'a mut StaRateControlAssociation,
    sequences: &'a mut StaTxSequenceCounters,
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

/// Board-owned resources required by the connected HIL path.
///
/// This is deliberately a HIL fixture, not a production service locator: all
/// protocol/link state lives in `StaConnectedSession`, and every field here
/// is a concrete hardware or scratch-buffer capability consumed by the same
/// WPA2-to-connected ownership transition.
struct RadioHilConnectedFixture<'a> {
    spawner: Spawner,
    platform: &'a mut EspHalRadioPeripheral,
    mmio: &'static mut RadioRegisters,
    interrupt_setup: &'a mut Option<MacInterruptSetup>,
    rx_storage: &'static RxStorage,
    tx_storage: &'static mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'static [u32; RX_DESCRIPTOR_COUNT],
    frame: &'static mut [u8; RX_BUFFER_SIZE],
    ethernet: &'static mut [u8; RX_BUFFER_SIZE],
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
static OPEN_RADIO_LOCAL_IPV4: AtomicU32 = AtomicU32::new(0);
static OPEN_RADIO_LAN_PROBE_RESPONSE: AtomicBool = AtomicBool::new(false);

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
static OPEN_RADIO_MAC_INTERRUPT_REGISTERS: StaticCell<MacInterruptRegisters> = StaticCell::new();
static OPEN_RADIO_MAC_INTERRUPT_PTR: AtomicPtr<MacInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());
static OPEN_RADIO_POWER_INTERRUPT_REGISTERS: StaticCell<MacPowerInterruptRegisters> =
    StaticCell::new();
static OPEN_RADIO_POWER_INTERRUPT_PTR: AtomicPtr<MacPowerInterruptRegisters> =
    AtomicPtr::new(core::ptr::null_mut());

async fn start_live_rx_ring<'a>(
    mmio: &mut RadioRegisters,
    rx_storage: &'a RxStorage,
    descriptor_base: u32,
    buffer_addresses: &'a [u32; RX_DESCRIPTOR_COUNT],
) -> Result<RxRingLive<'a, RX_DESCRIPTOR_COUNT>, RxRingError> {
    // A consumed zero-terminated cold list cannot be safely restarted from
    // descriptor zero: LAST_DESCRIPTOR still retains the old frontier. The
    // live owner rotates the new head past that retained tail and subsequently
    // appends completed halves with the recovered reload doorbell.
    //
    // SOURCE: promoted migration
    // `migration/esp32s31-hybrid-runtime/src/wdev.rs::
    // prepare_rx_recycle_chain` and `wDev_AppendRxBlocks`; qualified by the
    // connected-path HIL `HIL_OPEN_RX_LIVE_APPEND_2026_07_27`.
    let stopped = RxRingStopped::prepare(
        mmio,
        rx_storage.descriptors(),
        descriptor_base,
        buffer_addresses,
        RX_BUFFER_SIZE as u32,
        |index| {
            // SAFETY: RxRingStopped has confirmed that hardware released the
            // walker before it transfers each DMA buffer back for preparation.
            unsafe { rx_storage.buffers()[index].prepare_for_recycle() }
        },
    )?;
    Timer::after_micros(5).await;
    stopped.start(mmio)
}

/// HIL fixture which supplies the production STA join runner with the current
/// S31 PAC/DMA owners. Protocol retry/deadline state lives in `StaJoinRunner`;
/// this adapter performs only finite hardware operations and frame extraction.
struct RadioHilStaJoinBackend<'hardware, 'storage, 'scratch> {
    mmio: &'hardware mut RadioRegisters,
    rx_storage: &'storage RxStorage,
    tx_storage: &'hardware mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'storage [u32; RX_DESCRIPTOR_COUNT],
    frame: &'scratch mut [u8; RX_BUFFER_SIZE],
    station_address: [u8; 6],
    access_point: ScanRecord,
    ring: Option<RxRingLive<'storage, RX_DESCRIPTOR_COUNT>>,
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

impl<'hardware, 'storage, 'scratch> RadioHilStaJoinBackend<'hardware, 'storage, 'scratch> {
    fn take_live_ring(
        &mut self,
    ) -> Result<RxRingLive<'storage, RX_DESCRIPTOR_COUNT>, RadioHilStaJoinError> {
        self.ring
            .take()
            .ok_or(RadioHilStaJoinError::ReceiveNotStarted)
    }
}

impl StaJoinBackend for RadioHilStaJoinBackend<'_, '_, '_> {
    type Error = RadioHilStaJoinError;

    fn start_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            if self.ring.is_some() {
                return Err(RadioHilStaJoinError::ReceiveAlreadyStarted);
            }
            self.ring = Some(
                start_live_rx_ring(
                    self.mmio,
                    self.rx_storage,
                    self.descriptor_base,
                    self.buffer_addresses,
                )
                .await?,
            );
            Ok(())
        }
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            self.ring
                .take()
                .ok_or(RadioHilStaJoinError::ReceiveNotStarted)?;
            disable_receive(self.mmio)?;
            Ok(())
        }
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
            let ring = self
                .ring
                .as_mut()
                .ok_or(RadioHilStaJoinError::ReceiveNotStarted)?;
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
struct RadioHilWpa2Backend<'hardware, 'storage, 'scratch> {
    mmio: &'hardware mut RadioRegisters,
    rx_storage: &'storage RxStorage,
    tx_storage: &'hardware mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &'storage [u32; RX_DESCRIPTOR_COUNT],
    frame: &'scratch mut [u8; RX_BUFFER_SIZE],
    station_address: [u8; 6],
    bssid: [u8; 6],
    ring: Option<RxRingLive<'storage, RX_DESCRIPTOR_COUNT>>,
    message2_transmissions: u16,
}

impl Wpa2HandshakeBackend for RadioHilWpa2Backend<'_, '_, '_> {
    type Error = RadioHilStaJoinError;

    fn service_receive(
        &mut self,
    ) -> impl Future<Output = Result<Wpa2RxProgress, Self::Error>> + '_ {
        async move {
            let ring = self
                .ring
                .as_mut()
                .ok_or(RadioHilStaJoinError::ReceiveNotStarted)?;
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
            self.ring
                .take()
                .ok_or(RadioHilStaJoinError::ReceiveNotStarted)?;
            disable_receive(self.mmio)?;
            self.ring = Some(
                start_live_rx_ring(
                    self.mmio,
                    self.rx_storage,
                    self.descriptor_base,
                    self.buffer_addresses,
                )
                .await?,
            );
            Ok(())
        }
    }

    fn stop_receive(&mut self) -> impl Future<Output = Result<(), Self::Error>> + '_ {
        async move {
            self.ring
                .take()
                .ok_or(RadioHilStaJoinError::ReceiveNotStarted)?;
            disable_receive(self.mmio)?;
            Ok(())
        }
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

struct RadioHilWpa2KeyBackend<'hardware, 'sequence> {
    mmio: &'hardware mut RadioRegisters,
    tx_storage: &'hardware mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    peer_qos: bool,
    sequences: &'sequence mut StaTxSequenceCounters,
    completion: Option<TxCompletion>,
}

impl Wpa2KeyInstallBackend for RadioHilWpa2KeyBackend<'_, '_> {
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
    // SAFETY: `run` publishes the one-shot split capability before binding
    // this handler. The S31 masks the active interrupt while its handler runs,
    // and no task-side code receives this finite capability, so calls cannot
    // overlap and this is its sole mutable reference.
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
    scan_frame: &mut [u8; RX_BUFFER_SIZE],
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

async fn await_protected_arp_response(
    mmio: &mut RadioRegisters,
    rx_storage: &RxStorage,
    frame: &mut [u8; RX_BUFFER_SIZE],
    ethernet: &mut [u8; RX_BUFFER_SIZE],
    network_device: &mut NetworkDevice,
    network_runner: &NetworkRunner,
    station_address: [u8; 6],
    bssid: [u8; 6],
    mut rx_ring: RxRingLive<'_, RX_DESCRIPTOR_COUNT>,
) -> bool {
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
            let _ = disable_receive(mmio);
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
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-protected-rx-recycle \
                 error={error:?}"
            ));
            return false;
        }
        if rx_ring.all_observed() {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-protected-rx-recycle \
                 error=terminal-before-recycle"
            ));
            return false;
        }
        Timer::after_millis(1).await;
    }
    let _ = disable_receive(mmio);
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
}

impl<S: ConnectedRxSink> ConnectedRxSink for HilConnectedRxObserver<S> {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        if let ConnectedRxEvent::Ethernet { frame, raw, .. } = event {
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
    if OPEN_RADIO_RAW_MAC_BENCH {
        loop {
            Timer::after_secs(60).await;
        }
    } else if OPEN_RADIO_BIDIRECTIONAL_BENCH {
        match select(
            run_open_radio_udp_tx_benchmark(stack, association_phy, data_tx_rate),
            run_open_radio_bidirectional_rx_benchmark(
                stack,
                association_phy,
                data_tx_rate,
                registers,
            ),
        )
        .await {}
    } else if option_env!("OPEN_RADIO_TX_BENCH").is_some() {
        run_open_radio_udp_tx_benchmark(stack, association_phy, data_tx_rate).await
    } else {
        run_open_radio_udp_rx_benchmark(stack, association_phy, data_tx_rate, registers).await
    }
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
    Timer::after_secs(1).await;

    loop {
        let started = Instant::now();
        let aggregate_start = (!OPEN_RADIO_BIDIRECTIONAL_BENCH)
            .then(|| OPEN_RADIO_TX_AGGREGATE_COUNTERS.snapshot());
        let mut next_send = started;
        let mut bytes = 0_u64;
        let mut datagrams = 0_u32;
        let mut send_errors = 0_u32;
        while started.elapsed() < OPEN_RADIO_UDP_TX_BENCH_DURATION {
            packet[..4].copy_from_slice(&(datagrams as i32).to_be_bytes());
            match socket
                .send_to(packet, (server, OPEN_RADIO_UDP_TX_BENCH_PORT))
                .await
            {
                Ok(()) => {
                    bytes = bytes.saturating_add(packet.len() as u64);
                    datagrams = datagrams.saturating_add(1);
                }
                Err(_) => send_errors = send_errors.saturating_add(1),
            }
            if let Some(rate_kbps) = OPEN_RADIO_TX_BENCH_RATE_KBPS {
                // `kbit/s` and microseconds cancel directly after multiplying
                // the payload bytes by eight thousand. Pace absolute
                // deadlines so a temporarily blocking network queue does not
                // produce a compensating burst after it becomes writable.
                let interval_us = (packet.len() as u64)
                    .saturating_mul(8_000)
                    .saturating_add(rate_kbps - 1)
                    / rate_kbps;
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
        // Publish one compact bounded record synchronously per five-second
        // interval. The asynchronous logger repeatedly truncated this record
        // after `OTX b=` under sustained TX, whereas the same emergency path
        // already carries the compact ORX/ORXP records without starving RX.
        // This is HIL evidence, not a per-packet data-path log.
        emergency_log(format_args!(
            "OTX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} \
             e={send_errors} p={} w={} r={} g={} x={} l={} a={}",
            OPEN_RADIO_TX_BENCH_RATE_KBPS.unwrap_or(0),
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
        Timer::after_secs(2).await;
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
        // Close the previous benchmark poll before taking interval baselines.
        // In particular, synchronous UART evidence from a readiness probe
        // must not be charged to the following sustained traffic interval.
        yield_now().await;
        OPEN_RADIO_RX_LAST_UDP_FORMAT.store(u32::MAX, Ordering::Relaxed);
        OPEN_RADIO_RX_LAST_UDP_PHY.store(u32::MAX, Ordering::Relaxed);
        let hardware_start = registers.borrow().rx_statistics_snapshot().primary;
        let phy_start = OPEN_RADIO_RX_PHY_COUNTERS.snapshot();
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
        let receive_errors = 0_u32;
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
        "ORXD frames={} data={} waits={} wait_us={} wait_boot_max_us={} dispatch_us={} \
         dispatch_boot_max_us={} publications={} bytes={} publish_us={} publish_boot_max_us={}",
        pipeline.protocol_frames,
        pipeline.protocol_data_frames,
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

#[embassy_executor::task]
async fn connected_rx_protocol_task(mut protocol: ConnectedRxProtocol) {
    observe_open_radio_task_polls(protocol.run(), &OPEN_RADIO_TASK_POLLS.protocol).await
}

#[embassy_executor::task]
async fn connected_radio_task(mut runner: ConnectedWifiRunner) {
    match observe_open_radio_task_polls(runner.run(), &OPEN_RADIO_TASK_POLLS.radio).await {
        Ok(()) => emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner error=stopped"
        )),
        Err(error) => emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner error={error:?}"
        )),
    }
    loop {
        Timer::after_secs(60).await;
    }
}

#[embassy_executor::task]
async fn connected_network_report_task(stack: Stack<'static>) {
    report_network_configuration(stack).await
}

#[embassy_executor::task]
async fn connected_benchmark_task(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
    registers: &'static RefCell<&'static mut RadioRegisters>,
) {
    observe_open_radio_task_polls(
        run_open_radio_udp_benchmark(stack, association_phy, data_tx_rate, registers),
        &OPEN_RADIO_TASK_POLLS.benchmark,
    )
    .await
}

async fn run_connected_network(
    fixture: RadioHilConnectedFixture<'_>,
    session: StaConnectedSession<'_>,
    pairwise_slot: StaPairwiseCcmpSlot,
    _group_slot: StaGroupCcmpSlot,
) -> ! {
    let RadioHilConnectedFixture {
        spawner,
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
        network_device,
        network_runner,
        rate_control,
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
    let (interrupt_registers, power_interrupt_registers) = interrupt_setup
        .take()
        .expect("MAC interrupt setup activates only once")
        .activate(MAC_COLD_RX_INTERRUPT_MASK);
    let interrupt_registers =
        OPEN_RADIO_MAC_INTERRUPT_REGISTERS.init(interrupt_registers) as *mut MacInterruptRegisters;
    let power_interrupt_registers = OPEN_RADIO_POWER_INTERRUPT_REGISTERS
        .init(power_interrupt_registers)
        as *mut MacPowerInterruptRegisters;
    OPEN_RADIO_MAC_INTERRUPT_PTR.store(interrupt_registers, Ordering::Release);
    OPEN_RADIO_POWER_INTERRUPT_PTR.store(power_interrupt_registers, Ordering::Release);
    platform.bind_interrupts(open_radio_mac_interrupt, open_radio_power_interrupt);

    network_runner.set_link_state(LinkState::Up);
    let stack_resources = OPEN_RADIO_STACK_RESOURCES.init(StackResources::new());
    let mut seed = [0_u8; 8];
    seed[..6].copy_from_slice(&station_address);
    seed[6..].copy_from_slice(&0x31a5_u16.to_le_bytes());
    // Keep the controlled local throughput setup independent of DHCP while
    // preserving DHCP as an end-to-end test against the ordinary router.
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
        network_device,
        network_config,
        stack_resources,
        u64::from_le_bytes(seed),
    );
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
    // The production aggregate owner is descriptor-only (`BUFFER_SIZE == 0`),
    // so constructing it in the static cell does not materialize the former
    // 55-KiB payload arena on this task's stack. Prefer the fully typed
    // constructor here: all scalar policy fields, including the conservative
    // pre-association byte ceiling, are initialized as Rust values before the
    // object is pinned.
    let tx_ampdu_storage = HtAmpduTxStorage::pin_static(
        OPEN_RADIO_TX_AMPDU_STORAGE.init_with(HtAmpduTxStorage::new),
    );
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-start \
         frame_capacity={NETWORK_FRAME_CAPACITY} \
         rx_queue_depth={NETWORK_RX_QUEUE_DEPTH} tx_queue_depth={NETWORK_TX_QUEUE_DEPTH} \
         rx_stage_slots={RX_STAGE_SLOT_COUNT} rx_stage_capacity={RX_STAGE_CAPACITY} \
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

    let rx_ring =
        match start_live_rx_ring(mmio, rx_storage, descriptor_base, buffer_addresses).await {
            Ok(ring) => ring,
            Err(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=production-runner-rx-arm error={error:?}"
                ));
                loop {
                    Timer::after_secs(60).await;
                }
            }
        };
    let network_rx = network_runner.rx_publisher();
    let (control_publisher, control_receiver) = OPEN_RADIO_CONTROL_RESOURCES
        .init(ControlResources::new())
        .split();
    let rx_sink = EmbassyNetConnectedRxSink::new(
        network_rx,
        HilConnectedRxObserver {
            control: control_publisher,
            station_address,
            phy_sample_cursor: 0,
        },
    )
    .with_counters(&OPEN_RADIO_RX_ENQUEUE_COUNTERS)
    .with_pipeline_counters(&OPEN_RADIO_RX_PIPELINE_COUNTERS);
    let (staged_rx_sender, staged_rx_receiver) = OPEN_RADIO_STAGED_RX_QUEUE.split();
    let rx = Esp32s31ConnectedRx::new(
        rx_ring,
        rx_storage.buffers(),
        &OPEN_RADIO_RX_STAGE_POOL,
        OpenRadioRxReloadDelay,
        staged_rx_sender,
    )
    .with_pipeline_counters(&OPEN_RADIO_RX_PIPELINE_COUNTERS);
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
    );
    control.enable_beacon_loss(
        StaBeaconLossConfig::new(beacon_interval_tu, CONNECTED_BEACON_MISS_LIMIT)
            .expect("scan admitted a nonzero connected beacon interval"),
    );
    if peer_qos && matches!(benchmark_tx_rate, TxPhyRate::Ht(_) | TxPhyRate::He(_)) {
        control.queue_initial_tx_block_ack();
    }

    let registers: &'static RefCell<&'static mut RadioRegisters> =
        OPEN_RADIO_REGISTER_CELL.init(RefCell::new(mmio));
    let hardware = CooperativeTxHardware::new(registers);
    let backend = Esp32s31WifiBackend::with_control(hardware, rx, tx, control);
    let radio_runner = WifiRunner::new(&OPEN_RADIO_IRQ_RUNTIME, network_runner, backend);

    let stack_task = connected_network_stack_task(stack_runner)
        .unwrap_or_else(|_| panic!("connected network task allocation failed"));
    spawner.spawn(stack_task);
    let protocol_task = connected_rx_protocol_task(rx_protocol)
        .unwrap_or_else(|_| panic!("connected RX protocol task allocation failed"));
    spawner.spawn(protocol_task);
    let radio_task = connected_radio_task(radio_runner)
        .unwrap_or_else(|_| panic!("connected radio task allocation failed"));
    spawner.spawn(radio_task);
    let report_task = connected_network_report_task(stack)
        .unwrap_or_else(|_| panic!("connected network report task allocation failed"));
    spawner.spawn(report_task);
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
         network=independent rx_protocol=independent radio=independent \
         report=independent benchmark=independent"
    ));

    // Retain the platform token and one-shot interrupt-setup lifetime in the
    // parent task. Connected owners themselves live in their executor task
    // storage and never borrow this stack frame.
    loop {
        Timer::after_secs(60).await;
    }
}

async fn authenticate_target(
    fixture: &mut RadioHilJoinFixture<'_>,
    target: StaJoinTarget,
    sequence: &mut StaSequenceCounter,
) -> bool {
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
        return false;
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
        ring: None,
    };
    let mut runner = StaJoinRunner::new(backend, EmbassyStaJoinTimer);
    match runner
        .authenticate(station_address, access_point.bssid, sequence)
        .await
    {
        Ok(success) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=sta-auth-response \\
                 attempt={} frames={} bssid={:02x?}",
                success.attempt, success.total_received_frames, access_point.bssid,
            ));
            true
        }
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-runner \\
                 error={error:?} bssid={:02x?}",
                access_point.bssid,
            ));
            false
        }
    }
}
async fn associate_target(
    fixture: RadioHilConnectedFixture<'_>,
    target: StaJoinTarget,
    security: StaAssociationSecurity<'_>,
) -> (bool, bool, bool) {
    let RadioHilConnectedFixture {
        spawner,
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
    let StaJoinTarget {
        station_address,
        access_point,
    } = target;
    let StaAssociationSecurity {
        pmk,
        supplicant_nonce,
        sequences,
    } = security;
    let association_phy = select_sta_association(&access_point, STA_ASSOCIATION_PREFERENCE).phy;
    let peer_scan_policy = match StaPeerScanPolicy::new(&access_point) {
        Ok(policy) => policy,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-peer-scan-policy error={error:?}"
            ));
            return (false, false, false);
        }
    };
    tx_storage.install_ht_ampdu_policy(peer_scan_policy.ht_ampdu);
    tx_storage.install_he_bss_color(peer_scan_policy.he_bss_color);
    if let Some(parameters) = peer_scan_policy.wmm.parameters() {
        if let Err(error) = tx_storage.install_wmm_edca(parameters) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-wmm-edca \
                 source=scan error={error:?}"
            ));
            return (false, false, false);
        }
    }
    // Keep both 32-frame queues out of the task stack. Passing
    // `NetworkResources::new()` to `StaticCell::init` materializes a temporary
    // of more than 100 KiB before association and corrupts the saved channel
    // waker. HIL on 2026-07-29 observed the resulting misaligned load at
    // embassy-sync `Channel::poll_ready_to_send` immediately after WPA2 M4.
    let network_resources = NetworkResources::init_in_place(OPEN_RADIO_NETWORK_RESOURCES.uninit());
    let network_tx_pool = NetworkTxPool::pin_static(NetworkTxPool::init_in_place(
        OPEN_RADIO_NETWORK_TX_POOL.uninit(),
    ));
    let (network_device, network_runner) =
        network_resources.split(network_tx_pool, station_address);
    let backend = RadioHilStaJoinBackend {
        mmio,
        rx_storage,
        tx_storage,
        descriptor_base,
        buffer_addresses,
        frame,
        station_address,
        access_point,
        ring: None,
    };
    let mut runner = StaJoinRunner::new(backend, EmbassyStaJoinTimer);
    let success = match runner
        .associate(station_address, access_point.bssid, sequences.non_qos_mut())
        .await
    {
        Ok(success) => success,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-runner \\
                 error={error:?} bssid={:02x?}",
                access_point.bssid,
            ));
            return (false, false, false);
        }
    };
    let (mut backend, _) = runner.into_parts();
    let rx_ring = match backend.take_live_ring() {
        Ok(ring) => ring,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-rx-handoff \\
                 error={error:?}"
            ));
            return (false, false, false);
        }
    };
    drop(backend);
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
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-rsn-select error={error:?}"
                ));
                return (true, false, false);
            }
        };
        let noise_floor_dbm = mmio.read_noise_floor_dbm();
        let mut peer_plan = match peer_scan_policy.complete(
            &access_point,
            &response,
            association_phy,
            noise_floor_dbm,
        ) {
            Ok(plan) => plan,
            Err(error) => {
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=sta-peer-association-plan error={error:?}"
                ));
                return (false, false, false);
            }
        };
        tx_storage.install_ht_ampdu_policy(peer_plan.ht_ampdu);
        tx_storage.install_he_bss_color(peer_plan.he_bss_color);
        if peer_plan.wmm.source() == StaWmmSource::AssociationResponse {
            let Some(parameters) = peer_plan.wmm.parameters() else {
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-wmm-edca \
                             source=association-response error=missing-parameters"
                ));
                return (false, false, false);
            };
            if let Err(error) = tx_storage.install_wmm_edca(parameters) {
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-wmm-edca \
                             source=association-response error={error:?}"
                ));
                return (false, false, false);
            }
        }
        if let Some(state) = peer_plan.he_peer_state {
            if let Err(error) = program_he20_peer_state(mmio, state, response.association_id, 0, 0)
            {
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-he20-peer \
                             error={error:?}"
                ));
                return (false, false, false);
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
        if let Err(error) = rate_control.program_hardware(mmio) {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-rate-control \
                         error={error:?}"
            ));
            return (false, false, false);
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
            network_device,
            network_runner,
            rate_control,
            sequences,
        };
        let fixture = RadioHilConnectedFixture {
            spawner,
            platform,
            mmio,
            interrupt_setup,
            rx_storage,
            tx_storage,
            descriptor_base,
            buffer_addresses,
            frame,
            ethernet,
        };
        let (message1, message3) = await_wpa2_message_1(fixture, rx_ring, handshake, session).await;
        return (true, message1, message3);
    }
}

async fn await_wpa2_message_1(
    fixture: RadioHilConnectedFixture<'_>,
    rx_ring: RxRingLive<'_, RX_DESCRIPTOR_COUNT>,
    handshake: Wpa2HandshakeConfig<'_>,
    session: StaConnectedSession<'_>,
) -> (bool, bool) {
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
        ring: Some(rx_ring),
        message2_transmissions: 0,
    };
    let mut runner =
        Wpa2HandshakeRunner::new(backend, EmbassyWpa2HandshakeTimer, Wpa2SoftwareAes::new());
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
            drop(backend);
            return (message1_complete, false);
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-3-key-request \
         frames={} message2_transmissions={} replay={}",
        pending.completed_frames(),
        pending.message2_transmissions(),
        pending.request().replay_counter(),
    ));
    let (backend, _, _) = runner.into_parts();
    drop(backend);

    let message3 = complete_wpa2_key_install_and_connect(fixture, pending, session).await;
    (true, message3)
}

async fn complete_wpa2_key_install_and_connect(
    fixture: RadioHilConnectedFixture<'_>,
    pending: Wpa2PendingKeyInstall,
    session: StaConnectedSession<'_>,
) -> bool {
    let RadioHilConnectedFixture {
        spawner,
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
        mut network_device,
        network_runner,
        rate_control,
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
    let established = match runner.run(pending).await {
        Ok(established) => established,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-key-install-runner \
                 error={error:?} bssid={bssid:02x?}"
            ));
            return false;
        }
    };
    let metadata = established.metadata();
    let backend = runner.into_backend();
    let completion = backend
        .completion
        .expect("successful WPA2 key runner retains Message 4 completion");
    drop(backend);
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
        network_runner.set_link_state(LinkState::Up);
        let mut passed = false;
        for attempt in 1..=WPA2_PROTECTED_ARP_ATTEMPTS {
            let protected_rx_ring =
                match start_live_rx_ring(mmio, rx_storage, descriptor_base, buffer_addresses).await
                {
                    Ok(ring) => ring,
                    Err(error) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                                     stage=wpa2-protected-rx-arm attempt={attempt} \
                                     error={error:?}"
                        ));
                        break;
                    }
                };
            let Some(queued_arp) =
                queue_arp_probe(&mut network_device, &network_runner, station_address)
            else {
                let _ = disable_receive(mmio);
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
                            &mut network_device,
                            &network_runner,
                            station_address,
                            bssid,
                            protected_rx_ring,
                        )
                        .await
                    {
                        passed = true;
                        break;
                    }
                    if !transmitted {
                        let _ = disable_receive(mmio);
                    }
                }
                Err(error) => {
                    let _ = disable_receive(mmio);
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
    } else {
        false
    };
    if message4_valid && message4_sent && protected_arp_pass {
        run_connected_network(
            RadioHilConnectedFixture {
                spawner,
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
            StaConnectedSession {
                link,
                network_device,
                network_runner,
                rate_control,
                sequences,
            },
            key_slot,
            group_slot,
        )
        .await;
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
    return message4_valid
        && message4_sent
        && protected_arp_pass
        && group_key_cleared
        && key_cleared;
}

async fn run_promiscuous_rx_hil(
    spawner: Spawner,
    state: &mut PhyColdState,
    mut platform: EspHalRadioPeripheral,
    mut cold_mmio: ColdRadioRegisters,
    trng: &Trng,
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
    let scan_frame = SCAN_FRAME.init([0; RX_BUFFER_SIZE]);
    let ethernet_frame = ETHERNET_FRAME.init([0; RX_BUFFER_SIZE]);
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

    let mut raw_frames = 0_u32;
    let mut probe_responses = 0_u32;
    let mut tx_completions = 0_u32;
    let mut tx_failures = 0_u32;
    let mut active_tx_available = true;
    let mut ring_epochs = 0_u32;
    for channel_index in 0..STA_SCAN_CHANNEL_COUNT {
        let channel = sta_scan_channel(channel_index);
        if let Err(error) =
            switch_channel_with_mac_restart(state, u16::from(channel), 0, platform, mmio).await
        {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=scan-channel \
                 channel={channel} error={error:?}"
            ));
            return false;
        }
        let rx_start = if channel_index == 0 {
            enable_receive(mmio)
        } else {
            publish_cold_ring(mmio, descriptor_base, true)
        };
        if let Err(error) = rx_start {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-start \
                 channel={channel} error={error:?}"
            ));
            return false;
        }

        if active_tx_available {
            mmio.clear_mac_interrupts(u32::MAX);
            match transmit_probe_request(mmio, tx_storage, station_address, u16::from(channel))
                .await
            {
                Ok(completion) => {
                    tx_completions = tx_completions.saturating_add(1);
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
                        tx_failures = tx_failures.saturating_add(1);
                        active_tx_available = false;
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=passive-fallback \
                             channel={channel} tx_status={}",
                            completion.status,
                        ));
                    }
                }
                Err(error) => {
                    tx_failures = tx_failures.saturating_add(1);
                    active_tx_available = false;
                    // Close the timed-out hardware edge before continuing with
                    // passive RX. The TxSlot remains non-reusable, so no later
                    // channel can accidentally republish the same descriptor.
                    let control = mmio.read32(TX_Q_CONTROL[0]);
                    mmio.write32(TX_Q_CONTROL[0], control & !TX_Q_ENABLE_VALID);
                    mmio.fence();
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=passive-fallback \
                         channel={channel} tx_error={error:?}"
                    ));
                }
            }
        }

        let records_before = scan_table.summary().records;
        let frames_before = raw_frames;
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
            if observed_mask == RX_DESCRIPTOR_COMPLETE_MASK {
                if let Err(error) = disable_receive(mmio) {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-disable \
                         channel={channel} error={error:?}"
                    ));
                    return false;
                }
                if let Err(error) = build_cold_ring(
                    storage.descriptors(),
                    descriptor_base,
                    buffer_addresses,
                    RX_BUFFER_SIZE as u32,
                ) {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-rebuild \
                         channel={channel} error={error:?}"
                    ));
                    return false;
                }
                if let Err(error) = publish_cold_ring(mmio, descriptor_base, true) {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-restart \
                         channel={channel} error={error:?}"
                    ));
                    return false;
                }
                observed_mask = 0;
                ring_epochs += 1;
            }
            Timer::after_millis(1).await;
        }

        if let Err(error) = disable_receive(mmio) {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-disable \
                 channel={channel} error={error:?}"
            ));
            return false;
        }
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
        let channel_summary = scan_table.summary();
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=scan-channel-complete channel={channel} \
             raw_frames={} new_records={} mask={observed_mask:#010x}",
            raw_frames - frames_before,
            channel_summary.records - records_before,
        ));

        if channel_index + 1 != STA_SCAN_CHANNEL_COUNT {
            if let Err(error) = build_cold_ring(
                storage.descriptors(),
                descriptor_base,
                buffer_addresses,
                RX_BUFFER_SIZE as u32,
            ) {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-ring-rebuild \
                     channel={channel} error={error:?}"
                ));
                return false;
            }
        }
    }

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
    let target = best_matching_ssid(scan_table.records(), STA_TARGET_SSID).copied();
    // No cold MAC operation is permitted beyond this point. Consume the cold
    // owner before authentication and retain the one-shot interrupt setup
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
            let Some(passphrase) = STA_PASSPHRASE else {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-config \
                     error=missing-OPEN_RADIO_STA_PASSWORD"
                ));
                return false;
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=wpa2-pmk-derive start iterations=4096"
            ));
            let pmk = match Pmk::derive(passphrase.as_bytes(), STA_TARGET_SSID) {
                Ok(pmk) => pmk,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-pmk-derive error={error:?}"
                    ));
                    return false;
                }
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-pmk-derive"
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
            let mut join_fixture = RadioHilJoinFixture {
                state,
                radio: RadioHilConnectedFixture {
                    spawner,
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
            let authenticated =
                authenticate_target(&mut join_fixture, target, sequences.non_qos_mut()).await;
            let (associated, message1, message3) = if authenticated {
                associate_target(
                    join_fixture.into_connected(),
                    target,
                    StaAssociationSecurity {
                        pmk: &pmk,
                        supplicant_nonce,
                        sequences: &mut sequences,
                    },
                )
                .await
            } else {
                (false, false, false)
            };
            supplicant_nonce.fill(0);
            (authenticated, associated, message1, message3)
        }
        None => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-target ssid={STA_TARGET_SSID:?}"
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
pub async fn run(spawner: Spawner, platform: EspHalRadioPeripheral, trng: Trng) {
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
    let mut transition = PhyRegisterTransition::with_default_profile();
    let mut port =
        TargetPhyRegisterPort::<_, EmbassyPhyDelay, _>::new(&mut powered, HilPhyObserver);
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
            "OPEN_RADIO_PHY_HIL stage=phy-complete full_calibration={} \
                     mmio={} delays={} reset_samples={} rf_operations={} \
                     baseband_operations={}",
            outcome.full_calibration_performed,
            counters.mmio,
            counters.delays,
            counters.reset_samples,
            counters.rf_operations,
            counters.baseband_operations,
        ));
        // `TargetPhyRegisterPort` borrowed the complete radio while the PHY
        // transition was active.  The transition is now finished, so
        // release that borrow before lending the owned register block
        // to the MAC/RX HIL.
        drop(port);
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
        let _ = run_promiscuous_rx_hil(spawner, &mut state, platform, registers, &trng).await;
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
