#![no_std]
use oer_esp32s31_ieee80211_ap::security::ApPairwiseRxCandidate;

pub fn commit(candidate: ApPairwiseRxCandidate<'_>) {
    candidate.commit();
    #[cfg(ui_reject)]
    candidate.commit(); // ui-error: E0382
}
