#![no_std]

use core::ptr::{read_volatile, write_volatile};

mod agc;
mod baseband;
pub mod clock;
mod frequency;
mod iq_estimator;
pub mod mac;
mod mac_antenna_init;
mod mac_block_ack;
mod mac_channel;
mod mac_coex_init;
mod mac_cold_start;
mod mac_crypto;
mod mac_enable;
mod mac_hal_init_tail;
mod mac_he_init;
mod mac_he_init_suffix;
mod mac_he_peer;
mod mac_interface_address;
mod mac_interrupt;
mod mac_last_rx_buffer;
mod mac_rx_dma;
mod mac_rx_policy;
mod mac_sniffer;
mod mac_tsf;
mod mac_tx;
mod mac_tx_power_init;
mod mac_txrx_init;
pub mod pbus;
pub mod phy;
pub mod phy_i2c;
pub mod power;
mod table_memory;
pub use mac_cold_start::{MacColdHandshakeOutcome, MacColdHandshakeTimeout};
pub use mac_crypto::MacKeyInstallOutcome;
pub use mac_he_peer::{MacHe20PeerConfig, MacHe20PeerError};
pub use mac_interrupt::MacInterruptRegisters;
pub use mac_tx::{
    MacHeTxProgram, MacHeTxVectorSnapshot, MacHtAmpduCompletionRegisters, MacHtTxProgram,
    MacLegacyTxProgram, MacTxCompletionRegisters,
};
pub use mac_tx_power_init::{MacTxPowerPair, MacTxPowerTable, MAC_TX_POWER_RATE_COUNT};
pub use open_esp_radio_svd_esp32s31 as svd;
pub use table_memory::{PbusMemoryGroupBoundary, PhyMemoryError};

#[inline]
fn device_fence() {
    #[cfg(target_arch = "riscv32")]
    // SAFETY: this instruction only orders memory and device accesses.
    unsafe {
        core::arch::asm!("fence iorw, iorw")
    }

    #[cfg(not(target_arch = "riscv32"))]
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

/// Access policy recovered for one MMIO register.
///
/// The compatibility facade takes this value from
/// `svd/esp32s31-radio.svd`. New peripheral code should use the generated
/// [`svd`] register API directly. Handwritten legacy MAC entries default to
/// [`ReadWrite`](RegisterAccess::ReadWrite) until their access policy is
/// recovered.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RegisterAccess {
    /// Software may only observe the register.
    ReadOnly,
    /// Software may only publish a value or trigger.
    WriteOnly,
    /// Software may observe and update the register.
    ReadWrite,
}

/// One PAC-described 32-bit MMIO register.
///
/// The address is intentionally private: downstream crates can use registers
/// described by this PAC but cannot manufacture new MMIO addresses.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Register32 {
    address: usize,
    access: RegisterAccess,
    reset_value: Option<u32>,
}

impl Register32 {
    pub(crate) const fn new(address: usize) -> Self {
        Self {
            address,
            access: RegisterAccess::ReadWrite,
            reset_value: None,
        }
    }

    pub(crate) const fn described(
        address: usize,
        access: RegisterAccess,
        reset_value: Option<u32>,
    ) -> Self {
        Self {
            address,
            access,
            reset_value,
        }
    }

    /// Numeric address for diagnostics and host-side register models.
    pub const fn address(self) -> usize {
        self.address
    }

    /// SVD-described software access policy.
    pub const fn access(self) -> RegisterAccess {
        self.access
    }

    /// Reset value when the recovered source names one.
    ///
    /// `None` means unknown, not necessarily zero.
    pub const fn reset_value(self) -> Option<u32> {
        self.reset_value
    }
}

/// One named bit-field inside a 32-bit register.
///
/// Instances are generated from the recovered SVD. Construction remains
/// crate-private so higher layers cannot assign guessed bit positions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Field32 {
    offset: u8,
    width: u8,
}

impl Field32 {
    pub(crate) const fn new(offset: u8, width: u8) -> Self {
        assert!(width != 0 && width <= 32);
        assert!(offset < 32 && (offset as u16) + (width as u16) <= 32);
        Self { offset, width }
    }

    /// Least-significant bit position recovered for this field.
    pub const fn offset(self) -> u8 {
        self.offset
    }

    /// Number of adjacent bits recovered for this field.
    pub const fn width(self) -> u8 {
        self.width
    }

    /// Register mask occupied by this field.
    pub const fn mask(self) -> u32 {
        if self.width == 32 {
            u32::MAX
        } else {
            ((1_u32 << self.width) - 1) << self.offset
        }
    }

    /// Maximum unshifted value representable by this field.
    pub const fn max_value(self) -> u32 {
        if self.width == 32 {
            u32::MAX
        } else {
            (1_u32 << self.width) - 1
        }
    }

    /// Encode a value, returning `None` rather than truncating an invalid one.
    pub const fn checked_value(self, value: u32) -> Option<u32> {
        if value <= self.max_value() {
            Some(value << self.offset)
        } else {
            None
        }
    }

    /// Extract the unshifted field value from a register image.
    pub const fn extract(self, register: u32) -> u32 {
        (register & self.mask()) >> self.offset
    }

    /// Replace this field while preserving every other register bit.
    ///
    /// Returns `None` when `value` does not fit the recovered field width.
    pub const fn checked_insert(self, register: u32, value: u32) -> Option<u32> {
        match self.checked_value(value) {
            Some(encoded) => Some((register & !self.mask()) | encoded),
            None => None,
        }
    }
}

/// Unique logical owner of the ESP32-S31 radio register regions.
///
/// The generated [`svd::Peripherals`] singleton is kept private. Higher layers
/// retain semantic sequencing and borrow this owner mutably; the only
/// supported split is the one-shot, finite hard-interrupt capability.
pub struct RadioRegisters {
    peripherals: svd::Peripherals,
    wifi_baseband_enabled: bool,
    mac_interrupt_taken: bool,
}

impl RadioRegisters {
    /// Claim radio MMIO when the caller has established unique ownership.
    ///
    /// # Safety
    ///
    /// No other live owner may mutate the radio through raw pointers, ROM,
    /// vendor code, or another `RadioRegisters` value.
    pub unsafe fn steal() -> Self {
        Self {
            // SAFETY: the caller establishes the same unique ownership
            // invariant required by `svd2rust::Peripherals::steal`.
            peripherals: unsafe { svd::Peripherals::steal() },
            wifi_baseband_enabled: false,
            mac_interrupt_taken: false,
        }
    }

    /// Permanently split the hard-ISR register block from the task owner.
    ///
    /// The returned capability contains only MAC interrupt snapshot and
    /// acknowledge operations. `None` prevents a second safe split.
    pub fn take_mac_interrupt(&mut self) -> Option<MacInterruptRegisters> {
        if self.mac_interrupt_taken {
            return None;
        }
        self.mac_interrupt_taken = true;
        // SAFETY: the generated singleton is already held by `self`, but this
        // method permanently removes access to its private interrupt member
        // from every typed `RadioRegisters` API. The returned finite
        // capability is the sole safe owner of that disjoint register block.
        Some(unsafe { MacInterruptRegisters::steal_from_radio_owner() })
    }

    /// Synchronize the owned Wi-Fi-enable image after a platform PAC update.
    ///
    /// This state replaces deep calibration reads through a second, custom
    /// `MODEM_SYSCON` description. The unique [`RadioRegisters`] owner and
    /// its platform token update it together.
    #[doc(hidden)]
    pub fn set_wifi_baseband_enabled_image(&mut self, enabled: bool) {
        self.wifi_baseband_enabled = enabled;
    }

    /// Return the Wi-Fi-enable image owned by this radio instance.
    #[doc(hidden)]
    pub fn wifi_baseband_enabled_image(&self) -> bool {
        self.wifi_baseband_enabled
    }

    pub const fn contains(address: usize) -> bool {
        // The official platform PAC owns HP, PMU and LP peripherals. Legacy
        // raw compatibility is therefore limited to the remaining custom
        // modem/radio aperture and cannot manufacture access to those blocks.
        matches!(address, 0x2010_0000..=0x2010_ffff)
    }

    /// Read one PAC-described 32-bit register.
    pub fn read32(&self, register: Register32) -> u32 {
        debug_assert_ne!(register.access(), RegisterAccess::WriteOnly);
        // SAFETY: only this crate constructs `Register32`, and
        // `RadioRegisters` represents the unique live radio MMIO owner.
        unsafe { read_volatile(register.address as *const u32) }
    }

    /// Write one PAC-described 32-bit register.
    pub fn write32(&mut self, register: Register32, value: u32) {
        debug_assert_ne!(register.access(), RegisterAccess::ReadOnly);
        // SAFETY: only this crate constructs `Register32`, and the mutable
        // borrow serializes writes through the unique live radio owner.
        unsafe { write_volatile(register.address as *mut u32, value) }
    }

    /// Perform a finite read/modify/write on a PAC-described register.
    pub fn modify32(&mut self, register: Register32, clear_mask: u32, set_bits: u32) -> u32 {
        let previous = self.read32(register);
        let next = (previous & !clear_mask) | (set_bits & clear_mask);
        self.write32(register, next);
        previous
    }

    /// Order device-memory accesses at a descriptor or interrupt boundary.
    pub fn fence(&mut self) {
        device_fence();
    }

    // Temporary compatibility for PHY leaves that have not yet moved to
    // PAC-described registers. New target paths must use the typed methods.

    /// Read one evidenced 32-bit radio register.
    ///
    /// # Safety
    ///
    /// `address` must be aligned and identify a readable register.
    pub unsafe fn read(&self, address: usize) -> u32 {
        debug_assert!(Self::contains(address));
        unsafe { read_volatile(address as *const u32) }
    }

    /// Write one evidenced 32-bit radio register.
    ///
    /// # Safety
    ///
    /// `address` must be aligned and identify a writable register, and the
    /// value must obey that register's hardware contract.
    pub unsafe fn write(&mut self, address: usize, value: u32) {
        debug_assert!(Self::contains(address));
        unsafe { write_volatile(address as *mut u32, value) }
    }

    /// Perform a finite read/modify/write transaction.
    ///
    /// # Safety
    ///
    /// The register must permit read/modify/write with the supplied masks.
    pub unsafe fn replace_bits(&mut self, address: usize, clear_mask: u32, set_bits: u32) -> u32 {
        let previous = unsafe { self.read(address) };
        let next = (previous & !clear_mask) | (set_bits & clear_mask);
        unsafe { self.write(address, next) };
        previous
    }
}

#[cfg(test)]
mod tests {
    use super::{mac, power, Field32, RadioRegisters, Register32, RegisterAccess};

    fn assert_valid(register: Register32) {
        assert!(RadioRegisters::contains(register.address()));
        assert_eq!(register.address() & 3, 0);
    }

    #[test]
    fn every_power_register_belongs_to_a_known_mmio_region() {
        for register in power::ALL {
            assert_valid(register);
        }
    }

    #[test]
    fn fields_reject_values_that_do_not_fit() {
        let field = Field32::new(8, 4);
        assert_eq!(field.mask(), 0x0000_0f00);
        assert_eq!(field.checked_value(6), Some(0x0000_0600));
        assert_eq!(field.checked_value(16), None);
        assert_eq!(field.checked_insert(0xffff_f0ff, 6), Some(0xffff_f6ff));
        assert_eq!(field.extract(0x0000_0a00), 10);
    }

    #[test]
    fn generated_access_and_reset_metadata_are_preserved() {
        assert_eq!(
            power::phy_clock_oracle::FE_BB_CLOCK_CONTROL_OPAQUE.access(),
            RegisterAccess::ReadWrite
        );
        assert_eq!(
            power::phy_clock_oracle::FE_CLOCK_GATE_OPAQUE.reset_value(),
            None
        );
    }

    #[test]
    fn mac_interrupt_capability_can_only_be_split_once() {
        // SAFETY: this host test does not access MMIO and creates no second
        // `RadioRegisters` value; it exercises only the local split state.
        let mut registers = unsafe { RadioRegisters::steal() };
        assert!(registers.take_mac_interrupt().is_some());
        assert!(registers.take_mac_interrupt().is_none());
    }

    #[test]
    fn generated_tx_banks_reverse_physical_order_exactly_once() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let control = &registers.peripherals.wifi_mac_tx_queue_control;
        let vector = &registers.peripherals.wifi_mac_tx_queue_vector;
        let completion = &registers.peripherals.wifi_mac_tx_completion;
        assert_eq!(control.control(3).as_ptr() as usize, 0x2010_4d70);
        assert_eq!(control.control(0).as_ptr() as usize, 0x2010_4d40);
        assert_eq!(vector.plcp1(3).as_ptr() as usize, 0x2010_54d8);
        assert_eq!(vector.plcp1(0).as_ptr() as usize, 0x2010_5364);
        assert_eq!(completion.primary(3).as_ptr() as usize, 0x2010_553c);
        assert_eq!(completion.primary(0).as_ptr() as usize, 0x2010_53c8);
    }

    #[test]
    fn generated_sta_rx_policy_registers_match_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_bssid_policy
                .bssid_high(0)
                .as_ptr() as usize,
            0x2010_4004
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_interface_address
                .address_high(0)
                .as_ptr() as usize,
            0x2010_4060
        );
        assert_eq!(
            registers.peripherals.wifi_mac_rx_filter.policy(0).as_ptr() as usize,
            0x2010_40d8
        );
        assert_eq!(
            registers.peripherals.wifi_mac_rx_filter.policy(3).as_ptr() as usize,
            0x2010_40e4
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_rx_filter
                .misc_packet_policy()
                .as_ptr() as usize,
            0x2010_40f4
        );
        assert_eq!(
            registers.peripherals.wifi_mac_control.control().as_ptr() as usize,
            0x2010_4cac
        );
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_regdma_control
                .control()
                .as_ptr() as usize,
            0x2010_d83c
        );
    }

    #[test]
    fn generated_interface_address_pairs_match_complete_leaf_stride() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let addresses = &registers.peripherals.wifi_mac_interface_address;
        for interface in 0..4 {
            assert_eq!(
                addresses.address_low(interface).as_ptr() as usize,
                0x2010_405c + interface * 8
            );
            assert_eq!(
                addresses.address_high(interface).as_ptr() as usize,
                0x2010_4060 + interface * 8
            );
        }
    }

    #[test]
    fn generated_cold_handshake_matches_complete_hal_init_prefix() {
        // SAFETY: this host test inspects a generated register pointer only
        // and performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_cold_handshake
                .control()
                .as_ptr() as usize,
            0x2010_4de0
        );
    }

    #[test]
    fn generated_crypto_aux_register_matches_complete_cold_leaf() {
        // SAFETY: this host test inspects a generated register pointer only
        // and performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_crypto_control
                .init_aux_unknown()
                .as_ptr() as usize,
            0x2010_480c
        );
    }

    #[test]
    fn generated_rx_cold_prefix_registers_match_complete_leaf() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let dma = &registers.peripherals.wifi_mac_rx_dma;
        assert_eq!(dma.rx_cold_control_unknown().as_ptr() as usize, 0x2010_407c);
        assert_eq!(dma.rx_buffer_limit_unknown().as_ptr() as usize, 0x2010_4c68);
        assert_eq!(dma.rx_buffer_base_unknown().as_ptr() as usize, 0x2010_4c6c);
        assert_eq!(
            dma.rx_descriptor_high_window().as_ptr() as usize,
            0x2010_4c70
        );
    }

    #[test]
    fn generated_mac_enable_gate_matches_complete_leaf() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_core_enable
                .control()
                .as_ptr() as usize,
            0x2010_4c00
        );
        assert_eq!(
            registers.peripherals.wifi_mac_interrupt.enable().as_ptr() as usize,
            0x2010_4c40
        );
    }

    #[test]
    fn generated_phy_low_rate_registers_match_complete_rom_leaves() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let bb = &registers.peripherals.phy_agc_oracle;
        assert_eq!(bb.low_rate_primary_control().as_ptr() as usize, 0x2010_8060);
        assert_eq!(
            bb.low_rate_secondary_control().as_ptr() as usize,
            0x2010_807c
        );
    }

    #[test]
    fn generated_last_rx_buffer_table_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let table = &registers.peripherals.wifi_mac_last_rx_buffer;
        assert_eq!(table.control().as_ptr() as usize, 0x2010_4120);
        for entry in 0..6 {
            assert_eq!(
                table.entry_control(entry).as_ptr() as usize,
                0x2010_4124 + entry * 4
            );
            assert_eq!(
                table.entry_parameter_a(entry).as_ptr() as usize,
                0x2010_4140 + entry * 4
            );
            assert_eq!(
                table.entry_parameter_b(entry).as_ptr() as usize,
                0x2010_415c + entry * 4
            );
        }
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_rx_csi_control
                .control()
                .as_ptr() as usize,
            0x2010_4098
        );
    }

    #[test]
    fn generated_mac_txrx_prefix_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let init = &registers.peripherals.wifi_mac_txrx_prefix;
        for queue in 0..4 {
            assert_eq!(
                init.rx_queue_default(queue).as_ptr() as usize,
                0x2010_40fc + queue * 4
            );
        }
        assert_eq!(init.control_edges().as_ptr() as usize, 0x2010_4114);
        assert_eq!(init.timing_control().as_ptr() as usize, 0x2010_4118);
        assert_eq!(init.feature_edges().as_ptr() as usize, 0x2010_4c8c);
        assert_eq!(init.mode_control().as_ptr() as usize, 0x2010_4c98);
        assert_eq!(init.shared_enable_control().as_ptr() as usize, 0x2010_4ca0);
    }

    #[test]
    fn generated_mac_antenna_init_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let init = &registers.peripherals.wifi_mac_antenna_init;
        assert_eq!(init.common_control().as_ptr() as usize, 0x2010_42b0);
        for physical_bank in 0..8 {
            assert_eq!(
                init.bank_control(physical_bank).as_ptr() as usize,
                0x2010_51ac + physical_bank * 0x7c
            );
        }
        assert_eq!(init.bank_control(7).as_ptr() as usize, 0x2010_5510);
    }

    #[test]
    fn generated_mac_coex_init_matches_complete_setter_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let coex = &registers.peripherals.wifi_mac_coex_init;
        assert_eq!(coex.rx_pti().as_ptr() as usize, 0x2010_42fc);
        assert_eq!(
            coex.ofdma_tb_and_beamforming().as_ptr() as usize,
            0x2010_4dd4
        );
        assert_eq!(coex.beamforming().as_ptr() as usize, 0x2010_4dd8);
        assert_eq!(coex.default_control().as_ptr() as usize, 0x2010_4ddc);
    }

    #[test]
    fn generated_mac_he_prefix_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let init = &registers.peripherals.wifi_mac_he_init_prefix;
        assert_eq!(init.rx_field_control().as_ptr() as usize, 0x2010_4048);
        assert_eq!(init.bf_report_rate().as_ptr() as usize, 0x2010_4464);
        assert_eq!(init.bf_timing_control().as_ptr() as usize, 0x2010_4c78);
        assert_eq!(init.parent_control_edges().as_ptr() as usize, 0x2010_4c80);
        assert_eq!(init.tb_tx_control().as_ptr() as usize, 0x2010_4e04);
        assert_eq!(init.bf_sync_status_unknown().as_ptr() as usize, 0x2010_7128);
    }

    #[test]
    fn generated_mac_he_suffix_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let init = &registers.peripherals.wifi_mac_he_init_suffix;
        assert_eq!(init.multi_bssid_control().as_ptr() as usize, 0x2010_4020);
        assert_eq!(init.broadcast_ru_low().as_ptr() as usize, 0x2010_4038);
        assert_eq!(init.tx_mode_control().as_ptr() as usize, 0x2010_42b8);
        assert_eq!(init.common_power_control().as_ptr() as usize, 0x2010_4400);
        assert_eq!(init.ersu_ack_rate().as_ptr() as usize, 0x2010_4404);
        assert_eq!(init.ersu_and_vht_control().as_ptr() as usize, 0x2010_4c7c);
        for physical in 0..8 {
            assert_eq!(
                init.queue_control(physical).as_ptr() as usize,
                0x2010_4cf8 + physical * 0x10
            );
        }
        for physical in 0..4 {
            assert_eq!(
                init.protection(physical).as_ptr() as usize,
                0x2010_4d34 + physical * 0x10
            );
        }
        for word in 0..120 {
            assert_eq!(
                init.he_scratch(word).as_ptr() as usize,
                0x2010_55f0 + word * 4
            );
        }
    }

    #[test]
    fn generated_mac_tx_power_init_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let power = &registers.peripherals.wifi_mac_tx_power_init;
        for word in 0..10 {
            assert_eq!(
                power.immediate_response(word).as_ptr() as usize,
                0x2010_4408 + word * 4
            );
        }
        for word in 0..3 {
            assert_eq!(
                power.tb_power(word).as_ptr() as usize,
                0x2010_4430 + word * 4
            );
            assert_eq!(
                power.tb_ru_power(word).as_ptr() as usize,
                0x2010_4440 + word * 4
            );
        }
        assert_eq!(power.tb_ru_power_tail().as_ptr() as usize, 0x2010_443c);
    }

    #[test]
    fn generated_mac_hal_tail_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let rtc = &registers.peripherals.wifi_mac_rtc_timer_update;
        assert_eq!(rtc.control().as_ptr() as usize, 0x2010_d830);
        assert_eq!(rtc.sta_tsf_control().as_ptr() as usize, 0x2010_d858);
        assert_eq!(rtc.slow_clock_calibration().as_ptr() as usize, 0x2010_d878);
        assert_eq!(
            registers
                .peripherals
                .wifi_mac_rx_csi_control
                .control()
                .as_ptr() as usize,
            0x2010_4098
        );
    }

    #[test]
    fn mac_hal_tail_rejects_out_of_range_calibration_before_mmio() {
        // SAFETY: the rejected input returns before any generated register is
        // accessed; the host test therefore performs no volatile MMIO.
        let mut registers = unsafe { RadioRegisters::steal() };
        assert!(!registers.initialize_mac_hal_tail(0x19a8_79e0, 0x0004_0000));
        assert!(!registers.initialize_mac_hal_tail(0, u32::MAX));
    }

    #[test]
    fn generated_mac_txrx_callbacks_match_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let callbacks = &registers.peripherals.wifi_mac_txrx_callbacks;
        assert_eq!(callbacks.ack_rate_primary().as_ptr() as usize, 0x2010_444c);
        assert_eq!(
            callbacks.ack_policy_primary().as_ptr() as usize,
            0x2010_4450
        );
        assert_eq!(
            callbacks.ack_rate_secondary().as_ptr() as usize,
            0x2010_4458
        );
        assert_eq!(
            callbacks.ack_policy_secondary().as_ptr() as usize,
            0x2010_445c
        );
        assert_eq!(
            callbacks.bb_rx_hang_control().as_ptr() as usize,
            0x2010_4c1c
        );
        assert_eq!(callbacks.delay_secondary().as_ptr() as usize, 0x2010_4c54);
        assert_eq!(callbacks.delay_primary().as_ptr() as usize, 0x2010_4c58);
    }

    #[test]
    fn mac_txrx_callbacks_reject_out_of_range_slot_before_mmio() {
        // SAFETY: the rejected input returns before any generated register is
        // accessed; the host test therefore performs no volatile MMIO.
        let mut registers = unsafe { RadioRegisters::steal() };
        assert!(!registers.initialize_mac_txrx_callbacks(11));
        assert!(!registers.initialize_mac_txrx_callbacks(u8::MAX));
    }

    #[test]
    fn generated_mac_txrx_suffix_matches_complete_leaf_geometry() {
        // SAFETY: this host test inspects generated register pointers only and
        // performs no volatile access.
        let registers = unsafe { RadioRegisters::steal() };
        let init = &registers.peripherals.wifi_mac_txrx_suffix;
        assert_eq!(init.aux_enable().as_ptr() as usize, 0x2010_4308);
        assert_eq!(init.control_edges().as_ptr() as usize, 0x2010_4c1c);
        assert_eq!(init.default_image_a().as_ptr() as usize, 0x2010_4c20);
        assert_eq!(init.default_image_b().as_ptr() as usize, 0x2010_4c24);
        assert_eq!(init.gate_control().as_ptr() as usize, 0x2010_4c60);
        assert_eq!(init.field_control().as_ptr() as usize, 0x2010_4ca8);
        assert_eq!(
            registers.peripherals.wifi_mac_rx_dma.rx_control().as_ptr() as usize,
            0x2010_4080
        );
    }

    #[test]
    fn indexed_mac_registers_are_bounded_and_aligned() {
        for group in [
            &mac::init::BSSID_LOW[..],
            &mac::init::INTERFACE_ADDRESS_LOW[..],
            &mac::init::INTERFACE_ADDRESS_HIGH,
            &mac::init::RX_FILTER,
            &mac::init::BSSID_HIGH,
            &mac::init::RX_QUEUE_DEFAULT,
            &mac::init::LAST_RX_BUFFER,
            &mac::init::CRYPTO_BYPASS,
        ] {
            for &register in group {
                assert_valid(register);
            }
        }
    }

    #[test]
    fn mac_init_aliases_share_canonical_register_identities() {
        assert_eq!(mac::init::R_4098, mac::RX_CSI_CONFIG);
    }
}
