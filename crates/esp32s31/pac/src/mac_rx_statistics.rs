//! Typed snapshots of hardware RX counters recovered from the blob decoders.

use super::RadioRegisters;

/// BSS-color collision state read by the complete blob debug decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeColorCollisionSnapshot {
    pub observed_color_bitmap: u64,
    pub collision_threshold: u8,
    pub timeout_seconds: u8,
    pub bitmap_control: u8,
    pub color_bitmap_clear: bool,
}

/// Ordinary receive, completion and interface counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxPrimaryStatistics {
    pub mpdu_count: u16,
    /// Signed upper half of `WDEVRX_MPDU`, multiplied by 40 exactly as the
    /// complete blob decoder prints `WDEVRX_CFO`.
    pub cfo_scaled_40: i32,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub data_success: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bt_block_error: u16,
    pub frequency_hop_error: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
}

/// Wrapping deltas for the 16-bit counters in [`MacRxPrimaryStatistics`].
///
/// `cfo_scaled_40` is intentionally absent: it is a signed accumulator sample,
/// not an event counter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacRxPrimaryStatisticsDelta {
    pub mpdu_count: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub data_success: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bt_block_error: u16,
    pub frequency_hop_error: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
}

impl MacRxPrimaryStatistics {
    /// Return counter increments since an earlier hardware snapshot.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_debug.o]::
    /// dbg_read_rx_count` reads these values as unsigned halfwords. HIL shows
    /// the primary counters crossing `0xffff` during sustained HE20 traffic,
    /// so interval telemetry must use 16-bit wrapping subtraction rather than
    /// comparing absolute boot-lifetime values.
    pub fn wrapping_delta_since(self, earlier: Self) -> MacRxPrimaryStatisticsDelta {
        MacRxPrimaryStatisticsDelta {
            mpdu_count: self.mpdu_count.wrapping_sub(earlier.mpdu_count),
            fcs_error: self.fcs_error.wrapping_sub(earlier.fcs_error),
            abort: self.abort.wrapping_sub(earlier.abort),
            abort_fcs_pass: self.abort_fcs_pass.wrapping_sub(earlier.abort_fcs_pass),
            power_drop_error: self.power_drop_error.wrapping_sub(earlier.power_drop_error),
            he_sig_b_error: self.he_sig_b_error.wrapping_sub(earlier.he_sig_b_error),
            same_bm_error: self.same_bm_error.wrapping_sub(earlier.same_bm_error),
            signal_field: self.signal_field.wrapping_sub(earlier.signal_field),
            end: self.end.wrapping_sub(earlier.end),
            data_success: self.data_success.wrapping_sub(earlier.data_success),
            other_unicast: self.other_unicast.wrapping_sub(earlier.other_unicast),
            buffer_full: self.buffer_full.wrapping_sub(earlier.buffer_full),
            fifo_overflow: self.fifo_overflow.wrapping_sub(earlier.fifo_overflow),
            tkip_error: self.tkip_error.wrapping_sub(earlier.tkip_error),
            bt_block_error: self.bt_block_error.wrapping_sub(earlier.bt_block_error),
            frequency_hop_error: self
                .frequency_hop_error
                .wrapping_sub(earlier.frequency_hop_error),
            last_unmatched_error: self
                .last_unmatched_error
                .wrapping_sub(earlier.last_unmatched_error),
            ack_interrupt: self.ack_interrupt.wrapping_sub(earlier.ack_interrupt),
            rts_interrupt: self.rts_interrupt.wrapping_sub(earlier.rts_interrupt),
        }
    }
}

/// Ten-bit baseband and normal-RX decoder error counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxDecodeErrorStatistics {
    pub brx_agc: u16,
    pub brx: u16,
    pub nrx: u16,
    pub nrx_abort: u16,
    pub nrx_agc_exit: u16,
    pub nrx_baseband_off: u16,
    pub nrx_fdm_watchdog: u16,
    pub nrx_restart: u16,
    pub nrx_service: u16,
    pub nrx_tx_over: u16,
    pub nrx_unsupported: u16,
    pub nrx_he_format: u16,
    pub nrx_ht_sig: u16,
    pub nrx_he_unsupported: u16,
    pub nrx_he_sig_a_crc: u16,
}

/// MAC/baseband hang counters exposed by the same complete decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxHangStatistics {
    pub rx: u8,
    pub tx: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

/// One allocation-free view of every field printed by `dbg_read_rx_count`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxStatisticsSnapshot {
    pub primary: MacRxPrimaryStatistics,
    pub decode_errors: MacRxDecodeErrorStatistics,
    pub hang: MacRxHangStatistics,
}

impl RadioRegisters {
    /// Read only the receive-buffer starvation counter.
    ///
    /// Unlike [`Self::rx_statistics_snapshot`], this single-register accessor
    /// is cheap enough to sample at an RX service boundary. It exists so the
    /// live owner can correlate a newly observed starvation event with its
    /// descriptor frontier and software credits without decoding the complete
    /// diagnostic register bank on every wake.
    pub fn mac_rx_buffer_full_count(&self) -> u16 {
        self.peripherals
            .wifi_mac_rx_statistics
            .buffer_full()
            .read()
            .count()
            .bits()
    }

    /// Read the recovered HE BSS-color collision state.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_debug.o]`
    /// `dbg_read_color_collision`, size `0x64`, and its format strings.
    pub fn he_color_collision_snapshot(&self) -> MacHeColorCollisionSnapshot {
        let colors = &self.peripherals.wifi_mac_he_color_collision;
        let control = self
            .peripherals
            .wifi_mac_he_init_prefix
            .rx_field_control()
            .read();
        let low = colors.bss_color_bitmap_low().read().value().bits();
        let high = colors.bss_color_bitmap_high().read().value().bits();

        MacHeColorCollisionSnapshot {
            observed_color_bitmap: u64::from(low) | (u64::from(high) << 32),
            collision_threshold: control.collision_threshold().bits(),
            timeout_seconds: control.timeout_seconds().bits(),
            bitmap_control: control.bitmap_control().bits(),
            // SOURCE: complete `_oracles/libpp.a[hal_debug.o]::
            // dbg_read_color_collision` prints the low bit separately as
            // COLOR_BITMAP_CLR and the containing two-bit value as BITMAP.
            // The SVD keeps one non-overlapping field; this typed diagnostic
            // exposes the named low-bit view without duplicating MMIO fields.
            color_bitmap_clear: control.bitmap_control().bits() & 1 != 0,
        }
    }

    /// Read every statistic decoded by the complete `dbg_read_rx_count`.
    ///
    /// This is a best-effort multi-register diagnostic sample: the hardware
    /// counters continue changing while the snapshot is assembled.
    ///
    /// SOURCE: complete pinned `_oracles/libpp.a[hal_debug.o]`
    /// `dbg_read_rx_count`, size `0x21a`, and its two format strings.
    pub fn rx_statistics_snapshot(&self) -> MacRxStatisticsSnapshot {
        let registers = &self.peripherals.wifi_mac_rx_statistics;
        let mpdu_and_cfo = registers.mpdu_and_cfo().read();
        let abort = registers.abort().read();
        let hangs = &self.peripherals.wifi_mac_rx_hang_statistics;
        let hang = hangs.hang().read();

        MacRxStatisticsSnapshot {
            primary: MacRxPrimaryStatistics {
                mpdu_count: mpdu_and_cfo.mpdu_count().bits(),
                cfo_scaled_40: i32::from(mpdu_and_cfo.cfo_accumulator().bits() as i16) * 40,
                fcs_error: registers.fcs_error().read().count().bits(),
                abort: abort.count().bits(),
                abort_fcs_pass: abort.fcs_pass_count().bits(),
                power_drop_error: registers.nrx_error_power_drop().read().count().bits(),
                he_sig_b_error: registers.nrx_he_sig_b_error().read().count().bits(),
                same_bm_error: registers.same_bm_error().read().count().bits(),
                signal_field: registers.signal_field().read().count().bits(),
                end: registers.end().read().count().bits(),
                data_success: registers.data_success().read().count().bits(),
                other_unicast: registers.other_unicast().read().count().bits(),
                buffer_full: registers.buffer_full().read().count().bits(),
                fifo_overflow: registers.fifo_overflow().read().count().bits(),
                tkip_error: registers.tkip_error().read().count().bits(),
                bt_block_error: registers.bt_block_error().read().count().bits(),
                frequency_hop_error: registers.frequency_hop_error().read().count().bits(),
                last_unmatched_error: registers.last_unmatched_error().read().count().bits(),
                ack_interrupt: registers.ack_interrupt().read().count().bits(),
                rts_interrupt: registers.rts_interrupt().read().count().bits(),
            },
            decode_errors: MacRxDecodeErrorStatistics {
                brx_agc: registers.brx_error_agc().read().count().bits(),
                brx: registers.brx_error().read().count().bits(),
                nrx: registers.nrx_error().read().count().bits(),
                nrx_abort: registers.nrx_error_abort().read().count().bits(),
                nrx_agc_exit: registers.nrx_error_agc_exit().read().count().bits(),
                nrx_baseband_off: registers.nrx_error_baseband_off().read().count().bits(),
                nrx_fdm_watchdog: registers.nrx_error_fdm_watchdog().read().count().bits(),
                nrx_restart: registers.nrx_error_restart().read().count().bits(),
                nrx_service: registers.nrx_error_service().read().count().bits(),
                nrx_tx_over: registers.nrx_error_tx_over().read().count().bits(),
                nrx_unsupported: registers.nrx_unsupported().read().count().bits(),
                nrx_he_format: registers.nrx_he_format().read().count().bits(),
                nrx_ht_sig: registers.nrx_ht_sig_error().read().count().bits(),
                nrx_he_unsupported: registers.nrx_he_unsupported().read().count().bits(),
                nrx_he_sig_a_crc: registers.nrx_he_sig_a_crc().read().count().bits(),
            },
            hang: MacRxHangStatistics {
                rx: hang.rx_count().bits(),
                tx: hang.tx_count().bits(),
                rx_tx_hang: hangs.rx_tx_hang().read().count().bits(),
                rx_tx_panic: hangs.rx_tx_panic().read().count().bits(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MacRxPrimaryStatistics, MacRxPrimaryStatisticsDelta};

    fn primary(mpdu_count: u16, buffer_full: u16) -> MacRxPrimaryStatistics {
        MacRxPrimaryStatistics {
            mpdu_count,
            cfo_scaled_40: -80,
            fcs_error: 2,
            abort: 3,
            abort_fcs_pass: 4,
            power_drop_error: 5,
            he_sig_b_error: 6,
            same_bm_error: 7,
            signal_field: 8,
            end: 9,
            data_success: 10,
            other_unicast: 11,
            buffer_full,
            fifo_overflow: 12,
            tkip_error: 13,
            bt_block_error: 14,
            frequency_hop_error: 15,
            last_unmatched_error: 16,
            ack_interrupt: 17,
            rts_interrupt: 18,
        }
    }

    #[test]
    fn cfo_transform_matches_complete_blob_signed_halfword_arithmetic() {
        let transform = |raw: u16| i32::from(raw as i16) * 40;
        assert_eq!(transform(0), 0);
        assert_eq!(transform(1), 40);
        assert_eq!(transform(0xffff), -40);
        assert_eq!(transform(0x8000), -1_310_720);
        assert_eq!(transform(0x7fff), 1_310_680);
    }

    #[test]
    fn primary_counter_delta_preserves_sixteen_bit_wrap() {
        let earlier = primary(0xfffe, 0xffff);
        let mut current = primary(3, 2);
        current.fcs_error = 9;

        let delta = current.wrapping_delta_since(earlier);
        assert_eq!(
            delta,
            MacRxPrimaryStatisticsDelta {
                mpdu_count: 5,
                fcs_error: 7,
                buffer_full: 3,
                ..MacRxPrimaryStatisticsDelta::default()
            }
        );
    }
}
