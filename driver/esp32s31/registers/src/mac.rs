//! ESP32-S31 Wi-Fi MAC register identities.
//!
//! Semantically named registers cover the live interrupt, RX and TX paths.
//! `init` additionally contains instruction-recovered cold-start registers
//! whose field names are not yet proven. Numeric names are intentional there:
//! they localize MMIO without inventing hardware semantics.

use crate::{Register32, RegisterAccess};

pub const INT_RAW: Register32 = Register32::new(0x2010_4c44);
pub const INT_STATUS: Register32 = Register32::new(0x2010_4c48);
pub mod int_status {
    use crate::Field32;

    /// SOURCE\[BLOB_LIBPP_WDEV_PROCESS_FIQ,BLOB_LIBPP_HAL_INIT_TAIL,
    /// HIL_OPEN_MAC_IRQ_STATUS_CLASSIFICATION_2026_08_03\];
    /// CONFIDENCE\[instruction-exact-semantics-unknown\].
    ///
    /// Observed with `RX_SUCCESS` under sustained receive traffic and
    /// acknowledged only as part of the full STATUS image. It is not an
    /// independently dispatched RX work item.
    pub const RX_ASSOCIATED_AUXILIARY_5: Field32 = Field32::new(5, 1);

    /// SOURCE\[BLOB_LIBPP_WDEV_PROCESS_FIQ,BLOB_LIBPP_HAL_INIT_TAIL,
    /// HIL_OPEN_MAC_IRQ_STATUS_CLASSIFICATION_2026_08_03\];
    /// CONFIDENCE\[instruction-exact-semantics-unknown\].
    ///
    /// Observed with `RX_SUCCESS` under sustained receive traffic and
    /// acknowledged only as part of the full STATUS image. It is not an
    /// independently dispatched RX work item.
    pub const RX_ASSOCIATED_AUXILIARY_24: Field32 = Field32::new(24, 1);
}

/// RX descriptor-walker control.
///
/// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS,ROM_REV0_HAL_MAC_RX_GATE,
/// HIL_OPEN_RX_LIVE_APPEND_2026_07_27]; CONFIDENCE\[instruction-exact-and-hil].
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
/// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS,HIL_OPEN_RX_LIVE_APPEND_2026_07_27];
/// CONFIDENCE\[instruction-exact-and-hil]. The cold `wDev_AppendRxBlocks` path
/// writes this when the software list is empty. A live-list append normally
/// links the accepted tail and uses
/// [`rx_control::APPEND_DESCRIPTOR_RELOAD`].
pub const RX_DESCRIPTOR_BASE: Register32 =
    Register32::described(0x2010_4084, RegisterAccess::ReadWrite, None);
/// Current/next descriptor selected by the RX walker; zero denotes no current
/// descriptor at the observed terminal frontier.
///
/// SOURCE\[ROM_REV0_WDEV_APPEND_RX_BLOCKS,HIL_OPEN_RX_LIVE_APPEND_2026_07_27];
/// CONFIDENCE\[instruction-exact-and-hil]. ROM reads this after the reload bit
/// clears and repairs the base only when it is zero and the accepted hardware
/// tail has not reached the newly published software tail.
pub const RX_NEXT_DESCRIPTOR: Register32 =
    Register32::described(0x2010_4088, RegisterAccess::ReadOnly, None);
/// Last descriptor accepted by the RX walker.
///
/// SOURCE\[ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR,
/// HIL_OPEN_RX_LIVE_APPEND_2026_07_27]; CONFIDENCE\[instruction-exact-and-hil].
/// ROM reconstructs the pointer from this register's low 20 bits and
/// [`RX_LAST_DESCRIPTOR_HIGH`]'s high 12 bits. The HIL uses the accepted last
/// descriptor to rotate ownership instead of assuming descriptor zero.
pub const RX_LAST_DESCRIPTOR: Register32 =
    Register32::described(0x2010_408c, RegisterAccess::ReadOnly, None);
pub const RX_CSI_CONFIG: Register32 = Register32::new(0x2010_4098);
/// High address window shared by RX descriptor pointer registers.
///
/// SOURCE\[ROM_REV0_HAL_MAC_RX_LAST_DESCRIPTOR,
/// HIL_OPEN_RX_LIVE_APPEND_2026_07_27]; CONFIDENCE\[instruction-exact-and-hil].
/// The open driver programs `0x2f00_0000` for internal SRAM descriptors; the
/// pointer registers carry the low 20 address bits.
pub const RX_LAST_DESCRIPTOR_HIGH: Register32 =
    Register32::described(0x2010_4c70, RegisterAccess::ReadWrite, None);

/// Per-interface hardware crypto controls for STA, AP and NAN.
///
/// SOURCE\[BLOB_LIBPP_HAL_CRYPTO_ENABLE]; the interface number selects one
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
/// SOURCE: generated `WIFI_MAC_TX_QUEUE_VECTOR.HT_SIGNAL3`.
pub const TX_Q0_HT_SIGNAL: Register32 = Register32::new(0x2010_54e8);
pub const TX_Q0_POWER: Register32 = Register32::new(0x2010_5500);
/// SOURCE: generated `WIFI_MAC_TX_QUEUE_VECTOR.HT_DESCRIPTOR_COUNTS3`.
pub const TX_Q0_HT_DESCRIPTOR_COUNTS: Register32 = Register32::new(0x2010_5504);
/// SOURCE: generated `WIFI_MAC_TX_QUEUE_VECTOR.DATA_LENGTH3`.
pub const TX_Q0_DATA_LENGTH: Register32 = Register32::new(0x2010_550c);
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
pub const TX_Q_HT_SIGNAL: [Register32; 4] = [
    TX_Q0_HT_SIGNAL,
    Register32::new(0x2010_546c),
    Register32::new(0x2010_53f0),
    Register32::new(0x2010_5374),
];
pub const TX_Q_POWER: [Register32; 4] = [
    TX_Q0_POWER,
    Register32::new(0x2010_5484),
    Register32::new(0x2010_5408),
    Register32::new(0x2010_538c),
];
pub const TX_Q_HT_DESCRIPTOR_COUNTS: [Register32; 4] = [
    TX_Q0_HT_DESCRIPTOR_COUNTS,
    Register32::new(0x2010_5488),
    Register32::new(0x2010_540c),
    Register32::new(0x2010_5390),
];
pub const TX_Q_DATA_LENGTH: [Register32; 4] = [
    TX_Q0_DATA_LENGTH,
    Register32::new(0x2010_5490),
    Register32::new(0x2010_5414),
    Register32::new(0x2010_5398),
];
pub const TX_Q_LENGTH_CONTROL: [Register32; 4] = [
    TX_Q0_LENGTH_CONTROL,
    Register32::new(0x2010_5494),
    Register32::new(0x2010_5418),
    Register32::new(0x2010_539c),
];

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
    use crate::{Register32, mac};

    pub const HANDSHAKE: Register32 = Register32::new(0x2010_4de0);
    pub const CONTROL: Register32 = Register32::new(0x2010_4cac);

    pub const R_4C00: Register32 = Register32::new(0x2010_4c00);
    pub const R_407C: Register32 = Register32::new(0x2010_407c);
    pub const R_4098: Register32 = mac::RX_CSI_CONFIG;
    pub const R_4114: Register32 = Register32::new(0x2010_4114);
    pub const R_4118: Register32 = Register32::new(0x2010_4118);
    pub const R_4120: Register32 = Register32::new(0x2010_4120);
    pub const R_4308: Register32 = Register32::new(0x2010_4308);
    pub const R_444C: Register32 = Register32::new(0x2010_444c);
    pub const R_4450: Register32 = Register32::new(0x2010_4450);
    pub const R_4458: Register32 = Register32::new(0x2010_4458);
    pub const R_445C: Register32 = Register32::new(0x2010_445c);
    pub const R_4C1C: Register32 = Register32::new(0x2010_4c1c);
    pub const R_4C20: Register32 = Register32::new(0x2010_4c20);
    pub const R_4C24: Register32 = Register32::new(0x2010_4c24);
    pub const R_4C54: Register32 = Register32::new(0x2010_4c54);
    pub const R_4C58: Register32 = Register32::new(0x2010_4c58);
    pub const R_4C60: Register32 = Register32::new(0x2010_4c60);
    pub const R_4C68: Register32 = Register32::new(0x2010_4c68);
    pub const R_4C6C: Register32 = Register32::new(0x2010_4c6c);
    pub const R_4C8C: Register32 = Register32::new(0x2010_4c8c);
    pub const R_4C98: Register32 = Register32::new(0x2010_4c98);
    pub const R_4CA0: Register32 = Register32::new(0x2010_4ca0);
    pub const R_4CA8: Register32 = Register32::new(0x2010_4ca8);
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
    pub const BSSID_LOW: [Register32; 4] = [
        Register32::new(0x2010_4000),
        Register32::new(0x2010_4008),
        Register32::new(0x2010_4010),
        Register32::new(0x2010_4018),
    ];
    pub const BSSID_HIGH: [Register32; 4] = [
        Register32::new(0x2010_4004),
        Register32::new(0x2010_400c),
        Register32::new(0x2010_4014),
        Register32::new(0x2010_401c),
    ];
    pub const RX_QUEUE_DEFAULT: [Register32; 4] = [
        Register32::new(0x2010_40fc),
        Register32::new(0x2010_4100),
        Register32::new(0x2010_4104),
        Register32::new(0x2010_4108),
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
}
