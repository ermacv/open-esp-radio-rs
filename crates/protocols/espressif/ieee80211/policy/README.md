# Espressif IEEE 802.11 transmit policy

`oer-espressif-ieee80211-policy` is the Espressif family's policy data for
the portable transmit algorithms above the
[lower-MAC port](../../../ieee80211/lower-mac/README.md). The algorithms are
written once in [`oer-ieee80211-upper-mac`](../../../ieee80211/upper-mac/README.md)
and `oer-ieee80211-mac`; they take limits, rate ladders and estimates as
parameters, and this package supplies the values the vendor stack uses. It
is a `family` package (`family = "espressif"`): chips of the family may
depend on it, and portable code never does.

| Module | Contents | Source |
| --- | --- | --- |
| `rate_schedule` | The nine rate-schedule arenas in their post-`rcAttach` state, the `rcGetRate` record walk, the `rcReachRetryLimit` publication budget and the `rcUpdatePhyMode` rate-to-record maps | `libpp.a[trc.o]` |
| `rate_code` | `wifi_phy_rate_t` codes of the records as portable `PhyRate`s, by schedule | `esp_wifi_types_generic.h` |
| `retry_ladder` | The ordinary-MPDU retry ladder of legacy, HT and HE rates (`EspressifRetryLadder`, a portable `RateLadder`) | `libpp.a[trc.o]::{rcGetRate, rcUpdatePhyMode}` |
| `lmac` | Short and long retry limits (32), ACK-timeout accounting, the RTS threshold (2346 octets), the A-MPDU MSDU lifetime and aging margin, the A-MPDU retry policy and the default contention per access category | `libpp.a[lmac.o]::{lmacInit, lmacInitAc, lmacProcessAckTimeout, lmacMSDUAged}`, `libpp.a[pp.o]::ppResortTxAMPDU` |
| `he_txop` | The HE TXOP duration RTS byte budget (`EspressifHeTxopRtsBudget`, a portable `HeTxopRtsBudget`) and the peer packet-padding code | `libpp.a[if_hwctrl.o]::ic_set_he_rts_threshold_bytes_tab` |
| `ccmp` | The CCMP transmit packet-number step of three | `libnet80211.a[ieee80211_crypto_ccmp.o]::ccmp_encap` |
| `connection_coex` | The coexistence events (45, 46) and priorities a reconnecting station's Probe Request, Authentication, Association and EAPOL frames request, and the `ConnectionFrameCoex` port a chip implements | `libpp.a[pp.o]::pp_coex_tx_request`, `libpp.a[pm_coex.o]::pm_coex_reconnect_policy` |

The values were recovered from the ESP32-S31 libraries pinned in
`verification/esp32s31/artifacts.toml`; each item's `SOURCE` comment names
the vendor function it was read from. A chip of the family whose pinned
libraries differ needs its own reviewed values before it uses them.

The schedule walk and the retry ladder live here, not in the portable
package, because they are inseparable from the vendor schedule bytes. The
vendor's adaptive rate control (`trc`), which chooses a frame's first rate,
stays in the ESP32-S31 MAC (`oer-esp32s31-ieee80211-mac`, `rate/control.rs`)
for now: its interface is typed in chip rate and completion values that the
station and access-point runtimes consume.
