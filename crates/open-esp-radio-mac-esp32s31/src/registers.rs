//! Instruction-recovered ESP32-S31 Wi-Fi MAC register addresses.

/// Minimal target boundary used by the source-owned register transactions.
///
/// Implementations must perform volatile 32-bit accesses. `fence` must order
/// device memory (RISC-V targets use `fence iorw, iorw`).
pub trait Mmio {
    fn read32(&self, address: u32) -> u32;
    fn write32(&self, address: u32, value: u32);
    fn fence(&self);
}

pub const MAC_INT_ENABLE: u32 = 0x2010_4c40;
pub const MAC_INT_RAW: u32 = 0x2010_4c44;
pub const MAC_INT_STATUS: u32 = 0x2010_4c48;
pub const MAC_INT_CLEAR: u32 = 0x2010_4c4c;

pub const MAC_INT_TX_COMPLETE: u32 = 0x0000_0080;
pub const MAC_INT_COLLISION: u32 = 0x0000_0100;
pub const MAC_INT_WATCHDOG: u32 = 0x0000_0800;
pub const MAC_INT_RX_SUCCESS: u32 = 0x0000_4000;
pub const MAC_INT_TX_TIMEOUT: u32 = 0x0008_0000;

pub const RX_CONTROL: u32 = 0x2010_4080;
pub const RX_DESCRIPTOR_BASE: u32 = 0x2010_4084;
pub const RX_NEXT_DESCRIPTOR: u32 = 0x2010_4088;
pub const RX_LAST_DESCRIPTOR: u32 = 0x2010_408c;
pub const RX_CSI_CONFIG: u32 = 0x2010_4098;
pub const RX_LAST_DESCRIPTOR_HIGH: u32 = 0x2010_4c70;

pub const RX_ENABLE: u32 = 0x8000_0000;
pub const RX_RELOAD: u32 = 0x0000_0001;
pub const RX_DESCRIPTOR_HIGH_MASK: u32 = 0xfff0_0000;
pub const RX_DESCRIPTOR_LOW_MASK: u32 = 0x000f_ffff;
pub const RX_DESCRIPTOR_HIGH_WINDOW: u32 = 0x2f00_0000;

pub const TX_Q0_CONTROL: u32 = 0x2010_4d70;
pub const TX_Q0_CONFIG: u32 = 0x2010_4d6c;
pub const TX_Q_ENABLE: u32 = 0x8000_0000;
pub const TX_Q_VALID: u32 = 0x4000_0000;
pub const TX_Q_ENABLE_VALID: u32 = TX_Q_ENABLE | TX_Q_VALID;

pub const TX_STATE: u32 = 0x2010_4cb4;
pub const TX_COMPLETE_CLEAR: u32 = 0x2010_4cb8;
pub const TX_COMPLETE_STATE: u32 = 0x2010_4cbc;
pub const TX_COMPLETE_Q0: u32 = 1;
pub const TX_COMPLETE_HARDWARE_QUEUE_MASK: u32 = 0x0000_07ff;

pub const TX_COMPLETE_PRIMARY_Q0: u32 = 0x2010_553c;
pub const TX_COMPLETE_ALTERNATE_Q0: u32 = 0x2010_5540;
pub const TX_COMPLETE_AUX_A_Q0: u32 = 0x2010_5534;
pub const TX_COMPLETE_AUX_B_Q0: u32 = 0x2010_5524;
pub const TX_COMPLETE_AUX_C_Q0: u32 = 0x2010_554c;
