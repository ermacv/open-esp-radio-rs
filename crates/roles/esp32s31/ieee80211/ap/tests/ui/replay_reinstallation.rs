#![no_std]
use oer_esp32s31_ieee80211_ap::security::{ApPairwiseBinding, ApSecurity};
use oer_esp32s31_ieee80211_mac::crypto::CcmpKeyHardware;
use oer_ieee80211_mac::ccmp::{CcmpPacketNumber, CcmpReplayLane};

pub fn clear<H: CcmpKeyHardware>(
    security: &mut ApSecurity<'_>,
    hardware: &mut H,
    binding: ApPairwiseBinding,
    peer: [u8; 6],
    pn: CcmpPacketNumber,
) {
    let candidate = security
        .prepare_bound_pairwise_rx(binding, CcmpReplayLane::NonQos, pn)
        .unwrap();
    #[cfg(ui_reject)]
    {
        security.clear_peer(hardware, peer).unwrap(); // ui-error: E0499
        candidate.commit();
    }
    #[cfg(not(ui_reject))]
    {
        candidate.commit();
        security.clear_peer(hardware, peer).unwrap();
    }
}
