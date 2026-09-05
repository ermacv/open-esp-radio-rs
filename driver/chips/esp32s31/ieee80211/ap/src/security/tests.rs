use super::*;
use open_esp_radio_esp32s31_hal::types::MacKeyInstallOutcome;
use open_esp_radio_esp32s31_wifi_mac::crypto::CcmpKeyHardware;
use open_esp_radio_wpa2::{Pmk, PtkContext};

#[derive(Default)]
struct Hardware {
    installs: std::vec::Vec<u8>,
    clears: std::vec::Vec<u8>,
}

impl CcmpKeyHardware for Hardware {
    fn install_sta_ccmp_entry(
        &mut self,
        index: u8,
        _identity: open_esp_radio_esp32s31_hal::types::MacCcmpKeyIdentity,
        _temporal_key: &[u8; 16],
    ) -> MacKeyInstallOutcome {
        self.installs.push(index);
        MacKeyInstallOutcome::Installed
    }

    fn clear_ccmp_entry(&mut self, index: u8) {
        self.clears.push(index);
    }
}

#[test]
fn teardown_clears_pairwise_before_group_and_cannot_alias_a_second_peer() {
    let mut hardware = Hardware::default();
    let gtk = Wpa2Gtk::new(1, true, [0x55; 16]).unwrap();
    let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
    let mut security =
        Esp32s31ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
    let ptk = Pmk::derive(b"password", b"ssid")
        .unwrap()
        .derive_ptk(PtkContext {
            authenticator_address: [2; 6],
            supplicant_address: [3; 6],
            authenticator_nonce: [4; 32],
            supplicant_nonce: [5; 32],
        });
    security
        .install_pairwise(&mut hardware, [3; 6], 1, &ptk)
        .unwrap();
    let binding = security.bind_pairwise([3; 6], 1).unwrap();
    assert_eq!(binding.hardware_index(), 8);
    assert_eq!(
        security
            .next_bound_pairwise_tx_ccmp_header(binding)
            .unwrap(),
        [3, 0, 0, 0x20, 0, 0, 0, 0]
    );
    assert_eq!(
        security.bind_pairwise([3; 6], 2),
        Err(Esp32s31ApSecurityError::WrongPeer)
    );
    assert_eq!(security.pairwise_hardware_index([3; 6]), Ok(8));
    assert_eq!(
        security.install_pairwise(&mut hardware, [3; 6], 2, &ptk),
        Err(Esp32s31ApSecurityError::PairwiseAlreadyInstalled),
    );
    security
        .install_pairwise(&mut hardware, [6; 6], 2, &ptk)
        .unwrap();
    let (report, _pairwise) = security.stop(&mut hardware);
    assert_eq!(hardware.installs, [2, 8, 9]);
    assert_eq!(hardware.clears, [8, 9, 2]);
    assert_eq!(
        report,
        Esp32s31ApSecurityStopReport::Wpa2Personal {
            pairwise_slots_cleared: 2,
            group_hardware_index: 2,
        }
    );
}

#[test]
fn all_public_aids_own_disjoint_pairwise_slots() {
    let mut hardware = Hardware::default();
    let gtk = Wpa2Gtk::new(1, true, [0x55; 16]).unwrap();
    let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
    let mut security =
        Esp32s31ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
    let ptk = Pmk::derive(b"password", b"ssid")
        .unwrap()
        .derive_ptk(PtkContext {
            authenticator_address: [2; 6],
            supplicant_address: [3; 6],
            authenticator_nonce: [4; 32],
            supplicant_nonce: [5; 32],
        });

    for association_id in 1..=15_u16 {
        let peer = [2, 0, 0, 0, 1, association_id as u8];
        security
            .install_pairwise(&mut hardware, peer, association_id, &ptk)
            .unwrap();
        assert_eq!(
            security.pairwise_hardware_index(peer),
            Ok(7 + association_id as u8),
        );
    }
    assert_eq!(
        hardware.installs,
        (2..=22)
            .filter(|index| *index == 2 || *index >= 8)
            .collect::<std::vec::Vec<_>>()
    );

    let overflow = security.install_pairwise(&mut hardware, [2, 0, 0, 0, 2, 1], 16, &ptk);
    assert_eq!(
        overflow,
        Err(Esp32s31ApSecurityError::Crypto(
            CryptoKeyError::InvalidAccessPointAssociationId,
        )),
    );
    let (report, _pairwise) = security.stop(&mut hardware);
    assert_eq!(
        report,
        Esp32s31ApSecurityStopReport::Wpa2Personal {
            pairwise_slots_cleared: 15,
            group_hardware_index: 2,
        }
    );
    assert_eq!(
        hardware.clears,
        (8..=22)
            .chain(core::iter::once(2))
            .collect::<std::vec::Vec<_>>()
    );
}

#[test]
fn rx_replay_is_per_tid_and_fenced_across_pairwise_key_reinstall() {
    let peer = [3; 6];
    let mut hardware = Hardware::default();
    let gtk = Wpa2Gtk::new(1, true, [0x55; 16]).unwrap();
    let mut pairwise = Esp32s31ApPairwiseKeyStorage::new();
    let mut security =
        Esp32s31ApSecurity::install_group(&mut hardware, &gtk, &mut pairwise).unwrap();
    let ptk = Pmk::derive(b"password", b"ssid")
        .unwrap()
        .derive_ptk(PtkContext {
            authenticator_address: [2; 6],
            supplicant_address: peer,
            authenticator_nonce: [4; 32],
            supplicant_nonce: [5; 32],
        });
    security
        .install_pairwise(&mut hardware, peer, 1, &ptk)
        .unwrap();
    let first_generation = security.bind_pairwise(peer, 1).unwrap();
    let pn1 = CcmpPacketNumber::new(1).unwrap();
    let pn4 = CcmpPacketNumber::new(4).unwrap();
    let pn5 = CcmpPacketNumber::new(5).unwrap();

    let first = security
        .prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn4)
        .unwrap();
    security.commit_bound_pairwise_rx(first).unwrap();
    assert_eq!(
        security.prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn4,),
        Err(Esp32s31ApSecurityError::Replay(CcmpReplayError::Replayed {
            packet_number: pn4,
            highest: pn4,
        })),
        "a different 802.11 sequence cannot make a reused PN admissible"
    );

    for lane in [CcmpReplayLane::Tid(1), CcmpReplayLane::Tid(7)] {
        security
            .commit_bound_pairwise_rx_immediate(first_generation, lane, pn1)
            .unwrap();
        assert_eq!(
            security.commit_bound_pairwise_rx_immediate(first_generation, lane, pn1),
            Err(Esp32s31ApSecurityError::Replay(CcmpReplayError::Replayed {
                packet_number: pn1,
                highest: pn1,
            })),
            "ordinary replay admission advances exactly the selected lane"
        );
    }

    let prepared_before_clear = security
        .prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn5)
        .unwrap();
    security.clear_peer(&mut hardware, peer).unwrap();
    security
        .install_pairwise(&mut hardware, peer, 1, &ptk)
        .unwrap();
    let replacement_generation = security.bind_pairwise(peer, 1).unwrap();
    assert_ne!(replacement_generation, first_generation);
    assert_eq!(
        security.commit_bound_pairwise_rx(prepared_before_clear),
        Err(Esp32s31ApSecurityError::WrongPeer),
        "a candidate from the cleared key cannot advance its replacement"
    );
    assert_eq!(
        security.prepare_bound_pairwise_rx(first_generation, CcmpReplayLane::NonQos, pn5,),
        Err(Esp32s31ApSecurityError::WrongPeer),
    );
    assert_eq!(
        security.commit_bound_pairwise_rx_immediate(first_generation, CcmpReplayLane::Tid(3), pn5,),
        Err(Esp32s31ApSecurityError::WrongPeer),
        "the immediate path cannot mutate a replacement key through a stale binding"
    );

    let replacement = security
        .prepare_bound_pairwise_rx(replacement_generation, CcmpReplayLane::NonQos, pn1)
        .unwrap();
    security.commit_bound_pairwise_rx(replacement).unwrap();
}
