//! Typed HE trigger/OFDMA control and best-effort hardware diagnostics.

use super::RadioRegisters;

/// One IEEE 802.11 traffic identifier accepted by the recovered HE BSR block.
///
/// Keeping this as a bounded newtype means array and MMIO indexing cannot
/// receive an arbitrary `u8`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTid(u8);

impl MacHeTid {
    pub const COUNT: usize = 8;

    pub const fn new(value: u8) -> Option<Self> {
        if value < Self::COUNT as u8 {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    const fn mask(self) -> u8 {
        1 << self.0
    }
}

/// One non-latched view of all hardware-visible HE Buffer Status Reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeBufferStatusSnapshot {
    pub hardware: [u32; MacHeTid::COUNT],
    pub software: [u32; MacHeTid::COUNT],
    pub valid_tid_bitmap: u8,
    pub trigger_based_tid_bitmap: u8,
    pub ac_empty_software_tid: u8,
    pub ac_empty_uses_software_tid: bool,
    pub basic_special_bsr_sequence: bool,
    pub tid_limit_zero_bsr_sequence: bool,
}

/// Key fields from the last HE Trigger receive calculation.
///
/// The blob explicitly warns that `WDEVAXDIAG0..9` can be overwritten by a
/// following RX frame. This is a diagnostic sample, not durable protocol
/// state. Pair it with a change in `MacHeTbStatistics::rx_trigger_count`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeTriggerRxDiagnostics {
    pub trigger_state_cs: u8,
    pub station_match: bool,
    pub trigger_type: u8,
    pub uplink_length: u16,
    pub association_id: u16,
    pub ru_allocation_raw: u8,
    pub ru_allocation: u8,
    pub uplink_mcs: u8,
    pub uplink_dcm: bool,
    pub fec_coding_type: bool,
    pub gi_and_ltf_type: u8,
    pub uplink_bandwidth: u8,
    pub stbc: bool,
    pub mu_mimo_ltf: bool,
    pub spatial_reuse: u16,
    pub basic_target_rssi: u8,
    pub spatial_stream_allocation: u8,
    pub cs_required: bool,
    pub more_trigger_frames: bool,
    pub symbol_count: u16,
    pub tb_apep_length: u16,
    pub cbw20_packet_count: u8,
    pub cbw40_packet_count: u8,
    pub cbw80_packet_count: u8,
    pub qos_null_append_packet_count: u8,
}

impl RadioRegisters {
    /// Select whether one TID participates in HE trigger-based BSR handling.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]`
    /// `hal_he_set_tid_bitmap` and `hal_he_clr_tid_bitmap`, reached by the
    /// complete HE BlockAck response/deletion paths in
    /// `_oracles/libnet80211.a[ieee80211_ht.o]`. The separate initial read and
    /// fresh read inside `modify` preserve the two-read/one-write blob edge.
    pub fn set_he_trigger_based_tid_enabled(&mut self, tid: MacHeTid, enabled: bool) {
        let control = self.peripherals.wifi_mac_he_buffer_status.control();
        let old_bitmap = control.read().tid_bitmap().bits();
        let bitmap = if enabled {
            old_bitmap | tid.mask()
        } else {
            old_bitmap & !tid.mask()
        };

        // SAFETY: the bounded TID newtype makes `bitmap` exactly eight bits.
        control.modify(|_, w| unsafe { w.tid_bitmap().bits(bitmap) });
    }

    /// Read all eight interleaved hardware/software BSR values and control.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_debug.o]`
    /// `dbg_read_bsr_info` and `_oracles/libpp.a[hal_mac_ctl.o]`
    /// `hal_he_get_hw_txq_bsr`.
    pub fn he_buffer_status_snapshot(&self) -> MacHeBufferStatusSnapshot {
        let registers = &self.peripherals.wifi_mac_he_buffer_status;
        let mut hardware = [0_u32; MacHeTid::COUNT];
        let mut software = [0_u32; MacHeTid::COUNT];
        for tid in 0..MacHeTid::COUNT {
            hardware[tid] = registers.hardware_bsr(tid).read().value().bits();
            software[tid] = registers.software_bsr(tid).read().value().bits();
        }
        let control = registers.control().read();

        MacHeBufferStatusSnapshot {
            hardware,
            software,
            valid_tid_bitmap: control.valid_tid_bitmap().bits(),
            trigger_based_tid_bitmap: control.tid_bitmap().bits(),
            ac_empty_software_tid: control.ac_empty_software_tid().bits(),
            ac_empty_uses_software_tid: control.ac_empty_use_software_tid().bit(),
            basic_special_bsr_sequence: control.basic_special_bsr_sequence().bit(),
            tid_limit_zero_bsr_sequence: control.tid_limit_zero_bsr_sequence().bit(),
        }
    }

    /// Make trigger-based transmissions respect or ignore CCA and NAV.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_debug.o]`
    /// `dbg_tb_ignore_cca_enable` and its own strings. A set hardware bit means
    /// "tb care CCA and NAV"; the debug ignore mode clears it.
    pub fn set_he_trigger_based_cca_and_nav_care(&mut self, care: bool) {
        self.peripherals
            .wifi_mac_he_init_suffix
            .ersu_and_vht_control()
            .modify(|_, w| w.tb_care_cca_and_nav().bit(care));
    }

    /// Apply the complete OBSS narrow-band RU aggregate disable leaf.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_mac_ctl.o]`
    /// `hal_he_disable_obss_narrow_bw_ru`, size `0x3e`. The whole-leaf meaning
    /// is symbol-exact; the individual twenty-bit bitmap and five-bit class
    /// identities remain approximate and are marked as such in the SVD.
    pub fn set_he_obss_narrow_band_ru_disabled(&mut self, disabled: bool) {
        let registers = &self.peripherals.wifi_mac_he_obss_narrow_band_ru;
        // SAFETY: both complete images fit the generated twenty-bit field.
        unsafe {
            registers
                .disable_bitmap()
                .write_with_zero(|w| w.value().bits(if disabled { 0x000f_ffff } else { 0 }));
        }
        // SAFETY: both complete images fit the generated five-bit field.
        registers
            .control()
            .modify(|_, w| unsafe { w.class_disable().bits(if disabled { 0x1f } else { 0 }) });
        registers
            .control()
            .modify(|_, w| w.global_disable().bit(disabled));
    }

    /// Read key fields from the blob's ten-word Trigger RX diagnostic block.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_debug.o]`
    /// `dbg_read_ax_diag`, size `0x466`, and its `WDEVAXDIAG0..9` strings.
    pub fn he_trigger_receive_diagnostics(&self) -> MacHeTriggerRxDiagnostics {
        let registers = &self.peripherals.wifi_mac_he_trigger_rx_diagnostics;
        let state = registers.state().read();
        let length = registers.length().read();
        let dependent = registers.trigger_dependent().read();
        let user = registers.basic_user().read();
        let phy = registers.common_phy().read();
        let trigger = registers.common_trigger().read();
        let counts = registers.packet_counts().read();
        let ru_allocation_raw = user.ru_allocation_raw().bits();

        MacHeTriggerRxDiagnostics {
            trigger_state_cs: state.trigger_state_cs().bits(),
            station_match: dependent.sta_match().bit(),
            trigger_type: trigger.trigger_type().bits(),
            uplink_length: trigger.ul_length().bits(),
            association_id: user.rx_aid().bits(),
            ru_allocation_raw,
            ru_allocation: ru_allocation_raw >> 1,
            uplink_mcs: user.ul_mcs().bits(),
            uplink_dcm: dependent.basic_ul_dcm().bit(),
            fec_coding_type: user.fec_coding_type().bit(),
            gi_and_ltf_type: phy.gi_and_ltf_type().bits(),
            uplink_bandwidth: phy.ul_bandwidth().bits(),
            stbc: phy.stbc().bit(),
            mu_mimo_ltf: phy.mu_mimo_ltf().bit(),
            spatial_reuse: phy.spatial_reuse().bits(),
            basic_target_rssi: dependent.basic_target_rssi().bits(),
            spatial_stream_allocation: dependent.basic_spatial_stream_allocation().bits(),
            cs_required: trigger.cs_required().bit(),
            more_trigger_frames: trigger.more_tf().bit(),
            symbol_count: length.symbol_count().bits(),
            tb_apep_length: length.tb_apep_length().bits(),
            cbw20_packet_count: counts.cbw20_packet_count().bits(),
            cbw40_packet_count: counts.cbw40_packet_count().bits(),
            cbw80_packet_count: counts.cbw80_packet_count().bits(),
            qos_null_append_packet_count: counts.qos_null_append_packet_count().bits(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_identifier_is_bounded_before_mmio_indexing() {
        for tid in 0..MacHeTid::COUNT as u8 {
            let tid = MacHeTid::new(tid).unwrap();
            assert_eq!(tid.mask(), 1 << tid.value());
        }
        assert_eq!(MacHeTid::new(8), None);
        assert_eq!(MacHeTid::new(u8::MAX), None);
    }
}
