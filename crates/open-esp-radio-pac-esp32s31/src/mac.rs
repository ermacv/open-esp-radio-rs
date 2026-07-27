//! ESP32-S31 Wi-Fi MAC register identities.
//!
//! Semantically named registers cover the live interrupt, RX and TX paths.
//! `init` additionally contains instruction-recovered cold-start registers
//! whose field names are not yet proven. Numeric names are intentional there:
//! they localize MMIO without inventing hardware semantics.

use crate::Register32;

pub const INT_ENABLE: Register32 = Register32::new(0x2010_4c40);
pub const INT_RAW: Register32 = Register32::new(0x2010_4c44);
pub const INT_STATUS: Register32 = Register32::new(0x2010_4c48);
pub const INT_CLEAR: Register32 = Register32::new(0x2010_4c4c);

pub const RX_CONTROL: Register32 = Register32::new(0x2010_4080);
pub const RX_DESCRIPTOR_BASE: Register32 = Register32::new(0x2010_4084);
pub const RX_NEXT_DESCRIPTOR: Register32 = Register32::new(0x2010_4088);
pub const RX_LAST_DESCRIPTOR: Register32 = Register32::new(0x2010_408c);
pub const RX_CSI_CONFIG: Register32 = Register32::new(0x2010_4098);
pub const RX_LAST_DESCRIPTOR_HIGH: Register32 = Register32::new(0x2010_4c70);

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

pub const TX_STATE: Register32 = Register32::new(0x2010_4cb4);
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
    pub const R_42B0: Register32 = Register32::new(0x2010_42b0);
    pub const R_42B8: Register32 = Register32::new(0x2010_42b8);
    pub const R_42FC: Register32 = Register32::new(0x2010_42fc);
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
    pub const R_4DDC: Register32 = Register32::new(0x2010_4ddc);
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
    pub const ANTENNA_CONTROL_COUNT: usize = 8;

    pub const fn he_scratch(index: usize) -> Option<Register32> {
        if index < HE_SCRATCH_COUNT {
            Some(Register32::new(0x2010_55f0 + index * 4))
        } else {
            None
        }
    }

    pub const fn antenna_control(index: usize) -> Option<Register32> {
        if index < ANTENNA_CONTROL_COUNT {
            Some(if index == 0 {
                mac::TX_Q0_LENGTH_CONTROL
            } else {
                Register32::new(0x2010_5510 - index * 0x7c)
            })
        } else {
            None
        }
    }
}
