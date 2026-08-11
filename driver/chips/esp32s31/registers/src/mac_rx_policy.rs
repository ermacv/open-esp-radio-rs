//! Generated-PAC ownership for finite receive-policy transitions.

#![forbid(unsafe_code)]

use super::{RadioRegisters, device_fence};

/// Read-only associated-STA receive-policy evidence.
///
/// The two policy words are retained as exact register images because several
/// still-undocumented bits participate in the vendor transaction. All known
/// address and enable fields are decoded through the PAC so qualification
/// code does not need raw MMIO access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacStaReceivePolicySnapshot {
    pub queue_zero_policy: u32,
    pub queue_three_policy: u32,
    pub bssid: [u8; 6],
    pub association_id: u16,
    pub minimum_mpdu_start_spacing: u8,
    pub bssid_address_check_enabled: bool,
    pub interface_is_soft_ap: bool,
    pub interface_rx_policy_enabled: bool,
    pub beacon_filter_control: u8,
}

impl RadioRegisters {
    /// Snapshot every register frontier that can suppress associated-STA
    /// beacons while still admitting directed management traffic.
    pub fn sta_receive_policy_snapshot(&self) -> MacStaReceivePolicySnapshot {
        let filters = &self.peripherals.wifi_mac_rx_filter;
        let bssids = &self.peripherals.wifi_mac_bssid_policy;
        let bssid_low = bssids.bssid_low(0).read().bytes_0_3().bits();
        let bssid_high = bssids.bssid_high(0).read();
        let interface = self
            .peripherals
            .wifi_mac_interface_address
            .address_high(0)
            .read();
        let low = bssid_low.to_le_bytes();
        let high = bssid_high.bssid_high().bits().to_le_bytes();

        MacStaReceivePolicySnapshot {
            queue_zero_policy: filters.policy(0).read().bits(),
            queue_three_policy: filters.policy(3).read().bits(),
            bssid: [low[0], low[1], low[2], low[3], high[0], high[1]],
            association_id: bssid_high.association_id().bits(),
            minimum_mpdu_start_spacing: bssid_high.minimum_mpdu_start_spacing().bits(),
            bssid_address_check_enabled: bssid_high.address_check_enable().bit_is_set(),
            interface_is_soft_ap: bssid_high.interface_is_soft_ap().bit_is_set(),
            interface_rx_policy_enabled: interface.rx_policy_enable().bit_is_set(),
            beacon_filter_control: self
                .peripherals
                .wifi_mac_sta_beacon_filter
                .control()
                .read()
                .enables_unknown()
                .bits(),
        }
    }

    /// Apply the complete four-queue cold policy transaction from `hal_init`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac.o]::hal_init`,
    /// offsets `0x56..0x90`, and complete `hal_mac_rx_set_policy`.
    /// All 31 fresh-read RMW edges remain separate and in blob order.
    pub fn initialize_cold_receive_policy(&mut self) {
        let filters = &self.peripherals.wifi_mac_rx_filter;
        let bssids = &self.peripherals.wifi_mac_bssid_policy;
        let interfaces = &self.peripherals.wifi_mac_interface_address;

        for queue in 0..4 {
            let filter = filters.policy(queue);

            filter.modify(|_, w| {
                w.receive_broadcast_check_bssid()
                    .set_bit()
                    .receive_unicast_check_da()
                    .set_bit()
            });
            filter.modify(|_, w| w.receive_management_not_check_bssid().clear_bit());
            filter.modify(|_, w| {
                w.dump_unicast_check_da()
                    .set_bit()
                    .dump_broadcast_check_bssid()
                    .set_bit()
            });
            filter.modify(|_, w| {
                w.check_bssid()
                    .clear_bit()
                    .dump_management_not_check_bssid()
                    .clear_bit()
            });

            if queue < 3 {
                let bssid = bssids.bssid_high(queue);
                filter.modify(|_, w| {
                    w.receive_management_not_check_bssid()
                        .clear_bit()
                        .pass_beacon()
                        .clear_bit()
                });
                if queue == 1 {
                    bssid.modify(|_, w| w.interface_is_soft_ap().set_bit());
                } else {
                    bssid.modify(|_, w| {
                        w.interface_is_soft_ap()
                            .clear_bit()
                            .address_check_enable()
                            .clear_bit()
                    });
                }
                filter.modify(|_, w| w.dump_management_not_check_bssid().clear_bit());
                bssid.modify(|_, w| w.address_check_enable().clear_bit());
                // Zero is the complete low-half replacement performed by the
                // mode-zero cold leaf; high policy bits are preserved.
                interfaces
                    .address_high(queue)
                    .modify(|_, w| w.bytes_4_5().set(0));
            }
        }
    }

    /// Switch receive queue zero from scan policy to associated-STA policy.
    ///
    /// SOURCE: complete pinned `libpp.a::hal_mac_rx_set_policy` and
    /// `hal_mac_set_rxq_policy`, reached from the complete
    /// `libnet80211.a::wifi_set_rx_policy` policy-six branch used immediately
    /// before the first live vendor Authentication TX. The prefix is the
    /// complete pinned `libpp.a[hal_sniffer.o]::hal_sniffer_disable` leaf:
    /// unlike the vendor scan lifecycle, the open bootstrap deliberately uses
    /// queue three's promiscuous path, so the associated transition must
    /// restore its normal filtering and hardware auto-ACK policy.
    pub fn apply_sta_link_receive_policy(&mut self, bssid_address: [u8; 6]) {
        let sniffer = self.peripherals.wifi_mac_rx_filter.policy(3);
        let filter = self.peripherals.wifi_mac_rx_filter.policy(0);
        let bssids = &self.peripherals.wifi_mac_bssid_policy;
        let bssid = bssids.bssid_high(0);
        let interface = self.peripherals.wifi_mac_interface_address.address_high(0);

        // Complete `hal_sniffer_disable`, size 0x44. Preserve its seven
        // separate fresh-read RMW edges: clear AUTOACK_DISABLE, then restore
        // the six address/rejection checks in exact blob order.
        //
        // SOURCE: `libpp.a[hal_sniffer.o]::hal_sniffer_disable`.
        sniffer.modify(|_, w| w.auto_ack_disable().clear_bit());
        sniffer.modify(|_, w| w.dump_unicast_check_da().set_bit());
        sniffer.modify(|_, w| w.dump_unicast_check_bssid().set_bit());
        sniffer.modify(|_, w| w.dump_broadcast_check_bssid().set_bit());
        sniffer.modify(|_, w| w.abort_group().set_bit());
        sniffer.modify(|_, w| w.receive_unicast_check_bssid().set_bit());
        sniffer.modify(|_, w| {
            w.receive_broadcast_check_bssid()
                .set_bit()
                .receive_unicast_check_da()
                .set_bit()
        });

        // `ic_set_bssid(0, bssid)` delegates to the complete
        // `hal_mac_set_bssid` leaf. Keep its four hardware edges ordered:
        // disable matching, publish the low word, replace the two high bytes,
        // then make the new address valid.
        //
        // SOURCE: complete pinned `libpp.a[hal_mac.o]`
        // `hal_mac_set_bssid`, size 0x5a.
        bssid.modify(|_, w| w.address_check_enable().clear_bit());
        open_esp_radio_esp32s31_pac::zero_based_field_write::mac_bssid_address_low(
            bssids,
            0,
            u32::from_le_bytes([
                bssid_address[0],
                bssid_address[1],
                bssid_address[2],
                bssid_address[3],
            ]),
        );
        bssid.modify(|_, w| {
            w.bssid_high()
                .set(u16::from_le_bytes([bssid_address[4], bssid_address[5]]))
        });
        bssid.modify(|_, w| w.address_check_enable().set_bit());

        // `ic_set_rx_policy(0, mode=0, control=1, management=1)`.
        // Keep every complete-leaf fresh-read RMW edge separate.
        filter.modify(|_, w| {
            w.receive_management_not_check_bssid()
                .clear_bit()
                .pass_beacon()
                .clear_bit()
        });
        bssid.modify(|_, w| w.interface_is_soft_ap().clear_bit());
        filter.modify(|_, w| w.dump_management_not_check_bssid().clear_bit());
        bssid.modify(|_, w| w.address_check_enable().set_bit());
        interface.modify(|_, w| w.rx_policy_enable().set_bit());

        // `wifi_set_rx_policy(6)`, used by the live vendor STA immediately
        // before Open Authentication, finishes with
        // `ic_set_rx_policy_ubssid_check(0, true)`. Its complete libpp leaf
        // performs these two ordered RMWs. The earlier open transcription used
        // policy five/false from migration and produced filter 0x0001_c285;
        // the working vendor authentication capture on ESP32-S31 rev0 shows
        // 0x0001_c387, differing by exactly these bits.
        //
        // SOURCE: `libnet80211.a[ieee80211_supplicant.o]`
        // `wifi_set_rx_policy`, jump-table case six at `.L255` through
        // `.L262`; `libpp.a[if_hwctrl.o]`
        // `ic_set_rx_policy_ubssid_check`; live `wifi-sta-auth-probe` HIL on
        // 2026-07-28.
        filter.modify(|_, w| w.receive_unicast_check_bssid().set_bit());
        filter.modify(|_, w| w.dump_unicast_check_bssid().set_bit());

        device_fence();
    }
}
