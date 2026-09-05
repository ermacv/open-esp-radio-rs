//! ESP32-S31 access-point HT advertisement.

use open_esp_radio_ieee80211::ht::HtLocalCapabilities;

/// Exact local advertisement used by AP beacons and association responses.
///
/// Vendor `ieee80211_ht_attach` initializes capability base `0x100c`: SMPS
/// disabled and DSSS/CCK reception permitted in 40 MHz. The portable encoder
/// adds width and short-GI bits from the validated channel geometry.
///
/// Retain the reviewed A-MPDU value `0x03`: exponent three and no local MPDU
/// density limit. The original profile was described against 64 RX descriptors;
/// the current physical RX ring has 96 descriptors. This advertisement is a
/// fixed profile, not a runtime calculation from ring or reorder capacity.
/// Peer-specific responses retain the stricter exponent and spacing limits.
///
/// Receive MCS0..7 and TX MCS parameters `0x01` advertise the ordinary equal
/// one-stream sets. RX MCS32 remains unadvertised pending dedicated HIL proof;
/// peer parsing and RX diagnostics remain independent.
/// `ieee80211_add_htcap_body` writes TX MCS parameters at body offset 15,
/// corresponding to complete information-element byte 17.
pub const HT_CAPABILITIES: HtLocalCapabilities = HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01);

// The public request retains a privately constructed AccessPointClientLimit:
// its constructor rejects zero and values above this software ceiling before
// the request can move radio ownership. Prove at compile time that the chip's
// pairwise key pool covers every admitted peer. This new architectural guard
// does not require future hardware pools to have the same size. It is not a runtime
// comparison or an additional capability claim.
const _: () = assert!(
    open_esp_radio_wifi_ap::limits::AP_MAX_CLIENTS
        <= open_esp_radio_esp32s31_wifi_mac::crypto::AP_PAIRWISE_SLOT_COUNT as usize
);
