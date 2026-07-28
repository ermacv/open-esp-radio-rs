//! Instruction-recovered ESP32-S31 Wi-Fi MAC register boundary.

pub use open_esp_radio_pac_esp32s31::{
    mac::{
        INT_CLEAR as MAC_INT_CLEAR, INT_ENABLE as MAC_INT_ENABLE, INT_RAW as MAC_INT_RAW,
        INT_STATUS as MAC_INT_STATUS, RX_CONTROL, RX_CSI_CONFIG, RX_DESCRIPTOR_BASE,
        RX_LAST_DESCRIPTOR, RX_LAST_DESCRIPTOR_HIGH, RX_NEXT_DESCRIPTOR, TX_CCA_CONTROL,
        TX_COMPLETE_ALTERNATE, TX_COMPLETE_ALTERNATE_Q0, TX_COMPLETE_AUX_A, TX_COMPLETE_AUX_A_Q0,
        TX_COMPLETE_AUX_B, TX_COMPLETE_AUX_B_Q0, TX_COMPLETE_AUX_C, TX_COMPLETE_AUX_C_Q0,
        TX_COMPLETE_CLEAR, TX_COMPLETE_PRIMARY, TX_COMPLETE_PRIMARY_Q0, TX_COMPLETE_STATE,
        TX_Q0_CONFIG, TX_Q0_CONTROL, TX_Q0_LENGTH_CONTROL, TX_Q0_PLCP1, TX_Q0_POWER,
        TX_Q0_PPDU_CONTROL, TX_Q0_PROTECTION, TX_Q0_PTI, TX_Q_CONFIG, TX_Q_CONTROL,
        TX_Q_LENGTH_CONTROL, TX_Q_PLCP1, TX_Q_POWER, TX_Q_PPDU_CONTROL, TX_Q_PROTECTION, TX_Q_PTI,
        TX_STATE, TX_STATE_CLEAR,
    },
    RadioRegisters, Register32,
};

/// Host-testable access boundary used by source-owned MAC transactions.
///
/// A mutable borrow serializes each transaction. Production code can pass the
/// PAC's unique [`RadioRegisters`] owner directly; host tests supply a model.
pub trait Mmio {
    fn read32(&mut self, register: Register32) -> u32;
    fn write32(&mut self, register: Register32, value: u32);
    fn fence(&mut self);
}

impl Mmio for RadioRegisters {
    fn read32(&mut self, register: Register32) -> u32 {
        RadioRegisters::read32(self, register)
    }

    fn write32(&mut self, register: Register32, value: u32) {
        RadioRegisters::write32(self, register, value);
    }

    fn fence(&mut self) {
        RadioRegisters::fence(self);
    }
}

pub const RX_ENABLE: u32 = open_esp_radio_pac_esp32s31::mac::rx_control::WALKER_ENABLE.mask();
pub const RX_RELOAD: u32 =
    open_esp_radio_pac_esp32s31::mac::rx_control::APPEND_DESCRIPTOR_RELOAD.mask();
pub const RX_DESCRIPTOR_HIGH_MASK: u32 = 0xfff0_0000;
pub const RX_DESCRIPTOR_LOW_MASK: u32 = 0x000f_ffff;
pub const RX_DESCRIPTOR_HIGH_WINDOW: u32 = 0x2f00_0000;

pub const TX_Q_ENABLE: u32 = 0x8000_0000;
pub const TX_Q_VALID: u32 = 0x4000_0000;
pub const TX_Q_ENABLE_VALID: u32 = TX_Q_ENABLE | TX_Q_VALID;
pub const TX_CCA_FORCE_MASK: u32 = open_esp_radio_pac_esp32s31::mac::tx_cca_control::FORCE.mask();
pub const TX_CCA_FORCE_DISABLE: u32 = TX_CCA_FORCE_MASK;
pub const TX_TIMEOUT_SHIFT: u32 = open_esp_radio_pac_esp32s31::mac::tx_state::TIMEOUT_SHIFT;

pub const TX_COMPLETE_Q0: u32 = 1;
pub const TX_COMPLETE_HARDWARE_QUEUE_MASK: u32 = 0x0000_07ff;
