# ESP32-S31 automatic beacon-monitor source frontier

Date: 2026-08-24

This is a source/runtime frontier record, not a hardware qualification claim.
No qualification ledger state changes and no private oracle artifacts are
included.

## Reviewed facts

- `WIFI_MAC_BSSID_POLICY.BSSID_HIGH0` provides exact readback of the station
  BSSID high bytes, eleven-bit AID, address-check gate and interface role.
  The existing PAC snapshot combines it with the BSSID low word, station RX
  policy enable and `WIFI_MAC_STA_BEACON_FILTER.CONTROL`.
- Complete `hal_enable_sta_beacon_filter` and
  `hal_disable_sta_beacon_filter` leaves prove the ordered low-three-bit gate
  and MAC-interrupt bit-15 transactions. They do not decode the three
  individual gate meanings or prove that enabling them owns a complete
  connected-station beacon lifecycle.
- Complete `hal_pwr.o` leaves prove the raw sixteen-bit beacon-miss timeout,
  four-bit miss limit, limit wake gate and counter-clear pulses. The timeout
  unit and its conversion from an association's TU beacon interval remain
  unnamed.
- Complete WDEVPWR leaves prove full masked STATUS sampling and full-image
  CLEAR acknowledgement. Only the four generic TSF-timer cause identities
  are reviewed; no bit is identified as beacon miss.
- The production software monitor already authenticates beacon delivery by
  the associated BSSID and owns the monotonic miss deadline. It remains the
  authoritative disconnect mechanism.

## Runtime boundary

Each connected control owner now creates one non-clone automatic-monitor
admission epoch bound to its exact BSSID, typed infrastructure AID and
association-derived software policy. On its first scheduler service it reads
the live station RX-policy projection and fails closed in this order:

1. missing readback;
2. inactive or SoftAP interface-zero policy;
3. BSSID/AID mismatch;
4. an already-enabled hardware beacon-filter owner;
5. a miss limit wider than the reviewed four-bit field;
6. missing TU-to-raw beacon-miss-timeout conversion.

The furthest current production path reaches item six. It performs no modem,
beacon-filter, MAC-interrupt or WDEVPWR write, and it does not change RF, PHY,
baseband or clocks. Connected shutdown consumes the same affine admission
epoch and reports that no hardware restore was required. If a later change
crosses the MMIO boundary, shutdown must instead retain and consume an exact
PAC restore token.

## Minimum missing oracle

The first required oracle is a complete connected-STA parent transaction
which binds all of the following to one known Association Response and one
known beacon interval:

1. input beacon interval in TU and miss policy;
2. exact raw `RX_BEACON_TIME_LOW` value and its unit/conversion;
3. AID/BSSID state before the hardware filter is enabled;
4. ordered counter clear, filter gate and interrupt-enable operations;
5. the exact WDEVPWR bit raised after the configured number of missed
   beacons, plus its acknowledgement order;
6. the corresponding successful-beacon refresh/cancel behavior.

An idle register dump, an independently called raw setter, or a nonzero opaque
WDEVPWR status image cannot establish that binding. After source semantics are
complete, separate on-device evidence is still required before the software
monitor can be replaced or any automatic-wakeup capability can be claimed.
