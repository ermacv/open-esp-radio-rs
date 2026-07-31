use core::{
    cell::{RefCell, UnsafeCell},
    mem::MaybeUninit,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering},
    task::{Context, Waker},
};

use crate::console::emergency_log;
use embassy_futures::select::{Either, select, select4};
use embassy_net::{
    Config as NetworkConfig, Ipv4Address, Ipv4Cidr, Stack, StackResources, StaticConfigV4,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_net_driver::{Driver, LinkState, RxToken as _, TxToken as _};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::efuse::{self, InterfaceMacAddress};
use esp_hal::rng::{Rng, Trng};
use open_esp_radio::esp32s31::{
    pac::{MacHeTbTidLimit, MacHeTid, MacHeTriggerTxQueueSnapshot},
    phy::PhyTxTargetPowerProfile,
};
use open_esp_radio::ieee80211::wmm::WmmParameterSet;
use open_esp_radio::{
    embassy_net::{
        PinnedDevice as OpenRadioNetworkDevice, PinnedRadioRunner as OpenRadioNetworkRunner,
        PinnedResources as OpenRadioNetworkResources, PinnedTxFrame as OpenRadioNetworkTxFrame,
        PinnedTxPool as OpenRadioNetworkTxPool,
    },
    esp32s31::{
        cooperative_tx::CooperativeTxHardware,
        embassy_irq::EmbassyMacIrqRuntime,
        embassy_tx::{
            ReferencedAmpduIngressPolicy, ReferencedHtAmpduBatch, ReferencedHtAmpduError,
        },
        hal::{ColdRadioRegisters, Radio, RadioRegisters},
        mac::{
            crypto::{
                StaGroupCcmpSlot, StaPairwiseCcmpSlot, install_sta_group_ccmp,
                install_sta_pairwise_ccmp,
            },
            descriptor::{DESCRIPTOR_BYTES, Descriptor, length as descriptor_length, rx_done},
            edca::{EdcaContentionParameters, EdcaParametersError},
            he::program_he20_peer_state,
            init::{
                MAC_COLD_RX_INTERRUPT_MASK, StaPeerScanPolicy, StaWmmSource,
                configure_sta_link_receive_policy, initialize_promiscuous_receive,
            },
            irq::handle_mac_irq,
            rate_control::{AmpduRateDecision, StaRateControlAssociation, StaTxRatePolicy},
            rate_schedule::schedule_state,
            registers::{
                MAC_INT_RAW, MAC_INT_STATUS, Mmio, RX_CONTROL, RX_DESCRIPTOR_BASE,
                RX_LAST_DESCRIPTOR, RX_LAST_DESCRIPTOR_HIGH, RX_NEXT_DESCRIPTOR, TX_COMPLETE_STATE,
                TX_Q_CONFIG, TX_Q_CONTROL, TX_Q_DATA_LENGTH, TX_Q_ENABLE_VALID,
                TX_Q_HT_DESCRIPTOR_COUNTS, TX_Q_HT_SIGNAL, TX_Q_LENGTH_CONTROL, TX_Q_PLCP1,
                TX_Q_POWER, TX_Q_PPDU_CONTROL, TX_Q_PROTECTION, TX_Q_PTI, TX_STATE,
            },
            rx::{
                HeGuardIntervalAndLtf, PUBLIC_HEADER_SIZE, RxCompletedDescriptor, RxError,
                RxHe20MuSigBMimoUsersError, RxHe20MuSigBUsersError, RxIngressConfig, RxPhyInfo,
                RxRingError, RxRingLive, RxRingStopped, RxSegment, build_cold_ring,
                decode_rx_he_mu_sig_b, decode_rx_phy_info, disable_receive, enable_receive,
                extract_ccmp_data, extract_control, extract_data, extract_management,
                first_segment_layout, prepare_recycled_buffer, publish_cold_ring,
            },
            rx_ampdu::{
                RX_BLOCK_ACK_MAX_WINDOW, RxBlockAckReorder, write_successful_addba_response,
            },
            rx_ampdu_hw::{self, S31_RX_BLOCK_ACK_MAX_TID, S31RxBlockAckAgreement},
            rx_pool::{
                NetworkRxFrame, RxFrameQueue, RxStagePool, RxStageTransactionError,
                VENDOR_LARGE_RX_PAYLOAD_CAPACITY, VENDOR_LARGE_RX_SLOT_COUNT,
            },
            scan::{ScanObservation, ScanRecord, ScanTable},
            tx::{
                AmpduTxConfig, HeAmpduTxConfig, HeBccDcmMcs, HeDcmRate, HeEdcaTxopLimit,
                HeLdpcDcmMcs, HeMcs, HeRate, HeSmpduTxConfig, HeTriggerBasedTxConfig,
                HeTriggerScheduledRate, HeTriggerScheduledRateError, HtAmpduDensity,
                HtAmpduTxConfig, HtGuardInterval, HtMcs, HtPeerAmpduParameters, HtTxConfig,
                LegacyRate, LegacyTxConfig, LegacyTxQueue, TxCompletion, TxCookie, TxError,
                TxHardware, TxPhyRate, TxSlot,
            },
            tx_ampdu::{
                BlockAckAction, HtAmpduLengthAccumulator, HtAmpduTxCompletion, HtAmpduTxError,
                HtAmpduTxStorage, TxBlockAckAlarm, TxBlockAckConfig, TxBlockAckDialogToken,
                TxBlockAckDialogTokenSequence, TxBlockAckResponse, TxBlockAckSession,
                parse_block_ack_action,
            },
            tx_runtime::{
                AmpduRetryDecision, AmpduRetryError, AmpduRetryPolicy, AmpduRetryState,
                StaTxRuntimePolicy, UnicastRetryDecision, UnicastRetryError, UnicastRetryState,
            },
        },
        pac::{
            MacHeBeamformingDiagnostics, MacHeTxVectorSnapshot, MacInterruptRegisters,
            MacInterruptSetup, MacPowerInterruptRegisters,
            mac::{self as mac_pac, init as mac_registers},
        },
        phy::{
            PhyRegisterRunError, PhyRegisterTransition, PhyRfBoundary, PhyTargetObserver,
            TargetPhyRegisterPort,
            phy_channel::{PhyWifiTxGainImage, PhyWifiTxGainRequest},
            phy_cold::PhyColdState,
            run_phy_register, select_phy_channel, switch_phy_channel_with_mac_restart,
            target_executor::{PhyAsyncDelay, PhyTargetPortError},
        },
    },
    ieee80211::{
        data::{
            DataDecapError, DataHeControl, DataInterfaceRole, amsdu_subframes,
            decapsulate_amsdu_subframe, decapsulate_data,
        },
        he::HeDcmConstellation,
        management::{ProbeRequest, ProbeRequestError},
        ndpa::HeNdpa,
        scan::best_matching_ssid,
        station::{
            AssociationRequest, AssociationRequestError, HeUlMuPowerCapability,
            HeUlMuPowerCapabilityError, OpenAuthenticationRequest,
            STA_PROTECTED_QOS_ETHERNET_HEADROOM, STA_PROTECTED_QOS_ETHERNET_OVERHEAD,
            STA_RESPONSE_TIMEOUT_MS, StaActionFrame, StaAssociationPhy,
            StaAssociationPreference, StaAssociationRetrySchedule, StaAuthenticationEvent,
            StaAuthenticationFailure, StaAuthenticationRuntime, StaDataFrame, StaPowerCapability,
            StaPowerCapabilityError, StaProtectedAmsduFrame, StaProtectedDataFrame,
            StaRxDuplicateFilter, StaSequenceCounter, StaTxSequenceCounters, StationFrameError,
            parse_association_response, select_sta_association, select_wpa2_psk_rsn,
            sta_protected_amsdu_frame_length,
        },
        trigger::{
            TriggerCommonInfo, parse_trigger_frame, parse_trigger_user_ru,
            parse_trigger_user_spatial_stream,
        },
    },
    wpa2::{
        EapolKeyFrame, Message2, Message4, OwnedEapolFrame, Pmk, Ptk,
        PtkContext as CryptoPtkContext, Wpa2Interface,
        aes::software_aes128_key_unwrap,
        key_data::parse_gtk_key_data,
        state::{Wpa2StaAction, Wpa2StaState, Wpa2TxMessage},
    },
};
use open_esp_radio_esp_hal_esp32s31::EspHalRadioPeripheral;

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
static TX_GAIN_ORACLE_CAPTURED: AtomicBool = AtomicBool::new(false);

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
const RX_BUFFER_STORAGE_SIZE: usize = RX_BUFFER_SIZE + core::mem::size_of::<u32>();
const NETWORK_FRAME_CAPACITY: usize = 1_600;
// Raw A-MSDU/A-MPDU HIL generates TX below the network stack, and its direct
// UDP RX meter consumes the benchmark stream before the Embassy handoff.
// Deep Ethernet queues therefore only reduce the CPU stack available to this
// diagnostic image. A non-A-MSDU throughput image uses 1.6-KiB vendor-sized
// slots, so 64 entries still consume less DMA SRAM than the 32-entry jumbo
// A-MSDU profile. That admits one hardware-owned 32-MPDU aggregate and one
// producer-owned burst concurrently instead of serializing the producer
// behind every BlockAck.
//
// SOURCE: ESP32-S31 ESP-IDF Wi-Fi buffer documentation identifies 1.6 KiB as
// the fixed TX-buffer size and says TX throughput scales with the Wi-Fi/LwIP
// buffer counts; complete `_oracles/libnet80211.a[ieee80211_output.o]::
// ieee80211_encap_amsdu` only consumes already queued `s_tx_cacheq` entries.
const NETWORK_QUEUE_DEPTH: usize = if OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_RAW_MAC_BENCH {
    4
} else if OPEN_RADIO_THROUGHPUT_BENCH && !OPEN_RADIO_NETWORK_AMSDU_BENCH {
    64
} else {
    32
};
const OPEN_RADIO_UDP_RX_PORT: u16 = 4_323;
const OPEN_RADIO_UDP_RX_QUEUE_DEPTH: usize = 16;
const OPEN_RADIO_UDP_PAYLOAD_CAPACITY: usize = 1_472;
const OPEN_RADIO_UDP_TX_QUEUE_DEPTH: usize = 16;
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
const OPEN_RADIO_THROUGHPUT_BENCH: bool = option_env!("OPEN_RADIO_TX_BENCH").is_some();
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
const OPEN_RADIO_HE_LDPC_HIL: bool = option_env!("OPEN_RADIO_HE_LDPC_HIL").is_some();
const OPEN_RADIO_HE_DCM_HIL: bool = option_env!("OPEN_RADIO_HE_DCM_HIL").is_some();
const OPEN_RADIO_HE_TB_HIL: bool = option_env!("OPEN_RADIO_HE_TB_HIL").is_some();
const _: () = assert!(
    !(OPEN_RADIO_HE_TB_HIL && OPEN_RADIO_AMSDU_BENCH),
    "OPEN_RADIO_HE_TB_HIL currently requires one MSDU per MPDU"
);
// One slot must admit the complete baseline 3,839-byte A-MSDU class plus the
// outer QoS/CCMP headers, hardware MIC/FCS and S31 private metadata.
const TX_BUFFER_SIZE: usize = if OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_NETWORK_AMSDU_BENCH {
    3_904
} else {
    1_700
};
// Network throughput references the separate cache-TX pool, so its aggregate
// owner needs descriptors and scalar state but no duplicate payload array.
// Synthetic and HE oracle builds still encode into the internal owner.
const TX_AMPDU_BUFFER_SIZE: usize = if OPEN_RADIO_THROUGHPUT_BENCH {
    0
} else {
    TX_BUFFER_SIZE
};
// The vendor scheduler normally works at a 30..32 MPDU BlockAck window.
// Keep the open path at the full recovered static capacity; the negotiated
// peer A-MPDU byte limit may still stop a batch earlier.
const TX_AMPDU_FRAME_COUNT: usize = 32;
const fn selected_ampdu_coalesce_us(value: Option<&str>) -> u64 {
    let Some(value) = value else {
        return 200;
    };
    let bytes = value.as_bytes();
    if bytes.is_empty() {
        panic!("OPEN_RADIO_AMPDU_COALESCE_US must be 0..1000");
    }
    let mut result = 0_u64;
    let mut index = 0_usize;
    while index < bytes.len() {
        let digit = bytes[index];
        if digit < b'0' || digit > b'9' {
            panic!("OPEN_RADIO_AMPDU_COALESCE_US must be 0..1000");
        }
        result = result * 10 + (digit - b'0') as u64;
        index += 1;
    }
    if result > 1_000 {
        panic!("OPEN_RADIO_AMPDU_COALESCE_US must be 0..1000");
    }
    result
}
const TX_AMPDU_COALESCE_US: u64 =
    selected_ampdu_coalesce_us(option_env!("OPEN_RADIO_AMPDU_COALESCE_US"));
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
// One `embassy-net` UDP socket burst contains sixteen datagrams. A full
// 32-slot cache-TX queue can therefore initially supply sixteen two-MSDU
// A-MSDUs. Copying and recycling each second lease creates another burst of
// free slots; at most three producer polls are enough to reach the 32-MPDU or
// 65,535-byte A-MPDU frontier from a minimally populated initial pair.
const TX_AMSDU_REFILL_BURST_LIMIT: u8 = 3;
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
// esp-wifi-sys S31 oracle; open-esp-radio-mac-esp32s31::tx::LegacyRate records
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

const fn selected_open_radio_dcm_data_power(value: Option<&str>) -> Option<u8> {
    let Some(value) = value else {
        return None;
    };
    let bytes = value.as_bytes();
    let power = match bytes {
        [digit @ b'0'..=b'9'] => *digit - b'0',
        [tens @ b'1'..=b'2', ones @ b'0'..=b'9'] => (*tens - b'0') * 10 + (*ones - b'0'),
        [b'3', ones @ b'0'..=b'1'] => 30 + (*ones - b'0'),
        _ => panic!("OPEN_RADIO_HE_DCM_DATA_POWER_CODE must be 0..31"),
    };
    Some(power)
}

const OPEN_RADIO_HE_DCM_DATA_POWER_CODE: Option<u8> =
    selected_open_radio_dcm_data_power(option_env!("OPEN_RADIO_HE_DCM_DATA_POWER_CODE"));

const HE_MATRIX_PROFILE_COUNT: u8 = 40;
const HE_DCM_MATRIX_PROFILE_COUNT: u8 = 12;
const HE_MATRIX_AGGREGATES_PER_PROFILE: u32 = 64;

const fn he_matrix_first_profile(peer_supports_one_ltf_800ns_gi: bool) -> u8 {
    if peer_supports_one_ltf_800ns_gi {
        0
    } else {
        10
    }
}

fn he_matrix_rate(profile: u8) -> HeRate {
    let mcs = HeMcs::from_index(profile % 10).expect("HE matrix MCS is bounded to 0..9");
    let guard_interval_and_ltf = match profile / 10 {
        0 => HeGuardIntervalAndLtf::OneLtf800Ns,
        1 => HeGuardIntervalAndLtf::TwoLtf800Ns,
        2 => HeGuardIntervalAndLtf::TwoLtf1600Ns,
        3 => HeGuardIntervalAndLtf::FourLtf3200Ns,
        _ => unreachable!("HE matrix profile is bounded to 0..39"),
    };
    HeRate::new(mcs, guard_interval_and_ltf)
}

fn he_ldpc_matrix_rate(profile: u8) -> HeRate {
    // SOURCE[HIL_OPEN_HE20_LDPC_MATRIX_2026_07_30]: FRITZ!Box 7530 FN,
    // channel 6, payload-LDPC capability set. Three complete 30-profile
    // MCS0..9 x three-GI A-MPDU matrices passed with zero failed profiles and
    // zero terminal retries; MCS9/0.8 us admitted all 32 MPDUs.
    let bcc = he_matrix_rate(profile);
    HeRate::ldpc(bcc.mcs(), bcc.guard_interval_and_ltf())
}

fn he_dcm_matrix_rate(profile: u8) -> HeRate {
    // Profiles are grouped by constellation so a BPSK-only peer runs the
    // first three GI/LTF combinations, QPSK adds the next three, and 16-QAM
    // adds the final three.
    let guard_interval_and_ltf = match profile % 3 {
        0 => HeGuardIntervalAndLtf::TwoLtf800Ns,
        1 => HeGuardIntervalAndLtf::TwoLtf1600Ns,
        2 => HeGuardIntervalAndLtf::FourLtf3200Ns,
        _ => unreachable!("profile modulo three is bounded"),
    };
    match profile / 3 {
        0 => HeRate::bcc_dcm(HeBccDcmMcs::Mcs0, guard_interval_and_ltf),
        1 => HeRate::bcc_dcm(HeBccDcmMcs::Mcs1, guard_interval_and_ltf),
        2 => HeRate::bcc_dcm(HeBccDcmMcs::Mcs3, guard_interval_and_ltf),
        // Complete blob/ROM mac_tx_set_hesig selects the separate LDPC
        // HE-A2 control, and ROM he_rates_dcm_ru_242 owns this MCS4 column.
        3 => HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, guard_interval_and_ltf),
        _ => unreachable!("HE DCM matrix profile is bounded to 0..11"),
    }
}

const fn he_dcm_matrix_profile_count(
    peer_dcm_receive: HeDcmConstellation,
    peer_supports_ldpc: bool,
) -> u8 {
    match peer_dcm_receive {
        HeDcmConstellation::NotSupported => 0,
        HeDcmConstellation::Bpsk => 3,
        HeDcmConstellation::Qpsk => 6,
        HeDcmConstellation::Qam16 if peer_supports_ldpc => HE_DCM_MATRIX_PROFILE_COUNT,
        // MCS3 is the highest BCC DCM profile. Never infer LDPC merely from
        // the independently advertised 16-QAM DCM constellation.
        HeDcmConstellation::Qam16 => 9,
    }
}

fn active_he_matrix_rate(profile: u8) -> HeRate {
    if OPEN_RADIO_HE_DCM_HIL {
        he_dcm_matrix_rate(profile)
    } else if OPEN_RADIO_HE_LDPC_HIL {
        he_ldpc_matrix_rate(profile)
    } else {
        he_matrix_rate(profile)
    }
}

fn he_matrix_ampdu_limit(rate: HeRate, ethernet_bytes: usize, txop: HeEdcaTxopLimit) -> usize {
    // Use the scan-owned WMM EDCA TXOP and the complete blob APEP producer,
    // then count exact delimiter/MPDU/MIC/FCS bytes with the same bounded
    // accumulator as the live DMA owner. A zero TXOP selects the ROM table.
    let psdu_bytes =
        ethernet_bytes + STA_PROTECTED_QOS_ETHERNET_OVERHEAD + TX_CCMP_MIC_SIZE + TX_FCS_SIZE;
    let maximum_apep_bytes = rate.maximum_apep_bytes(txop).min(u32::from(u16::MAX)) as u16;
    let mut length = HtAmpduLengthAccumulator::new(TX_AMPDU_FRAME_COUNT as u8, maximum_apep_bytes)
        .expect("finite HE APEP policy");
    let mut subframes = 0;
    while length.push(psdu_bytes as u32, 0).is_ok() {
        subframes += 1;
    }
    subframes
}
const fn selected_open_radio_ampdu_limit(value: Option<&str>) -> usize {
    let Some(value) = value else {
        return TX_AMPDU_FRAME_COUNT;
    };
    let bytes = value.as_bytes();
    let limit = match bytes {
        [digit @ b'2'..=b'9'] => (*digit - b'0') as usize,
        [tens @ b'1'..=b'2', ones @ b'0'..=b'9'] => {
            ((*tens - b'0') as usize) * 10 + (*ones - b'0') as usize
        }
        [b'3', ones @ b'0'..=b'2'] => 30 + (*ones - b'0') as usize,
        _ => panic!("OPEN_RADIO_AMPDU_LIMIT must be 2..32"),
    };
    if limit < 2 || limit > TX_AMPDU_FRAME_COUNT {
        panic!("OPEN_RADIO_AMPDU_LIMIT must be 2..32");
    }
    limit
}
const OPEN_RADIO_AMPDU_LIMIT: usize =
    selected_open_radio_ampdu_limit(option_env!("OPEN_RADIO_AMPDU_LIMIT"));
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
const SCAN_DWELL_MS: u32 = 100;
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
const SCAN_CHANNELS: [u8; 13] = [11, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 12, 13];
// A full-domain performance survey is available explicitly, while the normal
// connection loop keeps the pinned channel-6 fast path. Forcing CBW40 against
// an HT Operation IE that forbids it is never valid; every parsed target still
// passes through `ht40_secondary_channel`.
const AUTH_DIAGNOSTIC_CHANNEL_COUNT: usize = if option_env!("OPEN_RADIO_FULL_SCAN").is_some() {
    SCAN_CHANNELS.len()
} else {
    1
};
const fn auth_diagnostic_channel(index: usize) -> u8 {
    if AUTH_DIAGNOSTIC_CHANNEL_COUNT == 1 {
        LISTEN_CHANNEL as u8
    } else {
        SCAN_CHANNELS[index]
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
// SOURCE: `_oracles/libnet80211.a[ieee80211_output.o]` passes coexistence
// event 5 in complete `ieee80211_send_probereq` and event 6 in complete
// `ieee80211_send_mgmt` for Authentication/Association.
// `_oracles/libcoexist.a[coexist_core.o]::coex_pti_tab` maps both packet
// events to PTI 1 and event 1 to PTI 5; complete
// `_oracles/libpp.a[hal_mac.o,hal_coex.o]::
// {mac_tx_set_pti,hal_set_tx_pti}` retains the four packet lanes at 1 while
// selecting the numerically smaller scheduler value, min(1, 5) = 1.
const VENDOR_MANAGEMENT_SCHEDULER_PRIORITY: u8 = 1;
const VENDOR_MANAGEMENT_PACKET_PRIORITY: u8 = 1;
const WPA2_MESSAGE_1_TIMEOUT_MS: u32 = 3_000;
const WPA2_MESSAGE_3_TIMEOUT_MS: u32 = 3_000;
const WPA2_PROTECTED_ARP_TIMEOUT_MS: u32 = 1_500;
const WPA2_CONTROLLED_PORT_SETTLE_MS: u64 = 10;
const WPA2_PROTECTED_ARP_ATTEMPTS: u8 = 3;
const WPA2_PROTECTED_ARP_RETRY_DELAY_MS: u64 = 20;
const WPA2_MESSAGE_2_ATTEMPTS: u16 = 2;
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

#[repr(C, align(4))]
struct DmaBuffer(UnsafeCell<[u8; RX_BUFFER_STORAGE_SIZE]>);

// The sole HIL task owns each buffer while the Wi-Fi DMA engine may mutate its
// contents. CPU observations are volatile and happen only after the matching
// descriptor has returned ownership.
unsafe impl Send for DmaBuffer {}

impl DmaBuffer {
    const fn new() -> Self {
        Self(UnsafeCell::new([0; RX_BUFFER_STORAGE_SIZE]))
    }

    fn address(&self) -> u32 {
        self.0.get().addr() as u32
    }

    unsafe fn read_word(&self, offset: usize) -> u32 {
        unsafe {
            let bytes = self.0.get().cast::<u8>().add(offset);
            u32::from_le_bytes([
                bytes.read_volatile(),
                bytes.add(1).read_volatile(),
                bytes.add(2).read_volatile(),
                bytes.add(3).read_volatile(),
            ])
        }
    }

    unsafe fn read_byte(&self, offset: usize) -> u8 {
        unsafe { self.0.get().cast::<u8>().add(offset).read_volatile() }
    }

    unsafe fn as_slice(&self) -> &[u8; RX_BUFFER_SIZE] {
        // SAFETY: the prefix is exactly the descriptor-advertised capacity;
        // the four following bytes remain the migration recycler's trailing
        // sentinel and are intentionally hidden from frame parsing.
        unsafe { &*self.0.get().cast::<[u8; RX_BUFFER_SIZE]>() }
    }

    unsafe fn prepare_for_recycle(&self) -> Result<(), RxRingError> {
        // SAFETY: callers proved that the matching completed descriptor has
        // transferred this allocation back to the sole radio owner.
        unsafe { prepare_recycled_buffer(&mut *self.0.get(), RX_BUFFER_SIZE) }
    }
}

struct RxStorage {
    descriptors: [Descriptor; RX_DESCRIPTOR_COUNT],
    buffers: [DmaBuffer; RX_DESCRIPTOR_COUNT],
}

impl RxStorage {
    const fn new() -> Self {
        Self {
            descriptors: [const { Descriptor::new() }; RX_DESCRIPTOR_COUNT],
            buffers: [const { DmaBuffer::new() }; RX_DESCRIPTOR_COUNT],
        }
    }

    /// Initialize the 148-KiB RX arena in its final DMA allocation.
    ///
    /// `StaticCell::init(Self::new())` first created the complete value in the
    /// async poll stack. The resulting 165-KiB frame crossed `_stack_end` in
    /// HIL before association. Both fields consist solely of `UnsafeCell<u32>`
    /// descriptor words and `UnsafeCell<u8>` buffers, so zero is their exact
    /// `new()` representation.
    fn init_in_place(storage: &mut MaybeUninit<Self>) -> &mut Self {
        let storage = storage.as_mut_ptr();
        // SAFETY: the caller provides an exclusive, aligned uninitialized
        // allocation. Every byte of both fields is initialized before the
        // reference is formed; no enum, reference or niche-bearing field is
        // present in RxStorage.
        unsafe {
            storage
                .cast::<u8>()
                .write_bytes(0, core::mem::size_of::<Self>());
            &mut *storage
        }
    }
}

struct TxStorage {
    slot: Pin<&'static mut TxSlot<TX_BUFFER_SIZE>>,
    tx_power_profile: Option<PhyTxTargetPowerProfile>,
    runtime_policy: StaTxRuntimePolicy,
    attempts: u32,
    successes: u32,
    ack_timeouts: u32,
    other_failures: u32,
    hardware_timeouts: u32,
    ampdu_success_wait_us: u64,
    ampdu_success_wait_samples: u32,
    ampdu_status5_wait_us: u64,
    ampdu_status5_wait_samples: u32,
    ampdu_other_wait_us: u64,
    ampdu_other_wait_samples: u32,
}

impl TxStorage {
    fn new(slot: Pin<&'static mut TxSlot<TX_BUFFER_SIZE>>) -> Self {
        Self {
            slot,
            tx_power_profile: None,
            runtime_policy: StaTxRuntimePolicy::vendor_defaults(),
            attempts: 0,
            successes: 0,
            ack_timeouts: 0,
            other_failures: 0,
            hardware_timeouts: 0,
            ampdu_success_wait_us: 0,
            ampdu_success_wait_samples: 0,
            ampdu_status5_wait_us: 0,
            ampdu_status5_wait_samples: 0,
            ampdu_other_wait_us: 0,
            ampdu_other_wait_samples: 0,
        }
    }

    fn install_tx_power_profile(&mut self, profile: PhyTxTargetPowerProfile) {
        self.tx_power_profile = Some(profile);
    }

    fn dma_buffer_mut(&mut self) -> &mut [u8; TX_BUFFER_SIZE] {
        self.slot
            .as_mut()
            .buffer_mut()
            .expect("ordinary TX DMA buffer is borrowed only while free")
    }

    fn install_ht_ampdu_policy(&mut self, parameters: HtPeerAmpduParameters) {
        self.runtime_policy.install_ht_ampdu(parameters);
    }

    fn install_he_bss_color(&mut self, bss_color: u8) {
        self.runtime_policy.install_he_bss_color(bss_color);
    }

    fn install_wmm_edca(&mut self, parameters: WmmParameterSet) -> Result<(), EdcaParametersError> {
        self.runtime_policy.install_wmm(parameters)
    }

    fn edca_parameters(&self, queue: LegacyTxQueue) -> EdcaContentionParameters {
        self.runtime_policy.contention_parameters(queue)
    }

    fn record_edca_retry_failure(&mut self, queue: LegacyTxQueue) {
        self.runtime_policy.record_retry_failure(queue);
    }

    fn record_edca_success(&mut self, queue: LegacyTxQueue) {
        self.runtime_policy.record_success(queue);
    }

    fn reset_terminal_edca_exchange(&mut self, queue: LegacyTxQueue) {
        self.runtime_policy.reset_terminal_exchange(queue);
    }

    fn next_edca_backoff(&mut self, queue: LegacyTxQueue) -> u16 {
        // SOURCE: complete `_oracles/libpp.a[hal_mac.o]::hal_random` jumps
        // through `wifi_osi_funcs_t::_random` at offset 0xbc; the pinned
        // esp-radio implementation of that callback constructs
        // `esp_hal::rng::Rng` and returns its hardware RNG word. The live
        // `Trng` owner keeps the S31 entropy source enabled for this run.
        //
        // Entropy production stays platform-owned; the bounded EDCA state
        // and `(1 << current_exponent) - 1` selection live in the MAC crate.
        self.runtime_policy
            .select_backoff(queue, Rng::new().random())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveScanTxError {
    Encode(ProbeRequestError),
    StationEncode(StationFrameError),
    AssociationEncode(AssociationRequestError),
    PowerCapability(StaPowerCapabilityError),
    HeUlMuPower(HeUlMuPowerCapabilityError),
    Reserve(TxError),
    Submit(TxError),
    Completion(TxError),
    HardwareTimedOut,
    CompletionTimedOut,
    Detach(TxError),
    MissingTxPowerProfile,
    Ampdu(HtAmpduTxError),
    AmpduRetry(AmpduRetryError),
    UnicastRetry(UnicastRetryError),
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
// Keep the same ownership boundary explicit in the open HIL. This ordinary
// object follows the selected data profile (PSRAM in psram-code-psram-data);
// unlike OPEN_RADIO_RX_DMA_STORAGE, the Wi-Fi DMA master never addresses it.
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".critical.bss.open_radio_rx_stage")]
static OPEN_RADIO_RX_STAGE_POOL: RxStagePool<
    VENDOR_LARGE_RX_SLOT_COUNT,
    VENDOR_LARGE_RX_PAYLOAD_CAPACITY,
> = RxStagePool::new();
type NetworkResources = OpenRadioNetworkResources<
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_QUEUE_DEPTH,
>;
type NetworkDevice = OpenRadioNetworkDevice<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_QUEUE_DEPTH,
>;
type NetworkRunner = OpenRadioNetworkRunner<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_QUEUE_DEPTH,
>;
type NetworkTxFrame = OpenRadioNetworkTxFrame<
    'static,
    CriticalSectionRawMutex,
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_QUEUE_DEPTH,
>;
type NetworkTxPool = OpenRadioNetworkTxPool<
    NETWORK_FRAME_CAPACITY,
    NETWORK_TX_HEADROOM,
    NETWORK_TX_TRAILER,
    NETWORK_QUEUE_DEPTH,
>;
// The embassy-net queues are CPU-owned and are never presented to the Wi-Fi
// DMA engine. In the qualified `psram-code-psram-data` runtime ordinary `.bss`
// already lives in PSRAM; an explicit `.psram.bss` input section would bypass
// the runtime payload layout and overlap `.runtime.payload_end`.
//
// Standalone flash-XIP A-MSDU HIL previously left 33,160 bytes between
// `_stack_end` and `_stack_start`. The WPA2 path crossed that frontier and
// overwrote `SharedLinkState`; its next `set_link_state` failed with a
// misaligned waker load at 0x400679e2. The benchmark-specific queue depth above
// removes that false memory pressure, while production stays at depth 32.
static OPEN_RADIO_NETWORK_RESOURCES: StaticCell<NetworkResources> = StaticCell::new();
// Only allocations actually addressed by Wi-Fi DMA are forced into SRAM.
// Embassy RX frames, channels and link state remain ordinary PSRAM data.
#[used]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".dma.bss.open_radio_network_tx")]
static OPEN_RADIO_NETWORK_TX_POOL: StaticCell<NetworkTxPool> = StaticCell::new();
static OPEN_RADIO_STACK_RESOURCES: StaticCell<StackResources<3>> = StaticCell::new();
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
static OPEN_RADIO_LOCAL_IPV4: AtomicU32 = AtomicU32::new(0);
static OPEN_RADIO_LAN_PROBE_READY: AtomicBool = AtomicBool::new(false);
static OPEN_RADIO_LAN_PROBE_RESPONSE: AtomicBool = AtomicBool::new(false);
#[unsafe(link_section = ".critical.bss.open_radio_irq")]
static OPEN_RADIO_IRQ_RUNTIME: EmbassyMacIrqRuntime<CriticalSectionRawMutex> =
    EmbassyMacIrqRuntime::new();
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
        &rx_storage.descriptors,
        descriptor_base,
        buffer_addresses,
        RX_BUFFER_SIZE as u32,
        |index| {
            // SAFETY: RxRingStopped has confirmed that hardware released the
            // walker before it transfers each DMA buffer back for preparation.
            unsafe { rx_storage.buffers[index].prepare_for_recycle() }
        },
    )?;
    Timer::after_micros(5).await;
    stopped.start(mmio)
}

#[esp_hal::handler]
#[unsafe(link_section = ".rwtext.open_radio_irq")]
fn open_radio_mac_interrupt() {
    let registers = OPEN_RADIO_MAC_INTERRUPT_PTR.load(Ordering::Acquire);
    if registers.is_null() {
        return;
    }
    // SAFETY: `run` publishes the one-shot split capability before binding
    // this handler. The S31 masks the active interrupt while its handler runs,
    // and no task-side code receives this finite capability, so calls cannot
    // overlap and this is its sole mutable reference.
    let interrupt = unsafe { &mut *registers };
    for _ in 0..32 {
        let (_, snapshot) = handle_mac_irq(&mut *interrupt, &OPEN_RADIO_IRQ_RUNTIME);
        if snapshot.status == 0 {
            break;
        }
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
    // This HIL runs with power save disabled, so no task-side PM action is
    // required after acknowledging the exact pending image.
    interrupt.acknowledge_pending_power_interrupts();
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

    fn channel_tx_gain(&mut self, request: PhyWifiTxGainRequest, image: PhyWifiTxGainImage) {
        if request.channel == LISTEN_CHANNEL
            && TX_GAIN_ORACLE_CAPTURED
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
        {
            compare_tx_gain_with_rom(request, image);
        }
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

    fn tx_dc_comparator(
        &mut self,
        gain_index: u8,
        iteration: u8,
        comparator_high: [bool; 2],
    ) {
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
        register_value: u32,
    ) {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL probe=pwdet-sample measurement={} sample={} \
             raw={:#010x} value={} tone={:#010x}/{:#010x}/{:#010x} \
             sar={:#010x}/{:#010x} reference={:#010x}",
            measurement_index,
            sample_index,
            register_value,
            open_esp_radio::esp32s31::phy::phy_pwdet::sar_sample_from_register(register_value),
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

#[cfg(target_arch = "riscv32")]
fn compare_tx_gain_with_rom(request: PhyWifiTxGainRequest, rust: PhyWifiTxGainImage) {
    // Diagnostic oracle only: complete rev0 ROM `phy_wifi_get_tx_gain` at
    // 0x2f82_6ff8. Its calling convention and the three tables are recovered
    // from `_oracles/esp32s31_rev0_rom.elf` and
    // `_oracles/libphy.a[phy_tx_gain.o]::phy_wifi_get_tx_tab_new`.
    // The open driver never calls this helper; the HIL removes it after the
    // live Rust/ROM equivalence check.
    const TABLE_LOW: [u16; 18] = [
        0x003f, 0x0037, 0x002f, 0x0027, 0x0027, 0x001f, 0x0017, 0x000f, 0x000f, 0x000d, 0x000c,
        0x0007, 0x0006, 0x0005, 0x0004, 0x0003, 0x0002, 0x0001,
    ];
    const TABLE_MID: [u16; 18] = [
        0x0100, 0x0100, 0x0100, 0x0100, 0x8000, 0x8000, 0x8000, 0x8000, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0,
    ];
    const TABLE_HIGH: [u16; 18] = [
        0x001b, 0x0018, 0x0014, 0x000e, 0x0006, 0x0000, 0xfff6, 0xffe9, 0xffe1, 0xffd7, 0xffd0,
        0xffc9, 0xffc4, 0xffbe, 0xffb8, 0xffb0, 0xffa5, 0xff97,
    ];
    type RomPhyWifiGetTxGain = unsafe extern "C" fn(
        u32,
        *const u8,
        i32,
        i32,
        *const u16,
        *const u16,
        *const u16,
        *mut u8,
        *mut u16,
        *mut u16,
        u32,
    );

    let mut rom_32 = [0_u8; 32];
    let mut rom_64 = [0_u16; 32];
    let mut rom_72 = [0_u16; 32];
    // SAFETY: this is a diagnostic call to the immutable, chip-revision-pinned
    // ROM oracle. All eleven arguments match the complete caller above and
    // point to fixed-size, aligned Rust storage valid for the duration.
    let oracle: RomPhyWifiGetTxGain = unsafe { core::mem::transmute(0x2f82_6ff8_usize) };
    unsafe {
        oracle(
            u32::from(request.channel),
            request.calibration_curve.as_ptr(),
            i32::from(request.correction),
            i32::from(request.base_and_delta),
            TABLE_LOW.as_ptr(),
            TABLE_MID.as_ptr(),
            TABLE_HIGH.as_ptr(),
            rom_32.as_mut_ptr(),
            rom_64.as_mut_ptr(),
            rom_72.as_mut_ptr(),
            0,
        );
    }

    let mut differences = 0_u32;
    for index in 0..32 {
        let shift = (index & 3) * 8;
        let rust_32 = ((rust.output_32[index >> 2] >> shift) & 0xff) as u8;
        let shift = (index & 1) * 16;
        let rust_64 = ((rust.output_64[index >> 1] >> shift) & 0xffff) as u16;
        let rust_72 = ((rust.output_72[index >> 1] >> shift) & 0xffff) as u16;
        differences += u32::from(rust_32 != rom_32[index]);
        differences += u32::from(rust_64 != rom_64[index]);
        differences += u32::from(rust_72 != rom_72[index]);
    }
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL probe=tx-gain-rom-equivalence channel={} \
         differences={} rust32={:08x?} rom32={:02x?}",
        request.channel, differences, rust.output_32, rom_32,
    ));
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
    for (index, descriptor) in storage.descriptors.iter().enumerate() {
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
                storage.buffers[index].as_slice()
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
                let rssi = unsafe { storage.buffers[index].read_byte(0) as i8 };
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
                    storage.buffers[index].read_word(0x28 + word * 4)
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
    let frame_length = ProbeRequest {
        source,
        sequence_number,
        ssid: b"",
        supported_rates: &PROBE_REQUEST_RATES,
    }
    .encode(&mut storage.dma_buffer_mut()[TX_METADATA_SIZE..])
    .map_err(ActiveScanTxError::Encode)?;
    storage.dma_buffer_mut()[TX_METADATA_SIZE + frame_length..TX_METADATA_SIZE + frame_length + 3]
        .copy_from_slice(&[3, 1, sequence_number as u8]);
    let frame_length = frame_length + 3;
    let result = transmit_encoded_frame(
        mmio,
        storage,
        LegacyTxQueue::Voice,
        frame_length,
        PROBE_TX_DESCRIPTOR_CAPACITY,
        None,
        VENDOR_MANAGEMENT_SCHEDULER_PRIORITY,
        VENDOR_MANAGEMENT_PACKET_PRIORITY,
        TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
        0,
        0,
    )
    .await;
    match result {
        Ok(completion) if completion.status == 0 => {
            storage.record_edca_success(LegacyTxQueue::Voice);
            Ok(completion)
        }
        Ok(completion) => {
            storage.reset_terminal_edca_exchange(LegacyTxQueue::Voice);
            Ok(completion)
        }
        Err(error) => {
            storage.reset_terminal_edca_exchange(LegacyTxQueue::Voice);
            Err(error)
        }
    }
}

async fn transmit_open_authentication<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    source: [u8; 6],
    bssid: [u8; 6],
    sequence_number: u16,
) -> Result<TxCompletion, ActiveScanTxError> {
    let frame_length = OpenAuthenticationRequest {
        source,
        bssid,
        sequence_number,
    }
    .encode(&mut storage.dma_buffer_mut()[TX_METADATA_SIZE..])
    .map_err(ActiveScanTxError::StationEncode)?;
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
    let hardware_storage_length = frame_length + TX_METADATA_SIZE + TX_FCS_SIZE;
    let descriptor_capacity = (hardware_storage_length + 3) & !3;
    let completion = transmit_encoded_unicast_with_retry(
        mmio,
        storage,
        LegacyTxQueue::Voice,
        frame_length,
        descriptor_capacity,
        None,
        VENDOR_MANAGEMENT_SCHEDULER_PRIORITY,
        VENDOR_MANAGEMENT_PACKET_PRIORITY,
        TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
        0,
        0,
    )
    .await;
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
        let profile = storage
            .tx_power_profile
            .ok_or(ActiveScanTxError::MissingTxPowerProfile)?;
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
    let frame_length = AssociationRequest {
        source,
        access_point,
        sequence_number,
        // SOURCE[HIL_VENDOR_HE20_NDPA_CBF_2026_07_24]: successful vendor
        // association frame 7624 uses listen interval three.
        listen_interval: 3,
        phy,
        power_capability,
        he_ul_mu_power,
    }
    .encode(&mut storage.dma_buffer_mut()[TX_METADATA_SIZE..])
    .map_err(ActiveScanTxError::AssociationEncode)?;
    // `transmit_encoded_management` publishes four additional bytes in the
    // descriptor length for the hardware-appended FCS. Keep the allocation
    // capacity large enough for that hardware-visible length before rounding
    // it to the recovered four-byte DMA granularity.
    let hardware_storage_length = frame_length + TX_METADATA_SIZE + TX_FCS_SIZE;
    let descriptor_capacity = hardware_storage_length
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
    transmit_encoded_unicast_with_retry(
        mmio,
        storage,
        LegacyTxQueue::Voice,
        frame_length,
        descriptor_capacity,
        None,
        VENDOR_MANAGEMENT_SCHEDULER_PRIORITY,
        VENDOR_MANAGEMENT_PACKET_PRIORITY,
        TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
        0,
        0,
    )
    .await
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

fn observe_ampdu_rate_control(
    rate_control: &mut StaRateControlAssociation,
    attempted_mpdu: u16,
    acknowledged_mpdu: u8,
) {
    let now_us = Instant::now().as_micros() as u32;
    match rate_control.observe_ampdu_block_ack(now_us, attempted_mpdu, u16::from(acknowledged_mpdu))
    {
        Ok(AmpduRateDecision::Promote {
            from,
            to,
            raw_success_ratio,
            filtered_success_ratio,
        }) => emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=ampdu-rate-promote \
             from={:?}/{} to={:?}/{} raw_ratio={} filtered_ratio={}",
            from.kind, from.index, to.kind, to.index, raw_success_ratio, filtered_success_ratio,
        )),
        Ok(AmpduRateDecision::Lower {
            from,
            to,
            raw_success_ratio,
            filtered_success_ratio,
        }) => emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=ampdu-rate-lower \
             from={:?}/{} to={:?}/{} raw_ratio={} filtered_ratio={}",
            from.kind, from.index, to.kind, to.index, raw_success_ratio, filtered_success_ratio,
        )),
        Ok(AmpduRateDecision::Accumulating | AmpduRateDecision::Retain { .. }) => {}
        Err(error) => emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=ampdu-rate-observe error={error:?} \
             attempted={attempted_mpdu} acknowledged={acknowledged_mpdu}"
        )),
    }
}

async fn transmit_encoded_frame<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    queue: LegacyTxQueue,
    frame_length: usize,
    descriptor_capacity: usize,
    signal_override: Option<u16>,
    scheduler_priority: u8,
    pti: u8,
    rate: TxPhyRate,
    hardware_key_selector: u8,
    security_mic_length: usize,
) -> Result<TxCompletion, ActiveScanTxError> {
    // HE SU owns the pinned multi-descriptor pool, including its distinct
    // S-MPDU/A-MPDU metadata and HE-SIG formatter. Rejecting it before
    // `TxSlot::reserve` is an ownership invariant: no unsupported formatter
    // may leave the ordinary single-descriptor slot in Reserved.
    //
    // SOURCE: complete `_oracles/libpp.a[pp_he.o]::
    // {ppCalTxHESMPDULength,ppCalTxHEAMPDULength}` and the corresponding
    // open `HtAmpduTxStorage::{submit_he_smpdu,submit_he}` implementations.
    if matches!(rate, TxPhyRate::He(_)) {
        return Err(ActiveScanTxError::Reserve(TxError::Invalid));
    }
    let queue_index = queue as usize;
    let hardware_trailer_length = TX_FCS_SIZE + security_mic_length;
    let hardware_frame_length = frame_length + hardware_trailer_length;
    let expected_signal = u16::try_from(hardware_frame_length)
        .map_err(|_| ActiveScanTxError::Reserve(TxError::Invalid))?;
    if signal_override.is_some_and(|signal| signal != expected_signal) {
        return Err(ActiveScanTxError::Reserve(TxError::Invalid));
    }
    let group_receiver = {
        let buffer = storage.dma_buffer_mut();
        buffer[..4].copy_from_slice(&(hardware_frame_length as u32).to_le_bytes());
        buffer[4..TX_METADATA_SIZE].fill(0);
        buffer[TX_METADATA_SIZE + frame_length..TX_METADATA_SIZE + hardware_frame_length].fill(0);
        buffer[TX_METADATA_SIZE + 4] & 1 != 0
    };
    let transfer_length = TX_METADATA_SIZE + hardware_frame_length;
    let cookie = storage
        .slot
        .as_mut()
        .reserve(descriptor_capacity as u32, transfer_length as u32)
        .map_err(ActiveScanTxError::Reserve)?;
    let power_profile = storage
        .tx_power_profile
        .ok_or(ActiveScanTxError::MissingTxPowerProfile)?;
    // Lifetime is selected by the descriptor's A-MPDU-container bit inside
    // the driver config constructor. It is not an EDCA queue property.
    // AIFSN is no longer hard-coded here: EdcaQueues owns the exact lmac
    // defaults and any later atomic WMM Parameter Set update.
    let aifsn = storage.edca_parameters(queue).aifsn();
    let contention_window = storage.next_edca_backoff(queue);

    // Do not clear the whole MAC interrupt word here. The recovered
    // `hal_mac_interrupt_clr_event` writes only the event image returned by
    // `hal_mac_interrupt_get_event`; TX completion has its own per-queue W1C
    // transaction. A blanket `u32::MAX` can acknowledge unrelated RX/TX
    // edges while the RX ring is armed.
    mmio.fence();
    match rate {
        TxPhyRate::Legacy(rate) => {
            let mut config = match signal_override {
                Some(signal) => LegacyTxConfig::management_1m(signal),
                None => LegacyTxConfig::management_1m_from_mpdu_length(frame_length as u16)
                    .expect("bounded HIL management MPDU length"),
            };
            // SOURCE: complete libpp.a[pp.o]::ppTxProtoProc copies the I/G bit
            // of address one into descriptor flag two. mac_tx_set_plcp0 then
            // retains format zero for group traffic.
            config.group_receiver = group_receiver;
            config.rate = rate;
            let rts_rate = rate.vendor_rts_rate();
            config.rts_rate = rts_rate;
            let data_power = power_profile.pair(rate.code());
            let rts_power = power_profile.pair(rts_rate.code());
            config.data_power = data_power.primary as u8;
            config.rts_power_low = rts_power.primary as u8;
            config.rts_power_high = rts_power.alternate as u8;
            config.scheduler_priority = scheduler_priority;
            config.pti = pti;
            config.pti_count = 1;
            config.aifsn = aifsn;
            config.contention_window = contention_window;
            config.hardware_key_selector = hardware_key_selector;
            storage
                .slot
                .as_mut()
                .submit_legacy(mmio, cookie, queue, config)
                .map_err(ActiveScanTxError::Submit)?;
        }
        TxPhyRate::Ht(rate) => {
            let mpdu_length = u16::try_from(frame_length)
                .map_err(|_| ActiveScanTxError::Reserve(TxError::Invalid))?;
            let hardware_mic_length = u8::try_from(security_mic_length)
                .map_err(|_| ActiveScanTxError::Reserve(TxError::Invalid))?;
            let mut config = HtTxConfig::single_mpdu(rate, mpdu_length, hardware_mic_length)
                .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
            debug_assert_eq!(config.length, expected_signal);
            let data_power = power_profile.pair(rate.power_lookup_code());
            let rts_rate = rate.vendor_rts_rate();
            let rts_power = power_profile.pair(rts_rate.code());
            config.data_power_primary = data_power.primary as u8;
            config.data_power_alternate = data_power.alternate as u8;
            config.rts_power_primary = rts_power.primary as u8;
            config.rts_power_alternate = rts_power.alternate as u8;
            config.protection_spacing = storage.runtime_policy.ht_ampdu().protection_spacing();
            config.scheduler_priority = scheduler_priority;
            config.pti = pti;
            config.pti_count = 1;
            config.aifsn = aifsn;
            config.contention_window = contention_window;
            config.hardware_key_selector = hardware_key_selector;
            storage
                .slot
                .as_mut()
                .submit_ht(mmio, cookie, queue, config)
                .map_err(ActiveScanTxError::Submit)?;
        }
        TxPhyRate::He(_) => unreachable!("HE was rejected before reserving the ordinary TxSlot"),
    }
    let completion_deadline = Instant::now() + Duration::from_millis(TX_COMPLETION_DEADLINE_MS);
    while Instant::now() < completion_deadline {
        if let Some(completion) = storage
            .slot
            .as_mut()
            .acknowledge_completion(mmio)
            .map_err(ActiveScanTxError::Completion)?
        {
            storage.attempts = storage.attempts.saturating_add(1);
            let diagnostic_failure = completion.status != 0
                && storage.ack_timeouts.saturating_add(storage.other_failures) < 8;
            match completion.status {
                0 => storage.successes = storage.successes.saturating_add(1),
                5 => storage.ack_timeouts = storage.ack_timeouts.saturating_add(1),
                _ => storage.other_failures = storage.other_failures.saturating_add(1),
            }
            if diagnostic_failure {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL probe=tx-failure \
                     status={} rate_kbps={} frame={} hardware={} \
                     control={:#010x} config={:#010x} plcp1={:#010x} htsig={:#010x} \
                     ppdu={:#010x} protection={:#010x} ht_counts={:#010x} \
                     data_length={:#010x} length={:#010x} \
                     power={:#010x} pti={:#010x} \
                     aux={:#010x}/{:#010x}/{:#010x} \
                     completion={:#010x}/{:#010x} alternate={}",
                    completion.status,
                    rate.nominal_kbps(),
                    frame_length,
                    hardware_frame_length,
                    mmio.read32(TX_Q_CONTROL[queue_index]),
                    mmio.read32(TX_Q_CONFIG[queue_index]),
                    mmio.read32(TX_Q_PLCP1[queue_index]),
                    mmio.read32(TX_Q_HT_SIGNAL[queue_index]),
                    mmio.read32(TX_Q_PPDU_CONTROL[queue_index]),
                    mmio.read32(TX_Q_PROTECTION[queue_index]),
                    mmio.read32(TX_Q_HT_DESCRIPTOR_COUNTS[queue_index]),
                    mmio.read32(TX_Q_DATA_LENGTH[queue_index]),
                    mmio.read32(TX_Q_LENGTH_CONTROL[queue_index]),
                    mmio.read32(TX_Q_POWER[queue_index]),
                    mmio.read32(TX_Q_PTI[queue_index]),
                    completion.auxiliary_a_word,
                    completion.auxiliary_b_word,
                    completion.auxiliary_c_word,
                    completion.primary_word,
                    completion.alternate_word,
                    completion.used_alternate,
                ));
            }
            storage
                .slot
                .as_mut()
                .detach_completed(mmio, cookie)
                .map_err(ActiveScanTxError::Detach)?;
            return Ok(completion);
        }
        if storage
            .slot
            .as_mut()
            .begin_timeout_abort(mmio, cookie)
            .map_err(ActiveScanTxError::Completion)?
        {
            storage.attempts = storage.attempts.saturating_add(1);
            storage.hardware_timeouts = storage.hardware_timeouts.saturating_add(1);
            // `migration/lmac.rs::begin_tx_timeout` owns this exact settling
            // edge before invalidating and disabling the timed-out queue.
            Timer::after_micros(16).await;
            storage
                .slot
                .as_mut()
                .finish_timeout_abort(mmio, cookie)
                .map_err(ActiveScanTxError::Detach)?;
            return Err(ActiveScanTxError::HardwareTimedOut);
        }
        Timer::after_micros(1).await;
    }
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL probe=tx-timeout control={:#010x} config={:#010x} \
         ppdu={:#010x} protection={:#010x} ht_counts={:#010x} \
         plcp1={:#010x} htsig={:#010x} \
         data_length={:#010x} length={:#010x} power={:#010x} \
         completion={:#010x} tx_state={:#010x} \
         int_raw={:#010x} int_status={:#010x} cca={:#010x} common_gate={:#010x} \
         tx_channel={:#010x} ersu_rate={:#010x} ersu={:#010x} tb={:#010x} \
         common={:#010x}/{:#010x}/{:#010x}",
        mmio.read32(TX_Q_CONTROL[queue_index]),
        mmio.read32(TX_Q_CONFIG[queue_index]),
        mmio.read32(TX_Q_PPDU_CONTROL[queue_index]),
        mmio.read32(TX_Q_PROTECTION[queue_index]),
        mmio.read32(TX_Q_HT_DESCRIPTOR_COUNTS[queue_index]),
        mmio.read32(TX_Q_PLCP1[queue_index]),
        mmio.read32(TX_Q_HT_SIGNAL[queue_index]),
        mmio.read32(TX_Q_DATA_LENGTH[queue_index]),
        mmio.read32(TX_Q_LENGTH_CONTROL[queue_index]),
        mmio.read32(TX_Q_POWER[queue_index]),
        mmio.read32(TX_COMPLETE_STATE),
        mmio.read32(TX_STATE),
        mmio.read32(MAC_INT_RAW),
        mmio.read32(MAC_INT_STATUS),
        read_diagnostic_mmio(0x2010_4c5c),
        mmio.read32(mac_registers::R_4C60),
        read_diagnostic_mmio(0x2010_4400),
        read_diagnostic_mmio(0x2010_4404),
        read_diagnostic_mmio(0x2010_4c7c),
        mmio.read32(mac_registers::R_4E04),
        read_diagnostic_mmio(0x2010_4048),
        read_diagnostic_mmio(0x2010_4110),
        mmio.read32(mac_registers::R_4C8C),
    ));
    Err(ActiveScanTxError::CompletionTimedOut)
}

async fn transmit_encoded_unicast_with_retry<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    queue: LegacyTxQueue,
    frame_length: usize,
    descriptor_capacity: usize,
    signal_override: Option<u16>,
    scheduler_priority: u8,
    pti: u8,
    rate: TxPhyRate,
    hardware_key_selector: u8,
    security_mic_length: usize,
) -> Result<TxCompletion, ActiveScanTxError> {
    let mut retry = UnicastRetryState::new(queue, rate, UNICAST_TX_ATTEMPT_LIMIT)
        .map_err(ActiveScanTxError::UnicastRetry)?;
    loop {
        let attempt = retry.attempt();
        let attempt_rate = retry
            .current_rate()
            .map_err(ActiveScanTxError::UnicastRetry)?;
        match transmit_encoded_frame(
            mmio,
            storage,
            queue,
            frame_length,
            descriptor_capacity,
            signal_override,
            scheduler_priority,
            pti,
            attempt_rate,
            hardware_key_selector,
            security_mic_length,
        )
        .await
        {
            Ok(completion) => {
                if retry.observe_completion(&mut storage.runtime_policy, completion.status)
                    == UnicastRetryDecision::Complete
                {
                    return Ok(completion);
                }
                if !OPEN_RADIO_THROUGHPUT_BENCH {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=RETRY stage=unicast-tx \
                         attempt={attempt} rate_kbps={} status={}",
                        attempt_rate.nominal_kbps(),
                        completion.status,
                    ));
                }
            }
            Err(error) => {
                let decision = if error == ActiveScanTxError::HardwareTimedOut {
                    retry.observe_hardware_timeout(&mut storage.runtime_policy)
                } else {
                    retry.abort(&mut storage.runtime_policy);
                    UnicastRetryDecision::Complete
                };
                if decision == UnicastRetryDecision::Complete {
                    return Err(error);
                }
                if !OPEN_RADIO_THROUGHPUT_BENCH {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=RETRY stage=unicast-tx \
                         attempt={attempt} rate_kbps={} error={error:?}",
                        attempt_rate.nominal_kbps(),
                    ));
                }
            }
        }

        // SOURCE: the promoted migration
        // `migration/esp32s31-hybrid-runtime/src/lmac.rs::process_tx_retry`
        // resubmits the same frame for CTS/ACK timeout, while
        // `mark_retry_scheduler` ORs 0x08 into byte one of its 802.11 header.
        // `_oracles/libpp.a[pp.o]` owns that original retry path. Reusing this
        // already encoded MPDU is essential: Sequence Control and the CCMP PN
        // must not advance for a MAC-layer retransmission.
        storage.dma_buffer_mut()[TX_METADATA_SIZE + 1] |= 0x08;
    }
}

async fn transmit_unprotected_eapol<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    eapol: &[u8],
    sequence_number: u16,
) -> Result<TxCompletion, ActiveScanTxError> {
    let frame_length = StaDataFrame {
        source: station_address,
        bssid,
        destination: bssid,
        sequence_number,
        ether_type: 0x888e,
        payload: eapol,
    }
    .encode(&mut storage.dma_buffer_mut()[TX_METADATA_SIZE..])
    .map_err(ActiveScanTxError::StationEncode)?;
    let hardware_storage_length = frame_length + TX_METADATA_SIZE + TX_FCS_SIZE;
    let descriptor_capacity = hardware_storage_length
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
    transmit_encoded_unicast_with_retry(
        mmio,
        storage,
        LegacyTxQueue::Voice,
        frame_length,
        descriptor_capacity,
        None,
        LegacyTxQueue::Voice.vendor_data_scheduler_priority(),
        LegacyTxQueue::Voice.vendor_data_packet_priority(),
        TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
        0,
        0,
    )
    .await
}

async fn transmit_eapol_message_4<M: Mmio + TxHardware>(
    mmio: &mut M,
    storage: &mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    message: &Message4,
    key_slot: &mut StaPairwiseCcmpSlot,
    sequence_number: u16,
    peer_qos: bool,
) -> Result<TxCompletion, ActiveScanTxError> {
    let ccmp_header = key_slot.next_tx_ccmp_header();
    let frame_length = StaProtectedDataFrame {
        source: station_address,
        bssid,
        destination: bssid,
        sequence_number,
        user_priority: 7,
        peer_qos,
        ccmp_header,
        ether_type: 0x888e,
        payload: message.as_bytes(),
    }
    .encode(&mut storage.dma_buffer_mut()[TX_METADATA_SIZE..])
    .map_err(ActiveScanTxError::StationEncode)?;
    let hardware_storage_length = frame_length + TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE;
    let descriptor_capacity = hardware_storage_length
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
    let signal = frame_length
        .checked_add(TX_CCMP_MIC_SIZE + TX_FCS_SIZE)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
    transmit_encoded_unicast_with_retry(
        mmio,
        storage,
        LegacyTxQueue::Voice,
        frame_length,
        descriptor_capacity,
        Some(signal),
        LegacyTxQueue::Voice.vendor_data_scheduler_priority(),
        LegacyTxQueue::Voice.vendor_data_packet_priority(),
        TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
        key_slot.hardware_index(),
        TX_CCMP_MIC_SIZE,
    )
    .await
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

fn lan_arp_probe(station_address: [u8; 6], local_ipv4: [u8; 4]) -> [u8; 42] {
    let mut ethernet = [0_u8; 42];
    ethernet[..6].fill(0xff);
    ethernet[6..12].copy_from_slice(&station_address);
    ethernet[12..14].copy_from_slice(&0x0806_u16.to_be_bytes());
    let arp = &mut ethernet[14..];
    arp[0..2].copy_from_slice(&1_u16.to_be_bytes());
    arp[2..4].copy_from_slice(&0x0800_u16.to_be_bytes());
    arp[4] = 6;
    arp[5] = 4;
    arp[6..8].copy_from_slice(&1_u16.to_be_bytes());
    arp[8..14].copy_from_slice(&station_address);
    arp[14..18].copy_from_slice(&local_ipv4);
    arp[24..28].copy_from_slice(&LAN_PROBE_IPV4);
    ethernet
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
        return Err(ActiveScanTxError::Reserve(TxError::Invalid));
    }
    let destination = ethernet[..6]
        .try_into()
        .map_err(|_| ActiveScanTxError::Reserve(TxError::Invalid))?;
    let source = ethernet[6..12]
        .try_into()
        .map_err(|_| ActiveScanTxError::Reserve(TxError::Invalid))?;
    let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
    let ccmp_header = key_slot.next_tx_ccmp_header();
    let frame_length = StaProtectedDataFrame {
        source,
        bssid,
        destination,
        sequence_number,
        user_priority: 0,
        peer_qos,
        ccmp_header,
        ether_type,
        payload: &ethernet[14..],
    }
    .encode(&mut storage.dma_buffer_mut()[TX_METADATA_SIZE..])
    .map_err(ActiveScanTxError::StationEncode)?;
    let hardware_storage_length = frame_length + TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE;
    let descriptor_capacity = hardware_storage_length
        .checked_add(3)
        .map(|length| length & !3)
        .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
    let signal = frame_length
        .checked_add(TX_CCMP_MIC_SIZE + TX_FCS_SIZE)
        .and_then(|length| u16::try_from(length).ok())
        .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
    transmit_encoded_unicast_with_retry(
        mmio,
        storage,
        LegacyTxQueue::BestEffort,
        frame_length,
        descriptor_capacity,
        Some(signal),
        LegacyTxQueue::BestEffort.vendor_data_scheduler_priority(),
        LegacyTxQueue::BestEffort.vendor_data_packet_priority(),
        data_rate,
        key_slot.hardware_index(),
        TX_CCMP_MIC_SIZE,
    )
    .await
}

fn append_protected_ethernet_ampdu_frame(
    mut storage: Pin<&mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>,
    cookie: TxCookie,
    bssid: [u8; 6],
    key_slot: &mut StaPairwiseCcmpSlot,
    sequence_number: u16,
    ethernet: &[u8],
    he_policy: Option<(HeRate, HtAmpduDensity, HeEdcaTxopLimit)>,
) -> Result<(), ActiveScanTxError> {
    if ethernet.len() < 14 {
        return Err(ActiveScanTxError::Reserve(TxError::Invalid));
    }
    let destination = ethernet[..6]
        .try_into()
        .map_err(|_| ActiveScanTxError::Reserve(TxError::Invalid))?;
    let source = ethernet[6..12]
        .try_into()
        .map_err(|_| ActiveScanTxError::Reserve(TxError::Invalid))?;
    let ether_type = u16::from_be_bytes([ethernet[12], ethernet[13]]);
    let ccmp_header = key_slot.next_tx_ccmp_header();
    let frame = StaProtectedDataFrame {
        source,
        bssid,
        destination,
        sequence_number,
        user_priority: 0,
        peer_qos: true,
        ccmp_header,
        ether_type,
        payload: &ethernet[14..],
    };
    let output = storage
        .as_mut()
        .next_frame_buffer(cookie)
        .map_err(ActiveScanTxError::Ampdu)?;
    let frame_length = if OPEN_RADIO_HE_TB_HIL && he_policy.is_some() {
        // SOURCE: complete libnet80211
        // `ieee80211_encap_esfbuf_htc` sets Frame Control Order for the
        // Trigger-eligible HE QoS/TID path. Complete libpp `hal_he_set_htc`
        // leaves the software override clear. Complete libpp
        // `pp_he.o::ppCalSubFrameLength` reads metadata byte seven bit zero
        // and adds the four hardware-inserted bytes to APEP without moving
        // CCMP in DMA. HIL_VENDOR_HE_CONTROL_INSERTION_2026_07_30 captured
        // vendor metadata word one `0x0100_0000`, CCMP immediately after QoS
        // in DMA, and an intact hardware BSR + CCMP boundary on air.
        frame.encode_with_he_control(DataHeControl::HardwareGeneratedBufferStatusReport, output)
    } else {
        frame.encode(output)
    }
    .map_err(ActiveScanTxError::StationEncode)?;
    if let Some((rate, density, txop_limit)) = he_policy {
        if OPEN_RADIO_HE_TB_HIL {
            // SOURCE: complete libpp/ROM `mac_tx_set_tb` sums frame-state
            // `msdu_len`, while this open ingress still owns the exact
            // Ethernet data unit passed to the 802.11 encoder. Treating that
            // input length as the vendor-visible MSDU length is the bounded
            // HIL hypothesis; the queue readback below proves the published
            // BSR sum before hardware takes the completion edge.
            storage
                .commit_hardware_he_control_msdu_frame_with_txop(
                    cookie,
                    frame_length,
                    TX_CCMP_MIC_SIZE as u8,
                    ethernet.len(),
                    rate,
                    density,
                    txop_limit,
                )
                .map_err(ActiveScanTxError::Ampdu)
        } else {
            storage
                .commit_he_frame_with_txop(
                    cookie,
                    frame_length,
                    TX_CCMP_MIC_SIZE as u8,
                    rate,
                    density,
                    txop_limit,
                )
                .map_err(ActiveScanTxError::Ampdu)
        }
    } else {
        storage
            .commit_frame(cookie, frame_length, TX_CCMP_MIC_SIZE as u8, 0)
            .map_err(ActiveScanTxError::Ampdu)
    }
}

fn append_protected_ethernet_amsdu_ampdu_frame(
    mut storage: Pin<&mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>,
    cookie: TxCookie,
    bssid: [u8; 6],
    key_slot: &mut StaPairwiseCcmpSlot,
    sequence_number: u16,
    first: &[u8],
    second: &[u8],
    refresh_body: bool,
    he_policy: Option<(HeRate, HtAmpduDensity, HeEdcaTxopLimit)>,
) -> Result<(), ActiveScanTxError> {
    let ethernet_frames = [first, second];
    let ccmp_header = key_slot.next_tx_ccmp_header();
    let frame = StaProtectedAmsduFrame {
        source: first
            .get(6..12)
            .and_then(|source| source.try_into().ok())
            .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?,
        bssid,
        sequence_number,
        user_priority: 0,
        ccmp_header,
        ethernet_frames: &ethernet_frames,
    };
    let output = storage
        .as_mut()
        .next_frame_buffer(cookie)
        .map_err(ActiveScanTxError::Ampdu)?;
    let frame_length = if refresh_body {
        frame.refresh_header(output)
    } else {
        frame.encode(output)
    }
    .map_err(ActiveScanTxError::StationEncode)?;
    if let Some((rate, density, txop_limit)) = he_policy {
        // The Trigger HIL deliberately excludes A-MSDU until the vendor
        // frame-state meaning for multiple MSDUs in one MPDU is recovered.
        debug_assert!(!OPEN_RADIO_HE_TB_HIL);
        storage
            .commit_he_frame_with_txop(
                cookie,
                frame_length,
                TX_CCMP_MIC_SIZE as u8,
                rate,
                density,
                txop_limit,
            )
            .map_err(ActiveScanTxError::Ampdu)
    } else {
        storage
            .commit_frame(cookie, frame_length, TX_CCMP_MIC_SIZE as u8, 0)
            .map_err(ActiveScanTxError::Ampdu)
    }
}

struct ProtectedEthernetAmpduReport {
    completion: HtAmpduTxCompletion,
    rate: TxPhyRate,
    he_vector: Option<MacHeTxVectorSnapshot>,
    he_trigger: Option<MacHeTriggerTxQueueSnapshot>,
    subframes: u8,
    ethernet_bytes: usize,
    acknowledged: u8,
    trigger_flow_completed: bool,
    retry_failures: u8,
    aggregate_attempts: u8,
    block_ack_mpdu_attempts: u16,
    individual_retry_mpdu: u8,
    spill_frames: u8,
    elapsed_us: u64,
    hardware_us: u64,
    rx_irqs_during_hardware: u32,
    rx_service_yields_during_preparation: u32,
    preparation_us: u64,
    first_empty_delimiters: u8,
}

#[inline]
async fn yield_to_pending_rx_bottom_half(rx_service_yields: &mut u32) {
    // SOURCE: complete `_oracles/libpp.a[wdev.o]::wDev_ProcessFiq`
    // services RX_SUCCESS before TX_COMPLETE, and complete
    // `_oracles/libpp.a[lmac.o]::lmacRxDone` publishes PP event 17.
    //
    // The open driver has one Embassy task instead of the vendor FIQ + PP
    // task pair. A completed MPDU encode is its smallest finite preparation
    // unit. Yield only after the ISR has published a durable RX edge; the
    // outer RX-first `select` then performs the same copy/recycle ownership
    // transition before this future may encode another MPDU.
    if OPEN_RADIO_IRQ_RUNTIME.rx_signaled() {
        *rx_service_yields = rx_service_yields.saturating_add(1);
        embassy_futures::yield_now().await;
    }
}

async fn transmit_he_dcm_smpdu_oracle<M: Mmio + TxHardware>(
    mmio: &mut M,
    tx_storage: &mut TxStorage,
    mut ampdu: Pin<&mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>,
    station_address: [u8; 6],
    bssid: [u8; 6],
    sequence_number: u16,
) -> Result<(TxCompletion, Option<MacHeTxVectorSnapshot>), ActiveScanTxError>
where
    M: open_esp_radio::esp32s31::mac::tx_ampdu::HtAmpduHardware,
{
    const RAW_FRAME_LENGTH: usize = 24;
    let cookie = ampdu.as_mut().begin().map_err(ActiveScanTxError::Ampdu)?;
    let output = ampdu
        .as_mut()
        .next_frame_buffer(cookie)
        .map_err(ActiveScanTxError::Ampdu)?;
    output[..RAW_FRAME_LENGTH].fill(0);
    output[0] = 0x08; // Non-QoS data.
    output[1] = 0x01; // To DS.
    output[4..10].copy_from_slice(&bssid);
    output[10..16].copy_from_slice(&station_address);
    output[16..22].copy_from_slice(&bssid);
    output[22..24].copy_from_slice(&(sequence_number << 4).to_le_bytes());
    ampdu
        .as_mut()
        .commit_frame(cookie, RAW_FRAME_LENGTH, 0, 0)
        .map_err(ActiveScanTxError::Ampdu)?;

    let rate = HeRate::bcc_dcm(HeBccDcmMcs::Mcs0, HeGuardIntervalAndLtf::TwoLtf800Ns);
    let mut config = HeSmpduTxConfig::new(
        rate,
        tx_storage.runtime_policy.he_bss_color(),
        RAW_FRAME_LENGTH as u16,
    )
    .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
    let power_profile = tx_storage
        .tx_power_profile
        .ok_or(ActiveScanTxError::MissingTxPowerProfile)?;
    let data_power = power_profile.pair(rate.power_lookup_code());
    let rts_power = power_profile.pair(rate.vendor_rts_rate().code());
    let dcm_power = OPEN_RADIO_HE_DCM_DATA_POWER_CODE;
    config.data_power_primary = dcm_power.unwrap_or(data_power.primary as u8);
    config.data_power_alternate = dcm_power.unwrap_or(data_power.alternate as u8);
    config.rts_power_primary = rts_power.primary as u8;
    config.rts_power_alternate = rts_power.alternate as u8;
    config.scheduler_priority = LegacyTxQueue::BestEffort.vendor_data_scheduler_priority();
    config.pti = LegacyTxQueue::BestEffort.vendor_data_packet_priority();
    config.aifsn = tx_storage
        .edca_parameters(LegacyTxQueue::BestEffort)
        .aifsn();
    config.contention_window = tx_storage.next_edca_backoff(LegacyTxQueue::BestEffort);

    while OPEN_RADIO_IRQ_RUNTIME.try_take_tx().is_some() {}
    if let Err(error) =
        ampdu
            .as_mut()
            .submit_he_smpdu(mmio, cookie, LegacyTxQueue::BestEffort, config)
    {
        let _ = ampdu.as_mut().cancel(cookie);
        tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
        return Err(ActiveScanTxError::Ampdu(error));
    }
    let vector = mmio.he_tx_vector_snapshot(LegacyTxQueue::BestEffort as u8);
    let deadline = Instant::now() + Duration::from_millis(TX_COMPLETION_DEADLINE_MS);
    let completion = loop {
        if let Some(completion) = ampdu
            .as_mut()
            .acknowledge_he_smpdu_completion(mmio)
            .map_err(ActiveScanTxError::Ampdu)?
        {
            break completion;
        }
        if ampdu
            .as_mut()
            .begin_timeout_abort(mmio, cookie)
            .map_err(ActiveScanTxError::Ampdu)?
        {
            Timer::after_micros(16).await;
            ampdu
                .as_mut()
                .finish_timeout_abort(mmio, cookie)
                .map_err(ActiveScanTxError::Ampdu)?;
            tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
            return Err(ActiveScanTxError::HardwareTimedOut);
        }
        if Instant::now() >= deadline {
            tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
            return Err(ActiveScanTxError::CompletionTimedOut);
        }
        OPEN_RADIO_IRQ_RUNTIME.wait_tx().await;
    };
    ampdu
        .as_mut()
        .detach_completed(mmio, cookie)
        .map_err(ActiveScanTxError::Ampdu)?;
    ampdu
        .as_mut()
        .release_completed(cookie)
        .map_err(ActiveScanTxError::Ampdu)?;
    if completion.status == 0 {
        tx_storage.record_edca_success(LegacyTxQueue::BestEffort);
    } else {
        tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
    }
    Ok((completion, vector))
}

async fn transmit_protected_ethernet_ampdu<M: Mmio + TxHardware>(
    mmio: &mut M,
    tx_storage: &mut TxStorage,
    mut ampdu: Pin<&mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>,
    bssid: [u8; 6],
    key_slot: &mut StaPairwiseCcmpSlot,
    sequence: &mut StaSequenceCounter,
    data_rate: TxPhyRate,
    network_runner: &NetworkRunner,
    first: &[u8],
    second: &[u8],
    synthetic_fill: Option<&[u8]>,
    reuse_synthetic_amsdu_body: bool,
    he_rate_override: Option<HeRate>,
    he_txop_limit: HeEdcaTxopLimit,
    ampdu_limit: usize,
) -> Result<ProtectedEthernetAmpduReport, ActiveScanTxError>
where
    M: open_esp_radio::esp32s31::mac::tx_ampdu::HtAmpduHardware,
{
    let transmission_started = Instant::now();
    if !(1..=TX_AMPDU_FRAME_COUNT).contains(&ampdu_limit) {
        return Err(ActiveScanTxError::Reserve(TxError::Invalid));
    }
    let he_commit_rate = match (he_rate_override, data_rate) {
        (Some(rate), _) => Some(rate),
        (None, TxPhyRate::He(rate)) => Some(rate),
        (None, _) => None,
    };
    let he_density = if OPEN_RADIO_HE_DELIMITER_HIL {
        // A larger spacing remains valid for a peer advertising a smaller
        // minimum and deterministically exercises ppCalDeliNum's nonzero
        // branch. This is a HIL-only formatter probe, not association policy.
        HtAmpduDensity::SixteenMicroseconds
    } else {
        tx_storage.runtime_policy.ht_ampdu().density()
    };
    let he_policy = he_commit_rate.map(|rate| (rate, he_density, he_txop_limit));
    let first_sequence = sequence.peek();
    let cookie = ampdu.as_mut().begin().map_err(ActiveScanTxError::Ampdu)?;
    // Until descriptors are published, every fallible formatter operation is
    // a software-owned transaction. Keep that ownership edge explicit: an
    // early return must release `Reserved`, otherwise the next caller can only
    // observe `Busy` even though hardware has never seen the pool.
    macro_rules! reserved_try {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => {
                    let _ = ampdu.as_mut().cancel(cookie);
                    return Err(error);
                }
            }
        };
    }
    let mut rx_service_yields_during_preparation = 0_u32;
    let mut ethernet_bytes: usize;
    if OPEN_RADIO_AMSDU_BENCH && synthetic_fill.is_some() {
        if let Err(error) = append_protected_ethernet_amsdu_ampdu_frame(
            ampdu.as_mut(),
            cookie,
            bssid,
            key_slot,
            sequence.take(),
            first,
            second,
            reuse_synthetic_amsdu_body,
            he_policy,
        ) {
            let _ = ampdu.as_mut().cancel(cookie);
            return Err(error);
        }
        ethernet_bytes = first.len() + second.len();
        yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
    } else {
        ethernet_bytes = 0;
        let initial_frames = if ampdu_limit == 1 {
            [Some(first), None]
        } else {
            [Some(first), Some(second)]
        };
        for ethernet in initial_frames.into_iter().flatten() {
            if let Err(error) = append_protected_ethernet_ampdu_frame(
                ampdu.as_mut(),
                cookie,
                bssid,
                key_slot,
                sequence.take(),
                ethernet,
                he_policy,
            ) {
                let _ = ampdu.as_mut().cancel(cookie);
                return Err(error);
            }
            ethernet_bytes += ethernet.len();
            yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
        }
    }
    // Give a busy network stack one short scheduling quantum to populate the
    // fixed TX queue. The synthetic raw-MAC benchmark already owns a complete
    // repeated frame and fills all 30..32 slots below, so yielding there only
    // inserts an unrelated executor bubble between consecutive PPDUs.
    if synthetic_fill.is_some() {
        if TX_AMPDU_COALESCE_US != 0 {
            // In raw mode this is an explicit inter-PPDU pacing experiment,
            // not queue coalescing: all synthetic MPDUs are already available.
            Timer::after_micros(TX_AMPDU_COALESCE_US).await;
        }
    } else {
        if TX_AMPDU_COALESCE_US != 0 {
            Timer::after_micros(TX_AMPDU_COALESCE_US).await;
        } else {
            embassy_futures::yield_now().await;
        }
    }
    let mut spill = None;
    while usize::from(ampdu.frame_count()) < ampdu_limit {
        if let Some(ethernet) = synthetic_fill {
            if OPEN_RADIO_AMSDU_BENCH {
                // Ask the IEEE 802.11 encoder for the exact wire length before
                // consuming a sequence number or CCMP PN at the negotiated
                // 64-KiB A-MPDU ceiling.
                let ethernet_frames: [&[u8]; 2] = [ethernet, ethernet];
                let frame_length = reserved_try!(
                    sta_protected_amsdu_frame_length(&ethernet_frames)
                        .map_err(ActiveScanTxError::StationEncode)
                );
                let fits = if let Some((rate, density, txop_limit)) = he_policy {
                    reserved_try!(
                        ampdu
                            .can_commit_he_frame_with_txop(
                                cookie,
                                frame_length,
                                TX_CCMP_MIC_SIZE as u8,
                                rate,
                                density,
                                txop_limit,
                            )
                            .map_err(ActiveScanTxError::Ampdu)
                    )
                } else {
                    reserved_try!(
                        ampdu
                            .can_commit_frame(cookie, frame_length, TX_CCMP_MIC_SIZE as u8, 0)
                            .map_err(ActiveScanTxError::Ampdu)
                    )
                };
                if !fits {
                    break;
                }
                if let Err(error) = append_protected_ethernet_amsdu_ampdu_frame(
                    ampdu.as_mut(),
                    cookie,
                    bssid,
                    key_slot,
                    sequence.take(),
                    ethernet,
                    ethernet,
                    reuse_synthetic_amsdu_body,
                    he_policy,
                ) {
                    let _ = ampdu.as_mut().cancel(cookie);
                    return Err(error);
                }
                ethernet_bytes += ethernet.len() * 2;
            } else {
                let frame_length = reserved_try!(
                    ethernet
                        .len()
                        .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
                        .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))
                );
                let fits = if let Some((rate, density, txop_limit)) = he_policy {
                    if OPEN_RADIO_HE_TB_HIL {
                        reserved_try!(
                            ampdu
                                .can_commit_hardware_he_control_frame_with_txop(
                                    cookie,
                                    frame_length,
                                    TX_CCMP_MIC_SIZE as u8,
                                    rate,
                                    density,
                                    txop_limit,
                                )
                                .map_err(ActiveScanTxError::Ampdu)
                        )
                    } else {
                        reserved_try!(
                            ampdu
                                .can_commit_he_frame_with_txop(
                                    cookie,
                                    frame_length,
                                    TX_CCMP_MIC_SIZE as u8,
                                    rate,
                                    density,
                                    txop_limit,
                                )
                                .map_err(ActiveScanTxError::Ampdu)
                        )
                    }
                } else {
                    reserved_try!(
                        ampdu
                            .can_commit_frame(cookie, frame_length, TX_CCMP_MIC_SIZE as u8, 0)
                            .map_err(ActiveScanTxError::Ampdu)
                    )
                };
                if !fits {
                    break;
                }
                if let Err(error) = append_protected_ethernet_ampdu_frame(
                    ampdu.as_mut(),
                    cookie,
                    bssid,
                    key_slot,
                    sequence.take(),
                    ethernet,
                    he_policy,
                ) {
                    let _ = ampdu.as_mut().cancel(cookie);
                    return Err(error);
                }
                ethernet_bytes += ethernet.len();
            }
            yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
            continue;
        }
        let Some(owned) = network_runner.try_receive_tx() else {
            break;
        };
        let frame_length = reserved_try!(
            owned
                .len()
                .checked_add(STA_PROTECTED_QOS_ETHERNET_OVERHEAD)
                .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))
        );
        let fits = if let Some((rate, density, txop_limit)) = he_policy {
            if OPEN_RADIO_HE_TB_HIL {
                reserved_try!(
                    ampdu
                        .can_commit_hardware_he_control_frame_with_txop(
                            cookie,
                            frame_length,
                            TX_CCMP_MIC_SIZE as u8,
                            rate,
                            density,
                            txop_limit,
                        )
                        .map_err(ActiveScanTxError::Ampdu)
                )
            } else {
                reserved_try!(
                    ampdu
                        .can_commit_he_frame_with_txop(
                            cookie,
                            frame_length,
                            TX_CCMP_MIC_SIZE as u8,
                            rate,
                            density,
                            txop_limit,
                        )
                        .map_err(ActiveScanTxError::Ampdu)
                )
            }
        } else {
            reserved_try!(
                ampdu
                    .can_commit_frame(cookie, frame_length, TX_CCMP_MIC_SIZE as u8, 0)
                    .map_err(ActiveScanTxError::Ampdu)
            )
        };
        if !fits {
            // This frame belongs to the next PPDU. It has already crossed the
            // embassy-net ownership boundary, so retain it locally and send
            // it after the aggregate instead of consuming a sequence/PN or
            // dropping it at the negotiated peer A-MPDU byte ceiling.
            spill = Some(owned);
            break;
        }
        if let Err(error) = append_protected_ethernet_ampdu_frame(
            ampdu.as_mut(),
            cookie,
            bssid,
            key_slot,
            sequence.take(),
            owned.as_slice(),
            he_policy,
        ) {
            let _ = ampdu.as_mut().cancel(cookie);
            return Err(error);
        }
        ethernet_bytes += owned.len();
        yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
    }
    let aggregate = reserved_try!(
        ampdu
            .prepared_aggregate(cookie)
            .map_err(ActiveScanTxError::Ampdu)
    );
    let first_empty_delimiters = reserved_try!(
        ampdu
            .prepared_empty_delimiters(cookie, 0)
            .map_err(ActiveScanTxError::Ampdu)
    );
    let power_profile = reserved_try!(
        tx_storage
            .tx_power_profile
            .ok_or(ActiveScanTxError::MissingTxPowerProfile)
    );
    // A typed matrix override is itself an explicit request for the HE
    // formatter. The former `OPEN_RADIO_FORCE_HE20`-only gate silently
    // discarded `he_rate_override`, transmitted the HT fallback, and then
    // labelled its BlockAck result with the requested HE/DCM rate. The
    // impossible observed payload rate (~20 Mbit/s while DCM MCS0 is only
    // 3.6..4.3 Mbit/s) exposed that harness error before it could be promoted
    // as hardware evidence.
    let mut config = if let Some(rate) = he_commit_rate {
        let mut config = reserved_try!(
            HeAmpduTxConfig::new_with_txop(
                rate,
                tx_storage.runtime_policy.he_bss_color(),
                aggregate.bytes,
                aggregate.subframes,
                he_density,
                he_txop_limit,
            )
            .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))
        );
        let data_power = power_profile.pair(rate.power_lookup_code());
        let rts_power = power_profile.pair(rate.vendor_rts_rate().code());
        // The only acknowledged vendor DCM oracle published power code five,
        // while the ordinary open 20-dBm profile publishes code twenty.
        // Keep the comparison as an explicit HIL-only override so a power
        // hypothesis cannot silently alter normal HE or management traffic.
        let dcm_power = if rate.is_dcm() {
            OPEN_RADIO_HE_DCM_DATA_POWER_CODE
        } else {
            None
        };
        config.data_power_primary = dcm_power.unwrap_or(data_power.primary as u8);
        config.data_power_alternate = dcm_power.unwrap_or(data_power.alternate as u8);
        config.rts_power_primary = rts_power.primary as u8;
        config.rts_power_alternate = rts_power.alternate as u8;
        config.scheduler_priority = LegacyTxQueue::BestEffort.vendor_data_scheduler_priority();
        config.pti = LegacyTxQueue::BestEffort.vendor_data_packet_priority();
        config.pti_count = 1;
        config.aifsn = tx_storage
            .edca_parameters(LegacyTxQueue::BestEffort)
            .aifsn();
        config.contention_window = tx_storage.next_edca_backoff(LegacyTxQueue::BestEffort);
        config.hardware_key_selector = key_slot.hardware_index();
        if OPEN_RADIO_HE_TB_HIL {
            let trigger = HeTriggerBasedTxConfig::new(
                MacHeTbTidLimit::default(),
                MacHeTid::new(0).expect("TID zero is representable"),
            )
            .expect("the recovered default Trigger policy admits TID zero");
            config = config.with_trigger_based(trigger);
        }
        AmpduTxConfig::He(config)
    } else {
        let TxPhyRate::Ht(rate) = data_rate else {
            let _ = ampdu.as_mut().cancel(cookie);
            return Err(ActiveScanTxError::Reserve(TxError::Invalid));
        };
        let mut config = reserved_try!(
            HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes)
                .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))
        );
        let data_power = power_profile.pair(rate.power_lookup_code());
        let rts_power = power_profile.pair(rate.vendor_rts_rate().code());
        config.data_power_primary = data_power.primary as u8;
        config.data_power_alternate = data_power.alternate as u8;
        config.rts_power_primary = rts_power.primary as u8;
        config.rts_power_alternate = rts_power.alternate as u8;
        config.protection_spacing = tx_storage.runtime_policy.ht_ampdu().protection_spacing();
        config.scheduler_priority = LegacyTxQueue::BestEffort.vendor_data_scheduler_priority();
        config.pti = LegacyTxQueue::BestEffort.vendor_data_packet_priority();
        config.pti_count = 1;
        config.aifsn = tx_storage
            .edca_parameters(LegacyTxQueue::BestEffort)
            .aifsn();
        config.contention_window = tx_storage.next_edca_backoff(LegacyTxQueue::BestEffort);
        config.hardware_key_selector = key_slot.hardware_index();
        AmpduTxConfig::Ht(config)
    };
    let selected_ampdu_rate = config.rate();
    // SOURCE: `_oracles/libpp.a[hal_mac.o]::mac_tx_set_pti` loads
    // descriptor +0x22 once and passes it to
    // `_oracles/libpp.a[hal_coex.o]::hal_set_tx_pti`; that leaf publishes it
    // in PTI bits 31:20. The actual MPDU count is independently written by
    // `mac_tx_set_mplen`. The vendor HT HIL vector used PTI count one.
    let original_subframes = aggregate.subframes;
    let mut retry_state = AmpduRetryState::<TX_AMPDU_FRAME_COUNT>::new(
        first_sequence,
        original_subframes,
        AmpduRetryPolicy {
            attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
            retain_single_mpdu: matches!(config, AmpduTxConfig::He(_)),
        },
    )
    .map_err(ActiveScanTxError::AmpduRetry)?;
    let mut hardware_wait_us = 0_u64;
    let mut rx_irqs_during_hardware = 0_u32;
    let mut first_he_vector = None;
    let mut first_he_trigger = None;
    let preparation_us = transmission_started.elapsed().as_micros();
    loop {
        // Discard an edge retained by `Signal` from the preceding one-owner
        // transmission before publishing this descriptor chain. Once the
        // hardware edge is armed, the ISR-to-Signal handoff cannot lose a TX
        // completion even when it arrives before `wait()` registers its
        // waker.
        while OPEN_RADIO_IRQ_RUNTIME.try_take_tx().is_some() {}
        let submit = match config {
            AmpduTxConfig::Ht(config) => {
                ampdu
                    .as_mut()
                    .submit(mmio, cookie, LegacyTxQueue::BestEffort, config)
            }
            AmpduTxConfig::He(config) => {
                ampdu
                    .as_mut()
                    .submit_he(mmio, cookie, LegacyTxQueue::BestEffort, config)
            }
        };
        if let Err(error) = submit {
            let _ = ampdu.as_mut().cancel(cookie);
            tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
            return Err(ActiveScanTxError::Ampdu(error));
        }
        // Copy the actual PAC-backed queue vector at the publication boundary.
        // Reading it after the full report is misleading when failed A-MPDU
        // members take the legacy/HT individual-retry path: that path replaces
        // PLCP1 while the previous HE-SIG words remain visible. Keep this
        // bounded snapshot in ordinary Rust data and log it only after
        // hardware ownership has ended.
        if first_he_vector.is_none() && matches!(config, AmpduTxConfig::He(_)) {
            first_he_vector = mmio.he_tx_vector_snapshot(LegacyTxQueue::BestEffort as u8);
        }
        if first_he_trigger.is_none() && OPEN_RADIO_HE_TB_HIL {
            first_he_trigger = ampdu
                .as_ref()
                .he_trigger_based_snapshot(mmio, cookie)
                .map_err(ActiveScanTxError::Ampdu)?;
        }
        let hardware_started = Instant::now();
        let rx_irqs_before = OPEN_RADIO_IRQ_RUNTIME.rx_post_count();
        let deadline = hardware_started + Duration::from_millis(TX_COMPLETION_DEADLINE_MS);
        let completion = loop {
            if let Some(completion) = ampdu
                .as_mut()
                .acknowledge_completion(mmio)
                .map_err(ActiveScanTxError::Ampdu)?
            {
                break completion;
            }
            if ampdu
                .as_mut()
                .begin_timeout_abort(mmio, cookie)
                .map_err(ActiveScanTxError::Ampdu)?
            {
                tx_storage.attempts = tx_storage.attempts.saturating_add(1);
                tx_storage.hardware_timeouts = tx_storage.hardware_timeouts.saturating_add(1);
                Timer::after_micros(16).await;
                ampdu
                    .as_mut()
                    .finish_timeout_abort(mmio, cookie)
                    .map_err(ActiveScanTxError::Ampdu)?;
                tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
                return Err(ActiveScanTxError::HardwareTimedOut);
            }
            if Instant::now() >= deadline {
                tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
                return Err(ActiveScanTxError::CompletionTimedOut);
            }
            OPEN_RADIO_IRQ_RUNTIME.wait_tx().await;
        };
        let attempt_hardware_us = hardware_started.elapsed().as_micros();
        hardware_wait_us = hardware_wait_us.saturating_add(attempt_hardware_us);
        rx_irqs_during_hardware = rx_irqs_during_hardware.saturating_add(
            OPEN_RADIO_IRQ_RUNTIME
                .rx_post_count()
                .wrapping_sub(rx_irqs_before),
        );

        tx_storage.attempts = tx_storage.attempts.saturating_add(1);
        if completion.tx.status == 0 {
            tx_storage.successes = tx_storage.successes.saturating_add(1);
            tx_storage.ampdu_success_wait_us = tx_storage
                .ampdu_success_wait_us
                .saturating_add(attempt_hardware_us);
            tx_storage.ampdu_success_wait_samples =
                tx_storage.ampdu_success_wait_samples.saturating_add(1);
        } else if completion.tx.status == 5 {
            tx_storage.ack_timeouts = tx_storage.ack_timeouts.saturating_add(1);
            tx_storage.ampdu_status5_wait_us = tx_storage
                .ampdu_status5_wait_us
                .saturating_add(attempt_hardware_us);
            tx_storage.ampdu_status5_wait_samples =
                tx_storage.ampdu_status5_wait_samples.saturating_add(1);
        } else {
            tx_storage.other_failures = tx_storage.other_failures.saturating_add(1);
            tx_storage.ampdu_other_wait_us = tx_storage
                .ampdu_other_wait_us
                .saturating_add(attempt_hardware_us);
            tx_storage.ampdu_other_wait_samples =
                tx_storage.ampdu_other_wait_samples.saturating_add(1);
        }
        ampdu
            .as_mut()
            .detach_completed(mmio, cookie)
            .map_err(ActiveScanTxError::Ampdu)?;

        let current_subframes = ampdu.frame_count();
        let retry_decision = retry_state
            .observe(completion, current_subframes)
            .map_err(ActiveScanTxError::AmpduRetry)?;
        let retry_mask = retry_decision.retry_mask();
        let missing = retry_decision.missing();

        if let AmpduRetryDecision::RetainAggregate { retry_mask } = retry_decision {
            // A retained A-MPDU retry does not walk the ordinary MPDU retry
            // ladder. Complete `_oracles/libpp.a[lmac.o]::
            // lmacRetryTxFrame` tests state byte `+0x12` against four and
            // branches around its `rcGetRate` call. Complete
            // `lmacProcessLongRetryFail` writes exactly four immediately
            // before calling that retry leaf for an aggregate failure.
            //
            // Lowering the rate here was an open-port error: a full MCS9
            // aggregate admitted by its 50,000-byte HE APEP ceiling could
            // then exceed the smaller fallback-rate ceiling, making
            // `he_ampdu_q0_image` reject an otherwise intact DMA chain.
            let retry_aggregate = ampdu
                .as_mut()
                .retain_for_ampdu_retry(cookie, retry_mask)
                .map_err(ActiveScanTxError::Ampdu)?;
            tx_storage.record_edca_retry_failure(LegacyTxQueue::BestEffort);
            config.update_retained_retry(
                retry_aggregate.bytes,
                retry_aggregate.subframes,
                tx_storage.next_edca_backoff(LegacyTxQueue::BestEffort),
            );
            continue;
        }

        if missing == 0 {
            tx_storage.record_edca_success(LegacyTxQueue::BestEffort);
        } else {
            tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
        }
        let mut retry_failures = 0_u8;
        for index in 0..current_subframes {
            if retry_mask & (1_u32 << index) == 0 {
                continue;
            }
            let (frame_length, hardware_mic_length) = {
                let completed = ampdu.as_ref();
                let (frame, hardware_mic_length) = match completed.completed_frame(cookie, index) {
                    Ok(frame) => frame,
                    Err(error) => {
                        let _ = ampdu.as_mut().release_completed(cookie);
                        return Err(ActiveScanTxError::Ampdu(error));
                    }
                };
                let frame_length = frame.len();
                tx_storage.dma_buffer_mut()[TX_METADATA_SIZE..TX_METADATA_SIZE + frame_length]
                    .copy_from_slice(frame);
                (frame_length, usize::from(hardware_mic_length))
            };
            // SOURCE: `_oracles/libpp.a[pp.o]::ppResortTxAMPDU` preserves
            // the encoded MPDU and marks only Frame Control.Retry for a
            // missing BlockAck bit. Sequence Control and CCMP PN remain.
            tx_storage.dma_buffer_mut()[TX_METADATA_SIZE + 1] |= 0x08;
            let descriptor_capacity = TX_METADATA_SIZE
                .checked_add(frame_length)
                .and_then(|length| length.checked_add(hardware_mic_length))
                .and_then(|length| length.checked_add(TX_FCS_SIZE))
                .and_then(|length| length.checked_add(3))
                .map(|length| length & !3);
            let Some(descriptor_capacity) = descriptor_capacity else {
                let _ = ampdu.as_mut().release_completed(cookie);
                return Err(ActiveScanTxError::Reserve(TxError::Invalid));
            };
            match transmit_encoded_unicast_with_retry(
                mmio,
                tx_storage,
                LegacyTxQueue::BestEffort,
                frame_length,
                descriptor_capacity,
                None,
                LegacyTxQueue::BestEffort.vendor_data_scheduler_priority(),
                LegacyTxQueue::BestEffort.vendor_data_packet_priority(),
                selected_ampdu_rate,
                config.hardware_key_selector(),
                hardware_mic_length,
            )
            .await
            {
                Ok(retry) if retry.status == 0 => {}
                Ok(_) | Err(_) => retry_failures = retry_failures.saturating_add(1),
            }
        }
        ampdu
            .as_mut()
            .release_completed(cookie)
            .map_err(ActiveScanTxError::Ampdu)?;
        let spill_frames = u8::from(spill.is_some());
        if let Some(spill) = spill {
            match transmit_protected_ethernet_frame(
                mmio,
                tx_storage,
                bssid,
                key_slot,
                sequence.take(),
                true,
                data_rate,
                spill.as_slice(),
            )
            .await
            {
                Ok(spill_completion) if spill_completion.status == 0 => {}
                Ok(_) | Err(_) => retry_failures = retry_failures.saturating_add(1),
            }
        }
        return Ok(ProtectedEthernetAmpduReport {
            completion,
            rate: selected_ampdu_rate,
            he_vector: first_he_vector,
            he_trigger: first_he_trigger,
            subframes: original_subframes,
            ethernet_bytes,
            acknowledged: retry_state.acknowledged(),
            trigger_flow_completed: retry_state.trigger_flow_completions() != 0,
            retry_failures,
            aggregate_attempts: retry_state.aggregate_attempts(),
            block_ack_mpdu_attempts: retry_state.block_ack_mpdu_attempts(),
            individual_retry_mpdu: retry_mask.count_ones() as u8,
            spill_frames,
            elapsed_us: transmission_started.elapsed().as_micros(),
            hardware_us: hardware_wait_us,
            rx_irqs_during_hardware,
            rx_service_yields_during_preparation,
            preparation_us,
            first_empty_delimiters,
        });
    }
}

fn referenced_ampdu_error(error: ReferencedHtAmpduError) -> ActiveScanTxError {
    match error {
        ReferencedHtAmpduError::Frame(error) => ActiveScanTxError::StationEncode(error),
        ReferencedHtAmpduError::Tx(error) => ActiveScanTxError::Ampdu(error),
        ReferencedHtAmpduError::BatchFull | ReferencedHtAmpduError::DmaPrefixGeometry { .. } => {
            ActiveScanTxError::Reserve(TxError::Invalid)
        }
    }
}

/// Transmit queued `embassy-net` frames through the cache-TX ownership path.
///
/// The network stack, IEEE 802.11 encoder and S31 DMA descriptor all refer to
/// one permanently located allocation. For an ordinary MPDU, the batch owns
/// that network lease until completion, queue detach, BlockAck processing and
/// retry are complete. For A-MSDU, the vendor-proven coalescing step copies the
/// second MSDU into the first allocation and immediately releases the second;
/// A-MPDU/DMA then retains that first allocation without another payload copy.
///
/// SOURCE: complete `_oracles/libnet80211.a[ieee80211_output.o]::
/// ieee80211_alloc_tx_buf` cache-TX/type-nine path retains the netstack buffer
/// with `s_netstack_ref`; complete `_oracles/libpp.a[pp.o]::
/// ppAssembleAMPDU` links those existing ESF descriptors, and
/// `ppResortTxAMPDU` retains the missing buffers across BlockAck retry.
async fn transmit_referenced_protected_ethernet_ampdu<M: Mmio + TxHardware>(
    mmio: &mut M,
    tx_storage: &mut TxStorage,
    ampdu: Pin<&mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>,
    bssid: [u8; 6],
    key_slot: &mut StaPairwiseCcmpSlot,
    sequence: &mut StaSequenceCounter,
    data_rate: TxPhyRate,
    network_runner: &NetworkRunner,
    first: NetworkTxFrame,
    second: Option<NetworkTxFrame>,
    ampdu_limit: usize,
    use_amsdu: bool,
    he_txop_limit: HeEdcaTxopLimit,
) -> Result<ProtectedEthernetAmpduReport, ActiveScanTxError>
where
    M: open_esp_radio::esp32s31::mac::tx_ampdu::HtAmpduHardware,
{
    let transmission_started = Instant::now();
    if !(1..=TX_AMPDU_FRAME_COUNT).contains(&ampdu_limit) {
        return Err(ActiveScanTxError::Reserve(TxError::Invalid));
    }
    let (ht_rate, he_rate) = match data_rate {
        TxPhyRate::Ht(rate) => (Some(rate), None),
        TxPhyRate::He(rate) => (None, Some(rate)),
        TxPhyRate::Legacy(_) => {
            return Err(ActiveScanTxError::Reserve(TxError::Invalid));
        }
    };
    if (he_rate.is_some() && use_amsdu) || (ht_rate.is_some() && second.is_none()) {
        return Err(ActiveScanTxError::Reserve(TxError::Invalid));
    }
    let he_density = tx_storage.runtime_policy.ht_ampdu().density();
    let first_sequence = sequence.peek();
    let mut batch = ReferencedHtAmpduBatch::begin(ampdu).map_err(ActiveScanTxError::Ampdu)?;
    let mut ethernet_bytes = 0_usize;
    let mut rx_service_yields_during_preparation = 0_u32;
    let mut spill = None;
    let mut second_spill = None;
    let mut amsdu_refill_bursts = 0_u8;

    if use_amsdu {
        let rate = ht_rate.expect("A-MSDU is restricted to the HT path");
        let first_length = first.len();
        let second = second.expect("HT A-MSDU was validated with two frames");
        let second_length = second.len();
        if !batch
            .can_push_ht_amsdu_pair(first_length, second_length, TX_CCMP_MIC_SIZE as u8, 0, rate)
            .map_err(referenced_ampdu_error)?
        {
            spill = Some(first);
            second_spill = Some(second);
        } else {
            batch
                .push_ht_amsdu_pair(
                    first,
                    second,
                    open_esp_radio::ieee80211::station::StaProtectedEthernetFrame {
                        bssid,
                        sequence_number: sequence.take(),
                        user_priority: 0,
                        peer_qos: true,
                        ccmp_header: key_slot.next_tx_ccmp_header(),
                    },
                    TX_CCMP_MIC_SIZE as u8,
                    0,
                    rate,
                )
                .map_err(referenced_ampdu_error)?;
            ethernet_bytes += first_length + second_length;
            yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
        }
    } else {
        for frame in [Some(first), second].into_iter().flatten() {
            let frame_length = frame.len();
            let fits = match (ht_rate, he_rate) {
                (Some(rate), None) => batch
                    .can_push_ht(frame_length, TX_CCMP_MIC_SIZE as u8, 0, rate)
                    .map_err(ActiveScanTxError::Ampdu)?,
                (None, Some(rate)) => batch
                    .can_push_he(
                        frame_length,
                        TX_CCMP_MIC_SIZE as u8,
                        rate,
                        he_density,
                        he_txop_limit,
                    )
                    .map_err(ActiveScanTxError::Ampdu)?,
                _ => unreachable!("one aggregate PHY format was selected"),
            };
            if !fits {
                spill = Some(frame);
                break;
            }
            let metadata = open_esp_radio::ieee80211::station::StaProtectedEthernetFrame {
                bssid,
                sequence_number: sequence.take(),
                user_priority: 0,
                peer_qos: true,
                ccmp_header: key_slot.next_tx_ccmp_header(),
            };
            match (ht_rate, he_rate) {
                (Some(rate), None) => batch
                    .push_ht(frame, metadata, TX_CCMP_MIC_SIZE as u8, 0, rate)
                    .map_err(referenced_ampdu_error)?,
                (None, Some(rate)) => batch
                    .push_he(
                        frame,
                        metadata,
                        TX_CCMP_MIC_SIZE as u8,
                        rate,
                        he_density,
                        he_txop_limit,
                    )
                    .map_err(referenced_ampdu_error)?,
                _ => unreachable!("one aggregate PHY format was selected"),
            };
            ethernet_bytes += frame_length;
            yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
        }
    }

    if !use_amsdu && usize::from(batch.frame_count()) + network_runner.tx_queue_len() < ampdu_limit
    {
        // A full cache queue is already the vendor `s_tx_cacheq` admission
        // condition: consume it immediately. Yield only when a sparse queue
        // can still grow this PPDU; an unconditional executor edge here
        // serialized an already prepared next aggregate after every
        // BlockAck.
        //
        // SOURCE: complete `_oracles/libnet80211.a
        // [ieee80211_output.o]::ieee80211_encap_amsdu` traverses only the
        // queue image captured under `g_wifi_global_lock`; complete
        // `_oracles/libpp.a[pp.o]::ppAssembleAMPDU` contains no wait.
        if TX_AMPDU_COALESCE_US != 0 {
            Timer::after_micros(TX_AMPDU_COALESCE_US).await;
        } else {
            embassy_futures::yield_now().await;
        }
    }
    while usize::from(batch.frame_count()) < ampdu_limit && spill.is_none() {
        if use_amsdu {
            let rate = ht_rate.expect("A-MSDU is restricted to the HT path");
            // The bounded Embassy channel has no non-consuming `peek`.
            // Check the largest legal pair before claiming either lease; if
            // that pair cannot fit, leave the next Ethernet frame queued for
            // the next PPDU. Without this guard, reaching the 65,535-byte
            // APEP ceiling consumed two more leases and forced two slow
            // single-MPDU fallback transmissions after every aggregate.
            if !batch
                .can_push_ht_amsdu_pair(
                    NETWORK_FRAME_CAPACITY,
                    NETWORK_FRAME_CAPACITY,
                    TX_CCMP_MIC_SIZE as u8,
                    0,
                    rate,
                )
                .map_err(referenced_ampdu_error)?
            {
                break;
            }
            if network_runner.tx_queue_len() < 2 {
                if amsdu_refill_bursts >= TX_AMSDU_REFILL_BURST_LIMIT {
                    break;
                }
                // Wait only at a drained ready-queue boundary. Copying the
                // second half of each preceding A-MSDU has returned a burst
                // of cache slots, so one producer poll can publish another
                // socket-sized burst. Repeat this finite edge instead of
                // selecting the first arriving MSDU: the latter woke the
                // radio once per packet and added several milliseconds.
                //
                // SOURCE: complete `_oracles/libnet80211.a
                // [ieee80211_output.o]::ieee80211_encap_amsdu`, branches
                // `.L940`/`.L950`, recycles every copied source ESF before
                // returning; complete `_oracles/libpp.a[pp.o]::
                // ppAssembleAMPDU` walks the retained first-ESF chain.
                amsdu_refill_bursts += 1;
                if TX_AMPDU_COALESCE_US == 0 {
                    embassy_futures::yield_now().await;
                } else {
                    Timer::after_micros(TX_AMPDU_COALESCE_US).await;
                }
                // There is only one ready-queue consumer. Leaving a lone
                // frame queued preserves it for the next PPDU without
                // claiming a lease that cannot yet form an A-MSDU pair.
                if network_runner.tx_queue_len() < 2 {
                    continue;
                }
            }
            let Some(first) = network_runner.try_receive_tx() else {
                break;
            };
            let Some(second) = network_runner.try_receive_tx() else {
                // Sparse traffic must not leave the claimed first lease
                // indefinitely inside an incomplete A-MSDU pair. The one
                // bounded burst-refill budget expired, so preserve the
                // ordinary MPDU fallback.
                spill = Some(first);
                break;
            };
            let first_length = first.len();
            let second_length = second.len();
            if !batch
                .can_push_ht_amsdu_pair(
                    first_length,
                    second_length,
                    TX_CCMP_MIC_SIZE as u8,
                    0,
                    rate,
                )
                .map_err(referenced_ampdu_error)?
            {
                spill = Some(first);
                second_spill = Some(second);
                break;
            }
            batch
                .push_ht_amsdu_pair(
                    first,
                    second,
                    open_esp_radio::ieee80211::station::StaProtectedEthernetFrame {
                        bssid,
                        sequence_number: sequence.take(),
                        user_priority: 0,
                        peer_qos: true,
                        ccmp_header: key_slot.next_tx_ccmp_header(),
                    },
                    TX_CCMP_MIC_SIZE as u8,
                    0,
                    rate,
                )
                .map_err(referenced_ampdu_error)?;
            ethernet_bytes += first_length + second_length;
            yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
        } else {
            let maximum_fits = match (ht_rate, he_rate) {
                (Some(rate), None) => batch
                    .can_push_ht(NETWORK_FRAME_CAPACITY, TX_CCMP_MIC_SIZE as u8, 0, rate)
                    .map_err(ActiveScanTxError::Ampdu)?,
                (None, Some(rate)) => batch
                    .can_push_he(
                        NETWORK_FRAME_CAPACITY,
                        TX_CCMP_MIC_SIZE as u8,
                        rate,
                        he_density,
                        he_txop_limit,
                    )
                    .map_err(ActiveScanTxError::Ampdu)?,
                _ => unreachable!("one aggregate PHY format was selected"),
            };
            if !maximum_fits {
                break;
            }
            let Some(frame) = network_runner.try_receive_tx() else {
                break;
            };
            let frame_length = frame.len();
            let fits = match (ht_rate, he_rate) {
                (Some(rate), None) => batch
                    .can_push_ht(frame_length, TX_CCMP_MIC_SIZE as u8, 0, rate)
                    .map_err(ActiveScanTxError::Ampdu)?,
                (None, Some(rate)) => batch
                    .can_push_he(
                        frame_length,
                        TX_CCMP_MIC_SIZE as u8,
                        rate,
                        he_density,
                        he_txop_limit,
                    )
                    .map_err(ActiveScanTxError::Ampdu)?,
                _ => unreachable!("one aggregate PHY format was selected"),
            };
            if !fits {
                spill = Some(frame);
                break;
            }
            let metadata = open_esp_radio::ieee80211::station::StaProtectedEthernetFrame {
                bssid,
                sequence_number: sequence.take(),
                user_priority: 0,
                peer_qos: true,
                ccmp_header: key_slot.next_tx_ccmp_header(),
            };
            match (ht_rate, he_rate) {
                (Some(rate), None) => batch
                    .push_ht(frame, metadata, TX_CCMP_MIC_SIZE as u8, 0, rate)
                    .map_err(referenced_ampdu_error)?,
                (None, Some(rate)) => batch
                    .push_he(
                        frame,
                        metadata,
                        TX_CCMP_MIC_SIZE as u8,
                        rate,
                        he_density,
                        he_txop_limit,
                    )
                    .map_err(referenced_ampdu_error)?,
                _ => unreachable!("one aggregate PHY format was selected"),
            };
            ethernet_bytes += frame_length;
            yield_to_pending_rx_bottom_half(&mut rx_service_yields_during_preparation).await;
        }
    }

    let aggregate = batch
        .prepared_aggregate()
        .map_err(ActiveScanTxError::Ampdu)?;
    let first_empty_delimiters = batch
        .prepared_empty_delimiters(0)
        .map_err(ActiveScanTxError::Ampdu)?;
    let power_profile = tx_storage
        .tx_power_profile
        .ok_or(ActiveScanTxError::MissingTxPowerProfile)?;
    let mut config = match (ht_rate, he_rate) {
        (Some(rate), None) => {
            let mut config = HtAmpduTxConfig::new(rate, aggregate.bytes, aggregate.subframes)
                .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
            let data_power = power_profile.pair(rate.power_lookup_code());
            let rts_power = power_profile.pair(rate.vendor_rts_rate().code());
            config.data_power_primary = data_power.primary as u8;
            config.data_power_alternate = data_power.alternate as u8;
            config.rts_power_primary = rts_power.primary as u8;
            config.rts_power_alternate = rts_power.alternate as u8;
            config.protection_spacing = tx_storage.runtime_policy.ht_ampdu().protection_spacing();
            config.scheduler_priority = LegacyTxQueue::BestEffort.vendor_data_scheduler_priority();
            config.pti = LegacyTxQueue::BestEffort.vendor_data_packet_priority();
            config.pti_count = 1;
            config.aifsn = tx_storage
                .edca_parameters(LegacyTxQueue::BestEffort)
                .aifsn();
            config.contention_window = tx_storage.next_edca_backoff(LegacyTxQueue::BestEffort);
            config.hardware_key_selector = key_slot.hardware_index();
            AmpduTxConfig::Ht(config)
        }
        (None, Some(rate)) => {
            let mut config = HeAmpduTxConfig::new_with_txop(
                rate,
                tx_storage.runtime_policy.he_bss_color(),
                aggregate.bytes,
                aggregate.subframes,
                he_density,
                he_txop_limit,
            )
            .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
            let data_power = power_profile.pair(rate.power_lookup_code());
            let rts_power = power_profile.pair(rate.vendor_rts_rate().code());
            config.data_power_primary = data_power.primary as u8;
            config.data_power_alternate = data_power.alternate as u8;
            config.rts_power_primary = rts_power.primary as u8;
            config.rts_power_alternate = rts_power.alternate as u8;
            config.scheduler_priority = LegacyTxQueue::BestEffort.vendor_data_scheduler_priority();
            config.pti = LegacyTxQueue::BestEffort.vendor_data_packet_priority();
            config.pti_count = 1;
            config.aifsn = tx_storage
                .edca_parameters(LegacyTxQueue::BestEffort)
                .aifsn();
            config.contention_window = tx_storage.next_edca_backoff(LegacyTxQueue::BestEffort);
            config.hardware_key_selector = key_slot.hardware_index();
            AmpduTxConfig::He(config)
        }
        _ => unreachable!("one aggregate PHY format was selected"),
    };

    let original_subframes = aggregate.subframes;
    let mut retry_state = AmpduRetryState::<TX_AMPDU_FRAME_COUNT>::new(
        first_sequence,
        original_subframes,
        AmpduRetryPolicy {
            attempt_limit: UNICAST_TX_ATTEMPT_LIMIT,
            retain_single_mpdu: he_rate.is_some(),
        },
    )
    .map_err(ActiveScanTxError::AmpduRetry)?;
    let mut hardware_wait_us = 0_u64;
    let mut rx_irqs_during_hardware = 0_u32;
    let mut first_he_vector = None;
    let preparation_us = transmission_started.elapsed().as_micros();

    loop {
        while OPEN_RADIO_IRQ_RUNTIME.try_take_tx().is_some() {}
        let submit = match config {
            AmpduTxConfig::Ht(config) => batch.submit(mmio, LegacyTxQueue::BestEffort, config),
            AmpduTxConfig::He(config) => batch.submit_he(mmio, LegacyTxQueue::BestEffort, config),
        };
        if let Err(error) = submit {
            tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
            return Err(ActiveScanTxError::Ampdu(error));
        }
        if first_he_vector.is_none() && he_rate.is_some() {
            first_he_vector = mmio.he_tx_vector_snapshot(LegacyTxQueue::BestEffort as u8);
        }
        let hardware_started = Instant::now();
        let rx_irqs_before = OPEN_RADIO_IRQ_RUNTIME.rx_post_count();
        let deadline = hardware_started + Duration::from_millis(TX_COMPLETION_DEADLINE_MS);
        let completion = loop {
            if let Some(completion) = batch
                .acknowledge_completion(mmio)
                .map_err(ActiveScanTxError::Ampdu)?
            {
                break completion;
            }
            if batch
                .begin_timeout_abort(mmio)
                .map_err(ActiveScanTxError::Ampdu)?
            {
                tx_storage.attempts = tx_storage.attempts.saturating_add(1);
                tx_storage.hardware_timeouts = tx_storage.hardware_timeouts.saturating_add(1);
                Timer::after_micros(16).await;
                batch
                    .finish_timeout_abort(mmio)
                    .map_err(ActiveScanTxError::Ampdu)?;
                tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
                return Err(ActiveScanTxError::HardwareTimedOut);
            }
            if Instant::now() >= deadline {
                tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
                return Err(ActiveScanTxError::CompletionTimedOut);
            }
            OPEN_RADIO_IRQ_RUNTIME.wait_tx().await;
        };
        let attempt_hardware_us = hardware_started.elapsed().as_micros();
        hardware_wait_us = hardware_wait_us.saturating_add(attempt_hardware_us);
        rx_irqs_during_hardware = rx_irqs_during_hardware.saturating_add(
            OPEN_RADIO_IRQ_RUNTIME
                .rx_post_count()
                .wrapping_sub(rx_irqs_before),
        );
        tx_storage.attempts = tx_storage.attempts.saturating_add(1);
        if completion.tx.status == 0 {
            tx_storage.successes = tx_storage.successes.saturating_add(1);
            tx_storage.ampdu_success_wait_us = tx_storage
                .ampdu_success_wait_us
                .saturating_add(attempt_hardware_us);
            tx_storage.ampdu_success_wait_samples =
                tx_storage.ampdu_success_wait_samples.saturating_add(1);
        } else if completion.tx.status == 5 {
            tx_storage.ack_timeouts = tx_storage.ack_timeouts.saturating_add(1);
            tx_storage.ampdu_status5_wait_us = tx_storage
                .ampdu_status5_wait_us
                .saturating_add(attempt_hardware_us);
            tx_storage.ampdu_status5_wait_samples =
                tx_storage.ampdu_status5_wait_samples.saturating_add(1);
        } else {
            tx_storage.other_failures = tx_storage.other_failures.saturating_add(1);
            tx_storage.ampdu_other_wait_us = tx_storage
                .ampdu_other_wait_us
                .saturating_add(attempt_hardware_us);
            tx_storage.ampdu_other_wait_samples =
                tx_storage.ampdu_other_wait_samples.saturating_add(1);
        }
        batch
            .detach_completed(mmio)
            .map_err(ActiveScanTxError::Ampdu)?;

        let current_subframes = batch.frame_count();
        let retry_decision = retry_state
            .observe(completion, current_subframes)
            .map_err(ActiveScanTxError::AmpduRetry)?;
        let retry_mask = retry_decision.retry_mask();
        let missing = retry_decision.missing();
        if let AmpduRetryDecision::RetainAggregate { retry_mask } = retry_decision {
            let retry_aggregate = batch
                .retain_for_ampdu_retry(retry_mask)
                .map_err(ActiveScanTxError::Ampdu)?;
            tx_storage.record_edca_retry_failure(LegacyTxQueue::BestEffort);
            config.update_retained_retry(
                retry_aggregate.bytes,
                retry_aggregate.subframes,
                tx_storage.next_edca_backoff(LegacyTxQueue::BestEffort),
            );
            continue;
        }

        if missing == 0 {
            tx_storage.record_edca_success(LegacyTxQueue::BestEffort);
        } else {
            tx_storage.reset_terminal_edca_exchange(LegacyTxQueue::BestEffort);
        }
        let mut retry_failures = 0_u8;
        let individual_retry_mpdu = retry_mask.count_ones() as u8;
        for index in 0..current_subframes {
            if retry_mask & (1_u32 << index) == 0 {
                continue;
            }
            let (frame_length, hardware_mic_length) = {
                let (frame, hardware_mic_length) = batch
                    .completed_frame(index)
                    .map_err(ActiveScanTxError::Ampdu)?;
                let frame_length = frame.len();
                tx_storage.dma_buffer_mut()[TX_METADATA_SIZE..TX_METADATA_SIZE + frame_length]
                    .copy_from_slice(frame);
                (frame_length, usize::from(hardware_mic_length))
            };
            tx_storage.dma_buffer_mut()[TX_METADATA_SIZE + 1] |= 0x08;
            let descriptor_capacity = TX_METADATA_SIZE
                .checked_add(frame_length)
                .and_then(|length| length.checked_add(hardware_mic_length))
                .and_then(|length| length.checked_add(TX_FCS_SIZE))
                .and_then(|length| length.checked_add(3))
                .map(|length| length & !3)
                .ok_or(ActiveScanTxError::Reserve(TxError::Invalid))?;
            match transmit_encoded_unicast_with_retry(
                mmio,
                tx_storage,
                LegacyTxQueue::BestEffort,
                frame_length,
                descriptor_capacity,
                None,
                LegacyTxQueue::BestEffort.vendor_data_scheduler_priority(),
                LegacyTxQueue::BestEffort.vendor_data_packet_priority(),
                data_rate,
                key_slot.hardware_index(),
                hardware_mic_length,
            )
            .await
            {
                Ok(retry) if retry.status == 0 => {}
                Ok(_) | Err(_) => retry_failures = retry_failures.saturating_add(1),
            }
        }
        batch
            .release_completed()
            .map_err(ActiveScanTxError::Ampdu)?;
        let spill_frames = u8::from(spill.is_some()) + u8::from(second_spill.is_some());
        // HE claims every frame after the exact maximum-capacity APEP check,
        // so it cannot own a spill here. Do not silently change a fixed HE
        // policy into legacy OFDM if that invariant regresses: the ordinary
        // single-descriptor owner deliberately rejects HE.
        debug_assert!(he_rate.is_none() || spill_frames == 0);
        for spill in [spill, second_spill].into_iter().flatten() {
            match transmit_protected_ethernet_frame(
                mmio,
                tx_storage,
                bssid,
                key_slot,
                sequence.take(),
                true,
                data_rate,
                spill.as_slice(),
            )
            .await
            {
                Ok(spill_completion) if spill_completion.status == 0 => {}
                Ok(_) | Err(_) => retry_failures = retry_failures.saturating_add(1),
            }
        }
        return Ok(ProtectedEthernetAmpduReport {
            completion,
            rate: data_rate,
            he_vector: first_he_vector,
            he_trigger: None,
            subframes: original_subframes,
            ethernet_bytes,
            acknowledged: retry_state.acknowledged(),
            trigger_flow_completed: retry_state.trigger_flow_completions() != 0,
            retry_failures,
            aggregate_attempts: retry_state.aggregate_attempts(),
            block_ack_mpdu_attempts: retry_state.block_ack_mpdu_attempts(),
            individual_retry_mpdu,
            spill_frames,
            elapsed_us: transmission_started.elapsed().as_micros(),
            hardware_us: hardware_wait_us,
            rx_irqs_during_hardware,
            rx_service_yields_during_preparation,
            preparation_us,
            first_empty_delimiters,
        });
    }
}

async fn transmit_connected_protected_ethernet_frame<M: Mmio + TxHardware>(
    mmio: &mut M,
    tx_storage: &mut TxStorage,
    _tx_ampdu_storage: Pin<&mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>,
    bssid: [u8; 6],
    key_slot: &mut StaPairwiseCcmpSlot,
    sequences: &mut StaTxSequenceCounters,
    _association_phy: StaAssociationPhy,
    peer_qos: bool,
    data_rate: TxPhyRate,
    _network_runner: &NetworkRunner,
    ethernet: &[u8],
) -> Result<TxCompletion, ActiveScanTxError>
where
    M: open_esp_radio::esp32s31::mac::tx_ampdu::HtAmpduHardware,
{
    // Internally generated control probes do not own a pinned embassy-net
    // lease. Keep them on the universally valid OFDM basic-rate descriptor
    // when the selected bulk-data rate is HE. All stack-produced HE traffic
    // retains its lease and uses the referenced HE A-MPDU path.
    let ordinary_rate = if matches!(data_rate, TxPhyRate::He(_)) {
        TxPhyRate::Legacy(LegacyRate::Ofdm54M)
    } else {
        data_rate
    };
    transmit_protected_ethernet_frame(
        mmio,
        tx_storage,
        bssid,
        key_slot,
        sequences
            .take_data(peer_qos.then_some(0))
            .expect("selected data sequence-number owner exists"),
        peer_qos,
        ordinary_rate,
        ethernet,
    )
    .await
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
                buffer: unsafe { rx_storage.buffers[index].as_slice() },
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
            unsafe { rx_storage.buffers[index].prepare_for_recycle() }
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

            // Publish the address before the request flag. The radio task
            // must build its ARP prime from the actual DHCP lease, not the
            // static PERF_AP_PROFILE fallback.
            OPEN_RADIO_LOCAL_IPV4.store(u32::from_be_bytes(local_ipv4), Ordering::Release);
            OPEN_RADIO_LAN_PROBE_READY.store(true, Ordering::Release);
            for _ in 0..5_000 {
                if OPEN_RADIO_LAN_PROBE_RESPONSE.load(Ordering::Acquire) {
                    // `try_send_rx` has transferred the matching ARP reply to
                    // embassy-net. Give its runner one scheduling interval to
                    // install the neighbor before advertising readiness.
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

async fn start_tx_block_ack_negotiation(
    mmio: &mut RadioRegisters,
    tx_storage: &mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    sequences: &mut StaTxSequenceCounters,
    connected_started: Instant,
    session: &mut TxBlockAckSession,
    dialog_token: TxBlockAckDialogToken,
    tid: u8,
) -> Option<TxBlockAckAlarm> {
    let action_sequence = sequences.take_non_qos();
    let starting_sequence = sequences
        .peek_qos(tid)
        .expect("fixed BlockAck TID has a sequence-number owner");
    let request = match session.begin_with_dialog_token(
        starting_sequence,
        connected_started.elapsed().as_micros(),
        dialog_token,
    ) {
        Ok(request) => request,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-begin \
                 tid={tid} error={error:?}"
            ));
            return None;
        }
    };
    let frame_length = match (StaActionFrame {
        source: station_address,
        bssid,
        sequence_number: action_sequence,
        body: &request.body,
    })
    .encode(&mut tx_storage.dma_buffer_mut()[TX_METADATA_SIZE..])
    {
        Ok(length) => length,
        Err(error) => {
            session.stop();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-encode \
                 tid={tid} error={error:?}"
            ));
            return None;
        }
    };
    let descriptor_capacity = (frame_length + TX_METADATA_SIZE + TX_FCS_SIZE + 3) & !3;
    match transmit_encoded_unicast_with_retry(
        mmio,
        tx_storage,
        LegacyTxQueue::Voice,
        frame_length,
        descriptor_capacity,
        None,
        LegacyTxQueue::Voice.vendor_data_scheduler_priority(),
        LegacyTxQueue::Voice.vendor_data_packet_priority(),
        TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
        0,
        0,
    )
    .await
    {
        Ok(completion) if completion.status == 0 => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=tx-addba-request \
                 tid={tid} dialog_token={} window={} starting_sequence={}",
                request.dialog_token, TX_AMPDU_FRAME_COUNT, request.starting_sequence,
            ));
            Some(request.alarm)
        }
        Ok(completion) => {
            session.stop();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-request \
                 tid={tid} status={}",
                completion.status,
            ));
            None
        }
        Err(error) => {
            session.stop();
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-request \
                 tid={tid} error={error:?}"
            ));
            None
        }
    }
}

#[derive(Clone, Copy)]
struct PendingRxAddba {
    dialog_token: u8,
    tid: u8,
    requested_window: u16,
    starting_sequence: u16,
}

struct ActiveRxAddba {
    hardware_index: u8,
    tid: u8,
    window: u16,
    starting_sequence: u16,
    // The software agreement must outlive the matching MMIO entry. Frame
    // retention/release will be connected to this owner after the first
    // aggregate HIL establishes the descriptor delivery contract.
    _reorder: RxBlockAckReorder,
}

// Keep one ordinary-code symbol alive so the host HIL can prove the runtime
// memory profile from periodic UART evidence. In the required
// psram-code-psram-data image its address is in 0x5000_0000..0x5100_0000; a
// directly linked app/Flash-XIP image reports 0x4000_0000..0x5000_0000.
#[inline(never)]
fn open_radio_runtime_code_marker() {}

type ConnectedNetworkRxFrame =
    NetworkRxFrame<'static, VENDOR_LARGE_RX_SLOT_COUNT, VENDOR_LARGE_RX_PAYLOAD_CAPACITY>;
type ConnectedRxStagingQueue =
    RxFrameQueue<'static, VENDOR_LARGE_RX_SLOT_COUNT, VENDOR_LARGE_RX_PAYLOAD_CAPACITY>;

fn stage_connected_rx_from_storage(
    mmio: &mut RadioRegisters,
    rx_storage: &RxStorage,
    rx_ring: &mut RxRingLive<'_, RX_DESCRIPTOR_COUNT>,
    completed: RxCompletedDescriptor,
) -> Result<ConnectedNetworkRxFrame, RxStageTransactionError> {
    let completed_index = completed.index();
    // SAFETY: `take_completed` transferred this descriptor to the sole
    // dispatcher. The driver transaction copies it before invoking the rearm
    // closure, so no slice survives renewed hardware ownership.
    let dma_buffer = unsafe { rx_storage.buffers[completed_index].as_slice() };
    OPEN_RADIO_RX_STAGE_POOL.stage_recycle_and_publish(
        completed,
        dma_buffer,
        mmio,
        rx_ring,
        |index| {
            // SAFETY: the driver invokes this closure only for the completed
            // prefix it owns and before publishing that descriptor again.
            unsafe { rx_storage.buffers[index].prepare_for_recycle() }
        },
    )
}

async fn connected_radio_loop(
    mmio: &mut RadioRegisters,
    rx_storage: &RxStorage,
    tx_storage: &mut TxStorage,
    mut tx_ampdu_storage: Pin<
        &'static mut HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>,
    >,
    descriptor_base: u32,
    buffer_addresses: &[u32; RX_DESCRIPTOR_COUNT],
    frame: &mut [u8; RX_BUFFER_SIZE],
    ethernet: &mut [u8; RX_BUFFER_SIZE],
    network_runner: &NetworkRunner,
    station_address: [u8; 6],
    bssid: [u8; 6],
    association_id: u16,
    pairwise_slot: &mut StaPairwiseCcmpSlot,
    peer_qos: bool,
    association_phy: StaAssociationPhy,
    peer_supports_one_ltf_800ns_gi: bool,
    peer_supports_ldpc: bool,
    peer_dcm_receive: HeDcmConstellation,
    best_effort_txop: HeEdcaTxopLimit,
    rate_control: &mut StaRateControlAssociation,
    sequences: &mut StaTxSequenceCounters,
) -> ! {
    let tx_rate_policy = configured_sta_tx_rate_policy(
        association_phy,
        peer_qos,
        peer_supports_one_ltf_800ns_gi,
        peer_supports_ldpc,
        peer_dcm_receive,
    );
    if let Some(dcm) = OPEN_RADIO_HE_DCM_RATE {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result={} stage=connected-he-dcm-capability \
             required={:?} peer={peer_dcm_receive:?} \
             ldpc={} peer_ldpc={peer_supports_ldpc}",
            if tx_rate_policy.he_dcm_override_is_supported() {
                "PASS"
            } else {
                "SKIP"
            },
            dcm.required_peer_constellation(),
            dcm.rate().is_ldpc(),
        ));
    }
    let initial_rate_schedule = schedule_state(rate_control.current_schedule());
    let initial_data_rate = rate_control.tx_rate(tx_rate_policy);
    let initial_ampdu_rate = rate_control.ampdu_tx_rate(tx_rate_policy);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL probe=connected-radio-enter \
         synthetic_amsdu={} network_amsdu={} tx_buffer_size={} ampdu_storage_bytes={} \
         rc_schedule={:?}/{} rc_rate={:#04x} rc_ampdu_limit_rate={:?} \
         tx_rate={:?} tx_rate_code={:#04x} tx_rate_kbps={} \
         ampdu_schedule={:?} ampdu_rate_code={:#04x} ampdu_rate_kbps={}",
        OPEN_RADIO_AMSDU_BENCH,
        OPEN_RADIO_NETWORK_AMSDU_BENCH,
        TX_BUFFER_SIZE,
        core::mem::size_of::<HtAmpduTxStorage<TX_AMPDU_FRAME_COUNT, TX_AMPDU_BUFFER_SIZE>>(),
        rate_control.current_schedule().kind,
        rate_control.current_schedule().index,
        initial_rate_schedule.rate,
        rate_control.ampdu_limit_rate(),
        initial_data_rate,
        initial_data_rate.code(),
        initial_data_rate.nominal_kbps(),
        rate_control.current_ampdu_schedule(),
        initial_ampdu_rate.code(),
        initial_ampdu_rate.nominal_kbps(),
    ));
    let best_effort_edca = tx_storage.edca_parameters(LegacyTxQueue::BestEffort);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=wmm-best-effort \
         aifsn={} ecw_min={} ecw_max={} txop_units_32_us={} txop_us={}",
        best_effort_edca.aifsn(),
        best_effort_edca.minimum_exponent(),
        best_effort_edca.maximum_exponent(),
        best_effort_txop.units_32_us(),
        u32::from(best_effort_txop.units_32_us()) * 32,
    ));
    let mut received = 0_u32;
    let mut enqueued = 0_u32;
    let mut dropped = 0_u32;
    let mut group_protected = 0_u32;
    let mut group_mic_failures = 0_u32;
    let mut group_rejections = 0_u32;
    let mut pairwise_mic_failures = 0_u32;
    let mut pairwise_hardware_rejections = 0_u32;
    let mut pairwise_rejections = 0_u32;
    let mut duplicate_frames = 0_u32;
    let mut amsdu_frames = 0_u32;
    let mut amsdu_msdu = 0_u32;
    let mut rx_interleave_non_data_consumed = 0_u32;
    let mut duplicate_filter = StaRxDuplicateFilter::new();
    let mut lan_probe_sent = false;
    let mut last_rx_state_report = Instant::now();
    let mut last_rx_statistics_at = Instant::now();
    let mut last_rx_primary_statistics = mmio.rx_statistics_snapshot().primary;
    let mut last_network_activity = Instant::now();
    let mut recycled_groups = 0_u32;
    let mut rx_staging_queue = ConnectedRxStagingQueue::new();
    // SOURCE: complete `_oracles/libnet80211.a[ieee80211_ht.o]::
    // ampdu_rx_start.constprop.0` stores an independent agreement pointer at
    // node offset `0x268 + tid * 4`; complete `_oracles/libpp.a[hal_ampdu.o]`
    // exposes eight ordinary receive-BA hardware banks. Keep both ownership
    // domains fixed-size and allocation-free.
    let mut pending_rx_addba: [Option<PendingRxAddba>; 8] = [None; 8];
    let mut active_rx_addba: [Option<ActiveRxAddba>; 8] = core::array::from_fn(|_| None);
    let mut last_he_rx_trigger_count = 0_u16;
    let mut observed_trigger_frames = 0_u32;
    let mut observed_he_ndpa_frames = 0_u32;
    let mut observed_he_ndpa_for_us = 0_u32;
    let mut last_he_ndpa_dialog_token = None;
    let mut last_he_ndpa_hardware = None;
    let mut last_trigger_common: Option<TriggerCommonInfo> = None;
    let mut last_trigger_user = None;
    let mut last_trigger_schedule: Option<
        Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>,
    > = None;
    let mut he_queue_snapshot_reported = false;
    let mut tx_block_ack = TxBlockAckSession::new(TxBlockAckConfig {
        tid: 0,
        window: TX_AMPDU_FRAME_COUNT as u16,
        timeout_tu: 0,
        negotiation_timeout_us: 500_000,
        amsdu: OPEN_RADIO_AMSDU_BENCH || OPEN_RADIO_NETWORK_AMSDU_BENCH,
    })
    .expect("fixed TX BlockAck configuration");
    let mut tx_block_ack_tid7 = TxBlockAckSession::new(TxBlockAckConfig {
        tid: 7,
        window: TX_AMPDU_FRAME_COUNT as u16,
        timeout_tu: 0,
        negotiation_timeout_us: 500_000,
        amsdu: false,
    })
    .expect("fixed TID7 TX BlockAck configuration");
    let mut tx_block_ack_tid5 = TxBlockAckSession::new(TxBlockAckConfig {
        tid: 5,
        window: TX_AMPDU_FRAME_COUNT as u16,
        timeout_tu: 0,
        negotiation_timeout_us: 500_000,
        amsdu: false,
    })
    .expect("fixed TID5 TX BlockAck configuration");
    let mut tx_block_ack_dialog_tokens = TxBlockAckDialogTokenSequence::new();
    let mut tx_block_ack_alarm = None;
    let mut tx_block_ack_tid7_alarm = None;
    let mut tx_block_ack_tid5_alarm = None;
    let connected_started = Instant::now();
    let mut tx_ampdu_submissions = 0_u32;
    let mut tx_ampdu_partial = 0_u32;
    let mut tx_ampdu_max_subframes = 0_u8;
    let mut tx_ampdu_max_bytes = 0_usize;
    let mut tx_ampdu_attempts = 0_u32;
    let mut tx_ampdu_individual_retry_mpdu = 0_u32;
    let mut tx_ampdu_spill_frames = 0_u32;
    let mut tx_ampdu_cadence_samples = 0_u32;
    let mut tx_ampdu_elapsed_us = 0_u64;
    let mut tx_ampdu_hardware_us = 0_u64;
    let mut tx_ampdu_rx_irqs_during_hardware = 0_u32;
    let mut tx_ampdu_rx_service_yields_during_preparation = 0_u32;
    let mut tx_ampdu_preparation_us = 0_u64;
    let mut tx_ampdu_ethernet_bytes = 0_u64;
    let mut tx_ampdu_subframes = 0_u64;
    let mut raw_mac_frame_storage = [0x5a_u8; OPEN_RADIO_UDP_PAYLOAD_CAPACITY];
    raw_mac_frame_storage[..6].copy_from_slice(&bssid);
    raw_mac_frame_storage[6..12].copy_from_slice(&station_address);
    raw_mac_frame_storage[12..14].copy_from_slice(&0x88b5_u16.to_be_bytes());
    let raw_mac_frame_length = if OPEN_RADIO_HE_DELIMITER_HIL || OPEN_RADIO_HE_DCM_HIL {
        // DCM MCS0 has only a 1,600..1,850-byte zero-TXOP APEP budget.
        // The ordinary 1,472-byte benchmark body admits one MPDU and would
        // accidentally turn the DCM A-MPDU matrix into a single-MPDU test.
        // A valid minimum Ethernet frame exercises the real multi-subframe
        // delimiter, BlockAck and retry paths within that hardware budget.
        //
        // SOURCE[HIL_OPEN_HE20_MCS0_DCM_AMPDU_2026_07_29]: an Android-13
        // OnePlus IN2023 HE20 SoftAP advertised BPSK DCM receive. The exact
        // bound admitted 30, 29 and 26 MPDUs at the three supported GI/LTF
        // selectors. The first 30-MPDU submission acknowledged every MPDU in
        // one attempt; 27 complete 3-profile x 64-submission rounds had zero
        // failed profiles and zero terminal retry failures.
        14
    } else {
        OPEN_RADIO_UDP_PAYLOAD_CAPACITY
    };
    let raw_mac_frame = &raw_mac_frame_storage[..raw_mac_frame_length];
    let mut raw_mac_started = Instant::now();
    let mut raw_mac_bytes = 0_u64;
    // Keep the rate that the completed A-MPDU owner actually published.
    // Re-deriving it from association mode is wrong for explicit HE matrix
    // and HE-TB profiles: `selected_data_tx_rate` intentionally returns the
    // ordinary HT fallback, while `AmpduTxConfig::He` owns the live
    // HE vector.
    let mut raw_mac_rate = rate_control.ampdu_tx_rate(tx_rate_policy);
    let mut raw_mac_control_tx = 0_u32;
    let peer_dcm_profile_count = he_dcm_matrix_profile_count(peer_dcm_receive, peer_supports_ldpc);
    let he_matrix_first_profile = if OPEN_RADIO_HE_DELIMITER_HIL {
        // MCS9, 2xLTF/0.8 us: 114.7 Mbit/s and a 230-byte minimum subframe
        // under the deliberately conservative 16-us HIL density.
        19
    } else if OPEN_RADIO_HE_DCM_HIL {
        0
    } else {
        he_matrix_first_profile(peer_supports_one_ltf_800ns_gi)
    };
    let he_matrix_profile_count = if OPEN_RADIO_HE_DELIMITER_HIL {
        20
    } else if OPEN_RADIO_HE_DCM_HIL {
        peer_dcm_profile_count
    } else {
        HE_MATRIX_PROFILE_COUNT
    };
    let mut he_matrix_profile = he_matrix_first_profile;
    let mut he_matrix_round = 0_u32;
    let mut he_matrix_started = Instant::now();
    let mut he_matrix_submissions = 0_u32;
    let mut he_matrix_complete = 0_u32;
    let mut he_matrix_errors = 0_u32;
    let mut he_matrix_aggregate_attempts = 0_u32;
    let mut he_matrix_retry_failures = 0_u32;
    let mut he_matrix_bytes = 0_u64;
    let mut he_matrix_max_subframes = 0_u8;
    let mut he_matrix_rx_mpdu = 0_u32;
    let mut he_matrix_rx_buffer_full = 0_u32;
    let mut he_matrix_rx_irq = 0_u32;
    let mut he_matrix_rx_staged = 0_u32;
    let mut he_matrix_failed_profiles = 0_u8;
    let he_matrix_requested =
        OPEN_RADIO_HE_MATRIX_HIL || OPEN_RADIO_HE_LDPC_HIL || OPEN_RADIO_HE_DCM_HIL;
    let he_matrix_active = he_matrix_requested
        && association_phy == StaAssociationPhy::He20
        && (!OPEN_RADIO_HE_LDPC_HIL || peer_supports_ldpc)
        && (!OPEN_RADIO_HE_DCM_HIL || peer_dcm_profile_count != 0);
    if OPEN_RADIO_HE_LDPC_HIL && !peer_supports_ldpc {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=SKIP stage=he20-ldpc-capability \
             reason=peer-does-not-advertise-payload-ldpc"
        ));
    }
    if OPEN_RADIO_HE_DCM_HIL && peer_dcm_profile_count == 0 {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=SKIP stage=he20-dcm-capability \
             peer_dcm_receive={peer_dcm_receive:?} \
             reason=peer-does-not-advertise-dcm-rx"
        ));
    }
    if he_matrix_active {
        // SOURCE[ESP32S31_HE_SU_GI_LTF_POLICY_2026_07_29]:
        // `_oracles/esp32s31_rev0_rom.elf::ppSelectTxFormat` at
        // 0x2f833870..0x2f833894 emits selector 1 for rate codes 26..35 and
        // selector 2 for 16..25. `ppCertSetRate` at
        // 0x2f8337c8..0x2f83383c additionally emits selector 3 only for
        // LTF=4/GI=4; neither producer emits selector 0. The pinned
        // `_oracles/libpp.a[pp_he.o]` implements the same three mappings and
        // rejects LTF values other than 2 or 4. The MAC formatter accepts the
        // raw two-bit selector but does not establish peer support.
        //
        // SOURCE[HIL_OPEN_HE20_GI_LTF_MATRIX_2026_07_29]: the controlled
        // Linux AX211 HE20 AP capability has PHY-capability byte 1 bit 0x40
        // clear. Across three bounded rounds, selectors 1, 2 and 3 completed
        // A-MPDU/BlockAck at every MCS0..9 with zero failed profiles and zero
        // terminal retry failures. Earlier FRITZ!Box HIL returned no BlockAck
        // for selector 0 at MCS0..9. Start at profile 10 for peers that do not
        // advertise that optional 1xLTF/0.8-us-GI selector instead of treating
        // its absence as a formatter failure.
        //
        // SOURCE[HIL_OPEN_HE20_ONE_LTF_REJECTED_2026_07_31]: a fresh forced
        // MCS9/selector-0 run against the same AX211 AP completed legacy
        // authentication, association, WPA2 and ADDBA, then received no
        // BlockAck for any HE data aggregate: status five advanced on every
        // attempt, with zero successful A-MPDU submissions. The AP's parsed
        // HE PHY capability had `one_ltf_800ns_gi=false`. This is a
        // peer-capability rejection, not permission to remove selector zero
        // from the typed formatter; skip it unless that capability is set.
        if OPEN_RADIO_HE_DCM_HIL {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=he20-dcm-capability \
                 peer_dcm_receive={peer_dcm_receive:?} peer_ldpc={peer_supports_ldpc} \
                 profiles={he_matrix_profile_count}"
            ));
        } else if OPEN_RADIO_HE_LDPC_HIL {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=he20-ldpc-capability \
                 peer_ldpc={peer_supports_ldpc} first_profile={he_matrix_first_profile} \
                 tested_profiles={}",
                he_matrix_profile_count - he_matrix_first_profile,
            ));
        } else {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=PASS stage=he20-matrix-capability \
                 one_ltf_800ns_gi={} first_profile={} tested_profiles={}",
                peer_supports_one_ltf_800ns_gi,
                he_matrix_first_profile,
                he_matrix_profile_count - he_matrix_first_profile,
            ));
        }
    }
    // SOURCE[HIL_OPEN_HT40_AMSDU_BODY_REUSE_2026_07_29]: the production
    // PSRAM/PSRAM profile negotiated WPA2 + HT40 MCS7 SGI + ADDBA window 32
    // with A-MSDU enabled. Reusing these statically owned bodies reduced
    // preparation from 768 us to 167 us and sustained 102.8..109.7 Mbit/s
    // over more than 8,300 accepted aggregates on the controlled Linux AP.
    let mut raw_mac_amsdu_slots_initialized = false;
    let mut direct_udp_benchmark = DirectUdpRxBenchmark::new();
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL probe=connected-radio-before-rx-prepare"
    ));
    let stopped_ring = match RxRingStopped::prepare(
        mmio,
        &rx_storage.descriptors,
        descriptor_base,
        buffer_addresses,
        RX_BUFFER_SIZE as u32,
        |index| {
            // SAFETY: RxRingStopped has confirmed the walker is stopped and
            // calls this closure before publishing any descriptor.
            unsafe { rx_storage.buffers[index].prepare_for_recycle() }
        },
    ) {
        Ok(ring) => ring,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-radio-rx-arm error={error:?}"
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
    };
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL probe=connected-radio-after-rx-prepare"
    ));
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-radio-rx-head \
         start={} tail={} last={:#010x}",
        stopped_ring.initial_start(),
        stopped_ring.accepted_tail(),
        stopped_ring.retained_last_low(),
    ));
    Timer::after_micros(5).await;
    let mut rx_ring = match stopped_ring.start(mmio) {
        Ok(ring) => ring,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL \
                 stage=embassy-net-radio-rx-handoff-start error={error:?}"
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }
    };

    if OPEN_RADIO_HE_DCM_HIL
        && association_phy == StaAssociationPhy::He20
        && peer_dcm_profile_count != 0
    {
        let smpdu_sequence = sequences
            .take_qos(0)
            .expect("TID0 sequence-number owner exists");
        match transmit_he_dcm_smpdu_oracle(
            mmio,
            tx_storage,
            tx_ampdu_storage.as_mut(),
            station_address,
            bssid,
            smpdu_sequence,
        )
        .await
        {
            Ok((completion, Some(vector))) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result={} stage=he20-dcm-smpdu \
                     status={} rate_code={:#04x} plcp0={:#010x} plcp1={:#010x} \
                     he_a1={:#010x} he_a2={:#010x} power={:#010x} \
                     length={:#010x}",
                    if completion.status == 0 {
                        "PASS"
                    } else {
                        "FAIL"
                    },
                    completion.status,
                    (vector.plcp1 >> 12) & 0x1f,
                    vector.plcp0,
                    vector.plcp1,
                    vector.he_signal_a1,
                    vector.he_signal_a2_length,
                    vector.power,
                    vector.length_control,
                ));
            }
            Ok((completion, None)) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=he20-dcm-smpdu \
                     status={} error=missing-vector-snapshot",
                    completion.status,
                ));
            }
            Err(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=he20-dcm-smpdu error={error:?}"
                ));
            }
        }
        Timer::after_millis(10).await;
    }

    if peer_qos
        && matches!(
            selected_data_tx_rate(association_phy, peer_qos),
            TxPhyRate::Ht(_)
        )
    {
        // SOURCE: complete `_oracles/libnet80211.a[wl_cnx.o]::cnx_auth_done`
        // calls `ieee80211_ampdu_request` after WPA completion for TIDs
        // 0, 7 and 5 in this exact order. The complete
        // `_oracles/libnet80211.a[ieee80211_ht.o]` leaf obtains a shared
        // modulo-63 Dialog Token and builds an independent session per TID.
        tx_block_ack_alarm = start_tx_block_ack_negotiation(
            mmio,
            tx_storage,
            station_address,
            bssid,
            sequences,
            connected_started,
            &mut tx_block_ack,
            tx_block_ack_dialog_tokens.take(),
            0,
        )
        .await;
        tx_block_ack_tid7_alarm = start_tx_block_ack_negotiation(
            mmio,
            tx_storage,
            station_address,
            bssid,
            sequences,
            connected_started,
            &mut tx_block_ack_tid7,
            tx_block_ack_dialog_tokens.take(),
            7,
        )
        .await;
        tx_block_ack_tid5_alarm = start_tx_block_ack_negotiation(
            mmio,
            tx_storage,
            station_address,
            bssid,
            sequences,
            connected_started,
            &mut tx_block_ack_tid5,
            tx_block_ack_dialog_tokens.take(),
            5,
        )
        .await;
    }

    loop {
        let connected_data_rate = rate_control.tx_rate(tx_rate_policy);
        let connected_ampdu_rate = rate_control.ampdu_tx_rate(tx_rate_policy);
        'connected_rx: loop {
            // Match `wdevProcessRxSucDataAll`: drain the complete hardware
            // frontier into independently owned storage before admitting one
            // staged frame to the upper parser. After that one frame, loop
            // back through this pump first so newly completed DMA units retain
            // priority over protocol work.
            while !rx_staging_queue.is_full() {
                let index = rx_ring.recycle_start();
                let Some(completed) = rx_ring.take_completed(index) else {
                    break;
                };
                let staged = match stage_connected_rx_from_storage(
                    mmio,
                    rx_storage,
                    &mut rx_ring,
                    completed,
                ) {
                    Ok(staged) => staged,
                    Err(error) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=embassy-net-radio-rx-stage-recycle \
                             error={error:?} received={received} \
                             enqueued={enqueued} dropped={dropped}"
                        ));
                        loop {
                            Timer::after_secs(60).await;
                        }
                    }
                };
                if rx_staging_queue.try_push(staged).is_err() {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL \
                         stage=embassy-net-radio-rx-stage-queue error=full-after-check"
                    ));
                    loop {
                        Timer::after_secs(60).await;
                    }
                }
                received = received.saturating_add(1);
                recycled_groups = recycled_groups.saturating_add(1);
            }

            let Some(staged) = rx_staging_queue.pop() else {
                break 'connected_rx;
            };
            let segment = staged.segment();
            let raw = segment.buffer;
            let raw_fc = u16::from_le_bytes([raw[PUBLIC_HEADER_SIZE], raw[PUBLIC_HEADER_SIZE + 1]]);
            let raw_destination = &raw[PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10];
            let raw_group_protected = raw_fc & 0x400c == 0x4008 && raw_destination[0] & 1 != 0;
            let raw_pairwise_protected =
                raw_fc & 0x400c == 0x4008 && raw_destination == station_address;
            if raw_group_protected {
                group_protected = group_protected.saturating_add(1);
            }

            // Trigger is a control MPDU (type/subtype low byte 0x24), not a
            // management or data frame. Keep descriptor/FCS ownership in the
            // S31 RX crate and wire-format ownership in the IEEE crate.
            //
            // SOURCE[BLOB_LIBPP_DBG_DUMP_TRIG_*]: the complete hal_debug.o
            // decoders define Common/User bit positions. The hardware
            // `hal_he_get_rx_trigger_cnt` counter is sampled independently
            // below, so a packet observation can be checked against the MAC
            // parser rather than inferred from register state alone.
            if raw_fc & 0x00fc == 0x0024 {
                let Ok(control) = extract_control(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                ) else {
                    continue;
                };
                if let Ok(trigger) = parse_trigger_frame(&frame[..control.length]) {
                    observed_trigger_frames = observed_trigger_frames.saturating_add(1);
                    last_trigger_common = Some(trigger.common);
                    last_trigger_schedule = Some(HeTriggerScheduledRate::from_trigger_frame(
                        &trigger,
                        association_id,
                    ));
                    if let Some(bytes) = trigger.user_info_and_padding.get(..5) {
                        let mut first_user = [0_u8; 5];
                        first_user.copy_from_slice(bytes);
                        last_trigger_user = Some(first_user);
                    }
                }
                continue;
            }

            // HE sounding begins with an NDP Announcement control MPDU.
            // Keep the same RX/FCS ownership split as Trigger parsing, then
            // use the allocation-free parser recovered from complete
            // `_oracles/libpp.a[wdev.o]::is_ndpa_to_dut`.
            if raw_fc & 0x00fc == 0x0054 {
                let Ok(control) = extract_control(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                ) else {
                    continue;
                };
                if let Ok(ndpa) = HeNdpa::parse(&frame[..control.length]) {
                    observed_he_ndpa_frames = observed_he_ndpa_frames.saturating_add(1);
                    last_he_ndpa_dialog_token = Some(ndpa.dialog_token());
                    // WDEVAXDIAG is explicitly non-latched. Sample it adjacent
                    // to the admitted NDPA rather than treating a later status
                    // report as durable beamforming state.
                    last_he_ndpa_hardware = Some(mmio.he_beamforming_diagnostics());
                    if ndpa.contains_association_id(association_id) {
                        observed_he_ndpa_for_us = observed_he_ndpa_for_us.saturating_add(1);
                    }
                }
                continue;
            }

            // Action management frames are intentionally handled before the
            // protected-data extractor. The promoted migration accepted only
            // an immediate, timeout-free TID-0 RX agreement during this first
            // STA HIL stage:
            // `migration/esp32s31-hybrid-runtime/src/rx_ampdu_ap.rs::
            // try_accept_sta_request`. `_oracles/libnet80211.a` supplies the
            // original BlockAck action dispatch.
            if raw_fc & 0x00fc == 0x00d0 {
                let Ok(management) = extract_management(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                ) else {
                    continue;
                };
                if management.length < 24
                    || frame[4..10] != station_address
                    || frame[10..16] != bssid
                    || frame[16..22] != bssid
                {
                    continue;
                }
                let action_body = &frame[24..management.length];
                match parse_block_ack_action(action_body) {
                    Some(BlockAckAction::AddbaRequest {
                        dialog_token,
                        tid,
                        immediate,
                        window,
                        timeout_tu,
                        starting_sequence,
                        ..
                    }) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=rx-addba-request \
                             tid={tid} immediate={} window={window} timeout_tu={timeout_tu} \
                             starting_sequence={starting_sequence}",
                            u8::from(immediate),
                        ));
                        if immediate
                            && window != 0
                            && timeout_tu == 0
                            // SOURCE: complete `libnet80211.a[ieee80211_ht.o]::
                            // ht_recv_action_ba_addba_request` rejects only
                            // TIDs with bit three set. The earlier TID-0-only
                            // restriction came from the migration HIL, not
                            // from the vendor implementation.
                            && tid <= S31_RX_BLOCK_ACK_MAX_TID
                            && starting_sequence <= 0x0fff
                        {
                            pending_rx_addba[usize::from(tid)] = Some(PendingRxAddba {
                                dialog_token,
                                tid,
                                requested_window: window,
                                starting_sequence,
                            });
                        }
                    }
                    Some(BlockAckAction::AddbaResponse { .. }) => {
                        let response_token = action_body[2];
                        let selected = if tx_block_ack.awaiting_dialog_token()
                            == Some(response_token)
                        {
                            Some((&mut tx_block_ack, &mut tx_block_ack_alarm))
                        } else if tx_block_ack_tid7.awaiting_dialog_token() == Some(response_token)
                        {
                            Some((&mut tx_block_ack_tid7, &mut tx_block_ack_tid7_alarm))
                        } else if tx_block_ack_tid5.awaiting_dialog_token() == Some(response_token)
                        {
                            Some((&mut tx_block_ack_tid5, &mut tx_block_ack_tid5_alarm))
                        } else {
                            None
                        };
                        let Some((session, alarm)) = selected else {
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-response \
                                 dialog_token={response_token} error=unexpected-dialog-token"
                            ));
                            continue;
                        };
                        match session.on_response(action_body) {
                            Ok(TxBlockAckResponse::Operational(agreement)) => {
                                *alarm = None;
                                if association_phy == StaAssociationPhy::He20 {
                                    // SOURCE[BLOB_LIBNET80211_HE_TID_BITMAP]:
                                    // complete HE AddBA/DELBA lifecycle updates
                                    // WDEVTXQBSR_CTRL only at these owned
                                    // protocol transitions.
                                    let tid = MacHeTid::new(agreement.tid)
                                        .expect("BlockAck parser admitted an IEEE TID");
                                    mmio.set_he_trigger_based_tid_enabled(tid, true);
                                }
                                emergency_log(format_args!(
                                    "OPEN_RADIO_PHY_HIL result=PASS stage=tx-addba-active \
                                     tid={} window={} timeout_tu={} starting_sequence={} amsdu={}",
                                    agreement.tid,
                                    agreement.window,
                                    agreement.timeout_tu,
                                    agreement.starting_sequence,
                                    agreement.amsdu,
                                ));
                            }
                            Ok(TxBlockAckResponse::Rejected(status)) => {
                                *alarm = None;
                                emergency_log(format_args!(
                                    "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-response \
                                     dialog_token={response_token} status={status}"
                                ));
                            }
                            Err(error) => emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-response \
                                 dialog_token={response_token} error={error:?}"
                            )),
                        }
                    }
                    Some(BlockAckAction::Delba {
                        tid,
                        initiator,
                        reason,
                    }) => {
                        if initiator {
                            // The peer is the BA originator, so this tears
                            // down our receive agreement for that TID.
                            if let Some(hardware_index) =
                                active_rx_addba.iter().position(|agreement| {
                                    agreement
                                        .as_ref()
                                        .is_some_and(|agreement| agreement.tid == tid)
                                })
                            {
                                let _ = rx_ampdu_hw::clear(mmio, hardware_index as u8);
                                active_rx_addba[hardware_index] = None;
                            }
                        } else {
                            // The peer is the BA recipient, so this tears down
                            // the matching transmit agreement.
                            match tid {
                                0 => {
                                    tx_block_ack.stop();
                                    tx_block_ack_alarm = None;
                                }
                                7 => {
                                    tx_block_ack_tid7.stop();
                                    tx_block_ack_tid7_alarm = None;
                                }
                                5 => {
                                    tx_block_ack_tid5.stop();
                                    tx_block_ack_tid5_alarm = None;
                                }
                                _ => {}
                            }
                            if association_phy == StaAssociationPhy::He20 {
                                let tid =
                                    MacHeTid::new(tid).expect("DELBA parser admitted an IEEE TID");
                                mmio.set_he_trigger_based_tid_enabled(tid, false);
                            }
                        }
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=delba tid={tid} \
                             peer_is_originator={} reason={reason}",
                            u8::from(initiator),
                        ));
                    }
                    None => {}
                }
                continue;
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
                Err(RxError::MicFailure) if raw_group_protected => {
                    group_mic_failures = group_mic_failures.saturating_add(1);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=embassy-net-radio-rx-group \
                         result=mic-failure frame={group_protected} fc={raw_fc:#06x} \
                         destination={raw_destination:02x?} state={:#04x} internal={:#04x}",
                        raw[PUBLIC_HEADER_SIZE - 4],
                        raw[PUBLIC_HEADER_SIZE - 3],
                    ));
                    continue;
                }
                Err(error) if raw_group_protected => {
                    group_rejections = group_rejections.saturating_add(1);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=embassy-net-radio-rx-group \
                         result=reject frame={group_protected} error={error:?} \
                         fc={raw_fc:#06x} destination={raw_destination:02x?}"
                    ));
                    continue;
                }
                Err(RxError::MicFailure) if raw_pairwise_protected => {
                    pairwise_mic_failures = pairwise_mic_failures.saturating_add(1);
                    continue;
                }
                Err(RxError::RxFailure | RxError::Quarantined) if raw_pairwise_protected => {
                    // These states are published by the RX hardware for a
                    // frame it already rejected (for example FCS/PHY
                    // failure). They are not a CCMP parser or ownership
                    // rejection and must not be reported as one.
                    pairwise_hardware_rejections = pairwise_hardware_rejections.saturating_add(1);
                    continue;
                }
                Err(error) if raw_pairwise_protected => {
                    pairwise_rejections = pairwise_rejections.saturating_add(1);
                    // USB emergency output is synchronous. Per-frame logging
                    // here held the 32-entry ring for tens of milliseconds,
                    // asserted the RX-starvation interrupt and forced the AP
                    // to reduce its rate. Retain the aggregate counter for
                    // the idle state report; no packet-path formatting.
                    let _ = error;
                    continue;
                }
                Err(_) => continue,
            };
            if data.mpdu.length < 24 || frame[10..16] != bssid {
                continue;
            }
            let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
            let sequence_control = u16::from_le_bytes([frame[22], frame[23]]);
            let tid = if frame_control & 0x0080 != 0 && data.mpdu.length >= 26 {
                Some(frame[24] & 0x0f)
            } else {
                None
            };
            if duplicate_filter.is_duplicate(frame_control & 0x0800 != 0, sequence_control, tid) {
                duplicate_frames = duplicate_frames.saturating_add(1);
                continue;
            }
            let decapsulation = decapsulate_data(
                DataInterfaceRole::Station,
                &frame[..data.mpdu.length],
                data.payload_offset,
                data.payload_length,
                ethernet,
            );
            match decapsulation {
                Ok(plan) => account_connected_rx_route(
                    route_connected_ethernet(
                        &ethernet[..plan.ethernet_length],
                        raw,
                        station_address,
                        &mut direct_udp_benchmark,
                        network_runner,
                    ),
                    &mut enqueued,
                    &mut dropped,
                    &mut last_network_activity,
                ),
                Err(DataDecapError::AmsduUnsupported) => {
                    let Ok(subframes) = amsdu_subframes(
                        DataInterfaceRole::Station,
                        &frame[..data.mpdu.length],
                        data.payload_offset,
                        data.payload_length,
                    ) else {
                        continue;
                    };
                    amsdu_frames = amsdu_frames.saturating_add(1);
                    for subframe in subframes {
                        let Ok(subframe) = subframe else {
                            break;
                        };
                        let Ok(ethernet_length) = decapsulate_amsdu_subframe(subframe, ethernet)
                        else {
                            break;
                        };
                        amsdu_msdu = amsdu_msdu.saturating_add(1);
                        account_connected_rx_route(
                            route_connected_ethernet(
                                &ethernet[..ethernet_length],
                                raw,
                                station_address,
                                &mut direct_udp_benchmark,
                                network_runner,
                            ),
                            &mut enqueued,
                            &mut dropped,
                            &mut last_network_activity,
                        );
                    }
                }
                Err(_) => {}
            }
        }

        if rx_ring.all_observed() {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-radio-rx-live-append \
                 error=terminal-before-recycle control={:#010x} received={received}",
                mmio.read32(RX_CONTROL),
            ));
            loop {
                Timer::after_secs(60).await;
            }
        }

        let now_us = connected_started.elapsed().as_micros();
        for (tid, session, alarm_slot) in [
            (0, &mut tx_block_ack, &mut tx_block_ack_alarm),
            (7, &mut tx_block_ack_tid7, &mut tx_block_ack_tid7_alarm),
            (5, &mut tx_block_ack_tid5, &mut tx_block_ack_tid5_alarm),
        ] {
            if alarm_slot.is_some_and(|alarm| now_us >= alarm.deadline_us) {
                let alarm = alarm_slot.take().unwrap();
                if session.on_alarm(alarm) {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-response \
                         tid={tid} error=timeout"
                    ));
                }
            }
        }

        if let Some((pending_index, request)) = pending_rx_addba
            .iter()
            .enumerate()
            .find_map(|(index, request)| request.map(|request| (index, request)))
        {
            pending_rx_addba[pending_index] = None;
            let hardware_index = active_rx_addba
                .iter()
                .position(|agreement| {
                    agreement
                        .as_ref()
                        .is_some_and(|agreement| agreement.tid == request.tid)
                })
                .or_else(|| active_rx_addba.iter().position(Option::is_none));
            let Some(hardware_index) = hardware_index else {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-addba-hardware \
                     tid={} error=no-free-agreement",
                    request.tid,
                ));
                continue;
            };
            let hardware_index = hardware_index as u8;
            if active_rx_addba[usize::from(hardware_index)].is_some() {
                let _ = rx_ampdu_hw::clear(mmio, hardware_index);
                active_rx_addba[usize::from(hardware_index)] = None;
            }
            let selected_window = request.requested_window.min(RX_BLOCK_ACK_MAX_WINDOW);
            // SOURCE: complete `_oracles/libnet80211.a[ieee80211_ht.o]::
            // ampdu_rx_start.constprop.0` selects the negotiated response/reorder
            // window above, but its ordinary STA activation branch passes the
            // literal 64 to `ic_add_rx_ba`. Keep that hardware receive window
            // distinct from the protocol-owned window.
            let hardware_window = RX_BLOCK_ACK_MAX_WINDOW;
            let reorder = match RxBlockAckReorder::new(request.starting_sequence, selected_window) {
                Ok(reorder) => reorder,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-addba-software \
                         error={error:?}"
                    ));
                    continue;
                }
            };
            let agreement = S31RxBlockAckAgreement {
                hardware_index,
                interface: 0,
                peer: bssid,
                tid: request.tid,
                starting_sequence: request.starting_sequence,
                window: hardware_window,
            };
            if let Err(error) = rx_ampdu_hw::program(mmio, agreement) {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-addba-hardware \
                     error={error:?}"
                ));
                continue;
            }

            let mut body = [0_u8; 9];
            if let Err(error) = write_successful_addba_response(
                &mut body,
                request.dialog_token,
                request.tid,
                selected_window,
            ) {
                let _ = rx_ampdu_hw::clear(mmio, hardware_index);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-addba-body error={error:?}"
                ));
                continue;
            }
            let frame_length = match (StaActionFrame {
                source: station_address,
                bssid,
                sequence_number: sequences.take_non_qos(),
                body: &body,
            })
            .encode(&mut tx_storage.dma_buffer_mut()[TX_METADATA_SIZE..])
            {
                Ok(length) => length,
                Err(error) => {
                    let _ = rx_ampdu_hw::clear(mmio, hardware_index);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-addba-encode error={error:?}"
                    ));
                    continue;
                }
            };
            let hardware_storage_length = frame_length + TX_METADATA_SIZE + TX_FCS_SIZE;
            let descriptor_capacity = (hardware_storage_length + 3) & !3;
            match transmit_encoded_unicast_with_retry(
                mmio,
                tx_storage,
                LegacyTxQueue::Voice,
                frame_length,
                descriptor_capacity,
                None,
                0,
                0,
                TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
                0,
                0,
            )
            .await
            {
                Ok(completion) if completion.status == 0 => {
                    active_rx_addba[usize::from(hardware_index)] = Some(ActiveRxAddba {
                        hardware_index,
                        tid: request.tid,
                        window: selected_window,
                        starting_sequence: request.starting_sequence,
                        _reorder: reorder,
                    });
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS stage=rx-addba-active \
                         hardware_index={} interface=0 tid={} window={} hardware_window={} \
                         starting_sequence={}",
                        hardware_index,
                        request.tid,
                        selected_window,
                        hardware_window,
                        request.starting_sequence,
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=rx-addba-hardware-state \
                         hardware_index={} value={:?}",
                        hardware_index,
                        mmio.rx_block_ack_entry_snapshot(hardware_index),
                    ));
                }
                Ok(completion) => {
                    let _ = rx_ampdu_hw::clear(mmio, hardware_index);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-addba-response \
                         status={}",
                        completion.status,
                    ));
                }
                Err(error) => {
                    let _ = rx_ampdu_hw::clear(mmio, hardware_index);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=rx-addba-response \
                         error={error:?}"
                    ));
                }
            }
        }

        // Keep HIL state reporting out of the packet latency window. ROM
        // emergency output is synchronous on this target: live HIL showed a
        // state report adding up to 75 ms to an otherwise 4-ms ICMP response.
        // Runtime reports below use the bounded asynchronous logger instead,
        // but retain the idle gate so formatting and PAC snapshots do not
        // compete with an active packet burst.
        if last_rx_state_report.elapsed() >= Duration::from_secs(10)
            && last_network_activity.elapsed() >= Duration::from_millis(250)
        {
            last_rx_state_report = Instant::now();
            let completed = rx_storage
                .descriptors
                .iter()
                .filter(|descriptor| rx_done(descriptor.word0()))
                .count();
            let mut rx_ba_state = [None; 8];
            for (index, agreement) in active_rx_addba.iter().enumerate() {
                rx_ba_state[index] = agreement.as_ref().map(|agreement| {
                    (
                        agreement.hardware_index,
                        agreement.tid,
                        agreement.window,
                        agreement.starting_sequence,
                    )
                });
            }
            // Keep the rate-control state on its own bounded line. The full
            // RX/DMA diagnostic record can exceed the USB emergency logger's
            // fixed staging buffer before its trailing fields are emitted.
            log::info!(
                "OPEN_RADIO_PHY_HIL stage=tx-rate-feedback \
                 rc_schedule={:?}/{} rc_rate={:#04x} \
                 tx_rate={:#04x} tx_rate_kbps={} \
                 he_gi_ltf={} he_dcm={} he_ldpc={} \
                 ampdu_schedule={:?} ampdu_rate={:#04x} ampdu_rate_kbps={} \
                 ampdu_raw_ratio={:?} ampdu_filtered_ratio={:?} \
                 ack_snr_latest={:?} ack_snr_filtered={:?}",
                rate_control.current_schedule().kind,
                rate_control.current_schedule().index,
                schedule_state(rate_control.current_schedule()).rate,
                connected_data_rate.code(),
                connected_data_rate.nominal_kbps(),
                match connected_data_rate {
                    TxPhyRate::He(rate) => rate.guard_interval_and_ltf().encoding(),
                    TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
                },
                match connected_data_rate {
                    TxPhyRate::He(rate) => rate.is_dcm() as u8,
                    TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
                },
                match connected_data_rate {
                    TxPhyRate::He(rate) => rate.is_ldpc() as u8,
                    TxPhyRate::Legacy(_) | TxPhyRate::Ht(_) => u8::MAX,
                },
                rate_control.current_ampdu_schedule(),
                connected_ampdu_rate.code(),
                connected_ampdu_rate.nominal_kbps(),
                rate_control
                    .ampdu_runtime()
                    .and_then(|runtime| runtime.raw_success_ratio()),
                rate_control
                    .ampdu_runtime()
                    .and_then(|runtime| runtime.filtered_success_ratio()),
                rate_control.latest_ack_snr(),
                rate_control.filtered_ack_snr(),
            );
            let he_tb = (association_phy == StaAssociationPhy::He20).then(|| {
                let statistics = mmio.he_trigger_based_statistics();
                let trigger = (statistics.rx_trigger_count != last_he_rx_trigger_count)
                    .then(|| mmio.he_trigger_receive_diagnostics());
                last_he_rx_trigger_count = statistics.rx_trigger_count;
                let queues =
                    (!he_queue_snapshot_reported).then(|| mmio.he_queue_scheduling_snapshot());
                let rx_diagnostics = (!he_queue_snapshot_reported).then(|| {
                    (
                        mmio.he_color_collision_snapshot(),
                        mmio.rx_statistics_snapshot(),
                    )
                });
                let rx_configuration =
                    (!he_queue_snapshot_reported).then(|| mmio.he_receive_configuration_snapshot());
                (
                    statistics,
                    mmio.he_trigger_based_tx_diagnostics(),
                    mmio.he_buffer_status_snapshot(),
                    queues,
                    rx_diagnostics,
                    rx_configuration,
                    trigger,
                )
            });
            log::info!(
                "OPEN_RADIO_PHY_HIL stage=embassy-net-radio-rx-state \
                 received={received} enqueued={enqueued} dropped={dropped} \
                 duplicates={duplicate_frames} amsdu_frames={amsdu_frames} \
                 amsdu_msdu={amsdu_msdu} \
                 interleave_non_data={rx_interleave_non_data_consumed} \
                 trigger_frames={observed_trigger_frames} \
                 tx_ampdu={tx_ampdu_submissions} tx_ampdu_partial={tx_ampdu_partial} \
                 tx_ampdu_max_subframes={tx_ampdu_max_subframes} \
                 tx_ampdu_max_bytes={tx_ampdu_max_bytes} \
                 tx_ampdu_attempts={tx_ampdu_attempts} \
                 tx_ampdu_individual_retry_mpdu={tx_ampdu_individual_retry_mpdu} \
                 tx_ampdu_spill_frames={tx_ampdu_spill_frames} \
                 tx_ampdu_cadence_samples={tx_ampdu_cadence_samples} \
                 tx_ampdu_avg_us={} tx_ampdu_prep_avg_us={} tx_ampdu_hw_avg_us={} \
                 tx_ampdu_ok_hw_avg_us={} tx_ampdu_s5_hw_avg_us={} \
                 tx_ampdu_other_hw_avg_us={} \
                 tx_attempts={} tx_ok={} tx_ack_timeout={} tx_other={} tx_hw_timeout={} \
                 observed={:#010x} completed={completed} \
                 rx_queue={} tx_queue={} \
                 control={:#010x} base={:#010x} next={:#010x} \
                 last={:#010x} last_high={:#010x} \
                 int_raw={:#010x} int_status={:#010x} tx_state={:#010x} \
                 rc_schedule={:?}/{} rc_rate={:#04x} \
                 tx_rate={:#04x} tx_rate_kbps={} \
                 ack_snr_latest={:?} ack_snr_filtered={:?} \
                 rx_ba={:?}",
                tx_ampdu_elapsed_us
                    .checked_div(u64::from(tx_ampdu_cadence_samples))
                    .unwrap_or(0),
                tx_ampdu_preparation_us
                    .checked_div(u64::from(tx_ampdu_cadence_samples))
                    .unwrap_or(0),
                tx_ampdu_hardware_us
                    .checked_div(u64::from(tx_ampdu_attempts))
                    .unwrap_or(0),
                tx_storage
                    .ampdu_success_wait_us
                    .checked_div(u64::from(tx_storage.ampdu_success_wait_samples))
                    .unwrap_or(0),
                tx_storage
                    .ampdu_status5_wait_us
                    .checked_div(u64::from(tx_storage.ampdu_status5_wait_samples))
                    .unwrap_or(0),
                tx_storage
                    .ampdu_other_wait_us
                    .checked_div(u64::from(tx_storage.ampdu_other_wait_samples))
                    .unwrap_or(0),
                tx_storage.attempts,
                tx_storage.successes,
                tx_storage.ack_timeouts,
                tx_storage.other_failures,
                tx_storage.hardware_timeouts,
                rx_ring.observed_mask(),
                network_runner.rx_queue_len(),
                network_runner.tx_queue_len(),
                mmio.read32(RX_CONTROL),
                mmio.read32(RX_DESCRIPTOR_BASE),
                mmio.read32(RX_NEXT_DESCRIPTOR),
                mmio.read32(RX_LAST_DESCRIPTOR),
                mmio.read32(RX_LAST_DESCRIPTOR_HIGH),
                mmio.read32(MAC_INT_RAW),
                mmio.read32(MAC_INT_STATUS),
                mmio.read32(TX_STATE),
                rate_control.current_schedule().kind,
                rate_control.current_schedule().index,
                schedule_state(rate_control.current_schedule()).rate,
                connected_data_rate.code(),
                connected_data_rate.nominal_kbps(),
                rate_control.latest_ack_snr(),
                rate_control.filtered_ack_snr(),
                rx_ba_state,
            );
            let best_effort_edca = tx_storage.edca_parameters(LegacyTxQueue::BestEffort);
            let best_effort_ecw_current = tx_storage
                .runtime_policy
                .contention_exponent(LegacyTxQueue::BestEffort);
            log::info!(
                "OPEN_RADIO_PHY_HIL stage=tx-runtime \
                 code_address={} \
                 ampdu_submissions={tx_ampdu_submissions} \
                 ampdu_attempts={tx_ampdu_attempts} partial={tx_ampdu_partial} \
                 rx_irqs_during_hw={tx_ampdu_rx_irqs_during_hardware} \
                 rx_service_yields_during_prep={tx_ampdu_rx_service_yields_during_preparation} \
                 tx_attempts={} tx_ok={} ack_timeout={} other={} hw_timeout={} \
                 be_aifsn={} be_ecw_min={} be_ecw_current={} be_ecw_max={}",
                open_radio_runtime_code_marker as *const () as usize,
                tx_storage.attempts,
                tx_storage.successes,
                tx_storage.ack_timeouts,
                tx_storage.other_failures,
                tx_storage.hardware_timeouts,
                best_effort_edca.aifsn(),
                best_effort_edca.minimum_exponent(),
                best_effort_ecw_current,
                best_effort_edca.maximum_exponent(),
            );
            // Keep the critical performance split in a separate bounded log
            // record. The full RX/TX state record is intentionally verbose
            // and the emergency UART formatter may truncate it before these
            // fields, which previously hid whether a throughput regression
            // came from cache-TX/A-MSDU preparation or hardware ownership.
            log::info!(
                "OPEN_RADIO_PHY_HIL stage=tx-ampdu-timing \
                 samples={tx_ampdu_cadence_samples} \
                 elapsed_avg_us={} prep_avg_us={} hw_avg_us={} \
                 bytes_avg={} subframes_avg={} \
                 attempts={tx_ampdu_attempts} partial={tx_ampdu_partial} \
                 individual_retry={tx_ampdu_individual_retry_mpdu} \
                 spill={tx_ampdu_spill_frames} tx_queue={}",
                tx_ampdu_elapsed_us
                    .checked_div(u64::from(tx_ampdu_cadence_samples))
                    .unwrap_or(0),
                tx_ampdu_preparation_us
                    .checked_div(u64::from(tx_ampdu_cadence_samples))
                    .unwrap_or(0),
                tx_ampdu_hardware_us
                    .checked_div(u64::from(tx_ampdu_attempts))
                    .unwrap_or(0),
                tx_ampdu_ethernet_bytes
                    .checked_div(u64::from(tx_ampdu_cadence_samples))
                    .unwrap_or(0),
                tx_ampdu_subframes
                    .checked_div(u64::from(tx_ampdu_cadence_samples))
                    .unwrap_or(0),
                network_runner.tx_queue_len(),
            );
            // SOURCE: complete `_oracles/libpp.a[hal_debug.o]::
            // dbg_read_rx_count`. These are common RX/MAC counters, not
            // HE-only state. Keep the periodic snapshot active for HT and
            // legacy associations too, so every bidirectional profile proves
            // DMA health from the same hardware evidence.
            let rx = mmio.rx_statistics_snapshot();
            let rx_interval_us = last_rx_statistics_at.elapsed().as_micros();
            let rx_delta = rx.primary.wrapping_delta_since(last_rx_primary_statistics);
            last_rx_statistics_at = Instant::now();
            last_rx_primary_statistics = rx.primary;
            log::info!(
                "OPEN_RADIO_PHY_HIL stage=rx-runtime-delta \
                 interval_us={rx_interval_us} mpdu={} fcs={} abort={} \
                 abort_fcs_pass={} data_success={} other_unicast={} \
                 buffer_full={} fifo_overflow={} power_drop={} same_bm={} \
                 signal_field={} end={}",
                rx_delta.mpdu_count,
                rx_delta.fcs_error,
                rx_delta.abort,
                rx_delta.abort_fcs_pass,
                rx_delta.data_success,
                rx_delta.other_unicast,
                rx_delta.buffer_full,
                rx_delta.fifo_overflow,
                rx_delta.power_drop_error,
                rx_delta.same_bm_error,
                rx_delta.signal_field,
                rx_delta.end,
            );
            log::info!(
                "OPEN_RADIO_PHY_HIL stage=rx-runtime-primary \
                 mpdu={} fcs={} abort={} abort_fcs_pass={} data_success={} \
                 other_unicast={} buffer_full={} fifo_overflow={} \
                 power_drop={} same_bm={} signal_field={} end={} \
                 pairwise_mic={} pairwise_hw_reject={} pairwise_reject={} group_mic={} \
                 group_reject={} duplicates={}",
                rx.primary.mpdu_count,
                rx.primary.fcs_error,
                rx.primary.abort,
                rx.primary.abort_fcs_pass,
                rx.primary.data_success,
                rx.primary.other_unicast,
                rx.primary.buffer_full,
                rx.primary.fifo_overflow,
                rx.primary.power_drop_error,
                rx.primary.same_bm_error,
                rx.primary.signal_field,
                rx.primary.end,
                pairwise_mic_failures,
                pairwise_hardware_rejections,
                pairwise_rejections,
                group_mic_failures,
                group_rejections,
                duplicate_frames,
            );
            log::info!(
                "OPEN_RADIO_PHY_HIL stage=rx-runtime-decode \
                 brx_agc={} brx={} nrx={} nrx_abort={} nrx_agc_exit={} \
                 nrx_baseband_off={} nrx_fdm_watchdog={} nrx_restart={} \
                 nrx_service={} nrx_tx_over={} nrx_unsupported={} \
                 nrx_he_format={} nrx_ht_sig={} nrx_he_unsupported={} \
                 nrx_he_sig_a_crc={} hang_rx={} hang_tx={} hang={} panic={}",
                rx.decode_errors.brx_agc,
                rx.decode_errors.brx,
                rx.decode_errors.nrx,
                rx.decode_errors.nrx_abort,
                rx.decode_errors.nrx_agc_exit,
                rx.decode_errors.nrx_baseband_off,
                rx.decode_errors.nrx_fdm_watchdog,
                rx.decode_errors.nrx_restart,
                rx.decode_errors.nrx_service,
                rx.decode_errors.nrx_tx_over,
                rx.decode_errors.nrx_unsupported,
                rx.decode_errors.nrx_he_format,
                rx.decode_errors.nrx_ht_sig,
                rx.decode_errors.nrx_he_unsupported,
                rx.decode_errors.nrx_he_sig_a_crc,
                rx.hang.rx,
                rx.hang.tx,
                rx.hang.rx_tx_hang,
                rx.hang.rx_tx_panic,
            );
            for agreement in active_rx_addba.iter().flatten() {
                log::info!(
                    "OPEN_RADIO_PHY_HIL stage=rx-addba-hardware-state \
                     hardware_index={} value={:?}",
                    agreement.hardware_index,
                    mmio.rx_block_ack_entry_snapshot(agreement.hardware_index),
                );
            }
            if let Some((
                statistics,
                diagnostics,
                bsr,
                queues,
                rx_diagnostics,
                rx_configuration,
                trigger,
            )) = he_tb
            {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=he-tb-statistics value={statistics:?}"
                ));
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=he-bsr value={bsr:?}"
                ));
                if let Some(configuration) = rx_configuration {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=he-rx-config-core \
                         color={} color_en={} partial={} bssid_sel={} he_bssid={} \
                         multi_bssid={} mask={:#04x} cohosted={} pe={} mplen_offset={} \
                         nfrp={} hw_txop={} bsr_update={} tb_stop={} trs={} \
                         ul_data_disable={} ul_mu_disable={} nores_continue={} \
                         autoack_ersu={} he_response_ack={} padding={:?}",
                        configuration.bss_color,
                        configuration.bss_color_enabled,
                        configuration.partial_bss_color_enabled,
                        configuration.bssid_select,
                        configuration.he_bssid_enabled,
                        configuration.multi_bssid_enabled,
                        configuration.multi_bssid_mask,
                        configuration.co_hosted_enabled,
                        configuration.default_packet_extension_duration,
                        configuration.mpdu_length_offset,
                        configuration.nfrp_buffer_threshold,
                        configuration.hardware_txop_enabled,
                        configuration.bsr_update_enabled,
                        configuration.trigger_based_stop_option,
                        configuration.trigger_response_scheduling_supported,
                        configuration.uplink_mu_data_disabled,
                        configuration.uplink_mu_disabled,
                        configuration.trigger_based_no_resource_continue_tx,
                        configuration.automatic_ack_allows_extended_range_su,
                        configuration.he_response_ack,
                        configuration.nominal_packet_padding_duration,
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=he-rx-config-power-save \
                         threshold={} enabled={} stop_rf={} ready={} hop={} \
                         phy_delay={} color_check={} intra_ppdu={} \
                         vht_addr_check={} vht_txop={}",
                        configuration.power_save.threshold,
                        configuration.power_save.enabled,
                        configuration.power_save.stop_rf,
                        configuration.power_save.ready,
                        configuration.power_save.front_end_frequency_hop_time,
                        configuration.power_save.phy_signal_delay,
                        configuration.power_save.intra_bss_color_check_enabled,
                        configuration.power_save.intra_ppdu_enabled,
                        configuration.power_save.vht_txop_address_check_enabled,
                        configuration.power_save.vht_txop_enabled,
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=he-rx-config-types-beam \
                         custom={:?} mem_write={} bfrp_time={} ndp_time={} \
                         hwseq_sel={} hwseq_en={} he_beam={} non_tb_ru={} ru={}",
                        configuration.custom_receive_types,
                        configuration.beamforming.memory_write_enabled,
                        configuration.beamforming.bfrp_time,
                        configuration.beamforming.ndp_time,
                        configuration.beamforming.hardware_sequence_select,
                        configuration.beamforming.hardware_sequence_enabled,
                        configuration.beamforming.he_beam_enabled,
                        configuration.beamforming.non_trigger_based_ru_select,
                        configuration.beamforming.ru_select,
                    ));
                }
                if let Some(queues) = queues {
                    for (queue, config) in queues.trigger_queues.iter().enumerate() {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=he-trigger-queue queue={queue} \
                             tid={} tb={} mu_edca={} mplen_link={} min_tx_power={}",
                            config.tid,
                            config.trigger_based_enabled,
                            config.mu_edca_timer_select,
                            config.mpdu_length_link_address,
                            config.minimum_tx_power,
                        ));
                    }
                    for (queue, config) in queues.edca_queues.iter().enumerate() {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=he-edca-queue queue={queue} \
                             min_mpdu=({},{},{}) sw_rts={} sw_cts={}",
                            config.minimum_mpdu_length_cbw20,
                            config.minimum_mpdu_length_cbw40,
                            config.minimum_mpdu_length_cbw80,
                            config.software_rts,
                            config.software_cts,
                        ));
                    }
                    for (index, timer) in queues.mu_edca_timers.iter().enumerate() {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=he-mu-edca-timer index={index} \
                             timer_8tu={} enabled={} reset={} current_8tu={} \
                             reached={} aifs={}",
                            timer.timer_8tu,
                            timer.enabled,
                            timer.reset,
                            timer.current_count_8tu,
                            timer.reached,
                            timer.aifs,
                        ));
                    }
                    he_queue_snapshot_reported = true;
                }
                if let Some((colors, rx)) = rx_diagnostics {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=he-color-collision \
                         bitmap={:#018x} threshold={} timeout_seconds={} \
                         bitmap_control={} clear={}",
                        colors.observed_color_bitmap,
                        colors.collision_threshold,
                        colors.timeout_seconds,
                        colors.bitmap_control,
                        colors.color_bitmap_clear,
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=rx-primary-a \
                         mpdu={} cfo_scaled_40={} fcs={} abort={} abort_fcs_pass={} \
                         power_drop={} he_sig_b={} same_bm={} signal_field={} end={}",
                        rx.primary.mpdu_count,
                        rx.primary.cfo_scaled_40,
                        rx.primary.fcs_error,
                        rx.primary.abort,
                        rx.primary.abort_fcs_pass,
                        rx.primary.power_drop_error,
                        rx.primary.he_sig_b_error,
                        rx.primary.same_bm_error,
                        rx.primary.signal_field,
                        rx.primary.end,
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=rx-primary-b \
                         data_success={} other_unicast={} buffer_full={} fifo_overflow={} \
                         tkip={} bt_block={} frequency_hop={} last_unmatched={} \
                         ack_interrupt={} rts_interrupt={}",
                        rx.primary.data_success,
                        rx.primary.other_unicast,
                        rx.primary.buffer_full,
                        rx.primary.fifo_overflow,
                        rx.primary.tkip_error,
                        rx.primary.bt_block_error,
                        rx.primary.frequency_hop_error,
                        rx.primary.last_unmatched_error,
                        rx.primary.ack_interrupt,
                        rx.primary.rts_interrupt,
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=rx-decode-errors value={:?}",
                        rx.decode_errors,
                    ));
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=rx-hang value={:?}",
                        rx.hang,
                    ));
                }
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=he-tb-diagnostics \
                     tx_time={} symbols={} pre_fec_padding={} psdu={} min_subframe={} \
                     packet_extension={} tx_20pack={} qos_null_append={} \
                     trigger_type={} uplink_length={} gi_ltf={} tid_limit={}",
                    diagnostics.tx_time,
                    diagnostics.symbol_count,
                    diagnostics.pre_fec_padding_phy,
                    diagnostics.psdu_length,
                    diagnostics.minimum_subframe_length,
                    diagnostics.packet_extension_time,
                    diagnostics.tx_20_packet_count,
                    diagnostics.qos_null_append_count,
                    diagnostics.trigger_type,
                    diagnostics.uplink_length,
                    diagnostics.gi_and_ltf,
                    diagnostics.tid_limit,
                ));
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=he-tb-user \
                     aid={} ru={} mcs={} preferred_ac={} spacing={} packet_extension={}",
                    diagnostics.association_id,
                    diagnostics.ru_allocation,
                    diagnostics.uplink_mcs,
                    diagnostics.basic_preferred_ac,
                    diagnostics.basic_spacing_factor,
                    diagnostics.uplink_packet_extension,
                ));
                // SOURCE[BLOB_LIBPP_DBG_READ_RX_BA,
                // BLOB_LIBPP_DBG_DUMP_TXQ_TXINFO,
                // BLOB_LIBPP_DBG_READ_INTERNAL_TXBA]: these typed PAC
                // snapshots cover the complete per-queue ACK/BlockAck result,
                // LAST_TX_IS_TB/TB_PACK_SENT state and the standalone
                // internal TXBA result. Keep them out of the completion hot
                // path; a ten-second idle snapshot is sufficient to prove
                // whether a received Trigger reached TB scheduling.
                for queue in 0_u8..8 {
                    if let Some(result) = mmio.tx_block_ack_diagnostic_snapshot(queue)
                        && (queue == LegacyTxQueue::BestEffort as u8
                            || result.last_tx_was_trigger_based
                            || result.trigger_based_packet_count != 0)
                    {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL stage=txq-result queue={queue} \
                             ack={} ba={} ack_tid_raw={} last_tb={} tb_packets={} \
                             ssn_control={:#010x} bitmap={:#018x} ta={:02x?}",
                            result.acknowledgement_received,
                            result.block_ack_received,
                            result.acknowledgement_tid,
                            result.last_tx_was_trigger_based,
                            result.trigger_based_packet_count,
                            result.control_and_sequence,
                            u64::from(result.bitmap_low) | (u64::from(result.bitmap_high) << 32),
                            result.transmitter_address,
                        ));
                    }
                }
                let internal_txba = mmio.internal_tx_block_ack_snapshot();
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=internal-txba bitmap={:#018x} \
                     ta_words={:08x?} fragment={} ssn={} tid={}",
                    internal_txba.bitmap,
                    internal_txba.transmitter_address_words,
                    internal_txba.fragment_number,
                    internal_txba.starting_sequence,
                    internal_txba.tid,
                ));
                if let Some(trigger) = trigger {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=he-trigger-rx value={trigger:?}"
                    ));
                }
                if let Some(common) = last_trigger_common {
                    let ru = last_trigger_user.and_then(|bytes| parse_trigger_user_ru(&bytes).ok());
                    let spatial_stream = last_trigger_user
                        .and_then(|bytes| parse_trigger_user_spatial_stream(&bytes).ok());
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=he-trigger-frame count={observed_trigger_frames} \
                         schedule_aid={association_id} \
                         common={common:?} first_user_raw={last_trigger_user:02x?} \
                         first_user_ru={ru:?} first_user_ss={spatial_stream:?} \
                         schedule={last_trigger_schedule:?}"
                    ));
                }
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL stage=he-ndpa count={observed_he_ndpa_frames} \
                     for_us={observed_he_ndpa_for_us} schedule_aid={association_id} \
                     last_dialog_token={last_he_ndpa_dialog_token:?} \
                     adjacent_hw={last_he_ndpa_hardware:?} \
                     current_hw={:?}",
                    mmio.he_beamforming_diagnostics(),
                ));
            }
            if mmio.read32(RX_NEXT_DESCRIPTOR) == 0 {
                for (chunk, descriptors) in rx_storage.descriptors.chunks(8).enumerate() {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL probe=rx-terminal-descriptors chunk={chunk} \
                         word0={:08x?}",
                        [
                            descriptors[0].word0(),
                            descriptors[1].word0(),
                            descriptors[2].word0(),
                            descriptors[3].word0(),
                            descriptors[4].word0(),
                            descriptors[5].word0(),
                            descriptors[6].word0(),
                            descriptors[7].word0(),
                        ],
                    ));
                }
            }
        }

        if !lan_probe_sent
            && OPEN_RADIO_LAN_PROBE_READY.load(Ordering::Acquire)
            && (!matches!(connected_data_rate, TxPhyRate::He(_))
                || tx_block_ack.operational().is_some())
        {
            lan_probe_sent = true;
            let local_ipv4 = OPEN_RADIO_LOCAL_IPV4.load(Ordering::Acquire).to_be_bytes();
            let ethernet = lan_arp_probe(station_address, local_ipv4);
            match transmit_connected_protected_ethernet_frame(
                mmio,
                tx_storage,
                tx_ampdu_storage.as_mut(),
                bssid,
                pairwise_slot,
                sequences,
                association_phy,
                peer_qos,
                connected_data_rate,
                network_runner,
                &ethernet,
            )
            .await
            {
                Ok(completion) if completion.status == 0 => {
                    rate_control.observe_tx_completion(completion);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-lan-arp-prime-tx \
                         source={}.{}.{}.{} target={}.{}.{}.{}",
                        local_ipv4[0],
                        local_ipv4[1],
                        local_ipv4[2],
                        local_ipv4[3],
                        LAN_PROBE_IPV4[0],
                        LAN_PROBE_IPV4[1],
                        LAN_PROBE_IPV4[2],
                        LAN_PROBE_IPV4[3],
                    ));
                }
                Ok(completion) => emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-lan-arp-prime \
                     status={}",
                    completion.status,
                )),
                Err(error) => emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-lan-arp-prime \
                     error={error:?}",
                )),
            }
        }

        if OPEN_RADIO_RAW_MAC_BENCH
            && tx_block_ack
                .operational()
                .is_some_and(|agreement| !OPEN_RADIO_AMSDU_BENCH || agreement.amsdu)
        {
            // The synthetic data source owns only benchmark airtime, not the
            // station control plane. Drain one stack-produced frame first so
            // ARP replies and DHCP renewal cannot remain behind an infinite
            // raw A-MPDU stream.
            //
            // SOURCE[HIL_OPEN_HT40_BIDIRECTIONAL_2026_07_29]: before this
            // drain, the Linux AP successfully sent the first 5-second UDP
            // window and then changed the station neighbor from REACHABLE to
            // FAILED. The RX path had enqueued the ARP request, but this raw
            // branch always reached `continue` before `receive_tx`, so the
            // Embassy-generated ARP reply had no hardware submission path.
            if let Some(control) = network_runner.try_receive_tx() {
                // HE has a distinct S-MPDU/A-MPDU formatter and must not be
                // routed through the legacy/HT `TxSlot`. Use the connected
                // dispatcher so HE control traffic becomes a one-member HE
                // A-MPDU while HT/legacy retains the ordinary descriptor.
                //
                // SOURCE[HIL_OPEN_HE20_AMSDU_CONTROL_2026_07_30]: FRITZ!Box
                // HE20/MCS9 HIL reached 78..85 Mbit/s with clean BlockAck,
                // but DHCP/ARP frames failed as `Reserve(Invalid)` because
                // this branch passed `TxPhyRate::He` to the deliberately
                // legacy/HT-only `transmit_encoded_frame`.
                match transmit_connected_protected_ethernet_frame(
                    mmio,
                    tx_storage,
                    tx_ampdu_storage.as_mut(),
                    bssid,
                    pairwise_slot,
                    sequences,
                    association_phy,
                    peer_qos,
                    connected_data_rate,
                    network_runner,
                    control.as_slice(),
                )
                .await
                {
                    Ok(completion) if completion.status == 0 => {
                        rate_control.observe_tx_completion(completion);
                        raw_mac_control_tx = raw_mac_control_tx.saturating_add(1);
                        last_network_activity = Instant::now();
                        if raw_mac_control_tx <= 4 {
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=PASS \
                                 stage=raw-mac-control-tx frame={} bytes={}",
                                raw_mac_control_tx,
                                control.len(),
                            ));
                        }
                    }
                    Ok(completion) => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=raw-mac-control-tx \
                         status={} bytes={}",
                        completion.status,
                        control.len(),
                    )),
                    Err(error) => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=raw-mac-control-tx \
                         error={error:?} bytes={}",
                        control.len(),
                    )),
                }
            }

            // SOURCE[HIL_OPEN_HT40_DMA_CONTENTION_2026_07_29]: immediately
            // filling a second 47,104-byte internal-SRAM pool while Wi-Fi DMA
            // transmitted the first increased status-five completions from
            // about 2.5% to 14% and reduced Ethernet goodput from 96..99 to
            // 85..88 Mbit/s. Busy-polling completion similarly reached about
            // 14% status-five and only 72..80 Mbit/s. Keep DMA-buffer writes
            // outside hardware ownership and retain cooperative completion
            // polling; only the unnecessary pre-fill yield is omitted above.
            let matrix_rate = he_matrix_active.then(|| active_he_matrix_rate(he_matrix_profile));
            let active_ampdu_limit = matrix_rate
                .map(|rate| he_matrix_ampdu_limit(rate, raw_mac_frame.len(), best_effort_txop))
                .unwrap_or(OPEN_RADIO_AMPDU_LIMIT);
            if he_matrix_active {
                he_matrix_submissions = he_matrix_submissions.saturating_add(1);
            }
            // Bracket each synthetic aggregate with the complete vendor RX
            // statistics image. A DCM run once showed WDEVRX_MPDU and
            // WDEVRX_BUF_FULLCNT advancing by nearly the number of outbound
            // aggregate members, despite clean BlockAck and only a few
            // RX-success edges. Profile-local accounting distinguishes a
            // genuinely exhausted descriptor ring from a DCM/BlockAck
            // hardware-statistics side effect without adding logging to the
            // packet-latency window.
            //
            // SOURCE: complete `_oracles/libpp.a[hal_debug.o]::
            // dbg_read_rx_count` proves both counter addresses and widths.
            // SOURCE: complete `_oracles/libpp.a[wdev.o]::
            // wDev_ProcessFiq` proves RX-success is MAC interrupt bit 0x4000.
            let matrix_rx_before = he_matrix_active.then(|| {
                (
                    mmio.rx_statistics_snapshot().primary,
                    OPEN_RADIO_IRQ_RUNTIME.rx_post_count(),
                    received,
                )
            });
            let transmission_result = {
                // The cell contains the existing unique PAC owner; it does
                // not steal or duplicate any register token. TX borrows it
                // only for finite synchronous transactions, allowing the
                // same task to honor a pending RX-success event while the
                // aggregate future is asleep on its completion signal.
                let registers = RefCell::new(&mut *mmio);
                let mut tx_hardware = CooperativeTxHardware::new(&registers);
                let mut transmission = core::pin::pin!(transmit_protected_ethernet_ampdu(
                    &mut tx_hardware,
                    tx_storage,
                    tx_ampdu_storage.as_mut(),
                    bssid,
                    pairwise_slot,
                    sequences
                        .qos_mut(0)
                        .expect("TID0 sequence-number owner exists"),
                    connected_ampdu_rate,
                    network_runner,
                    raw_mac_frame,
                    raw_mac_frame,
                    Some(raw_mac_frame),
                    raw_mac_amsdu_slots_initialized,
                    matrix_rate,
                    best_effort_txop,
                    active_ampdu_limit,
                ));
                let mut rx_service_failed = false;
                loop {
                    // Poll RX first. This reproduces the exact ordering in
                    // wDev_ProcessFiq when RX_SUCCESS and TX_COMPLETE share
                    // one hardware status snapshot.
                    match select(OPEN_RADIO_IRQ_RUNTIME.wait_rx(), transmission.as_mut()).await {
                        Either::First(()) if !rx_service_failed => {
                            let mut registers = registers.borrow_mut();
                            if let Err(error) = service_benchmark_rx_during_tx(
                                &mut registers,
                                rx_storage,
                                &mut rx_ring,
                                &mut rx_staging_queue,
                                frame,
                                ethernet,
                                network_runner,
                                station_address,
                                bssid,
                                association_id,
                                association_phy,
                                &mut pending_rx_addba,
                                &mut active_rx_addba,
                                &mut tx_block_ack,
                                &mut tx_block_ack_alarm,
                                &mut tx_block_ack_tid7,
                                &mut tx_block_ack_tid7_alarm,
                                &mut tx_block_ack_tid5,
                                &mut tx_block_ack_tid5_alarm,
                                &mut duplicate_filter,
                                &mut direct_udp_benchmark,
                                &mut received,
                                &mut enqueued,
                                &mut dropped,
                                &mut pairwise_mic_failures,
                                &mut pairwise_hardware_rejections,
                                &mut pairwise_rejections,
                                &mut duplicate_frames,
                                &mut amsdu_frames,
                                &mut amsdu_msdu,
                                &mut rx_interleave_non_data_consumed,
                                &mut observed_trigger_frames,
                                &mut last_trigger_common,
                                &mut last_trigger_schedule,
                                &mut last_trigger_user,
                                &mut observed_he_ndpa_frames,
                                &mut observed_he_ndpa_for_us,
                                &mut last_he_ndpa_dialog_token,
                                &mut last_he_ndpa_hardware,
                                &mut last_network_activity,
                            ) {
                                rx_service_failed = true;
                                emergency_log(format_args!(
                                    "OPEN_RADIO_PHY_HIL result=FAIL \
                                     stage=rx-interleave error={error:?}"
                                ));
                            }
                        }
                        Either::First(()) => {}
                        Either::Second(result) => break result,
                    }
                }
            };
            if let Some((before, irq_before, staged_before)) = matrix_rx_before {
                let after = mmio.rx_statistics_snapshot().primary;
                let delta = after.wrapping_delta_since(before);
                he_matrix_rx_mpdu = he_matrix_rx_mpdu.saturating_add(u32::from(delta.mpdu_count));
                he_matrix_rx_buffer_full =
                    he_matrix_rx_buffer_full.saturating_add(u32::from(delta.buffer_full));
                he_matrix_rx_irq = he_matrix_rx_irq.saturating_add(
                    OPEN_RADIO_IRQ_RUNTIME
                        .rx_post_count()
                        .wrapping_sub(irq_before),
                );
                he_matrix_rx_staged =
                    he_matrix_rx_staged.saturating_add(received.wrapping_sub(staged_before));
            }
            match transmission_result {
                Ok(report) => {
                    rate_control.observe_tx_completion(report.completion.tx);
                    if !he_matrix_active && !report.trigger_flow_completed {
                        observe_ampdu_rate_control(
                            rate_control,
                            report.block_ack_mpdu_attempts,
                            report.acknowledged,
                        );
                    }
                    if tx_ampdu_cadence_samples == 0 {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=PASS stage=raw-mac-first-ampdu \
                             phy={} rate_code={:#04x} rate_kbps={} \
                             subframes={} acknowledged={} tb_terminal={} attempts={} status={} \
                             ba_start={} ba_bitmap={:#018x} \
                             empty_delimiters={} \
                             plcp1={:#010x} he_a1={:#010x} he_a2={:#010x} \
                             he_control={:#010x} he_control_sw={} \
                             power={:#010x}",
                            association_phy.name(),
                            report.rate.code(),
                            report.rate.nominal_kbps(),
                            report.subframes,
                            report.acknowledged,
                            report.trigger_flow_completed,
                            report.aggregate_attempts,
                            report.completion.tx.status,
                            report.completion.block_ack.block_ack.starting_sequence,
                            report.completion.block_ack.block_ack.bitmap,
                            report.first_empty_delimiters,
                            report.he_vector.map_or(0, |vector| vector.plcp1),
                            report.he_vector.map_or(0, |vector| vector.he_signal_a1),
                            report
                                .he_vector
                                .map_or(0, |vector| vector.he_signal_a2_length),
                            report.he_vector.map_or(0, |vector| vector.he_control),
                            report
                                .he_vector
                                .is_some_and(|vector| vector.software_he_control_enabled),
                            report.he_vector.map_or(0, |vector| vector.power),
                        ));
                        if let Some(trigger) = report.he_trigger {
                            let structure_valid = trigger.logical_queue
                                == LegacyTxQueue::BestEffort as u8
                                && trigger.tid == 0
                                && trigger.trigger_based_enabled
                                && trigger.mu_edca_timer_select == LegacyTxQueue::BestEffort as u8
                                && trigger.mu_edca_timer_enabled
                                && trigger.first_mpdu_length != 0
                                && trigger.first_next_link != 0x7f
                                && trigger.programmed_msdu_bytes == report.ethernet_bytes as u32
                                && trigger.queue_valid;
                            let publication_result = if structure_valid
                                && trigger.queued_msdu_bytes == report.ethernet_bytes as u32
                            {
                                "PASS"
                            } else if structure_valid && trigger.queued_msdu_bytes == 0 {
                                // On rev0 with BSR_UPDATE_ENABLE set, the
                                // instruction-exact write/valid sequence can
                                // expose validity while the read-side value
                                // has already returned to zero. A real AP
                                // Trigger is required to distinguish a
                                // consumed latch from a rejected write.
                                "OBSERVE"
                            } else {
                                "FAIL"
                            };
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result={} \
                                 stage=he-tb-queue-publish queue={} tid={} \
                                 tb_enable={} mu_edca_select={} mu_edca_enable={} \
                                 first_link={} first_length={} first_next={} tail={} \
                                 programmed_msdu_bytes={} \
                                 queued_msdu_bytes_after_valid={} queue_valid={}",
                                publication_result,
                                trigger.logical_queue,
                                trigger.tid,
                                trigger.trigger_based_enabled,
                                trigger.mu_edca_timer_select,
                                trigger.mu_edca_timer_enabled,
                                trigger.first_link,
                                trigger.first_mpdu_length,
                                trigger.first_next_link,
                                trigger.tail_link,
                                trigger.programmed_msdu_bytes,
                                trigger.queued_msdu_bytes,
                                trigger.queue_valid,
                            ));
                        }
                    }
                    raw_mac_amsdu_slots_initialized |= OPEN_RADIO_AMSDU_BENCH;
                    tx_ampdu_cadence_samples = tx_ampdu_cadence_samples.saturating_add(1);
                    tx_ampdu_attempts =
                        tx_ampdu_attempts.saturating_add(u32::from(report.aggregate_attempts));
                    tx_ampdu_elapsed_us = tx_ampdu_elapsed_us.saturating_add(report.elapsed_us);
                    tx_ampdu_hardware_us = tx_ampdu_hardware_us.saturating_add(report.hardware_us);
                    tx_ampdu_rx_irqs_during_hardware = tx_ampdu_rx_irqs_during_hardware
                        .saturating_add(report.rx_irqs_during_hardware);
                    tx_ampdu_rx_service_yields_during_preparation =
                        tx_ampdu_rx_service_yields_during_preparation
                            .saturating_add(report.rx_service_yields_during_preparation);
                    tx_ampdu_preparation_us =
                        tx_ampdu_preparation_us.saturating_add(report.preparation_us);
                    tx_ampdu_ethernet_bytes =
                        tx_ampdu_ethernet_bytes.saturating_add(report.ethernet_bytes as u64);
                    tx_ampdu_subframes =
                        tx_ampdu_subframes.saturating_add(u64::from(report.subframes));
                    tx_ampdu_submissions = tx_ampdu_submissions.saturating_add(1);
                    tx_ampdu_max_subframes = tx_ampdu_max_subframes.max(report.subframes);
                    tx_ampdu_max_bytes = tx_ampdu_max_bytes.max(report.ethernet_bytes);
                    if report.retry_failures != 0 {
                        tx_ampdu_partial = tx_ampdu_partial.saturating_add(1);
                    }
                    raw_mac_rate = report.rate;
                    raw_mac_bytes = raw_mac_bytes.saturating_add(report.ethernet_bytes as u64);
                    if he_matrix_active {
                        he_matrix_aggregate_attempts = he_matrix_aggregate_attempts
                            .saturating_add(u32::from(report.aggregate_attempts));
                        he_matrix_retry_failures = he_matrix_retry_failures
                            .saturating_add(u32::from(report.retry_failures));
                        he_matrix_bytes =
                            he_matrix_bytes.saturating_add(report.ethernet_bytes as u64);
                        he_matrix_max_subframes = he_matrix_max_subframes.max(report.subframes);
                        if (report.acknowledged == report.subframes
                            || report.trigger_flow_completed)
                            && report.retry_failures == 0
                        {
                            he_matrix_complete = he_matrix_complete.saturating_add(1);
                        }
                    }
                }
                Err(error) => {
                    if he_matrix_active {
                        he_matrix_errors = he_matrix_errors.saturating_add(1);
                    }
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=raw-mac-tx error={error:?}"
                    ));
                }
            }
            if he_matrix_active && he_matrix_submissions >= HE_MATRIX_AGGREGATES_PER_PROFILE {
                let rate = active_he_matrix_rate(he_matrix_profile);
                let elapsed_us = he_matrix_started.elapsed().as_micros().max(1);
                let throughput_kbps = he_matrix_bytes
                    .saturating_mul(8)
                    .saturating_mul(1_000)
                    .checked_div(elapsed_us)
                    .unwrap_or(0);
                // A partial BlockAck is a normal MAC retry event. It is not a
                // lost MPDU when the bounded retry/fallback path reports zero
                // retry failures. Require at least one fully acknowledged HE
                // aggregate to prove that this PHY profile is understood; this
                // still distinguishes an unsupported selector whose every HE
                // attempt times out before HT fallback delivers the payload.
                let profile_passed = he_matrix_complete != 0
                    && he_matrix_errors == 0
                    && he_matrix_retry_failures == 0;
                if !profile_passed {
                    he_matrix_failed_profiles = he_matrix_failed_profiles.saturating_add(1);
                }
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result={} stage=he20-matrix \
                     round={he_matrix_round} profile={he_matrix_profile} \
                     mcs={} dcm={} ldpc={} gi_ltf={} gi_ns={} ltf={} rate_code={:#04x} \
                     rate_kbps={} ampdu_limit={active_ampdu_limit} \
                     submissions={he_matrix_submissions} complete={he_matrix_complete} \
                     errors={he_matrix_errors} aggregate_attempts={he_matrix_aggregate_attempts} \
                     retry_failures={he_matrix_retry_failures} \
                     max_subframes={he_matrix_max_subframes} bytes={he_matrix_bytes} \
                     rx_mpdu={he_matrix_rx_mpdu} \
                     rx_buffer_full={he_matrix_rx_buffer_full} \
                     rx_irq={he_matrix_rx_irq} rx_staged={he_matrix_rx_staged} \
                     elapsed_us={elapsed_us} throughput_kbps={throughput_kbps}",
                    if profile_passed { "PASS" } else { "FAIL" },
                    rate.mcs().index(),
                    rate.is_dcm(),
                    rate.is_ldpc(),
                    rate.guard_interval_and_ltf().encoding(),
                    rate.guard_interval_and_ltf().guard_interval_ns(),
                    rate.guard_interval_and_ltf().ltf_count(),
                    rate.code(),
                    rate.nominal_kbps(),
                ));
                he_matrix_profile += 1;
                if he_matrix_profile == he_matrix_profile_count {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result={} stage=he20-matrix-round \
                         round={he_matrix_round} profiles={} failed_profiles={}",
                        if he_matrix_failed_profiles == 0 {
                            "PASS"
                        } else {
                            "FAIL"
                        },
                        he_matrix_profile_count - he_matrix_first_profile,
                        he_matrix_failed_profiles,
                    ));
                    he_matrix_profile = he_matrix_first_profile;
                    he_matrix_round = he_matrix_round.saturating_add(1);
                    he_matrix_failed_profiles = 0;
                }
                he_matrix_started = Instant::now();
                he_matrix_submissions = 0;
                he_matrix_complete = 0;
                he_matrix_errors = 0;
                he_matrix_aggregate_attempts = 0;
                he_matrix_retry_failures = 0;
                he_matrix_bytes = 0;
                he_matrix_max_subframes = 0;
                he_matrix_rx_mpdu = 0;
                he_matrix_rx_buffer_full = 0;
                he_matrix_rx_irq = 0;
                he_matrix_rx_staged = 0;
            }
            if raw_mac_started.elapsed() >= OPEN_RADIO_UDP_TX_BENCH_DURATION {
                let elapsed_us = raw_mac_started.elapsed().as_micros().max(1);
                let throughput_kbps = raw_mac_bytes
                    .saturating_mul(8)
                    .saturating_mul(1_000)
                    .checked_div(elapsed_us)
                    .unwrap_or(0);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=BENCH stage=raw-mac-tx \
                     bytes={raw_mac_bytes} elapsed_us={elapsed_us} \
                     throughput_kbps={throughput_kbps} \
                     bandwidth_mhz={} phy={} rate_code={:#04x} rate_kbps={} \
                     ampdu_limit={OPEN_RADIO_AMPDU_LIMIT} \
                     ampdu_coalesce_us={TX_AMPDU_COALESCE_US} \
                     amsdu_body_reuse={raw_mac_amsdu_slots_initialized}",
                    association_phy.bandwidth_mhz(),
                    association_phy.name(),
                    raw_mac_rate.code(),
                    raw_mac_rate.nominal_kbps(),
                ));
                raw_mac_started = Instant::now();
                raw_mac_bytes = 0;
            }
            continue;
        }

        match select(network_runner.receive_tx(), Timer::after_millis(1)).await {
            Either::First(owned) => {
                if let Some(agreement) = tx_block_ack.operational() {
                    let agreement_amsdu = agreement.amsdu;
                    let ingress_policy =
                        ReferencedAmpduIngressPolicy::for_rate(connected_ampdu_rate)
                            .expect("an operational BlockAck session uses HT or HE");
                    // HE A-MPDU can legally contain one MPDU. Claim only the
                    // first pinned network lease here; the referenced batch
                    // checks the exact rate/TXOP APEP ceiling before claiming
                    // every following lease. This matters for DCM MCS0:
                    // its 1,850-byte vendor APEP ceiling admits one full-size
                    // Ethernet frame but not the two frames previously
                    // claimed here. The rejected second frame then escaped
                    // through the 54-Mbit/s legacy spill path, so a nominal
                    // fixed-DCM run mixed two physical PHY formats.
                    //
                    // SOURCE: ROM rev0 `he_max_apep_length` and complete
                    // `_oracles/libpp.a[pp_he.o]::ppCheckTxHEAMPDUlength`;
                    // live Linux AP station statistics on 2026-07-31 exposed
                    // alternating HE-MCS0/DCM1 at 4.3 Mbit/s and legacy
                    // 54 Mbit/s before this ownership edge was corrected.
                    let mut second = if ingress_policy.prefetch_second() {
                        network_runner.try_receive_tx()
                    } else {
                        None
                    };
                    if second.is_none()
                        && OPEN_RADIO_NETWORK_AMSDU_BENCH
                        && agreement_amsdu
                        && matches!(connected_ampdu_rate, TxPhyRate::Ht(_))
                    {
                        // Pairing two MSDUs is the minimum complete cache-TX
                        // unit for this A-MSDU policy. After a full aggregate,
                        // the producer and radio tasks can meet at opposite
                        // sides of the same Embassy scheduling edge: the first
                        // lease is ready while the second is about to be
                        // published. Wait only within the configured coalesce
                        // budget, then preserve the ordinary single-MPDU
                        // fallback for sparse traffic.
                        if TX_AMPDU_COALESCE_US == 0 {
                            embassy_futures::yield_now().await;
                            second = network_runner.try_receive_tx();
                        } else {
                            second = match select(
                                network_runner.receive_tx(),
                                Timer::after_micros(TX_AMPDU_COALESCE_US),
                            )
                            .await
                            {
                                Either::First(frame) => Some(frame),
                                Either::Second(()) => None,
                            };
                        }
                    }
                    // HE has a distinct S-MPDU/A-MPDU DMA owner. A first
                    // Ethernet lease starts a legal one-member HE A-MPDU;
                    // the referenced batch may append more only after its
                    // exact `can_push_he` gate says the next maximum-sized
                    // frame fits. This remains an actual HE20 transmission
                    // and uses the negotiated BlockAck agreement.
                    //
                    // SOURCE: complete `_oracles/libpp.a[pp_he.o]` separates
                    // HE S-MPDU and A-MPDU formatters; open HIL on 2026-07-30
                    // proved the HE A-MPDU formatter at MCS9/LDPC and exposed
                    // the ordinary TxSlot ownership leak when no second
                    // Embassy frame was queued.
                    if ingress_policy.ready(second.is_some()) {
                        let first_bytes = owned.len();
                        let second_bytes = second.as_ref().map_or(0, NetworkTxFrame::len);
                        let ampdu_limit = OPEN_RADIO_AMPDU_LIMIT;
                        let transmission = {
                            // RX-success has priority over TX-complete in the
                            // complete vendor FIQ path. Drive the ordinary
                            // cache-TX future through the same RX-first
                            // dispatcher already qualified by raw A-MPDU HIL;
                            // otherwise a saturated network aggregate can
                            // hold the 32-entry RX ring until BlockAck and
                            // individual retry have both completed.
                            //
                            // SOURCE: complete `_oracles/libpp.a[wdev.o]::
                            // wDev_ProcessFiq` handles RX_SUCCESS 0x4000
                            // before TX_COMPLETE 0x80.
                            let registers = RefCell::new(&mut *mmio);
                            let mut tx_hardware = CooperativeTxHardware::new(&registers);
                            let mut transmission = core::pin::pin!(async {
                                match connected_ampdu_rate {
                                    TxPhyRate::Ht(_) => {
                                        transmit_referenced_protected_ethernet_ampdu(
                                            &mut tx_hardware,
                                            tx_storage,
                                            tx_ampdu_storage.as_mut(),
                                            bssid,
                                            pairwise_slot,
                                            sequences
                                                .qos_mut(0)
                                                .expect("TID0 sequence-number owner exists"),
                                            connected_ampdu_rate,
                                            network_runner,
                                            owned,
                                            Some(
                                                second
                                                    .expect("HT A-MPDU requires two queued frames"),
                                            ),
                                            ampdu_limit,
                                            OPEN_RADIO_NETWORK_AMSDU_BENCH && agreement_amsdu,
                                            best_effort_txop,
                                        )
                                        .await
                                    }
                                    TxPhyRate::He(_) => {
                                        transmit_referenced_protected_ethernet_ampdu(
                                            &mut tx_hardware,
                                            tx_storage,
                                            tx_ampdu_storage.as_mut(),
                                            bssid,
                                            pairwise_slot,
                                            sequences
                                                .qos_mut(0)
                                                .expect("TID0 sequence-number owner exists"),
                                            connected_ampdu_rate,
                                            network_runner,
                                            owned,
                                            second,
                                            ampdu_limit,
                                            false,
                                            best_effort_txop,
                                        )
                                        .await
                                    }
                                    TxPhyRate::Legacy(_) => {
                                        unreachable!("BlockAck data rate is HT or HE")
                                    }
                                }
                            });
                            let mut rx_service_failed = false;
                            loop {
                                match select(
                                    OPEN_RADIO_IRQ_RUNTIME.wait_rx(),
                                    transmission.as_mut(),
                                )
                                .await
                                {
                                    Either::First(()) if !rx_service_failed => {
                                        let mut registers = registers.borrow_mut();
                                        if let Err(error) = service_benchmark_rx_during_tx(
                                            &mut registers,
                                            rx_storage,
                                            &mut rx_ring,
                                            &mut rx_staging_queue,
                                            frame,
                                            ethernet,
                                            network_runner,
                                            station_address,
                                            bssid,
                                            association_id,
                                            association_phy,
                                            &mut pending_rx_addba,
                                            &mut active_rx_addba,
                                            &mut tx_block_ack,
                                            &mut tx_block_ack_alarm,
                                            &mut tx_block_ack_tid7,
                                            &mut tx_block_ack_tid7_alarm,
                                            &mut tx_block_ack_tid5,
                                            &mut tx_block_ack_tid5_alarm,
                                            &mut duplicate_filter,
                                            &mut direct_udp_benchmark,
                                            &mut received,
                                            &mut enqueued,
                                            &mut dropped,
                                            &mut pairwise_mic_failures,
                                            &mut pairwise_hardware_rejections,
                                            &mut pairwise_rejections,
                                            &mut duplicate_frames,
                                            &mut amsdu_frames,
                                            &mut amsdu_msdu,
                                            &mut rx_interleave_non_data_consumed,
                                            &mut observed_trigger_frames,
                                            &mut last_trigger_common,
                                            &mut last_trigger_schedule,
                                            &mut last_trigger_user,
                                            &mut observed_he_ndpa_frames,
                                            &mut observed_he_ndpa_for_us,
                                            &mut last_he_ndpa_dialog_token,
                                            &mut last_he_ndpa_hardware,
                                            &mut last_network_activity,
                                        ) {
                                            rx_service_failed = true;
                                            emergency_log(format_args!(
                                                "OPEN_RADIO_PHY_HIL result=FAIL \
                                                 stage=rx-interleave error={error:?}"
                                            ));
                                        }
                                    }
                                    Either::First(()) => {}
                                    Either::Second(result) => break result,
                                }
                            }
                        };
                        match transmission {
                            Ok(report) => {
                                rate_control.observe_tx_completion(report.completion.tx);
                                if !report.trigger_flow_completed {
                                    observe_ampdu_rate_control(
                                        rate_control,
                                        report.block_ack_mpdu_attempts,
                                        report.acknowledged,
                                    );
                                }
                                let ProtectedEthernetAmpduReport {
                                    completion,
                                    rate,
                                    he_vector: _,
                                    he_trigger: _,
                                    subframes,
                                    ethernet_bytes,
                                    acknowledged,
                                    trigger_flow_completed,
                                    retry_failures,
                                    aggregate_attempts,
                                    block_ack_mpdu_attempts: _,
                                    individual_retry_mpdu,
                                    spill_frames,
                                    elapsed_us,
                                    hardware_us,
                                    rx_irqs_during_hardware,
                                    rx_service_yields_during_preparation,
                                    preparation_us,
                                    first_empty_delimiters: _,
                                } = report;
                                tx_ampdu_cadence_samples =
                                    tx_ampdu_cadence_samples.saturating_add(1);
                                tx_ampdu_attempts =
                                    tx_ampdu_attempts.saturating_add(u32::from(aggregate_attempts));
                                tx_ampdu_individual_retry_mpdu = tx_ampdu_individual_retry_mpdu
                                    .saturating_add(u32::from(individual_retry_mpdu));
                                tx_ampdu_spill_frames =
                                    tx_ampdu_spill_frames.saturating_add(u32::from(spill_frames));
                                tx_ampdu_elapsed_us =
                                    tx_ampdu_elapsed_us.saturating_add(elapsed_us);
                                tx_ampdu_hardware_us =
                                    tx_ampdu_hardware_us.saturating_add(hardware_us);
                                tx_ampdu_rx_irqs_during_hardware = tx_ampdu_rx_irqs_during_hardware
                                    .saturating_add(rx_irqs_during_hardware);
                                tx_ampdu_rx_service_yields_during_preparation =
                                    tx_ampdu_rx_service_yields_during_preparation
                                        .saturating_add(rx_service_yields_during_preparation);
                                tx_ampdu_preparation_us =
                                    tx_ampdu_preparation_us.saturating_add(preparation_us);
                                tx_ampdu_ethernet_bytes =
                                    tx_ampdu_ethernet_bytes.saturating_add(ethernet_bytes as u64);
                                tx_ampdu_subframes =
                                    tx_ampdu_subframes.saturating_add(u64::from(subframes));
                                tx_ampdu_max_subframes = tx_ampdu_max_subframes.max(subframes);
                                tx_ampdu_max_bytes = tx_ampdu_max_bytes.max(ethernet_bytes);
                                if (acknowledged == subframes || trigger_flow_completed)
                                    && retry_failures == 0
                                {
                                    last_network_activity = Instant::now();
                                    tx_ampdu_submissions = tx_ampdu_submissions.saturating_add(1);
                                    if tx_ampdu_submissions == 1 {
                                        emergency_log(format_args!(
                                            "OPEN_RADIO_PHY_HIL result=PASS stage=tx-ampdu-first \
                                         subframes={subframes} bytes={ethernet_bytes} \
                                         tb_terminal={trigger_flow_completed} \
                                         rate_code={:#04x} rate_kbps={} \
                                         ba_start={} ba_bitmap={:#018x}",
                                            rate.code(),
                                            rate.nominal_kbps(),
                                            completion.block_ack.block_ack.starting_sequence,
                                            completion.block_ack.block_ack.bitmap,
                                        ));
                                        emergency_log(format_args!(
                                            "OPEN_RADIO_PHY_HIL stage=tx-rate-feedback \
                                         ack_snr_latest={:?} ack_snr_filtered={:?}",
                                            rate_control.latest_ack_snr(),
                                            rate_control.filtered_ack_snr(),
                                        ));
                                    }
                                } else if retry_failures == 0 {
                                    last_network_activity = Instant::now();
                                    tx_ampdu_submissions = tx_ampdu_submissions.saturating_add(1);
                                    tx_ampdu_partial = tx_ampdu_partial.saturating_add(1);
                                    if !OPEN_RADIO_THROUGHPUT_BENCH || tx_ampdu_partial <= 4 {
                                        emergency_log(format_args!(
                                            "OPEN_RADIO_PHY_HIL result=PASS \
                                         stage=tx-ampdu-partial-retry status={} \
                                         subframes={subframes} acknowledged={acknowledged} \
                                         ba_control={:#04x} ba_start={} ba_bitmap={:#018x}",
                                            completion.tx.status,
                                            completion.block_ack.control,
                                            completion.block_ack.block_ack.starting_sequence,
                                            completion.block_ack.block_ack.bitmap,
                                        ));
                                    }
                                } else {
                                    tx_ampdu_partial = tx_ampdu_partial.saturating_add(1);
                                    if !OPEN_RADIO_THROUGHPUT_BENCH || tx_ampdu_partial <= 4 {
                                        emergency_log(format_args!(
                                            "OPEN_RADIO_PHY_HIL result=FAIL \
                                         stage=tx-ampdu-partial-retry status={} \
                                         subframes={subframes} acknowledged={acknowledged} \
                                         retry_failures={retry_failures} ba_control={:#04x} \
                                         ba_start={} ba_bitmap={:#018x}",
                                            completion.tx.status,
                                            completion.block_ack.control,
                                            completion.block_ack.block_ack.starting_sequence,
                                            completion.block_ack.block_ack.bitmap,
                                        ));
                                    }
                                }
                            }
                            Err(error) => emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-ampdu error={error:?} \
                             first_bytes={} second_bytes={}",
                                first_bytes, second_bytes,
                            )),
                        }
                        continue;
                    }
                }
                match transmit_protected_ethernet_frame(
                    mmio,
                    tx_storage,
                    bssid,
                    pairwise_slot,
                    sequences
                        .take_data(peer_qos.then_some(0))
                        .expect("selected data sequence-number owner exists"),
                    peer_qos,
                    connected_data_rate,
                    owned.as_slice(),
                )
                .await
                {
                    Ok(completion) if completion.status == 0 => {
                        rate_control.observe_tx_completion(completion);
                        last_network_activity = Instant::now();
                    }
                    Ok(completion) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-radio-tx \
                             status={} bytes={}",
                            completion.status,
                            owned.len(),
                        ));
                    }
                    Err(error) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=embassy-net-radio-tx \
                             error={error:?} bytes={}",
                            owned.len(),
                        ));
                    }
                }
            }
            Either::Second(()) => {}
        }
    }
}

enum ConnectedRxRoute {
    Ignored,
    Benchmark(Option<DirectUdpRxSample>),
    Enqueued,
    Dropped,
}

#[inline(never)]
#[unsafe(link_section = ".rwtext.open_radio_rx_hot")]
fn route_connected_ethernet(
    ethernet: &[u8],
    raw: &[u8],
    station_address: [u8; 6],
    benchmark: &mut DirectUdpRxBenchmark,
    network_runner: &NetworkRunner,
) -> ConnectedRxRoute {
    if ethernet.len() < 14 || (ethernet[..6] != station_address && ethernet[0] & 1 == 0) {
        return ConnectedRxRoute::Ignored;
    }
    let sample = benchmark.observe(ethernet, raw);
    if benchmark.last_packet_was_benchmark() {
        return ConnectedRxRoute::Benchmark(sample);
    }
    let local_ipv4 = OPEN_RADIO_LOCAL_IPV4.load(Ordering::Acquire).to_be_bytes();
    let lan_probe_response = ethernet.len() >= 42
        && ethernet[12..14] == 0x0806_u16.to_be_bytes()
        && ethernet[20..22] == 2_u16.to_be_bytes()
        && ethernet[28..32] == LAN_PROBE_IPV4
        && ethernet[32..38] == station_address
        && ethernet[38..42] == local_ipv4;
    match network_runner.try_send_rx(ethernet) {
        Ok(()) => {
            if lan_probe_response {
                // SOURCE[HIL_OPEN_DHCP_ARP_PRIME_2026_07_30]: a wired pcap
                // proved that the old probe advertised static .138 while
                // DHCP owned .140. Embassy therefore had no .140 neighbor,
                // emitted its first valid ARP only after ICMP seq=1, and
                // replied starting at seq=2. Publish completion only after
                // the correctly addressed ARP reply enters embassy-net. The
                // post-fix cold pcap observed `tell .140` 12.4 seconds before
                // the ICMP run; seq=1 then completed in 4.116 ms and all
                // 100/100 requests received replies.
                OPEN_RADIO_LAN_PROBE_RESPONSE.store(true, Ordering::Release);
            }
            ConnectedRxRoute::Enqueued
        }
        Err(_) => ConnectedRxRoute::Dropped,
    }
}

fn account_connected_rx_route(
    route: ConnectedRxRoute,
    enqueued: &mut u32,
    dropped: &mut u32,
    last_network_activity: &mut Instant,
) {
    match route {
        ConnectedRxRoute::Ignored => {}
        ConnectedRxRoute::Benchmark(sample) => {
            *last_network_activity = Instant::now();
            if let Some(sample) = sample {
                // One compact synchronous record per five-second interval is
                // bounded enough to avoid the RX starvation caused by the old
                // 384-byte report, while also avoiding the asynchronous
                // logger's module-prefix truncation under simultaneous load.
                emergency_log(format_args!(
                    "ORX b={} d={} u={} k={}",
                    sample.bytes, sample.datagrams, sample.elapsed_us, sample.throughput_kbps,
                ));
                emergency_log(format_args!(
                    "ORXP f={} r={} m={}",
                    sample.dominant_bb_format, sample.dominant_rate, sample.maximum_rate,
                ));
                let phy = RxPhyInfo {
                    rate: sample.dominant_rate,
                    bb_format: sample.dominant_bb_format,
                    he_siga1: sample.first_he_siga1,
                    he_siga2: sample.first_he_siga2,
                };
                if let Some(he) = phy.he_su_signal() {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx-he-su \
                         mcs={} dcm={} bandwidth_mhz={} gi_ns={} ltf={} \
                         nsts_encoding={} space_time_streams={:?} spatial_streams={:?} \
                         bss_color={} ldpc={} stbc={} \
                         beamformed={}",
                        he.mcs,
                        u8::from(he.dcm),
                        he.bandwidth.mhz(),
                        he.guard_interval_and_ltf.guard_interval_ns(),
                        he.guard_interval_and_ltf.ltf_count(),
                        he.nsts_and_midamble_periodicity,
                        he.space_time_stream_count(),
                        he.spatial_stream_count(),
                        he.bss_color,
                        u8::from(he.ldpc),
                        u8::from(he.stbc),
                        u8::from(he.beamformed),
                    ));
                }
            }
        }
        ConnectedRxRoute::Enqueued => {
            *enqueued = enqueued.saturating_add(1);
            *last_network_activity = Instant::now();
        }
        ConnectedRxRoute::Dropped => *dropped = dropped.saturating_add(1),
    }
}

/// Drain protected data while a raw benchmark aggregate awaits completion.
///
/// This interleaving stage is enabled only by the raw HIL profile, but it
/// services every stateful connected frame: Trigger, NDPA, AddBA/DELBA and
/// protected data. Beacons and unrelated control traffic are the only
/// `non_data_consumed` class. The next structural step is to make this the
/// sole connected RX dispatcher and remove the equivalent ordinary-loop
/// branches; both paths already mutate the same uniquely borrowed state.
///
/// SOURCE: complete `_oracles/libpp.a[wdev.o]::wDev_ProcessFiq` handles
/// RX-success bit 0x4000 before TX-complete bit 0x80. Complete
/// `_oracles/libpp.a[pp.o]::{pp_post,ppTask}` retains them as separate
/// coalesced FIFO work items. The former monolithic TX future inverted this
/// boundary by returning to RX only after aggregate completion and retry.
///
/// SOURCE[HIL_OPEN_HE20_RX_TX_INTERLEAVE_2026_07_30]: before this dispatcher,
/// 5,968 saturated A-MPDU submissions deferred 13,497 RX-success interrupts,
/// asserted `BUFFER_FULL` 531 times and produced about 14-Mbit/s downlink plus
/// 35-Mbit/s uplink. Servicing RX at this boundary reduced `BUFFER_FULL` to
/// zero and produced about 12-Mbit/s downlink plus 59-Mbit/s uplink with the
/// same 32-MPDU HE20/MCS9 workload. An eight-MPDU experiment redistributed
/// airtime to about 19+33 Mbit/s but reduced aggregate goodput. A separate
/// ordinary HE20 image delivered 1,000/1,000 ICMP replies: RX starvation is a
/// saturated bidirectional scheduling defect, not an idle PHY loss.
#[allow(clippy::too_many_arguments)]
fn service_benchmark_rx_during_tx(
    mmio: &mut RadioRegisters,
    rx_storage: &RxStorage,
    rx_ring: &mut RxRingLive<'_, RX_DESCRIPTOR_COUNT>,
    rx_staging_queue: &mut ConnectedRxStagingQueue,
    frame: &mut [u8; RX_BUFFER_SIZE],
    ethernet: &mut [u8; RX_BUFFER_SIZE],
    network_runner: &NetworkRunner,
    station_address: [u8; 6],
    bssid: [u8; 6],
    association_id: u16,
    association_phy: StaAssociationPhy,
    pending_rx_addba: &mut [Option<PendingRxAddba>; 8],
    active_rx_addba: &mut [Option<ActiveRxAddba>; 8],
    tx_block_ack: &mut TxBlockAckSession,
    tx_block_ack_alarm: &mut Option<TxBlockAckAlarm>,
    tx_block_ack_tid7: &mut TxBlockAckSession,
    tx_block_ack_tid7_alarm: &mut Option<TxBlockAckAlarm>,
    tx_block_ack_tid5: &mut TxBlockAckSession,
    tx_block_ack_tid5_alarm: &mut Option<TxBlockAckAlarm>,
    duplicate_filter: &mut StaRxDuplicateFilter,
    benchmark: &mut DirectUdpRxBenchmark,
    received: &mut u32,
    enqueued: &mut u32,
    dropped: &mut u32,
    pairwise_mic_failures: &mut u32,
    pairwise_hardware_rejections: &mut u32,
    pairwise_rejections: &mut u32,
    duplicate_frames: &mut u32,
    amsdu_frames: &mut u32,
    amsdu_msdu: &mut u32,
    non_data_consumed: &mut u32,
    observed_trigger_frames: &mut u32,
    last_trigger_common: &mut Option<TriggerCommonInfo>,
    last_trigger_schedule: &mut Option<Result<HeTriggerScheduledRate, HeTriggerScheduledRateError>>,
    last_trigger_user: &mut Option<[u8; 5]>,
    observed_he_ndpa_frames: &mut u32,
    observed_he_ndpa_for_us: &mut u32,
    last_he_ndpa_dialog_token: &mut Option<u8>,
    last_he_ndpa_hardware: &mut Option<MacHeBeamformingDiagnostics>,
    last_network_activity: &mut Instant,
) -> Result<(), RxStageTransactionError> {
    'connected_rx: loop {
        while !rx_staging_queue.is_full() {
            let index = rx_ring.recycle_start();
            let Some(completed) = rx_ring.take_completed(index) else {
                break;
            };
            let staged = stage_connected_rx_from_storage(mmio, rx_storage, rx_ring, completed)?;
            if rx_staging_queue.try_push(staged).is_err() {
                return Err(RxStageTransactionError::Ring(RxRingError::Corrupt));
            }
            *received = received.saturating_add(1);
        }
        let Some(staged) = rx_staging_queue.pop() else {
            break 'connected_rx;
        };
        let segment = staged.segment();
        let raw = segment.buffer;
        let raw_fc = u16::from_le_bytes([raw[PUBLIC_HEADER_SIZE], raw[PUBLIC_HEADER_SIZE + 1]]);
        let raw_destination = &raw[PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10];

        // Do not let a control exchange disappear merely because its RX edge
        // coincided with an A-MPDU completion. This is the same bounded parser
        // and state update used by the ordinary connected loop.
        if raw_fc & 0x00fc == 0x0024 {
            let Ok(control) = extract_control(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                frame,
            ) else {
                continue;
            };
            if let Ok(trigger) = parse_trigger_frame(&frame[..control.length]) {
                *observed_trigger_frames = observed_trigger_frames.saturating_add(1);
                *last_trigger_common = Some(trigger.common);
                *last_trigger_schedule = Some(HeTriggerScheduledRate::from_trigger_frame(
                    &trigger,
                    association_id,
                ));
                if let Some(bytes) = trigger.user_info_and_padding.get(..5) {
                    let mut first_user = [0_u8; 5];
                    first_user.copy_from_slice(bytes);
                    *last_trigger_user = Some(first_user);
                }
            }
            continue;
        }

        if raw_fc & 0x00fc == 0x0054 {
            let Ok(control) = extract_control(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                frame,
            ) else {
                continue;
            };
            if let Ok(ndpa) = HeNdpa::parse(&frame[..control.length]) {
                *observed_he_ndpa_frames = observed_he_ndpa_frames.saturating_add(1);
                *last_he_ndpa_dialog_token = Some(ndpa.dialog_token());
                *last_he_ndpa_hardware = Some(mmio.he_beamforming_diagnostics());
                if ndpa.contains_association_id(association_id) {
                    *observed_he_ndpa_for_us = observed_he_ndpa_for_us.saturating_add(1);
                }
            }
            continue;
        }

        if raw_fc & 0x00fc == 0x00d0 {
            let Ok(management) = extract_management(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                frame,
            ) else {
                continue;
            };
            if management.length < 24
                || frame[4..10] != station_address
                || frame[10..16] != bssid
                || frame[16..22] != bssid
            {
                continue;
            }
            let action_body = &frame[24..management.length];
            match parse_block_ack_action(action_body) {
                Some(BlockAckAction::AddbaRequest {
                    dialog_token,
                    tid,
                    immediate,
                    window,
                    timeout_tu,
                    starting_sequence,
                    ..
                }) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=rx-addba-request \
                         tid={tid} immediate={} window={window} timeout_tu={timeout_tu} \
                         starting_sequence={starting_sequence}",
                        u8::from(immediate),
                    ));
                    if immediate
                        && window != 0
                        && timeout_tu == 0
                        && tid <= S31_RX_BLOCK_ACK_MAX_TID
                        && starting_sequence <= 0x0fff
                    {
                        pending_rx_addba[usize::from(tid)] = Some(PendingRxAddba {
                            dialog_token,
                            tid,
                            requested_window: window,
                            starting_sequence,
                        });
                    }
                }
                Some(BlockAckAction::AddbaResponse { .. }) => {
                    let response_token = action_body[2];
                    let selected = if tx_block_ack.awaiting_dialog_token() == Some(response_token) {
                        Some((&mut *tx_block_ack, &mut *tx_block_ack_alarm))
                    } else if tx_block_ack_tid7.awaiting_dialog_token() == Some(response_token) {
                        Some((&mut *tx_block_ack_tid7, &mut *tx_block_ack_tid7_alarm))
                    } else if tx_block_ack_tid5.awaiting_dialog_token() == Some(response_token) {
                        Some((&mut *tx_block_ack_tid5, &mut *tx_block_ack_tid5_alarm))
                    } else {
                        None
                    };
                    let Some((session, alarm)) = selected else {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-response \
                             dialog_token={response_token} error=unexpected-dialog-token"
                        ));
                        continue;
                    };
                    match session.on_response(action_body) {
                        Ok(TxBlockAckResponse::Operational(agreement)) => {
                            *alarm = None;
                            if association_phy == StaAssociationPhy::He20 {
                                let tid = MacHeTid::new(agreement.tid)
                                    .expect("BlockAck parser admitted an IEEE TID");
                                mmio.set_he_trigger_based_tid_enabled(tid, true);
                            }
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=PASS stage=tx-addba-active \
                                 tid={} window={} timeout_tu={} starting_sequence={} amsdu={}",
                                agreement.tid,
                                agreement.window,
                                agreement.timeout_tu,
                                agreement.starting_sequence,
                                agreement.amsdu,
                            ));
                        }
                        Ok(TxBlockAckResponse::Rejected(status)) => {
                            *alarm = None;
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-response \
                                 dialog_token={response_token} status={status}"
                            ));
                        }
                        Err(error) => emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=tx-addba-response \
                             dialog_token={response_token} error={error:?}"
                        )),
                    }
                }
                Some(BlockAckAction::Delba {
                    tid,
                    initiator,
                    reason,
                }) => {
                    if initiator {
                        if let Some(hardware_index) = active_rx_addba.iter().position(|agreement| {
                            agreement
                                .as_ref()
                                .is_some_and(|agreement| agreement.tid == tid)
                        }) {
                            let _ = rx_ampdu_hw::clear(mmio, hardware_index as u8);
                            active_rx_addba[hardware_index] = None;
                        }
                    } else {
                        match tid {
                            0 => {
                                tx_block_ack.stop();
                                *tx_block_ack_alarm = None;
                            }
                            7 => {
                                tx_block_ack_tid7.stop();
                                *tx_block_ack_tid7_alarm = None;
                            }
                            5 => {
                                tx_block_ack_tid5.stop();
                                *tx_block_ack_tid5_alarm = None;
                            }
                            _ => {}
                        }
                        if association_phy == StaAssociationPhy::He20 {
                            let tid =
                                MacHeTid::new(tid).expect("DELBA parser admitted an IEEE TID");
                            mmio.set_he_trigger_based_tid_enabled(tid, false);
                        }
                    }
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL stage=delba tid={tid} \
                         peer_is_originator={} reason={reason}",
                        u8::from(initiator),
                    ));
                }
                None => {}
            }
            continue;
        }

        let raw_pairwise_protected =
            raw_fc & 0x400c == 0x4008 && raw_destination == station_address;
        if !raw_pairwise_protected {
            *non_data_consumed = non_data_consumed.saturating_add(1);
            continue;
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
                *pairwise_mic_failures = pairwise_mic_failures.saturating_add(1);
                continue;
            }
            Err(RxError::RxFailure | RxError::Quarantined) => {
                *pairwise_hardware_rejections = pairwise_hardware_rejections.saturating_add(1);
                continue;
            }
            Err(_) => {
                *pairwise_rejections = pairwise_rejections.saturating_add(1);
                continue;
            }
        };
        if data.mpdu.length < 24 || frame[10..16] != bssid {
            continue;
        }
        let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
        let sequence_control = u16::from_le_bytes([frame[22], frame[23]]);
        let tid = if frame_control & 0x0080 != 0 && data.mpdu.length >= 26 {
            Some(frame[24] & 0x0f)
        } else {
            None
        };
        if duplicate_filter.is_duplicate(frame_control & 0x0800 != 0, sequence_control, tid) {
            *duplicate_frames = duplicate_frames.saturating_add(1);
            continue;
        }
        match decapsulate_data(
            DataInterfaceRole::Station,
            &frame[..data.mpdu.length],
            data.payload_offset,
            data.payload_length,
            ethernet,
        ) {
            Ok(plan) => account_connected_rx_route(
                route_connected_ethernet(
                    &ethernet[..plan.ethernet_length],
                    raw,
                    station_address,
                    benchmark,
                    network_runner,
                ),
                enqueued,
                dropped,
                last_network_activity,
            ),
            Err(DataDecapError::AmsduUnsupported) => {
                let Ok(subframes) = amsdu_subframes(
                    DataInterfaceRole::Station,
                    &frame[..data.mpdu.length],
                    data.payload_offset,
                    data.payload_length,
                ) else {
                    continue;
                };
                *amsdu_frames = amsdu_frames.saturating_add(1);
                for subframe in subframes {
                    let Ok(subframe) = subframe else {
                        break;
                    };
                    let Ok(ethernet_length) = decapsulate_amsdu_subframe(subframe, ethernet) else {
                        break;
                    };
                    *amsdu_msdu = amsdu_msdu.saturating_add(1);
                    account_connected_rx_route(
                        route_connected_ethernet(
                            &ethernet[..ethernet_length],
                            raw,
                            station_address,
                            benchmark,
                            network_runner,
                        ),
                        enqueued,
                        dropped,
                        last_network_activity,
                    );
                }
            }
            Err(_) => {}
        }
    }

    if rx_ring.all_observed() {
        return Err(RxStageTransactionError::Ring(RxRingError::Busy));
    }
    Ok(())
}

fn iperf2_udp_sequence(packet: &[u8]) -> Option<i32> {
    let encoded: [u8; 4] = packet.get(..4)?.try_into().ok()?;
    Some(i32::from_be_bytes(encoded))
}

#[derive(Clone, Copy)]
struct DirectUdpRxSample {
    bytes: u64,
    datagrams: u64,
    elapsed_us: u64,
    throughput_kbps: u64,
    dominant_bb_format: u8,
    dominant_rate: u8,
    maximum_rate: u8,
    first_he_siga1: u32,
    first_he_siga2: u16,
    he_mu_frames: u32,
    he_mu_complete_frames: u32,
    he20_non_mimo_users_max: u8,
    he20_mimo_users_max: u8,
    first_he_mu_ru_allocation: Option<u8>,
    he_mu_invalid_ru_allocations: u32,
    first_he_mu_spatial_configuration: Option<u8>,
    he_mu_total_nsts_max: u8,
    he_mu_invalid_spatial_configurations: u32,
    he_mu_other_layout_frames: u32,
    he_mu_invalid_streams: u32,
    first_he_mu_user_raw: Option<u32>,
}

struct DirectUdpRxBenchmark {
    started: Option<Instant>,
    last_packet: Option<Instant>,
    bytes: u64,
    datagrams: u64,
    last_packet_was_benchmark: bool,
    bb_format_counts: [u32; 16],
    rate_counts: [u32; 32],
    maximum_rate: u8,
    first_he_siga1: u32,
    first_he_siga2: u16,
    he_mu_frames: u32,
    he_mu_complete_frames: u32,
    he20_non_mimo_users_max: u8,
    he20_mimo_users_max: u8,
    first_he_mu_ru_allocation: Option<u8>,
    he_mu_invalid_ru_allocations: u32,
    first_he_mu_spatial_configuration: Option<u8>,
    he_mu_total_nsts_max: u8,
    he_mu_invalid_spatial_configurations: u32,
    he_mu_other_layout_frames: u32,
    he_mu_invalid_streams: u32,
    first_he_mu_user_raw: Option<u32>,
}

impl DirectUdpRxBenchmark {
    const fn new() -> Self {
        Self {
            started: None,
            last_packet: None,
            bytes: 0,
            datagrams: 0,
            last_packet_was_benchmark: false,
            bb_format_counts: [0; 16],
            rate_counts: [0; 32],
            maximum_rate: 0,
            first_he_siga1: 0,
            first_he_siga2: 0,
            he_mu_frames: 0,
            he_mu_complete_frames: 0,
            he20_non_mimo_users_max: 0,
            he20_mimo_users_max: 0,
            first_he_mu_ru_allocation: None,
            he_mu_invalid_ru_allocations: 0,
            first_he_mu_spatial_configuration: None,
            he_mu_total_nsts_max: 0,
            he_mu_invalid_spatial_configurations: 0,
            he_mu_other_layout_frames: 0,
            he_mu_invalid_streams: 0,
            first_he_mu_user_raw: None,
        }
    }

    fn last_packet_was_benchmark(&self) -> bool {
        self.last_packet_was_benchmark
    }

    fn finish_sample(&mut self) -> Option<DirectUdpRxSample> {
        let (Some(started), Some(last_packet)) = (self.started, self.last_packet) else {
            return None;
        };
        let dominant_bb_format = dominant_index(&self.bb_format_counts);
        let dominant_rate = dominant_index(&self.rate_counts);
        let elapsed_us = last_packet.duration_since(started).as_micros().max(1);
        let throughput_kbps = self
            .bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        let sample = DirectUdpRxSample {
            bytes: self.bytes,
            datagrams: self.datagrams,
            elapsed_us,
            throughput_kbps,
            dominant_bb_format,
            dominant_rate,
            maximum_rate: self.maximum_rate,
            first_he_siga1: self.first_he_siga1,
            first_he_siga2: self.first_he_siga2,
            he_mu_frames: self.he_mu_frames,
            he_mu_complete_frames: self.he_mu_complete_frames,
            he20_non_mimo_users_max: self.he20_non_mimo_users_max,
            he20_mimo_users_max: self.he20_mimo_users_max,
            first_he_mu_ru_allocation: self.first_he_mu_ru_allocation,
            he_mu_invalid_ru_allocations: self.he_mu_invalid_ru_allocations,
            first_he_mu_spatial_configuration: self.first_he_mu_spatial_configuration,
            he_mu_total_nsts_max: self.he_mu_total_nsts_max,
            he_mu_invalid_spatial_configurations: self.he_mu_invalid_spatial_configurations,
            he_mu_other_layout_frames: self.he_mu_other_layout_frames,
            he_mu_invalid_streams: self.he_mu_invalid_streams,
            first_he_mu_user_raw: self.first_he_mu_user_raw,
        };
        self.started = None;
        self.last_packet = None;
        self.bytes = 0;
        self.datagrams = 0;
        self.bb_format_counts.fill(0);
        self.rate_counts.fill(0);
        self.maximum_rate = 0;
        self.first_he_siga1 = 0;
        self.first_he_siga2 = 0;
        self.he_mu_frames = 0;
        self.he_mu_complete_frames = 0;
        self.he20_non_mimo_users_max = 0;
        self.he20_mimo_users_max = 0;
        self.first_he_mu_ru_allocation = None;
        self.he_mu_invalid_ru_allocations = 0;
        self.first_he_mu_spatial_configuration = None;
        self.he_mu_total_nsts_max = 0;
        self.he_mu_invalid_spatial_configurations = 0;
        self.he_mu_other_layout_frames = 0;
        self.he_mu_invalid_streams = 0;
        self.first_he_mu_user_raw = None;
        Some(sample)
    }

    /// Count the already decrypted/decapsulated UDP stream before the
    /// additional Embassy network queue and socket copies.
    ///
    /// This is the radio-driver throughput boundary: CCMP, RX metadata,
    /// 802.11 decapsulation and DMA ownership have all completed. DHCP, ARP
    /// and non-benchmark UDP still use Embassy unchanged.
    ///
    /// SOURCE[HIL_OPEN_HE20_RX_RING_STARVATION_2026_07_29]: the ordinary
    /// socket benchmark saturated the finite RX ring (`MAC_INT_RAW` bit
    /// 0x200). The iperf2 payload sequence and negative terminal convention
    /// match `run_open_radio_udp_rx_benchmark`.
    #[inline(never)]
    #[unsafe(link_section = ".rwtext.open_radio_rx_hot")]
    fn observe(&mut self, ethernet: &[u8], rx_buffer: &[u8]) -> Option<DirectUdpRxSample> {
        self.last_packet_was_benchmark = false;
        if ethernet.get(12..14) != Some(&[0x08, 0x00]) {
            return None;
        }
        let version_and_ihl = *ethernet.get(14)?;
        if version_and_ihl >> 4 != 4 || ethernet.get(23).copied() != Some(17) {
            return None;
        }
        let ip_header_length = usize::from(version_and_ihl & 0x0f) * 4;
        if ip_header_length < 20 {
            return None;
        }
        let udp = 14_usize.checked_add(ip_header_length)?;
        let destination_port = u16::from_be_bytes(ethernet.get(udp + 2..udp + 4)?.try_into().ok()?);
        if destination_port != OPEN_RADIO_UDP_RX_PORT {
            return None;
        }
        let udp_length = usize::from(u16::from_be_bytes(
            ethernet.get(udp + 4..udp + 6)?.try_into().ok()?,
        ));
        if udp_length < 8 {
            return None;
        }
        let payload_start = udp + 8;
        let payload_end = payload_start.checked_add(udp_length - 8)?;
        let payload = ethernet.get(payload_start..payload_end)?;
        let sequence = iperf2_udp_sequence(payload)?;
        self.last_packet_was_benchmark = true;

        if sequence < 0 {
            return self.finish_sample();
        }

        let now = Instant::now();
        if let Some(phy) = decode_rx_phy_info(rx_buffer) {
            self.bb_format_counts[usize::from(phy.bb_format)] =
                self.bb_format_counts[usize::from(phy.bb_format)].saturating_add(1);
            self.rate_counts[usize::from(phy.rate)] =
                self.rate_counts[usize::from(phy.rate)].saturating_add(1);
            self.maximum_rate = self.maximum_rate.max(phy.rate);
            if self.started.is_none() {
                self.first_he_siga1 = phy.he_siga1;
                self.first_he_siga2 = phy.he_siga2;
            }
            if phy.he_mu_signal().is_some() {
                self.he_mu_frames = self.he_mu_frames.saturating_add(1);
                match decode_rx_he_mu_sig_b(rx_buffer) {
                    Some(sig_b) if !sig_b.complete_bytes.is_empty() => {
                        self.he_mu_complete_frames = self.he_mu_complete_frames.saturating_add(1);
                        match sig_b.he20_non_mimo_users() {
                            Ok(mut users) => {
                                self.he20_non_mimo_users_max =
                                    self.he20_non_mimo_users_max.max(users.user_count());
                                match users.ru_allocation() {
                                    Ok(allocation) => {
                                        if self.first_he_mu_ru_allocation.is_none() {
                                            self.first_he_mu_ru_allocation =
                                                Some(allocation.encoding());
                                        }
                                    }
                                    Err(_) => {
                                        self.he_mu_invalid_ru_allocations =
                                            self.he_mu_invalid_ru_allocations.saturating_add(1);
                                    }
                                }
                                if self.first_he_mu_user_raw.is_none() {
                                    self.first_he_mu_user_raw = users.next().map(|entry| entry.raw);
                                }
                            }
                            Err(RxHe20MuSigBUsersError::MuMimoCompressed) => {
                                match sig_b.he20_mimo_users() {
                                    Ok(mut users) => {
                                        self.he20_mimo_users_max =
                                            self.he20_mimo_users_max.max(users.user_count());
                                        match users.spatial_configuration() {
                                            Ok(spatial) => {
                                                if self.first_he_mu_spatial_configuration.is_none()
                                                {
                                                    self.first_he_mu_spatial_configuration =
                                                        Some(spatial.encoding());
                                                }
                                                self.he_mu_total_nsts_max = self
                                                    .he_mu_total_nsts_max
                                                    .max(spatial.total_nsts());
                                            }
                                            Err(_) => {
                                                self.he_mu_invalid_spatial_configurations = self
                                                    .he_mu_invalid_spatial_configurations
                                                    .saturating_add(1);
                                            }
                                        }
                                        if self.first_he_mu_user_raw.is_none() {
                                            self.first_he_mu_user_raw =
                                                users.next().map(|entry| entry.raw);
                                        }
                                    }
                                    Err(
                                        RxHe20MuSigBMimoUsersError::WiderOrUnknownBandwidth
                                        | RxHe20MuSigBMimoUsersError::NotMuMimoCompressed,
                                    ) => {
                                        self.he_mu_other_layout_frames =
                                            self.he_mu_other_layout_frames.saturating_add(1);
                                    }
                                    Err(RxHe20MuSigBMimoUsersError::CompleteStream(_)) => {
                                        self.he_mu_invalid_streams =
                                            self.he_mu_invalid_streams.saturating_add(1);
                                    }
                                }
                            }
                            Err(RxHe20MuSigBUsersError::WiderOrUnknownBandwidth) => {
                                self.he_mu_other_layout_frames =
                                    self.he_mu_other_layout_frames.saturating_add(1);
                            }
                            Err(RxHe20MuSigBUsersError::CompleteStream(_)) => {
                                self.he_mu_invalid_streams =
                                    self.he_mu_invalid_streams.saturating_add(1);
                            }
                        }
                    }
                    Some(_) => {}
                    None => {
                        self.he_mu_invalid_streams = self.he_mu_invalid_streams.saturating_add(1);
                    }
                }
            }
        }
        let started = *self.started.get_or_insert(now);
        self.last_packet = Some(now);
        self.bytes = self.bytes.saturating_add(payload.len() as u64);
        self.datagrams = self.datagrams.saturating_add(1);
        // SOURCE[HIL_OPEN_HT40_BIDIRECTIONAL_2026_07_29]: with the raw
        // A-MSDU/A-MPDU uplink active, the Linux AP delivered 9,226 unicast
        // downlink frames at HT40 MCS7 SGI without increasing `tx failed`.
        // SOURCE[HIL_OPEN_HT40_SGI_BIDIRECTIONAL_2026_07_30]: the strict Rust
        // host runner qualified format-2 RX and 150,000-kbit/s open TX at a
        // 10-Mbit/s offered downlink: RX median 10.030 Mbit/s, raw TX median
        // 96.159 Mbit/s, sum 106.189 Mbit/s, with no data-path failure.
        // iperf2 then retried its terminal datagram because this driver-level
        // observer intentionally consumes benchmark traffic before the UDP
        // socket can return a server report. Fixed-duration samples therefore
        // provide the deterministic driver-boundary result; a received
        // negative iperf2 sequence still closes a short final sample.
        if now.duration_since(started) >= OPEN_RADIO_UDP_TX_BENCH_DURATION {
            self.finish_sample()
        } else {
            None
        }
    }
}

fn dominant_index<const COUNT: usize>(counts: &[u32; COUNT]) -> u8 {
    counts
        .iter()
        .enumerate()
        .max_by_key(|(_, count)| *count)
        .and_then(|(index, _)| u8::try_from(index).ok())
        .unwrap_or(0)
}

async fn run_open_radio_udp_benchmark(
    stack: Stack<'static>,
    association_phy: StaAssociationPhy,
    data_tx_rate: TxPhyRate,
) -> ! {
    if OPEN_RADIO_RAW_MAC_BENCH {
        loop {
            Timer::after_secs(60).await;
        }
    } else if option_env!("OPEN_RADIO_TX_BENCH").is_some() {
        run_open_radio_udp_tx_benchmark(stack, association_phy, data_tx_rate).await
    } else {
        run_open_radio_udp_rx_benchmark(stack, association_phy, data_tx_rate).await
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
    let server = Ipv4Address::from_octets(STA_ARP_TARGET_IPV4);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
         target={server}:{OPEN_RADIO_UDP_TX_BENCH_PORT} \
         queue={OPEN_RADIO_UDP_TX_QUEUE_DEPTH} payload={OPEN_RADIO_UDP_PAYLOAD_CAPACITY} \
         ampdu_window={TX_AMPDU_FRAME_COUNT} ampdu_limit={OPEN_RADIO_AMPDU_LIMIT} \
         ampdu_coalesce_us={TX_AMPDU_COALESCE_US} \
         offered_tx_kbps={OPEN_RADIO_TX_BENCH_RATE_KBPS:?} \
         rate_code={:#04x} rate_kbps={}",
        data_tx_rate.code(),
        data_tx_rate.nominal_kbps(),
    ));
    Timer::after_secs(1).await;

    loop {
        let started = Instant::now();
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
        Timer::after_secs(2).await;
    }
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
    let packet = OPEN_RADIO_UDP_PACKET.init([0; OPEN_RADIO_UDP_PAYLOAD_CAPACITY]);
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
         port={OPEN_RADIO_UDP_RX_PORT} queue={OPEN_RADIO_UDP_RX_QUEUE_DEPTH} \
         payload_capacity={OPEN_RADIO_UDP_PAYLOAD_CAPACITY} \
         bandwidth_mhz={} phy={} rate_code={:#04x} rate_kbps={}",
        association_phy.bandwidth_mhz(),
        association_phy.name(),
        data_tx_rate.code(),
        data_tx_rate.nominal_kbps(),
    ));

    loop {
        let first_length = loop {
            let Ok((length, _)) = socket.recv_from(packet).await else {
                continue;
            };
            if iperf2_udp_sequence(&packet[..length]).is_some_and(|sequence| sequence < 0) {
                continue;
            }
            break length;
        };
        let started = Instant::now();
        let mut last_packet = started;
        let mut bytes = first_length as u64;
        let mut datagrams = 1_u64;
        let mut receive_errors = 0_u32;
        let mut terminal_seen = false;

        loop {
            match with_timeout(OPEN_RADIO_UDP_RX_IDLE, socket.recv_from(packet)).await {
                Ok(Ok((length, _))) => {
                    if iperf2_udp_sequence(&packet[..length]).is_some_and(|sequence| sequence < 0) {
                        terminal_seen = true;
                        break;
                    }
                    bytes = bytes.saturating_add(length as u64);
                    datagrams = datagrams.saturating_add(1);
                    last_packet = Instant::now();
                }
                Ok(Err(_)) => receive_errors = receive_errors.saturating_add(1),
                Err(_) => break,
            }
        }

        let elapsed_us = last_packet.duration_since(started).as_micros().max(1);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx \
             bytes={bytes} datagrams={datagrams} elapsed_us={elapsed_us} \
             throughput_kbps={throughput_kbps} receive_errors={receive_errors} \
             terminal={} bandwidth_mhz={} phy={} \
             rate_code={:#04x} rate_kbps={}",
            u8::from(terminal_seen),
            association_phy.bandwidth_mhz(),
            association_phy.name(),
            data_tx_rate.code(),
            data_tx_rate.nominal_kbps(),
        ));
    }
}

async fn run_connected_network(
    platform: &mut EspHalRadioPeripheral,
    mmio: &mut RadioRegisters,
    interrupt_setup: &mut Option<MacInterruptSetup>,
    rx_storage: &RxStorage,
    tx_storage: &mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &[u32; RX_DESCRIPTOR_COUNT],
    frame: &mut [u8; RX_BUFFER_SIZE],
    ethernet: &mut [u8; RX_BUFFER_SIZE],
    network_device: &mut NetworkDevice,
    network_runner: &NetworkRunner,
    station_address: [u8; 6],
    bssid: [u8; 6],
    association_id: u16,
    mut pairwise_slot: StaPairwiseCcmpSlot,
    _group_slot: StaGroupCcmpSlot,
    peer_qos: bool,
    association_phy: StaAssociationPhy,
    peer_supports_one_ltf_800ns_gi: bool,
    peer_supports_ldpc: bool,
    peer_dcm_receive: HeDcmConstellation,
    best_effort_txop: HeEdcaTxopLimit,
    rate_control: &mut StaRateControlAssociation,
    sequences: &mut StaTxSequenceCounters,
) -> ! {
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
    let mut tx_ampdu_storage = HtAmpduTxStorage::pin_static(HtAmpduTxStorage::init_in_place(
        OPEN_RADIO_TX_AMPDU_STORAGE.uninit(),
    ));
    tx_ampdu_storage
        .as_mut()
        .configure_max_aggregate_bytes(
            tx_storage
                .runtime_policy
                .ht_ampdu()
                .maximum_aggregate_bytes(),
        )
        .expect("valid negotiated HT A-MPDU byte limit");
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
    let (stack, mut stack_runner) = embassy_net::new(
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
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=embassy-net-start \
         frame_capacity={NETWORK_FRAME_CAPACITY} queue_depth={NETWORK_QUEUE_DEPTH} \
         bandwidth_mhz={} phy={} data_rate_code={:#04x} data_rate_kbps={}",
        association_phy.bandwidth_mhz(),
        association_phy.name(),
        data_tx_rate.code(),
        data_tx_rate.nominal_kbps(),
    ));

    match select4(
        stack_runner.run(),
        connected_radio_loop(
            mmio,
            rx_storage,
            tx_storage,
            tx_ampdu_storage,
            descriptor_base,
            buffer_addresses,
            frame,
            ethernet,
            network_runner,
            station_address,
            bssid,
            association_id,
            &mut pairwise_slot,
            peer_qos,
            association_phy,
            peer_supports_one_ltf_800ns_gi,
            peer_supports_ldpc,
            peer_dcm_receive,
            best_effort_txop,
            rate_control,
            sequences,
        ),
        report_network_configuration(stack),
        run_open_radio_udp_benchmark(stack, association_phy, benchmark_tx_rate),
    )
    .await {}
}

async fn authenticate_target(
    state: &mut PhyColdState,
    platform: &mut EspHalRadioPeripheral,
    mmio: &mut RadioRegisters,
    rx_storage: &RxStorage,
    tx_storage: &mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &[u32; RX_DESCRIPTOR_COUNT],
    frame: &mut [u8; RX_BUFFER_SIZE],
    station_address: [u8; 6],
    access_point: ScanRecord,
    sequence: &mut StaSequenceCounter,
) -> bool {
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

    let mut authentication =
        StaAuthenticationRuntime::new(station_address, access_point.bssid);
    loop {
        let attempt = match authentication.begin_attempt(sequence) {
            Ok(attempt) => attempt,
            Err(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-runtime error={error:?}"
                ));
                return false;
            }
        };
        let mut rx_ring =
            match start_live_rx_ring(mmio, rx_storage, descriptor_base, buffer_addresses).await {
                Ok(ring) => ring,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-rx-arm \
                         attempt={} error={error:?}",
                        attempt.ordinal,
                    ));
                    return false;
                }
            };
        let completion = match transmit_open_authentication(
            mmio,
            tx_storage,
            station_address,
            access_point.bssid,
            attempt.sequence_number,
        )
        .await
        {
            Ok(completion) => completion,
            Err(error) => {
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-tx \
                     attempt={} error={error:?}",
                    attempt.ordinal,
                ));
                return false;
            }
        };
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=sta-auth-tx attempt={} channel={} \
             bssid={:02x?} status={} alternate={} aux={:#010x}/{:#010x}/{:#010x} \
             primary={:#010x} alternate_word={:#010x} \
             config={:#010x} ppdu={:#010x} protection={:#010x} \
             plcp1={:#010x} pti={:#010x} power={:#010x} length={:#010x} \
             duration={:#010x} htsig={:#010x} ht_control={:#010x} \
             data_length={:#010x}",
            attempt.ordinal,
            access_point.channel,
            access_point.bssid,
            completion.status,
            completion.used_alternate,
            completion.auxiliary_a_word,
            completion.auxiliary_b_word,
            completion.auxiliary_c_word,
            completion.primary_word,
            completion.alternate_word,
            mmio.read32(TX_Q_CONFIG[LegacyTxQueue::Voice as usize]),
            mmio.read32(TX_Q_PPDU_CONTROL[LegacyTxQueue::Voice as usize]),
            mmio.read32(TX_Q_PROTECTION[LegacyTxQueue::Voice as usize]),
            mmio.read32(TX_Q_PLCP1[LegacyTxQueue::Voice as usize]),
            read_diagnostic_mmio(0x2010_54e0),
            mmio.read32(TX_Q_POWER[LegacyTxQueue::Voice as usize]),
            mmio.read32(TX_Q_LENGTH_CONTROL[LegacyTxQueue::Voice as usize]),
            read_diagnostic_mmio(0x2010_54dc),
            read_diagnostic_mmio(0x2010_54e8),
            read_diagnostic_mmio(0x2010_5504),
            read_diagnostic_mmio(0x2010_550c),
        ));

        let mut attempt_end = None;
        'response_wait: for _ in 0..attempt.response_timeout_ms {
            for index in 0..RX_DESCRIPTOR_COUNT {
                let Some(completed) = rx_ring.take_completed(index) else {
                    continue;
                };
                if let Err(error) = authentication.observe_received_frame() {
                    let _ = disable_receive(mmio);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-runtime error={error:?}"
                    ));
                    return false;
                }
                let segment = RxSegment {
                    descriptor_address: completed.descriptor_address(),
                    descriptor_word0: completed.word0(),
                    buffer: unsafe {
                        // RxRingLive transferred this completed descriptor and
                        // buffer to the sole radio task until recycle.
                        rx_storage.buffers[index].as_slice()
                    },
                    next_descriptor_address: completed.next_descriptor_address(),
                };
                let raw = segment.buffer;
                let raw_fc =
                    u16::from_le_bytes([raw[PUBLIC_HEADER_SIZE], raw[PUBLIC_HEADER_SIZE + 1]]);
                if raw_fc & 0x00fc == 0x00b0
                    && raw[PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10] == station_address
                {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL probe=sta-auth-rx-raw attempt={} frame={} \
                         descriptor={:#010x} fc={raw_fc:#06x} state={:#04x} \
                         internal={:#04x} da={:02x?} sa={:02x?}",
                        attempt.ordinal,
                        authentication.active_received_frames(),
                        completed.word0(),
                        raw[PUBLIC_HEADER_SIZE - 4],
                        raw[PUBLIC_HEADER_SIZE - 3],
                        &raw[PUBLIC_HEADER_SIZE + 4..PUBLIC_HEADER_SIZE + 10],
                        &raw[PUBLIC_HEADER_SIZE + 10..PUBLIC_HEADER_SIZE + 16],
                    ));
                }
                let Ok(extracted) = extract_management(
                    core::slice::from_ref(&segment),
                    RxIngressConfig {
                        ring_entry_limit: 1,
                        csi_config: 0,
                        flags: 0,
                    },
                    frame,
                ) else {
                    continue;
                };
                let management_subtype = frame[0] & 0xfc;
                if management_subtype == 0xb0
                    || authentication.active_received_frames() <= 3
                {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL probe=sta-auth-rx attempt={} frame={} \
                         subtype={:#04x} length={} da={:02x?} sa={:02x?} \
                         bssid={:02x?}",
                        attempt.ordinal,
                        authentication.active_received_frames(),
                        management_subtype,
                        extracted.length,
                        &frame[4..10],
                        &frame[10..16],
                        &frame[16..22],
                    ));
                }
                let event = match authentication
                    .observe_management_frame(&frame[..extracted.length])
                {
                    Ok(event) => event,
                    Err(error) => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-runtime error={error:?}"
                        ));
                        return false;
                    }
                };
                match event {
                    StaAuthenticationEvent::Irrelevant => {}
                    StaAuthenticationEvent::Authenticated {
                        attempt,
                        total_received_frames,
                    } => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=PASS stage=sta-auth-response \
                             attempt={attempt} status=0 frames={total_received_frames} \
                             bssid={:02x?}",
                            access_point.bssid,
                        ));
                        return true;
                    }
                    StaAuthenticationEvent::Failed {
                        attempts,
                        failure: StaAuthenticationFailure::Rejected { status_code },
                        total_received_frames,
                    } => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-response \
                             attempt={attempts} status={status_code} \
                             frames={total_received_frames} bssid={:02x?}",
                            access_point.bssid,
                        ));
                        return false;
                    }
                    event @ (StaAuthenticationEvent::Retry { .. }
                    | StaAuthenticationEvent::Failed { .. }) => {
                        attempt_end = Some(event);
                        break 'response_wait;
                    }
                }
            }

            if let Err(error) = rx_ring.recycle_completed_half(mmio, |index| {
                // SAFETY: this callback runs only for a detached completed
                // half before the ring republishes it to hardware.
                unsafe { rx_storage.buffers[index].prepare_for_recycle() }
            }) {
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-rx-recycle \
                     error={error:?}"
                ));
                return false;
            }
            if rx_ring.all_observed() {
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-rx-recycle \
                     error=terminal-before-recycle"
                ));
                return false;
            }
            Timer::after_millis(1).await;
        }

        let _ = disable_receive(mmio);
        let event = match attempt_end {
            Some(event) => event,
            None => match authentication.response_timed_out() {
                Ok(event) => event,
                Err(error) => {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-runtime error={error:?}"
                    ));
                    return false;
                }
            },
        };
        match event {
            StaAuthenticationEvent::Retry {
                attempt,
                failure,
                received_frames,
                ..
            } => {
                match failure {
                    StaAuthenticationFailure::Timeout => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=RETRY stage=sta-auth-response \
                         attempt={attempt} error=timeout timeout_ms={} \
                         frames={received_frames}",
                        STA_RESPONSE_TIMEOUT_MS,
                    )),
                    StaAuthenticationFailure::PeerDisconnect(disconnect) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=RETRY stage=sta-auth-response \
                             attempt={attempt} error=peer-disconnect kind={:?} reason={} \
                             frames={received_frames}",
                            disconnect.kind, disconnect.reason_code,
                        ));
                    }
                    StaAuthenticationFailure::Rejected { status_code } => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-response \
                             attempt={attempt} status={status_code}"
                        ));
                        return false;
                    }
                }
            }
            StaAuthenticationEvent::Failed {
                attempts,
                failure,
                total_received_frames,
            } => {
                match failure {
                    StaAuthenticationFailure::Timeout => emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-response error=timeout \
                         attempts={attempts} frames={total_received_frames} bssid={:02x?}",
                        access_point.bssid,
                    )),
                    StaAuthenticationFailure::PeerDisconnect(disconnect) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-response \
                             error=peer-disconnect kind={:?} reason={} attempts={attempts} \
                             frames={total_received_frames} bssid={:02x?}",
                            disconnect.kind, disconnect.reason_code, access_point.bssid,
                        ));
                    }
                    StaAuthenticationFailure::Rejected { status_code } => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-response \
                             attempt={attempts} status={status_code} \
                             frames={total_received_frames} bssid={:02x?}",
                            access_point.bssid,
                        ));
                    }
                }
                return false;
            }
            StaAuthenticationEvent::Irrelevant
            | StaAuthenticationEvent::Authenticated { .. } => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-auth-runtime error=invalid-terminal"
                ));
                return false;
            }
        }
    }
}
async fn associate_target(
    platform: &mut EspHalRadioPeripheral,
    mmio: &mut RadioRegisters,
    interrupt_setup: &mut Option<MacInterruptSetup>,
    rx_storage: &RxStorage,
    tx_storage: &mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &[u32; RX_DESCRIPTOR_COUNT],
    frame: &mut [u8; RX_BUFFER_SIZE],
    ethernet: &mut [u8; RX_BUFFER_SIZE],
    station_address: [u8; 6],
    access_point: ScanRecord,
    pmk: &Pmk,
    supplicant_nonce: [u8; 32],
    sequences: &mut StaTxSequenceCounters,
) -> (bool, bool, bool) {
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
    let (mut network_device, network_runner) =
        network_resources.split(network_tx_pool, station_address);
    let mut rx_ring =
        match start_live_rx_ring(mmio, rx_storage, descriptor_base, buffer_addresses).await {
            Ok(ring) => ring,
            Err(error) => {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-rx-arm error={error:?}"
                ));
                return (false, false, false);
            }
        };

    let mut received_frames = 0_u32;
    for tick in 0..STA_RESPONSE_TIMEOUT_MS {
        if let Some(attempt) = StaAssociationRetrySchedule::attempt_at(tick) {
            let completion = match transmit_association_request(
                mmio,
                tx_storage,
                station_address,
                &access_point,
                sequences.take_non_qos(),
            )
            .await
            {
                Ok(completion) => completion,
                Err(error) => {
                    let _ = disable_receive(mmio);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-retry-tx \
                         attempt={attempt} error={error:?}"
                    ));
                    return (false, false, false);
                }
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=sta-assoc-tx attempt={attempt} \
                 channel={} bssid={:02x?} status={} primary={:#010x}",
                access_point.channel,
                access_point.bssid,
                completion.status,
                completion.primary_word,
            ));
        }
        for index in 0..RX_DESCRIPTOR_COUNT {
            let Some(completed) = rx_ring.take_completed(index) else {
                continue;
            };
            received_frames = received_frames.saturating_add(1);
            let segment = RxSegment {
                descriptor_address: completed.descriptor_address(),
                descriptor_word0: completed.word0(),
                buffer: unsafe {
                    // RxRingLive transferred this completed descriptor and its
                    // buffer to the sole radio task until recycle.
                    rx_storage.buffers[index].as_slice()
                },
                next_descriptor_address: completed.next_descriptor_address(),
            };
            let Ok(extracted) = extract_management(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                frame,
            ) else {
                continue;
            };
            let management_subtype = frame[0] & 0xfc;
            if management_subtype == 0x10 || received_frames <= 3 {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL probe=sta-assoc-rx frame={} subtype={:#04x} \
                     length={} da={:02x?} sa={:02x?} bssid={:02x?}",
                    received_frames,
                    management_subtype,
                    extracted.length,
                    &frame[4..10],
                    &frame[10..16],
                    &frame[16..22],
                ));
            }
            if let Some(response) = parse_association_response(
                &frame[..extracted.length],
                station_address,
                access_point.bssid,
            ) {
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result={} stage=sta-assoc-response \
                     status={} aid={} ht={} he_cap={} he_op={} wmm={} \
                     frames={received_frames} bssid={:02x?}",
                    if response.status_code == 0 {
                        "PASS"
                    } else {
                        "FAIL"
                    },
                    response.status_code,
                    response.association_id,
                    response.ht_capability,
                    response.he_capability,
                    response.he_operation,
                    response.wmm,
                    access_point.bssid,
                ));
                if response.status_code != 0 {
                    let _ = disable_receive(mmio);
                    return (false, false, false);
                }
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
                    if let Err(error) =
                        program_he20_peer_state(mmio, state, response.association_id, 0, 0)
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
                let best_effort_txop = peer_plan.wmm.best_effort_txop();
                let peer_supports_short_guard_interval = peer_he_capabilities
                    .is_some_and(|capability| capability.supports_one_ltf_800ns_gi());
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
                let (message1, message3) = await_wpa2_message_1(
                    platform,
                    mmio,
                    interrupt_setup,
                    rx_storage,
                    tx_storage,
                    descriptor_base,
                    buffer_addresses,
                    frame,
                    ethernet,
                    &mut network_device,
                    &network_runner,
                    station_address,
                    access_point.bssid,
                    response.association_id,
                    rx_ring,
                    pmk,
                    supplicant_nonce,
                    selected_rsn.as_bytes(),
                    access_point.rsn_ie_bytes(),
                    access_point.rsnxe_bytes(),
                    peer_qos,
                    association_phy,
                    peer_supports_short_guard_interval,
                    peer_supports_ldpc,
                    peer_dcm_constellation,
                    best_effort_txop,
                    rate_control,
                    sequences,
                )
                .await;
                return (true, message1, message3);
            }
        }

        if let Err(error) = rx_ring.recycle_completed_half(mmio, |index| {
            // SAFETY: RxRingLive invokes this only for a fully completed,
            // detached half before republishing it to hardware.
            unsafe { rx_storage.buffers[index].prepare_for_recycle() }
        }) {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-rx-recycle error={error:?}"
            ));
            return (false, false, false);
        }
        if rx_ring.all_observed() {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-rx-recycle \
                 error=terminal-before-recycle"
            ));
            return (false, false, false);
        }
        Timer::after_millis(1).await;
    }

    let _ = disable_receive(mmio);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=FAIL stage=sta-assoc-response error=timeout \
         frames={received_frames} bssid={:02x?}",
        access_point.bssid,
    ));
    (false, false, false)
}

async fn await_wpa2_message_1(
    platform: &mut EspHalRadioPeripheral,
    mmio: &mut RadioRegisters,
    interrupt_setup: &mut Option<MacInterruptSetup>,
    rx_storage: &RxStorage,
    tx_storage: &mut TxStorage,
    descriptor_base: u32,
    buffer_addresses: &[u32; RX_DESCRIPTOR_COUNT],
    frame: &mut [u8; RX_BUFFER_SIZE],
    ethernet: &mut [u8; RX_BUFFER_SIZE],
    network_device: &mut NetworkDevice,
    network_runner: &NetworkRunner,
    station_address: [u8; 6],
    bssid: [u8; 6],
    association_id: u16,
    mut rx_ring: RxRingLive<'_, RX_DESCRIPTOR_COUNT>,
    pmk: &Pmk,
    supplicant_nonce: [u8; 32],
    association_security_ies: &[u8],
    authenticator_rsn_ie: &[u8],
    authenticator_rsnxe: &[u8],
    peer_qos: bool,
    association_phy: StaAssociationPhy,
    peer_supports_one_ltf_800ns_gi: bool,
    peer_supports_ldpc: bool,
    peer_dcm_receive: HeDcmConstellation,
    best_effort_txop: HeEdcaTxopLimit,
    rate_control: &mut StaRateControlAssociation,
    sequences: &mut StaTxSequenceCounters,
) -> (bool, bool) {
    let mut handshake = match Wpa2StaState::new(station_address, bssid, supplicant_nonce) {
        Ok(handshake) => handshake,
        Err(error) => {
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-state-create error={error:?}"
            ));
            return (false, false);
        }
    };
    let mut received_frames = 0_u32;
    for _ in 0..WPA2_MESSAGE_1_TIMEOUT_MS {
        for index in 0..RX_DESCRIPTOR_COUNT {
            let Some(completed) = rx_ring.take_completed(index) else {
                continue;
            };
            received_frames = received_frames.saturating_add(1);
            let segment = RxSegment {
                descriptor_address: completed.descriptor_address(),
                descriptor_word0: completed.word0(),
                buffer: unsafe {
                    // RxRingLive transferred this completed descriptor and its
                    // buffer to the sole radio task until recycle.
                    rx_storage.buffers[index].as_slice()
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
                frame,
            ) else {
                continue;
            };
            if data.mpdu.length < 24 || frame[4..10] != station_address || frame[10..16] != bssid {
                continue;
            }
            let Some(eapol_offset) = data.payload_offset.checked_add(LLC_SNAP_EAPOL.len()) else {
                continue;
            };
            if frame
                .get(data.payload_offset..eapol_offset)
                .is_none_or(|header| header != LLC_SNAP_EAPOL)
            {
                continue;
            }
            let Some(eapol) = frame.get(eapol_offset..data.mpdu.length) else {
                continue;
            };
            let Ok(key) = EapolKeyFrame::parse(eapol) else {
                continue;
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=wpa2-eapol-rx message={:?} version={} \
                 descriptor_version={} replay={} key_length={} key_data={} frames={received_frames}",
                key.message(),
                key.protocol_version(),
                key.key_info().descriptor_version(),
                key.replay_counter(),
                key.key_length(),
                key.key_data().len(),
            ));
            let owned: OwnedEapolFrame<512> =
                match OwnedEapolFrame::try_copy(Wpa2Interface::Station, bssid, eapol) {
                    Ok(frame) => frame,
                    Err(_) => continue,
                };
            let action = match handshake.on_frame(owned) {
                Ok(action) => action,
                Err(_) => continue,
            };
            if let Wpa2StaAction::DerivePtk { ticket, context } = action {
                let replay_counter = key.replay_counter();
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-1 \
                     replay={} bssid={bssid:02x?}",
                    replay_counter,
                ));
                let mut ptk = pmk.derive_ptk(CryptoPtkContext {
                    authenticator_address: context.authenticator_address,
                    supplicant_address: context.supplicant_address,
                    authenticator_nonce: context.authenticator_nonce,
                    supplicant_nonce: context.supplicant_nonce,
                });
                if !matches!(
                    handshake.complete_ptk::<512>(ticket, true),
                    Ok(Wpa2StaAction::Transmit(transmit))
                        if transmit.message == Wpa2TxMessage::PairwiseMessage2
                            && transmit.replay_counter == replay_counter
                ) {
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-2-state"
                    ));
                    return (true, false);
                }
                let message2 = match Message2::build(
                    replay_counter,
                    supplicant_nonce,
                    association_security_ies,
                    &ptk,
                ) {
                    Ok(message) => message,
                    Err(error) => {
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-2-build \
                             error={error:?}"
                        ));
                        return (true, false);
                    }
                };
                for attempt in 1..=WPA2_MESSAGE_2_ATTEMPTS {
                    let message3_rx_ring = match start_live_rx_ring(
                        mmio,
                        rx_storage,
                        descriptor_base,
                        buffer_addresses,
                    )
                    .await
                    {
                        Ok(ring) => ring,
                        Err(error) => {
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-2-rx-arm \
                                 attempt={attempt} error={error:?}"
                            ));
                            return (true, false);
                        }
                    };
                    let completion = match transmit_unprotected_eapol(
                        mmio,
                        tx_storage,
                        station_address,
                        bssid,
                        message2.as_bytes(),
                        sequences.take_non_qos(),
                    )
                    .await
                    {
                        Ok(completion) => completion,
                        Err(error) => {
                            let _ = disable_receive(mmio);
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-2-tx \
                                 attempt={attempt} error={error:?}"
                            ));
                            return (true, false);
                        }
                    };
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-2-tx \
                         attempt={attempt} replay={replay_counter} status={} primary={:#010x}",
                        completion.status, completion.primary_word,
                    ));
                    if await_wpa2_message_3(
                        platform,
                        mmio,
                        interrupt_setup,
                        rx_storage,
                        descriptor_base,
                        buffer_addresses,
                        frame,
                        ethernet,
                        network_device,
                        network_runner,
                        tx_storage,
                        station_address,
                        bssid,
                        association_id,
                        &mut ptk,
                        pmk,
                        supplicant_nonce,
                        association_security_ies,
                        attempt,
                        attempt == WPA2_MESSAGE_2_ATTEMPTS,
                        peer_qos,
                        authenticator_rsn_ie,
                        authenticator_rsnxe,
                        association_phy,
                        peer_supports_one_ltf_800ns_gi,
                        peer_supports_ldpc,
                        peer_dcm_receive,
                        best_effort_txop,
                        message3_rx_ring,
                        &mut handshake,
                        rate_control,
                        sequences,
                    )
                    .await
                    {
                        return (true, true);
                    }
                }
                return (true, false);
            }
        }

        if let Err(error) = rx_ring.recycle_completed_half(mmio, |index| {
            // SAFETY: RxRingLive invokes this only for a fully completed,
            // detached half before republishing it to hardware.
            unsafe { rx_storage.buffers[index].prepare_for_recycle() }
        }) {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-1-rx-recycle \
                 error={error:?}"
            ));
            return (false, false);
        }
        if rx_ring.all_observed() {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-1-rx-recycle \
                 error=terminal-before-recycle"
            ));
            return (false, false);
        }
        Timer::after_millis(1).await;
    }

    let _ = disable_receive(mmio);
    emergency_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-1 error=timeout \
         frames={received_frames} bssid={bssid:02x?}"
    ));
    (false, false)
}

async fn await_wpa2_message_3(
    platform: &mut EspHalRadioPeripheral,
    mmio: &mut RadioRegisters,
    interrupt_setup: &mut Option<MacInterruptSetup>,
    rx_storage: &RxStorage,
    descriptor_base: u32,
    buffer_addresses: &[u32; RX_DESCRIPTOR_COUNT],
    frame: &mut [u8; RX_BUFFER_SIZE],
    ethernet: &mut [u8; RX_BUFFER_SIZE],
    network_device: &mut NetworkDevice,
    network_runner: &NetworkRunner,
    tx_storage: &mut TxStorage,
    station_address: [u8; 6],
    bssid: [u8; 6],
    association_id: u16,
    ptk: &mut Ptk,
    pmk: &Pmk,
    supplicant_nonce: [u8; 32],
    association_security_ies: &[u8],
    attempt: u16,
    final_attempt: bool,
    peer_qos: bool,
    authenticator_rsn_ie: &[u8],
    authenticator_rsnxe: &[u8],
    association_phy: StaAssociationPhy,
    peer_supports_one_ltf_800ns_gi: bool,
    peer_supports_ldpc: bool,
    peer_dcm_receive: HeDcmConstellation,
    best_effort_txop: HeEdcaTxopLimit,
    mut rx_ring: RxRingLive<'_, RX_DESCRIPTOR_COUNT>,
    handshake: &mut Wpa2StaState,
    rate_control: &mut StaRateControlAssociation,
    sequences: &mut StaTxSequenceCounters,
) -> bool {
    let mut received_frames = 0_u32;
    for _ in 0..WPA2_MESSAGE_3_TIMEOUT_MS {
        for index in 0..RX_DESCRIPTOR_COUNT {
            let Some(completed) = rx_ring.take_completed(index) else {
                continue;
            };
            received_frames = received_frames.saturating_add(1);
            let segment = RxSegment {
                descriptor_address: completed.descriptor_address(),
                descriptor_word0: completed.word0(),
                buffer: unsafe { rx_storage.buffers[index].as_slice() },
                next_descriptor_address: completed.next_descriptor_address(),
            };
            let Ok(data) = extract_data(
                core::slice::from_ref(&segment),
                RxIngressConfig {
                    ring_entry_limit: 1,
                    csi_config: 0,
                    flags: 0,
                },
                frame,
            ) else {
                continue;
            };
            if data.mpdu.length < 24 || frame[4..10] != station_address || frame[10..16] != bssid {
                continue;
            }
            let Some(eapol_offset) = data.payload_offset.checked_add(LLC_SNAP_EAPOL.len()) else {
                continue;
            };
            if frame.get(data.payload_offset..eapol_offset) != Some(&LLC_SNAP_EAPOL) {
                continue;
            }
            let Some(eapol) = frame.get(eapol_offset..data.mpdu.length) else {
                continue;
            };
            let Ok(key) = EapolKeyFrame::parse(eapol) else {
                continue;
            };
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL stage=wpa2-eapol-rx message={:?} replay={} \
                 key_data={} frames={received_frames}",
                key.message(),
                key.replay_counter(),
                key.key_data().len(),
            ));
            let owned: OwnedEapolFrame<512> =
                match OwnedEapolFrame::try_copy(Wpa2Interface::Station, bssid, eapol) {
                    Ok(frame) => frame,
                    Err(_) => continue,
                };
            let action = match handshake.on_frame(owned) {
                Ok(action) => action,
                Err(_) => continue,
            };
            let mut replacement_ptk = None;
            let message2_replay = match &action {
                Wpa2StaAction::Transmit(transmit)
                    if transmit.message == Wpa2TxMessage::PairwiseMessage2 =>
                {
                    Some(transmit.replay_counter)
                }
                Wpa2StaAction::DerivePtk { ticket, context } => {
                    let replay_counter = key.replay_counter();
                    let derived = pmk.derive_ptk(CryptoPtkContext {
                        authenticator_address: context.authenticator_address,
                        supplicant_address: context.supplicant_address,
                        authenticator_nonce: context.authenticator_nonce,
                        supplicant_nonce: context.supplicant_nonce,
                    });
                    if !matches!(
                        handshake.complete_ptk::<512>(*ticket, true),
                        Ok(Wpa2StaAction::Transmit(transmit))
                            if transmit.message == Wpa2TxMessage::PairwiseMessage2
                                && transmit.replay_counter == replay_counter
                    ) {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=wpa2-message-2-refresh-state replay={replay_counter}"
                        ));
                        return false;
                    }
                    replacement_ptk = Some(derived);
                    Some(replay_counter)
                }
                _ => None,
            };
            if let Some(replay_counter) = message2_replay {
                if let Some(derived) = replacement_ptk {
                    *ptk = derived;
                }
                let message2 = match Message2::build(
                    replay_counter,
                    supplicant_nonce,
                    association_security_ies,
                    ptk,
                ) {
                    Ok(message) => message,
                    Err(error) => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=wpa2-message-2-refresh-build \
                             replay={replay_counter} error={error:?}"
                        ));
                        return false;
                    }
                };
                let completion = match transmit_unprotected_eapol(
                    mmio,
                    tx_storage,
                    station_address,
                    bssid,
                    message2.as_bytes(),
                    sequences.take_non_qos(),
                )
                .await
                {
                    Ok(completion) => completion,
                    Err(error) => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL \
                             stage=wpa2-message-2-refresh-tx \
                             replay={replay_counter} error={error:?}"
                        ));
                        return false;
                    }
                };
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-2-refresh-tx \
                     replay={replay_counter} status={} primary={:#010x}",
                    completion.status, completion.primary_word,
                ));
                continue;
            }
            if let Wpa2StaAction::VerifyMessage3Mic {
                ticket,
                frame: eapol_frame,
            } = action
            {
                let mic_valid = eapol_frame.key_frame().verify_mic(ptk);
                if !mic_valid {
                    let _ = handshake.complete_message3_mic::<512>(ticket, eapol_frame, false);
                    let _ = disable_receive(mmio);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-3-mic \
                         replay={} bssid={bssid:02x?}",
                        key.replay_counter(),
                    ));
                    return false;
                }
                let next = match handshake.complete_message3_mic(ticket, eapol_frame, true) {
                    Ok(action) => action,
                    Err(error) => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-3-state \
                             error={error:?}"
                        ));
                        return false;
                    }
                };
                let mut unwrapped_key_data = None;
                let (install_ticket, retained_frame) = match next {
                    Wpa2StaAction::DecryptMessage3KeyData {
                        ticket,
                        frame: eapol_frame,
                    } => {
                        let key = eapol_frame.key_frame();
                        unwrapped_key_data =
                            match software_aes128_key_unwrap(ptk.kek(), key.key_data()) {
                                Ok(key_data) => Some(key_data),
                                Err(error) => {
                                    let _ = handshake.complete_key_data::<512>(
                                        ticket,
                                        eapol_frame,
                                        false,
                                    );
                                    let _ = disable_receive(mmio);
                                    emergency_log(format_args!(
                                        "OPEN_RADIO_PHY_HIL result=FAIL \
                                         stage=wpa2-message-3-key-unwrap error={error:?}"
                                    ));
                                    return false;
                                }
                            };
                        match handshake.complete_key_data(ticket, eapol_frame, true) {
                            Ok(Wpa2StaAction::InstallKeys {
                                ticket,
                                frame: eapol_frame,
                            }) => (ticket, eapol_frame),
                            _ => {
                                let _ = disable_receive(mmio);
                                emergency_log(format_args!(
                                    "OPEN_RADIO_PHY_HIL result=FAIL \
                                     stage=wpa2-message-3-key-data-state"
                                ));
                                return false;
                            }
                        }
                    }
                    Wpa2StaAction::InstallKeys {
                        ticket,
                        frame: eapol_frame,
                    } => (ticket, eapol_frame),
                    _ => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-3-action"
                        ));
                        return false;
                    }
                };
                let key = retained_frame.key_frame();
                let plain_key_data = unwrapped_key_data
                    .as_ref()
                    .map_or(key.key_data(), |key_data| key_data.as_bytes());
                let gtk = match parse_gtk_key_data(
                    plain_key_data,
                    authenticator_rsn_ie,
                    authenticator_rsnxe,
                ) {
                    Ok(gtk) => gtk,
                    Err(error) => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-3-gtk-parse \
                             error={error:?} plain_key_data={}",
                            plain_key_data.len(),
                        ));
                        return false;
                    }
                };
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-3-key-data \
                     encrypted={} plain={} gtk_id={} gtk_tx={}",
                    key.key_info().encrypted_key_data(),
                    plain_key_data.len(),
                    gtk.key_id(),
                    gtk.transmit(),
                ));
                let mut key_slot = match install_sta_pairwise_ccmp(mmio, bssid, ptk.temporal_key())
                {
                    Ok(slot) => slot,
                    Err(error) => {
                        let _ = disable_receive(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-pairwise-key-install \
                                 error={error:?}"
                        ));
                        return false;
                    }
                };
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-pairwise-key-install \
                     slot={} valid={} peer_control={:#010x} crypto_control={:#010x} \
                     crypto_policy={:#010x}",
                    key_slot.hardware_index(),
                    mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP)
                        & (1 << key_slot.hardware_index())
                        != 0,
                    mmio.read32(
                        mac_pac::crypto_key_entry_word(key_slot.hardware_index(), 1)
                            .expect("fixed pairwise slot metadata word"),
                    ),
                    mmio.read32(mac_pac::CRYPTO_INTERFACE_CONTROL[0]),
                    mmio.read32(mac_pac::CRYPTO_POLICY_CONTROL),
                ));
                let group_slot = match install_sta_group_ccmp(mmio, gtk.key_id(), gtk.key()) {
                    Ok(slot) => slot,
                    Err(error) => {
                        let _ = disable_receive(mmio);
                        key_slot.clear(mmio);
                        emergency_log(format_args!(
                            "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-group-key-install \
                             gtk_id={} error={error:?}",
                            gtk.key_id(),
                        ));
                        return false;
                    }
                };
                if !matches!(
                    handshake.complete_key_install::<512>(install_ticket, true),
                    Ok(Wpa2StaAction::Transmit(transmit))
                        if transmit.message == Wpa2TxMessage::PairwiseMessage4
                            && transmit.replay_counter == key.replay_counter()
                ) {
                    let _ = disable_receive(mmio);
                    key_slot.clear(mmio);
                    group_slot.clear(mmio);
                    emergency_log(format_args!(
                        "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-key-install-state"
                    ));
                    return false;
                }
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-group-key-install \
                     slot={} gtk_id={} valid={} control={:#010x}",
                    group_slot.hardware_index(),
                    group_slot.key_id(),
                    mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP)
                        & (1 << group_slot.hardware_index())
                        != 0,
                    mmio.read32(
                        mac_pac::crypto_key_entry_word(group_slot.hardware_index(), 1)
                            .expect("fixed group slot metadata word"),
                    ),
                ));
                let message4 = Message4::build(key.replay_counter(), ptk);
                let message4_valid = EapolKeyFrame::parse(message4.as_bytes())
                    .is_ok_and(|frame| frame.verify_mic(ptk));
                let _ = disable_receive(mmio);
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=PASS stage=wpa2-message-3 \
                     replay={} mic=true encrypted_key_data={} bssid={bssid:02x?}",
                    key.replay_counter(),
                    key.key_info().encrypted_key_data(),
                ));
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result={} stage=wpa2-message-4-build \
                     protocol_version=1 replay={} bytes={}",
                    if message4_valid { "PASS" } else { "FAIL" },
                    key.replay_counter(),
                    message4.as_bytes().len(),
                ));
                let hardware_index = key_slot.hardware_index();
                let message4_sent = if message4_valid {
                    // One protocol MPDU is enough. The lower unicast path
                    // performs bounded MAC retries with the same sequence
                    // number (and, for protected M4, the same CCMP PN).
                    let completion = if WPA2_MESSAGE_4_HARDWARE_PROTECTED {
                        transmit_eapol_message_4(
                            mmio,
                            tx_storage,
                            station_address,
                            bssid,
                            &message4,
                            &mut key_slot,
                            sequences
                                .take_data(peer_qos.then_some(0))
                                .expect("selected EAPOL sequence-number owner exists"),
                            peer_qos,
                        )
                        .await
                    } else {
                        transmit_unprotected_eapol(
                            mmio,
                            tx_storage,
                            station_address,
                            bssid,
                            message4.as_bytes(),
                            sequences.take_non_qos(),
                        )
                        .await
                    };
                    match completion {
                        Ok(completion) => {
                            let passed = completion.status == 0;
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result={} stage=wpa2-message-4-tx \
                                 protected={} replay={} status={} primary={:#010x}",
                                if passed { "PASS" } else { "FAIL" },
                                WPA2_MESSAGE_4_HARDWARE_PROTECTED,
                                key.replay_counter(),
                                completion.status,
                                completion.primary_word,
                            ));
                            passed
                        }
                        Err(error) => {
                            emergency_log(format_args!(
                                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-4-tx \
                                 replay={} error={error:?}",
                                key.replay_counter(),
                            ));
                            false
                        }
                    }
                } else {
                    false
                };
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
                        let protected_rx_ring = match start_live_rx_ring(
                            mmio,
                            rx_storage,
                            descriptor_base,
                            buffer_addresses,
                        )
                        .await
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
                            queue_arp_probe(network_device, network_runner, station_address)
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
                                        network_device,
                                        network_runner,
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
                        platform,
                        mmio,
                        interrupt_setup,
                        rx_storage,
                        tx_storage,
                        descriptor_base,
                        buffer_addresses,
                        frame,
                        ethernet,
                        network_device,
                        network_runner,
                        station_address,
                        bssid,
                        association_id,
                        key_slot,
                        group_slot,
                        peer_qos,
                        association_phy,
                        peer_supports_one_ltf_800ns_gi,
                        peer_supports_ldpc,
                        peer_dcm_receive,
                        best_effort_txop,
                        rate_control,
                        sequences,
                    )
                    .await;
                }
                let group_hardware_index = group_slot.hardware_index();
                group_slot.clear(mmio);
                let group_key_cleared = mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP)
                    & (1 << group_hardware_index)
                    == 0;
                emergency_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result={} stage=wpa2-group-key-clear \
                     slot={group_hardware_index}",
                    if group_key_cleared { "PASS" } else { "FAIL" },
                ));
                key_slot.clear(mmio);
                let key_cleared =
                    mmio.read32(mac_pac::CRYPTO_KEY_VALID_BITMAP) & (1 << hardware_index) == 0;
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
        }
        if let Err(error) = rx_ring.recycle_completed_half(mmio, |index| {
            // SAFETY: RxRingLive invokes this only for a fully completed,
            // detached half before republishing it to hardware.
            unsafe { rx_storage.buffers[index].prepare_for_recycle() }
        }) {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-3-rx-recycle \
                 error={error:?}"
            ));
            return false;
        }
        if rx_ring.all_observed() {
            let _ = disable_receive(mmio);
            emergency_log(format_args!(
                "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-3-rx-recycle \
                 error=terminal-before-recycle"
            ));
            return false;
        }
        Timer::after_millis(1).await;
    }
    let _ = disable_receive(mmio);
    if final_attempt {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=FAIL stage=wpa2-message-3 error=timeout \
             attempt={attempt} frames={received_frames} bssid={bssid:02x?}"
        ));
    } else {
        emergency_log(format_args!(
            "OPEN_RADIO_PHY_HIL stage=wpa2-message-3-timeout attempt={attempt} \
             frames={received_frames} bssid={bssid:02x?}"
        ));
    }
    false
}

async fn run_promiscuous_rx_hil(
    state: &mut PhyColdState,
    mut platform: EspHalRadioPeripheral,
    mut cold_mmio: ColdRadioRegisters,
    trng: &Trng,
) -> bool {
    let platform = &mut platform;
    let mmio = &mut cold_mmio;
    let storage = RxStorage::init_in_place(OPEN_RADIO_RX_DMA_STORAGE.uninit());
    let tx_slot = TxSlot::pin_static(TxSlot::init_in_place(OPEN_RADIO_TX_DMA_STORAGE.uninit()));
    let tx_storage = OPEN_RADIO_TX_STATE.init(TxStorage::new(tx_slot));
    tx_storage.install_tx_power_profile(
        state
            .tx_target_power_profile()
            .with_maximum_quarter_dbm(OPEN_RADIO_MAX_TX_POWER_QUARTER_DBM),
    );
    let descriptor_base = storage.descriptors.as_ptr().addr() as u32;
    let buffer_addresses: [u32; RX_DESCRIPTOR_COUNT] =
        core::array::from_fn(|index| storage.buffers[index].address());

    if let Err(error) = build_cold_ring(
        &storage.descriptors,
        descriptor_base,
        &buffer_addresses,
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
    for channel_index in 0..AUTH_DIAGNOSTIC_CHANNEL_COUNT {
        let channel = auth_diagnostic_channel(channel_index);
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
                    &storage.descriptors,
                    descriptor_base,
                    &buffer_addresses,
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

        if channel_index + 1 != AUTH_DIAGNOSTIC_CHANNEL_COUNT {
            if let Err(error) = build_cold_ring(
                &storage.descriptors,
                descriptor_base,
                &buffer_addresses,
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
        let channel = auth_diagnostic_channel(0);
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
                &storage.descriptors,
                descriptor_base,
                &buffer_addresses,
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
        let channel = auth_diagnostic_channel(0);
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
                &storage.descriptors,
                descriptor_base,
                &buffer_addresses,
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
    let active_scan_pass = tx_completions >= AUTH_DIAGNOSTIC_CHANNEL_COUNT as u32
        && probe_responses != 0
        && tx_failures == 0;
    let target = best_matching_ssid(scan_table.records(), STA_TARGET_SSID).copied();
    // No cold MAC operation is permitted beyond this point. Consume the cold
    // owner before authentication and retain the one-shot interrupt setup
    // token until WPA2 has opened the controlled port.
    let (mut running_mmio, interrupt_setup) = cold_mmio.into_running();
    let mmio = &mut running_mmio;
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
            let authenticated = authenticate_target(
                state,
                platform,
                mmio,
                storage,
                tx_storage,
                descriptor_base,
                &buffer_addresses,
                scan_frame,
                station_address,
                access_point,
                sequences.non_qos_mut(),
            )
            .await;
            let (associated, message1, message3) = if authenticated {
                associate_target(
                    platform,
                    mmio,
                    &mut interrupt_setup,
                    storage,
                    tx_storage,
                    descriptor_base,
                    &buffer_addresses,
                    scan_frame,
                    ethernet_frame,
                    station_address,
                    access_point,
                    &pmk,
                    supplicant_nonce,
                    &mut sequences,
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
            AUTH_DIAGNOSTIC_CHANNEL_COUNT,
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
            AUTH_DIAGNOSTIC_CHANNEL_COUNT,
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
            AUTH_DIAGNOSTIC_CHANNEL_COUNT,
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
pub async fn run(platform: EspHalRadioPeripheral, trng: Trng) {
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
        let _ = run_promiscuous_rx_hil(&mut state, platform, registers, &trng).await;
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
