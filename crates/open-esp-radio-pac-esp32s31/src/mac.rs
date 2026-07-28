//! ESP32-S31 Wi-Fi MAC register identities.
//!
//! Semantically named registers cover the live interrupt, RX and TX paths.
//! `init` additionally contains instruction-recovered cold-start registers
//! whose field names are not yet proven. Numeric names are intentional there:
//! they localize MMIO without inventing hardware semantics.

use crate::{Register32, RegisterAccess};

pub const INT_ENABLE: Register32 = Register32::new(0x2010_4c40);
pub const INT_RAW: Register32 = Register32::new(0x2010_4c44);
pub const INT_STATUS: Register32 = Register32::new(0x2010_4c48);
pub const INT_CLEAR: Register32 = Register32::new(0x2010_4c4c);

/// RX descriptor-walker control.
///
/// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS,ROM_REV0_HAL_MAC_RX_GATE,
/// HIL_OPEN_RX_LIVE_APPEND_2026_07_27]; CONFIDENCE[instruction-exact-and-hil].
/// Bit 31 is the start/stop gate. Bit 0 is a self-clearing reload doorbell for
/// appending descriptors to a live list; it is not used to publish the first
/// cold list. The HIL sustained more than 7,000 receives and completed
/// scan/WPA2/DHCP/ICMP while recycling a rotating 32-entry ring through this
/// doorbell.
pub const RX_CONTROL: Register32 =
    Register32::described(0x2010_4080, RegisterAccess::ReadWrite, None);
pub mod rx_control {
    use crate::Field32;

    /// Doorbell used after linking a new chain to a non-empty live RX list.
    pub const APPEND_DESCRIPTOR_RELOAD: Field32 = Field32::new(0, 1);
    /// Opens or stops the RX descriptor walker.
    pub const WALKER_ENABLE: Field32 = Field32::new(31, 1);
}

/// First RX descriptor address, expressed in the selected high-address window.
///
/// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS,HIL_OPEN_RX_LIVE_APPEND_2026_07_27];
/// CONFIDENCE[instruction-exact-and-hil]. The cold `wDev_AppendRxBlocks` path
/// writes this when the software list is empty. A live-list append normally
/// links the accepted tail and uses
/// [`rx_control::APPEND_DESCRIPTOR_RELOAD`].
pub const RX_DESCRIPTOR_BASE: Register32 =
    Register32::described(0x2010_4084, RegisterAccess::ReadWrite, None);
/// Current/next descriptor selected by the RX walker; zero denotes no current
/// descriptor at the observed terminal frontier.
///
/// SOURCE[ROM_REV0_WDEV_APPEND_RX_BLOCKS,HIL_OPEN_RX_LIVE_APPEND_2026_07_27];
/// CONFIDENCE[instruction-exact-and-hil]. ROM reads this after the reload bit
/// clears and repairs the base only when it is zero and the accepted hardware
/// tail has not reached the newly published software tail.
pub const RX_NEXT_DESCRIPTOR: Register32 =
    Register32::described(0x2010_4088, RegisterAccess::ReadOnly, None);
/// Last descriptor accepted by the RX walker.
///
/// SOURCE[ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR,
/// HIL_OPEN_RX_LIVE_APPEND_2026_07_27]; CONFIDENCE[instruction-exact-and-hil].
/// ROM reconstructs the pointer from this register's low 20 bits and
/// [`RX_LAST_DESCRIPTOR_HIGH`]'s high 12 bits. The HIL uses the accepted last
/// descriptor to rotate ownership instead of assuming descriptor zero.
pub const RX_LAST_DESCRIPTOR: Register32 =
    Register32::described(0x2010_408c, RegisterAccess::ReadOnly, None);
pub const RX_CSI_CONFIG: Register32 = Register32::new(0x2010_4098);
/// High address window shared by RX descriptor pointer registers.
///
/// SOURCE[ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR,
/// HIL_OPEN_RX_LIVE_APPEND_2026_07_27]; CONFIDENCE[instruction-exact-and-hil].
/// The open driver programs `0x2f00_0000` for internal SRAM descriptors; the
/// pointer registers carry the low 20 address bits.
pub const RX_LAST_DESCRIPTOR_HIGH: Register32 =
    Register32::described(0x2010_4c70, RegisterAccess::ReadWrite, None);

/// Per-interface hardware crypto controls for STA, AP and NAN.
///
/// SOURCE[BLOB_LIBPP_HAL_CRYPTO_ENABLE]; the interface number selects one
/// adjacent word. Higher layers still own the algorithm-specific value.
pub const CRYPTO_INTERFACE_CONTROL: [Register32; 3] = [
    Register32::described(0x2010_4800, RegisterAccess::ReadWrite, None),
    Register32::described(0x2010_4804, RegisterAccess::ReadWrite, None),
    Register32::described(0x2010_4808, RegisterAccess::ReadWrite, None),
];
/// Shared hardware crypto policy mask updated by `hal_crypto_enable`.
pub const CRYPTO_POLICY_CONTROL: Register32 =
    Register32::described(0x2010_4810, RegisterAccess::ReadWrite, None);
/// One validity bit per hardware key-table entry.
pub const CRYPTO_KEY_VALID_BITMAP: Register32 =
    Register32::described(0x2010_4814, RegisterAccess::ReadWrite, None);

/// Receive BlockAck agreement staging registers.
///
/// SOURCE[`migration/esp32s31-hybrid-runtime/src/rx_ampdu_hw.rs`,
/// pinned blob leaf `hal_mac_set_rx_ba`]; CONFIDENCE[instruction-exact].
/// The names describe the fields packed by that recovered leaf. Hardware
/// testing has not yet independently qualified this transaction.
pub mod rx_block_ack {
    pub use crate::power::wifi_mac_rx_dma::{
        RX_BLOCK_ACK_AGREEMENT_UPDATE as AGREEMENT_UPDATE, RX_BLOCK_ACK_BITMAP_HIGH as BITMAP_HIGH,
        RX_BLOCK_ACK_BITMAP_LOW as BITMAP_LOW, RX_BLOCK_ACK_CONTROL as CONTROL,
        RX_BLOCK_ACK_PEER_HEAD as PEER_HEAD,
        RX_BLOCK_ACK_PEER_TAIL_AND_POLICY as PEER_TAIL_AND_POLICY,
        RX_BLOCK_ACK_START_SEQUENCE as START_SEQUENCE,
    };

    pub mod agreement_update {
        pub use crate::power::wifi_mac_rx_dma::rx_block_ack_agreement_update::{
            COMMIT, READBACK_LATCH,
        };
    }
    pub mod control {
        pub use crate::power::wifi_mac_rx_dma::rx_block_ack_control::{
            ENABLE, INDEX, TID, VALID, WRITE,
        };
    }
    pub mod peer_tail_and_policy {
        pub use crate::power::wifi_mac_rx_dma::rx_block_ack_peer_tail_and_policy::{
            INTERFACE, WINDOW,
        };
    }

    pub const CAPACITY: u8 = 8;
}

/// Per-queue TX BlockAck result registers in hardware queue order.
///
/// These aliases are generated from `svd/esp32s31-radio.svd`; the explicit
/// arrays hide the hardware's descending 0x7c-byte queue-bank layout.
pub const TX_BLOCK_ACK_CONTROL_SEQUENCE: [Register32; 4] = [
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_CONTROL_SEQUENCE_Q0,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_CONTROL_SEQUENCE_Q1,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_CONTROL_SEQUENCE_Q2,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_CONTROL_SEQUENCE_Q3,
];
pub const TX_BLOCK_ACK_BITMAP_LOW: [Register32; 4] = [
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_LOW_Q0,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_LOW_Q1,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_LOW_Q2,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_LOW_Q3,
];
pub const TX_BLOCK_ACK_BITMAP_HIGH: [Register32; 4] = [
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_HIGH_Q0,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_HIGH_Q1,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_HIGH_Q2,
    crate::power::wifi_mac_rx_dma::TX_BLOCK_ACK_BITMAP_HIGH_Q3,
];

pub const CRYPTO_KEY_ENTRY_COUNT: u8 = 25;
pub const CRYPTO_KEY_ENTRY_WORDS: u8 = 10;

/// Resolves one word in the 25-entry, 40-byte hardware key table.
///
/// The returned register may contain peer address, key metadata or key bytes;
/// interpretation belongs to a cipher-specific MAC transaction.
pub const fn crypto_key_entry_word(index: u8, word: u8) -> Option<Register32> {
    if index >= CRYPTO_KEY_ENTRY_COUNT || word >= CRYPTO_KEY_ENTRY_WORDS {
        return None;
    }
    Some(Register32::described(
        0x2010_5800 + index as usize * 40 + word as usize * 4,
        RegisterAccess::ReadWrite,
        None,
    ))
}

pub const TX_Q0_CONTROL: Register32 = Register32::new(0x2010_4d70);
pub const TX_Q0_CONFIG: Register32 = Register32::new(0x2010_4d6c);
pub const TX_Q0_PPDU_CONTROL: Register32 = Register32::new(0x2010_4d68);
pub const TX_Q0_PROTECTION: Register32 = Register32::new(0x2010_4d64);
pub const TX_Q0_PLCP1: Register32 = Register32::new(0x2010_54d8);
pub const TX_Q0_PTI: Register32 = Register32::new(0x2010_54e0);
pub const TX_Q0_POWER: Register32 = Register32::new(0x2010_5500);
pub const TX_Q0_LENGTH_CONTROL: Register32 = Register32::new(0x2010_5510);

/// The four ordinary EDCA hardware queues, indexed by the recovered PP queue
/// number. Queue register banks run downward from q0.
pub const TX_Q_CONFIG: [Register32; 4] = [
    TX_Q0_CONFIG,
    Register32::new(0x2010_4d5c),
    Register32::new(0x2010_4d4c),
    Register32::new(0x2010_4d3c),
];
pub const TX_Q_CONTROL: [Register32; 4] = [
    TX_Q0_CONTROL,
    Register32::new(0x2010_4d60),
    Register32::new(0x2010_4d50),
    Register32::new(0x2010_4d40),
];
pub const TX_Q_PPDU_CONTROL: [Register32; 4] = [
    TX_Q0_PPDU_CONTROL,
    Register32::new(0x2010_4d58),
    Register32::new(0x2010_4d48),
    Register32::new(0x2010_4d38),
];
pub const TX_Q_PROTECTION: [Register32; 4] = [
    TX_Q0_PROTECTION,
    Register32::new(0x2010_4d54),
    Register32::new(0x2010_4d44),
    Register32::new(0x2010_4d34),
];
pub const TX_Q_PLCP1: [Register32; 4] = [
    TX_Q0_PLCP1,
    Register32::new(0x2010_545c),
    Register32::new(0x2010_53e0),
    Register32::new(0x2010_5364),
];
pub const TX_Q_PTI: [Register32; 4] = [
    TX_Q0_PTI,
    Register32::new(0x2010_5464),
    Register32::new(0x2010_53e8),
    Register32::new(0x2010_536c),
];
pub const TX_Q_POWER: [Register32; 4] = [
    TX_Q0_POWER,
    Register32::new(0x2010_5484),
    Register32::new(0x2010_5408),
    Register32::new(0x2010_538c),
];
pub const TX_Q_LENGTH_CONTROL: [Register32; 4] = [
    TX_Q0_LENGTH_CONTROL,
    Register32::new(0x2010_5494),
    Register32::new(0x2010_5418),
    Register32::new(0x2010_539c),
];

/// Approximate identities for STA-start/TX registers exercised by the HIL.
///
/// SOURCE[HIL_VENDOR_STA_START_DIFF,HIL_OPEN_TX]; values and access widths
/// are hardware-proven on rev0. Names describe observed software function,
/// not undocumented electrical implementation. This module is intentionally
/// suitable as a staging area for later SVD names.
pub mod sta_tx_oracle {
    use crate::{Register32, RegisterAccess};

    pub const STATION_ADDRESS_LOW: Register32 =
        Register32::described(0x2010_4000, RegisterAccess::ReadWrite, None);
    pub const STATION_ADDRESS_HIGH_CONTROL: Register32 =
        Register32::described(0x2010_4004, RegisterAccess::ReadWrite, None);
    pub mod station_address_high_control {
        use crate::Field32;
        pub const ADDRESS_HIGH: Field32 = Field32::new(0, 16);
        /// Vendor STA start asserts this bit together with the interface MAC.
        pub const ADDRESS_VALID_OR_ENABLE: Field32 = Field32::new(31, 1);
    }

    /// Two vendor STA-start policy images consumed before the TX queue VALID
    /// edge. Their constituent bit meanings remain unknown.
    pub const TX_INTERFACE_POLICY_0: Register32 =
        Register32::described(0x2010_4038, RegisterAccess::ReadWrite, None);
    pub const TX_INTERFACE_POLICY_1: Register32 =
        Register32::described(0x2010_403c, RegisterAccess::ReadWrite, None);
    pub const TX_SCHEDULER_PARAMETER: Register32 =
        Register32::described(0x2010_42f4, RegisterAccess::ReadWrite, None);
    pub const TX_COMMON_STATE_CLEAR: Register32 =
        Register32::described(0x2010_448c, RegisterAccess::ReadWrite, None);

    pub const TX_COMMON_CONTROL: Register32 =
        Register32::described(0x2010_4c30, RegisterAccess::ReadWrite, None);
    pub mod tx_common_control {
        use crate::Field32;
        /// Required by the vendor STA-start image before EDCA queue publish.
        pub const STA_TX_ENABLE: Field32 = Field32::new(4, 1);
    }
    pub const TX_COMMON_TIMING_0: Register32 =
        Register32::described(0x2010_4c54, RegisterAccess::ReadWrite, None);
    pub const TX_COMMON_TIMING_1: Register32 =
        Register32::described(0x2010_4c58, RegisterAccess::ReadWrite, None);

    /// Four packed seven-bit path policies observed around STA TX start.
    /// Working vendor authentication uses `0x7f3f3f7f`; open bring-up
    /// previously left the low path at `0x3f`.
    pub const TX_RX_PATH_POLICY_PACKED: Register32 =
        Register32::described(0x2010_4830, RegisterAccess::ReadWrite, None);
    pub mod tx_rx_path_policy_packed {
        use crate::Field32;
        pub const PATH_0_POLICY: Field32 = Field32::new(0, 7);
        pub const PATH_1_POLICY: Field32 = Field32::new(8, 7);
        pub const PATH_2_POLICY: Field32 = Field32::new(16, 7);
        pub const PATH_3_POLICY: Field32 = Field32::new(24, 7);
    }

    /// Packed per-rate PHY gain-table indices captured after calibration.
    ///
    /// The bytes are gain-memory selectors, not dBm values. The working
    /// vendor profile caps the input table at 20 dBm in quarter-dBm units,
    /// then calibration turns that profile into these hardware indices.
    pub const TX_POWER_COMMAND: [Register32; 14] = [
        Register32::new(0x2010_4408),
        Register32::new(0x2010_440c),
        Register32::new(0x2010_4410),
        Register32::new(0x2010_4414),
        Register32::new(0x2010_4418),
        Register32::new(0x2010_441c),
        Register32::new(0x2010_4420),
        Register32::new(0x2010_4424),
        Register32::new(0x2010_4428),
        Register32::new(0x2010_442c),
        Register32::new(0x2010_4430),
        Register32::new(0x2010_4434),
        Register32::new(0x2010_4438),
        Register32::new(0x2010_443c),
    ];
    /// Auxiliary enable/policy associated with the packed power commands.
    ///
    /// The working STA authentication path reads `0x0104_0000`; replaying it
    /// together with the vendor command words did not by itself recover ACK.
    pub const TX_POWER_AUX_CONTROL: Register32 =
        Register32::described(0x2010_4448, RegisterAccess::ReadWrite, None);
    /// Scheduler configuration observed as `0x0400_0000` in vendor STA mode.
    pub const TX_SCHEDULER_ORACLE_0: Register32 =
        Register32::described(0x2010_4dd4, RegisterAccess::ReadWrite, None);
    /// Scheduler state/configuration word; its low bits vary while STA runs.
    pub const TX_SCHEDULER_ORACLE_1: Register32 =
        Register32::described(0x2010_4dd8, RegisterAccess::ReadWrite, None);
    /// Scheduler policy/limit word observed as `0x0000_0071` in both paths.
    ///
    /// This is not the vendor-only bit-26 word (that is at `0x4dd4`).
    pub const TX_SCHEDULER_POLICY_OR_LIMIT: Register32 =
        Register32::described(0x2010_4ddc, RegisterAccess::ReadWrite, None);

    /// Legacy TX-vector fields observed during open-authentication comparison.
    pub const LEGACY_VECTOR_DURATION: Register32 =
        Register32::described(0x2010_54dc, RegisterAccess::ReadWrite, None);
    pub const LEGACY_VECTOR_AUX: Register32 =
        Register32::described(0x2010_54e4, RegisterAccess::ReadWrite, None);

    /// Local TSF/EDCA scheduler domain initialized after generic `hal_init`.
    pub const LOCAL_TSF_LOAD_LOW: Register32 =
        Register32::described(0x2010_d818, RegisterAccess::ReadWrite, None);
    pub const LOCAL_TSF_LOAD_HIGH: Register32 =
        Register32::described(0x2010_d81c, RegisterAccess::ReadWrite, None);
    pub const LOCAL_TSF_CONTROL: Register32 =
        Register32::described(0x2010_d814, RegisterAccess::ReadWrite, None);
    pub mod local_tsf_control {
        use crate::Field32;
        pub const LOCAL_DOMAIN_ENABLE: Field32 = Field32::new(4, 1);
    }

    pub const EDCA_SCHEDULER_CONTROL: Register32 =
        Register32::described(0x2010_d858, RegisterAccess::ReadWrite, None);
    pub mod edca_scheduler_control {
        use crate::Field32;
        /// Vendor STA-start leaves both high scheduler gates asserted.
        pub const ENABLE_HIGH_27: Field32 = Field32::new(27, 1);
        pub const ENABLE_HIGH_31: Field32 = Field32::new(31, 1);
        /// Four-bit mode selected as one by the working STA-start image.
        pub const MODE: Field32 = Field32::new(19, 4);
    }
}

/// Per-queue collision/timeout state and its write-one-to-clear register.
///
/// Recovered `hal_mac_get_txq_state(1)` reads timeout bits 16..19 from
/// `TX_STATE`; `hal_mac_clr_txq_state(1, queue)` writes the corresponding bit
/// to `TX_STATE_CLEAR`.
pub const TX_STATE_CLEAR: Register32 = Register32::new(0x2010_4cb0);
pub const TX_STATE: Register32 = Register32::new(0x2010_4cb4);
pub mod tx_state {
    pub const TIMEOUT_SHIFT: u32 = 16;
}

/// Global two-bit TX CCA force field used while disabling a timed-out queue.
pub const TX_CCA_CONTROL: Register32 = Register32::new(0x2010_4c5c);
pub mod tx_cca_control {
    use crate::Field32;
    pub const FORCE: Field32 = Field32::new(30, 2);
}

pub const TX_COMPLETE_CLEAR: Register32 = Register32::new(0x2010_4cb8);
pub const TX_COMPLETE_STATE: Register32 = Register32::new(0x2010_4cbc);
pub const TX_COMPLETE_PRIMARY_Q0: Register32 = Register32::new(0x2010_553c);
pub const TX_COMPLETE_ALTERNATE_Q0: Register32 = Register32::new(0x2010_5540);
pub const TX_COMPLETE_AUX_A_Q0: Register32 = Register32::new(0x2010_5534);
pub const TX_COMPLETE_AUX_B_Q0: Register32 = Register32::new(0x2010_5524);
pub const TX_COMPLETE_AUX_C_Q0: Register32 = Register32::new(0x2010_554c);
pub const TX_COMPLETE_PRIMARY: [Register32; 4] = [
    TX_COMPLETE_PRIMARY_Q0,
    Register32::new(0x2010_54c0),
    Register32::new(0x2010_5444),
    Register32::new(0x2010_53c8),
];
pub const TX_COMPLETE_ALTERNATE: [Register32; 4] = [
    TX_COMPLETE_ALTERNATE_Q0,
    Register32::new(0x2010_54c4),
    Register32::new(0x2010_5448),
    Register32::new(0x2010_53cc),
];
pub const TX_COMPLETE_AUX_A: [Register32; 4] = [
    TX_COMPLETE_AUX_A_Q0,
    Register32::new(0x2010_54b8),
    Register32::new(0x2010_543c),
    Register32::new(0x2010_53c0),
];
pub const TX_COMPLETE_AUX_B: [Register32; 4] = [
    TX_COMPLETE_AUX_B_Q0,
    Register32::new(0x2010_54a8),
    Register32::new(0x2010_542c),
    Register32::new(0x2010_53b0),
];
pub const TX_COMPLETE_AUX_C: [Register32; 4] = [
    TX_COMPLETE_AUX_C_Q0,
    Register32::new(0x2010_54d0),
    Register32::new(0x2010_5454),
    Register32::new(0x2010_53d8),
];

pub mod init {
    use crate::{mac, Register32};

    pub const HANDSHAKE: Register32 = Register32::new(0x2010_4de0);
    pub const CONTROL: Register32 = Register32::new(0x2010_4cac);
    pub const RX_SNIFFER_CONTROL: Register32 = RX_FILTER[3];

    pub const R_4C00: Register32 = Register32::new(0x2010_4c00);
    pub const R_4020: Register32 = Register32::new(0x2010_4020);
    pub const R_4028: Register32 = Register32::new(0x2010_4028);
    pub const R_4048: Register32 = Register32::new(0x2010_4048);
    pub const R_407C: Register32 = Register32::new(0x2010_407c);
    pub const R_4098: Register32 = mac::RX_CSI_CONFIG;
    pub const R_409C: Register32 = Register32::new(0x2010_409c);
    pub const R_40F4: Register32 = Register32::new(0x2010_40f4);
    pub const R_410C: Register32 = Register32::new(0x2010_410c);
    pub const R_4110: Register32 = Register32::new(0x2010_4110);
    pub const R_4114: Register32 = Register32::new(0x2010_4114);
    pub const R_4118: Register32 = Register32::new(0x2010_4118);
    pub const R_4120: Register32 = Register32::new(0x2010_4120);
    pub const R_42B8: Register32 = Register32::new(0x2010_42b8);
    pub const R_4308: Register32 = Register32::new(0x2010_4308);
    pub const R_4400: Register32 = Register32::new(0x2010_4400);
    pub const R_4404: Register32 = Register32::new(0x2010_4404);
    pub const R_444C: Register32 = Register32::new(0x2010_444c);
    pub const R_4450: Register32 = Register32::new(0x2010_4450);
    pub const R_4458: Register32 = Register32::new(0x2010_4458);
    pub const R_445C: Register32 = Register32::new(0x2010_445c);
    pub const R_447C: Register32 = Register32::new(0x2010_447c);
    pub const R_4480: Register32 = Register32::new(0x2010_4480);
    pub const R_4C1C: Register32 = Register32::new(0x2010_4c1c);
    pub const R_4C20: Register32 = Register32::new(0x2010_4c20);
    pub const R_4C24: Register32 = Register32::new(0x2010_4c24);
    pub const R_4C2C: Register32 = Register32::new(0x2010_4c2c);
    pub const R_4C54: Register32 = Register32::new(0x2010_4c54);
    pub const R_4C58: Register32 = Register32::new(0x2010_4c58);
    pub const R_4C60: Register32 = Register32::new(0x2010_4c60);
    pub const R_4C68: Register32 = Register32::new(0x2010_4c68);
    pub const R_4C6C: Register32 = Register32::new(0x2010_4c6c);
    pub const R_4C78: Register32 = Register32::new(0x2010_4c78);
    pub const R_4C7C: Register32 = Register32::new(0x2010_4c7c);
    pub const R_4C80: Register32 = Register32::new(0x2010_4c80);
    pub const R_4C88: Register32 = Register32::new(0x2010_4c88);
    pub const R_4C8C: Register32 = Register32::new(0x2010_4c8c);
    pub const R_4C98: Register32 = Register32::new(0x2010_4c98);
    pub const R_4CA0: Register32 = Register32::new(0x2010_4ca0);
    pub const R_4CA8: Register32 = Register32::new(0x2010_4ca8);
    pub const R_4CC0: Register32 = Register32::new(0x2010_4cc0);
    pub const R_4DE4: Register32 = Register32::new(0x2010_4de4);
    pub const R_4E04: Register32 = Register32::new(0x2010_4e04);
    pub const R_8060: Register32 = Register32::new(0x2010_8060);
    pub const R_807C: Register32 = Register32::new(0x2010_807c);
    pub const R_D83C: Register32 = Register32::new(0x2010_d83c);

    pub const INTERFACE_ADDRESS_LOW: [Register32; 4] = [
        Register32::new(0x2010_405c),
        Register32::new(0x2010_4064),
        Register32::new(0x2010_406c),
        Register32::new(0x2010_4074),
    ];
    pub const INTERFACE_ADDRESS_HIGH: [Register32; 4] = [
        Register32::new(0x2010_4060),
        Register32::new(0x2010_4068),
        Register32::new(0x2010_4070),
        Register32::new(0x2010_4078),
    ];
    pub const RX_FILTER: [Register32; 4] = [
        Register32::new(0x2010_40d8),
        Register32::new(0x2010_40dc),
        Register32::new(0x2010_40e0),
        Register32::new(0x2010_40e4),
    ];
    pub const BSSID_HIGH: [Register32; 3] = [
        Register32::new(0x2010_4004),
        Register32::new(0x2010_400c),
        Register32::new(0x2010_4014),
    ];
    pub const RX_QUEUE_DEFAULT: [Register32; 4] = [
        Register32::new(0x2010_40fc),
        Register32::new(0x2010_4100),
        Register32::new(0x2010_4104),
        Register32::new(0x2010_4108),
    ];
    pub const HE_PROTECTION: [Register32; 4] = [
        mac::TX_Q0_PROTECTION,
        Register32::new(0x2010_4d54),
        Register32::new(0x2010_4d44),
        Register32::new(0x2010_4d34),
    ];
    pub const HE_QUEUE_CONTROL: [Register32; 8] = [
        mac::TX_Q0_PPDU_CONTROL,
        Register32::new(0x2010_4d58),
        Register32::new(0x2010_4d48),
        Register32::new(0x2010_4d38),
        Register32::new(0x2010_4d28),
        Register32::new(0x2010_4d18),
        Register32::new(0x2010_4d08),
        Register32::new(0x2010_4cf8),
    ];
    pub const LAST_RX_BUFFER: [Register32; 18] = [
        Register32::new(0x2010_4124),
        Register32::new(0x2010_4140),
        Register32::new(0x2010_415c),
        Register32::new(0x2010_4128),
        Register32::new(0x2010_4144),
        Register32::new(0x2010_4160),
        Register32::new(0x2010_412c),
        Register32::new(0x2010_4148),
        Register32::new(0x2010_4164),
        Register32::new(0x2010_4130),
        Register32::new(0x2010_414c),
        Register32::new(0x2010_4168),
        Register32::new(0x2010_4134),
        Register32::new(0x2010_4150),
        Register32::new(0x2010_416c),
        Register32::new(0x2010_4138),
        Register32::new(0x2010_4154),
        Register32::new(0x2010_4170),
    ];
    pub const CRYPTO_BYPASS: [Register32; 5] = [
        Register32::new(0x2010_4800),
        Register32::new(0x2010_4804),
        Register32::new(0x2010_4808),
        Register32::new(0x2010_480c),
        Register32::new(0x2010_4810),
    ];

    pub const HE_SCRATCH_COUNT: usize = 120;

    pub const fn he_scratch(index: usize) -> Option<Register32> {
        if index < HE_SCRATCH_COUNT {
            Some(Register32::new(0x2010_55f0 + index * 4))
        } else {
            None
        }
    }
}
