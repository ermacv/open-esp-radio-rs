# ESP32-S31 upper-stack porting map

This directory is an intentionally incomplete source archive, not a Cargo
crate. It retains only work that still needs review and extraction into the
live source-only driver. Do not restore dependencies merely to make the
archive compile.

The complete pre-cleanup archive remains recoverable from Git commit
`6a2b358b6dc3458d2f49d80ff14d20aa3d6fabab`.

## Qualified code removed from this archive

| Removed frontier | Maintained implementation | Qualification |
| --- | --- | --- |
| Cold PHY, PHY-I2C, PBUS, RFPLL, RX DCO/saturation, DC/IQ, frequency, temperature, signal-power, PWDET, XTAL duty, channel transition | `crates/open-esp-radio-phy-esp32s31` | Source-only cold start and channel transitions passed HIL without vendor radio initialization |
| Passive-scan records, beacon/probe-response parsing, BSS de-duplication and selection | `crates/open-esp-radio-ieee80211/src/scan.rs` (compatibly re-exported as `mac::scan`) | Open RX passive scan passed across 2.4 GHz channels 1–13 |
| CCMP TX packet-number arithmetic and header encoding | `crates/open-esp-radio-ieee80211/src/ccmp.rs` | Host vectors match the recovered policy; protected WPA2 Message 4 passed twice on hardware with PN 3 and STA pairwise slot 4 |

Those implementations now have one source of truth in the live crates. Code
remaining here may still name the former `crate::phy_*` or `crate::scan`
interfaces; that is an explicit porting dependency, not a reason to restore
the duplicates.

## Remaining extraction groups

| Group | Representative archive files | Intended destination |
| --- | --- | --- |
| Hardware TX scheduling, queues, completion and retry | `lmac.rs`, `tx_queue.rs`, `tx_mapper.rs`, `tx_plcp.rs`, `tx_proto.rs`, `txdone.rs`, parts of `radio_hal.rs` | live HAL/MAC crates with explicit descriptor and peripheral ownership |
| RX and data path above the live descriptor layer | `rx.rs`, `rx_proto.rs`, `rx_ampdu*.rs`, `data_rx.rs`, `data_tx.rs` | live MAC plus a future network-facing crate |
| 802.11 framing and state | `net80211_*.rs`, `beacon.rs`, `he.rs`, `rate_*.rs` | `open-esp-radio-ieee80211`; Probe Request construction and scan parsing are already live |
| STA and AP control | `sta_link.rs`, `ap_power_save.rs`, beacon/TBTT and channel state modules | future STA/AP crates over the live MAC |
| WPA2, EAPOL, remaining CCMP ownership/adapters and integrity | `wpa2*.rs`, `net80211_crypto_tx.rs`, `crypto.rs`, `michael.rs`, `eap.rs` | live WPA2/IEEE80211/MAC crates with owned keys and buffers; pure CCMP PN/header policy is already live |
| Async/runtime experiments | `runtime.rs`, `task.rs`, `timer.rs`, `event*.rs`, `queue.rs`, `command.rs` | reuse only finite source-owned state machines that fit the live radio owner |
| Vendor compatibility glue | `adapter.rs`, `osi.rs`, `direct_api.rs`, `static_*.rs`, `handoff.rs` | normally delete; port only finite logic whose hardware invariant is independently established |

`radio_hal.rs` and `rx.rs` intentionally remain for now: they mix duplicate
low-level shapes with unique upper-path logic, so deleting them wholesale
would lose useful work. They should be split while the corresponding live
ownership boundary is implemented.

## Rule for leaving the archive

For each remaining feature:

1. Extract the smallest source-owned transformation or state machine; do not
   copy an entire hybrid module.
2. Replace vendor/global pointers with owned descriptors, buffers, keys and
   peripheral capabilities from the live crates.
3. Document every unavoidable `unsafe` block with the invariant that makes it
   sound.
4. Add host tests for pure logic and a focused HIL stage for hardware behavior.
5. Delete the archived source only after the live implementation is qualified.

The old blob map/state/strict analyzers were removed after the library-analysis
phase. Their reports remain under `docs/`, and Git history preserves their
source.
