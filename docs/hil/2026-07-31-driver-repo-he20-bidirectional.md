# Driver-repository HE20 bidirectional HIL

This cell proves that the ESP32-S31 HIL system no longer needs the neighboring
`esp32s31_rust` application to build, flash or qualify the normal open-radio
data path. Both firmware stages and the Rust host runner came from
`open-esp-radio-rs`. The final image resolved the published `esp-hal`
`esp32s31-async-platform` branch at commit `4d42ec36249a4f10a43cc5fb63f54eb24252feea`;
the local sibling checkout was explicitly disabled for the build and flash.

## Exact cell

- Board: ESP32-S31 revision 0, MAC `30:ed:a0:f3:f6:d0`.
- Peer: FRITZ!Box 7530, BSSID `dc:15:c8:54:bc:1e`.
- Memory profile: `psram-code-psram-data`.
- PHY: HE20 SU, NSS1, MCS9, LDPC, 2xLTF/0.8-us GI, nominal 114.7 Mbit/s.
- MAC: 32-member A-MPDU with BlockAck and concurrent RX servicing.
- Downlink offer: 10 Mbit/s, 1,200-byte UDP datagrams, 12 seconds.
- Uplink source: the existing HIL synthetic Ethernet/A-MPDU producer.

The image was built and flashed with:

```text
OPEN_RADIO_STA_SSID=<ssid> OPEN_RADIO_STA_PASSWORD=<password> \
  cargo hil flash bidirectional --port /dev/ttyACM0
```

The traffic and strict qualification were run without shell helpers or the
old application `xtask`:

```text
cargo hil traffic bidirectional 192.168.178.141 \
  --phy he20 --rate 10M --seconds 12 --serial /dev/ttyACM0
```

## Result

- Host offer: 10.001 Mbit/s, 15,001,200 bytes / 12,501 datagrams.
- Device direct-RX median: 10.011 Mbit/s.
- Concurrent open-radio TX floor: 63.660 Mbit/s.
- Conservative sum: 73.671 Mbit/s.
- RX baseband format remained HE (`format=4`).
- TX vector remained HE20 MCS9 at 114.7 Mbit/s.
- Both captured RX runtime intervals reported `buffer_full=0` and
  `fifo_overflow=0`.
- Runtime code markers were in PSRAM; build-time placement audit separately
  required ISR, DMA and stack ranges to remain in internal SRAM.
- The strict host result was `OPENRADIOHOST result=PASS`.

## Artifact identity

- UART qualification log SHA-256:
  `32d55733bc6a467c22f94135d49884a5edd71a54cf5a6556c81ee7e884ee4831`.
- ESP application image SHA-256:
  `0e894bf1030d399fd953ee7cb74ba6d65504c7515091e81b6ae8a22d63c49434`.
- Packed stage-two runtime SHA-256:
  `db11b111f3ad99ffbc05080d06420436c4aa21d6b68aa703a9751721ce5900b1`.
- Qualification report SHA-256:
  `01aba5c945150a7ef06d60cc1be9844d61eb826a5877f0b8eabdb146bdbc41a2`.

The bulky UART log and binaries remain generated artifacts under
`target/hil/esp32s31`; this record preserves their hashes and the exact cell.

## Ownership boundary

Reusable rate, aggregate, retry, pinned-frame and Embassy network ownership is
already in the driver crates named in the feature ledger. The final run also
used the driver-owned `select_sta_association` channel/CBW decision and
`StaAssociationRuntime`. It now also uses `StaPeerScanPolicy` and
`StaPeerAssociationPlan` as the sole post-response join for HT A-MPDU, WMM,
HE BSS color/capabilities, peer QoS, link metric and rate-control state; the
former HIL copies were deleted before this qualification. `StaTxRuntimePolicy`
now owns that negotiated TX state and all four EDCA contention windows, while
`UnicastRetryState` owns the bounded attempt count, exact legacy/HT rate
selection and success/failure CW transitions. The HIL retains only platform
entropy, DMA/IRQ waiting, PHY power application and Retry-bit publication.
Board/bootstrap setup, credentials, synthetic traffic generation and evidence
reporting also remain HIL concerns. `Wpa2StaSupplicant` now owns PTK renewal,
M3 MIC verification, async key-data unwrap, strict GTK parsing and the exact
pairwise/group `Wpa2StaKeyInstallRequest`. The HIL only executes that request
against the two S31 hardware slots and reports one completion before receiving
the authenticated M4 frame. `Wpa2StaResponseDeadline` now owns both finite
response windows. The HIL's spontaneous second M2 was deleted: complete vendor
`wpa.c.obj` sends M2 only from `wpa_supplicant_process_1_of_4`, while a repeated
M1 re-enters through `wpa_sm_rx_eapol` and the driver state emits the response.
The connected path now also uses one `StaTxBlockAckSessions` owner for the
vendor TID order 0/7/5, shared Dialog Token sequence, independent alarms and
ADDBA/DELBA routing. RX agreement ownership and the remaining connected
event/route dispatcher still need extraction before the application HIL can
become only a platform executor.

The same image now calls the PHY crate's `run_phy_register` through the
driver-owned `TargetPhyRegisterPort`. The complete nested RF/baseband/channel
completion graph, finite polling bounds and MAC stop/retune/restart contract
therefore have one owner in `open-esp-radio-phy-esp32s31`; the 1,206-line HIL
copy was deleted. Operation ordinals, ROM TX-gain comparison and raw MMIO
snapshots are isolated behind `PhyTargetObserver` and cannot change a PHY
completion. The HIL now supplies only a zero-sized Embassy delay adapter and a
diagnostic observer. The encoded application remains 998,912 bytes, the
placement audit passed, and the reset-separated strict run produced the result
above with zero `BUFFER_FULL` and zero `FIFO_OVERFLOW`.

`StaAuthenticationRuntime` now owns the complete three-attempt Open
Authentication epoch: one management sequence number per attempt, the
vendor-proven 1,000-ms response deadline, peer response/deauthentication
classification, retry decisions and terminal failure identity. The HIL keeps
only RX-ring recycling, frame extraction, TX submission, the Embassy timer and
diagnostic reporting. Three host tests cover timeout exhaustion, sequence
wrap, peer success, deauthentication retry and status rejection. The strict
connected run above proves the resulting Authentication, Association, WPA2 and
traffic path on hardware.

`StaAssociationRuntime` now owns the complete ordinary Association epoch too:
the 1,000-ms vendor state deadline, finite 160-ms transmission schedule, one
non-QoS sequence number per newly encoded request, complete RX descriptor
count and selected-peer Association/Deauthentication classification. The HIL
only opens and closes millisecond ticks, submits a scheduled MPDU, extracts
management frames and executes the returned terminal result. Three host tests
cover the exact seven-attempt schedule and twelve-bit sequence wrap, the
1,000-tick timeout, selected-peer success, peer filtering, rejection and
disconnect. A cold boot of this exact image received the successful
Association response on attempt one, completed WPA2 and obtained
`192.168.178.141/24` by DHCP before the strict run above passed.

The same cold boot also qualifies the canonical WPA2 transmit-frame owner.
`Wpa2StaState` produces the M2/M4 transmit action;
`build_sta_action_frame` binds that action to the selected peer, supplicant
nonce, replay counter and exact Association RSN/RSNXE image; and
`Wpa2TxFrame::authenticate` supplies the HMAC-SHA1 MIC from the owned PTK.
The former parallel `Message2` and `Message4` byte builders were deleted, as
was the duplicate `key_data` GTK parser/type. The retained `Wpa2Gtk` zeroizes
on drop and is now the same type accepted by `Wpa2KeyInstall`. Cold UART proved
M1, M2, M3 MIC/decryption, pairwise and GTK installation, M4, protected ARP,
DHCP and external-probe readiness before the strict traffic result above.

The subsequent supplicant-runtime transfer removed the remaining PMK/PTK,
M3 MIC, AES key-unwrap, GTK parser and state-ticket dispatch from the HIL.
`Wpa2StaSupplicant::on_frame` now resolves all hardware-independent actions
and returns one `Wpa2StaKeyInstallRequest` containing zeroizing pairwise/group
keys plus the Message-3 receive sequence. The S31 executor borrows those keys
only for the finite MAC slot writes, returns the private ticket through
`complete_key_install`, and receives an authenticated Message 4. The exact
image then booted, associated, completed WPA2/DHCP and passed the strict run
recorded above.

The final timing transfer replaced both HIL millisecond loop bounds with
`Wpa2StaResponseDeadline`. The Message-3 window retains the same total six
seconds as the former two three-second receive attempts, but no longer emits a
timer-originated M2 that the vendor supplicant never produces. A reset-separated
cold trace showed one M1, one successful M2, immediate M3, key installation,
M4, protected ARP and DHCP before the strict result above.

The connected BlockAck transfer replaced three HIL-owned sessions, three
alarm slots and manual response-token selection with one fixed driver owner.
The reset-separated trace proved ADDBA requests with Dialog Tokens 1, 2 and 3
for TIDs 0, 7 and 5, followed by three operational 32-entry agreements. DHCP,
the external ARP probe and the strict 10.011-Mbit/s RX plus 63.660-Mbit/s
concurrent-TX run then passed with no DMA-starvation failure.

The registration tail's read-only PHY-I2C loop is no longer part of that
application port. `complete_final_i2c` owns its fixed 10,000-edge bound,
read-only invariant and deadline-as-transition-completion semantics in the PHY
target executor; HIL supplies only the Embassy delay and platform I2C owner.
