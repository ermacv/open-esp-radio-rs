# Migration archive completion

The former `migration/esp32s31-hybrid-runtime` source archive was removed
after its maintained source-owned logic was placed in buildable crates. The
complete pre-cleanup archive remains recoverable from Git commit
`6a2b358b6dc3458d2f49d80ff14d20aa3d6fabab`; the final extracted archive is
also present in the parent history of this change.

## Maintained destinations

| Former workset | Live source of truth |
| --- | --- |
| PHY initialization, calibration and channel transitions | `open-esp-radio-esp32s31-phy` and `open-esp-radio-esp32s31-hal` |
| Radio register identities and fields | `open-esp-radio-esp32s31-pac` and `svd/esp32s31-radio.svd` |
| Scan records, management parsing, STA framing, data framing and CCMP headers | `open-esp-radio-ieee80211` |
| HE20 parsing and peer-state transform | `open-esp-radio-ieee80211::he` |
| AP association response, TIM and power-save parsing | `open-esp-radio-ieee80211::ap` |
| Descriptor ownership, RX/TX, key slots, rate control, queues and A-MPDU | `open-esp-radio-esp32s31-wifi-mac` |
| RX BlockAck register transaction | `open-esp-radio-esp32s31-wifi-mac::rx_ampdu_hw`, using PAC/SVD registers |
| EAPOL parsing, AES unwrap, key-data parsing and WPA2 cryptography | `open-esp-radio-wpa2` |
| STA/AP four-way-handshake and retry state | `open-esp-radio-wpa2::{state,retry,frames}` |
| Owned aligned keys and fixed key table | `open-esp-radio-wpa2::keys` |
| AP WPA2-PSK/CCMP RSN admission | `open-esp-radio-wpa2::ap` |
| Bounded Ethernet ownership boundary | `open-esp-radio-embassy-net` |
| End-to-end open PHY/MAC/scan/STA/WPA2 HIL integration | `esp32s31_rust/firmware/esp32s31/app/src/open_radio_phy_prelude_hil.rs`; remaining reusable runtime extraction is tracked in [`ESP32S31_RUST_INTEGRATION_AUDIT.md`](../../ESP32S31_RUST_INTEGRATION_AUDIT.md) |

The firmware integration delegates WPA2 phase, replay and completion ordering
to `Wpa2StaState`, but it still contains reusable PHY target dispatch, MAC
submission/completion policy and STA orchestration. It is a qualified HIL
source, not the final architectural home of those layers.

## Deleted rather than promoted

The following archive material was intentionally not made part of the live
dependency graph:

- vendor/ROM ABI bindings, symbol interposition and linker override scripts;
- allocator, RTOS/OSI callback and vendor-task emulation;
- global pointer ownership and callback diagnostics;
- duplicate SHA-1, AES, framing, descriptor, PHY and scan implementations;
- strict-mode experiments whose useful finite transforms are now represented
  by owned live types;
- Enterprise EAP and TKIP/Michael compatibility paths, which are outside the
  source-only WPA2-Personal CCMP scope.

These deletions do not discard evidence: Git preserves the exact sources, and
the live SVD/PAC comments identify the blob, ROM, migration file or HIL source
for recovered register hypotheses.

## Stage-one verification

This cleanup stage requires builds and host tests only. It does not claim new
hardware qualification. Hardware-specific additions use explicit confidence
labels such as `instruction-exact-not-hil` until a later HIL run confirms them.
