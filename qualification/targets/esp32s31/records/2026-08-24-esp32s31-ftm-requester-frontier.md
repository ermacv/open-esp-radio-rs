# ESP32-S31 IEEE 802.11 FTM requester source frontier

Date: 2026-08-24

This is a source-frontier record, not an FTM qualification or ranging claim.
It adds no qualification-ledger result, HIL success, vendor binary, private
oracle output, captured frame or distance-accuracy statement.

## Reviewed protocol boundary

The allocation-free codec follows IEEE Std 802.11-2020:

- Table 9-51 assigns category 4 to Public Action frames.
- Table 9-364 assigns Public Action values 32 and 33 to Fine Timing
  Measurement Request and Fine Timing Measurement.
- 9.6.7.32 defines the request body as Category, Public Action, Trigger and
  optional information elements. Trigger values other than 0 and 1 are
  reserved.
- 9.6.7.33 defines the measurement prefix as Category, Public Action, Dialog
  Token, Follow Up Dialog Token, six-octet TOD, six-octet TOA and two error
  fields. TOD and TOA use picoseconds. A zero Follow Up Dialog Token reserves
  the timestamp and error fields.
- 9.4.2.167 assigns Element ID 206 and a nine-octet field to the Fine Timing
  Measurement Parameters element and defines its bit allocation.
- 11.21.6.4 defines the four-timestamp interval equation, but explicitly leaves
  derivation of antenna-point responder times from TOD/TOA and capture of the
  initiating STA timestamps implementation dependent.
- Figure 11-37 and 11.21.6.4 show that `FTMs Per Burst` counts successfully
  transmitted FTM frames. In the supported single-burst ASAP profile, the
  initial frame has no predecessor and the final frame supplies the preceding
  responder timestamps, so at most `FTMs Per Burst - 1` complete exchanges are
  delivered. An allocation of one is a terminal initial frame with no sample.

The portable requester deliberately supports only an associated, single-burst
ASAP profile. Multiple bursts need Partial TSF scheduling and later FTM Trigger
publication; neither is silently approximated from executor time. A request has
a finite frame count, fixed `frame count - 1` result capacity, at most eight
initial-request attempts, a response deadline and a session deadline. Initial
information elements used for exact retransmission identity are bounded to 64
octets; larger responses fail closed rather than being truncated or hashed.
The requester-issued transmission keeps its identity fields and encoded Action
body private. Completion and hardware rejection compare the complete pending
value, including every body octet; matching generations cannot authorize a
mutated body, and a rejected mismatch does not consume the valid pending value.

Dialog identity follows the measurement-pair rule rather than inventing a
request token: the responding STA chooses the first nonzero Dialog Token and
each later nonterminal token must be consecutive, wrapping from 255 to 1. The
following measurement identifies its predecessor with Follow Up Dialog Token.
Exact repeated deliveries are idempotent. An initial-FTM retransmission must
preserve the initial body and parameters while advancing the Dialog Token; it
atomically replaces the abandoned local timestamp obligation. A later FTM
retransmission may likewise advance the Dialog Token while repeating an already
completed follow-up sample; that sample is deduplicated while the new local
timestamp obligation remains independently owned. Foreign, stale, conflicting,
skipped and reused tokens cannot mutate samples before validation succeeds.

## Maintained implementation boundary

| Boundary | Status | Maintained behavior |
| --- | --- | --- |
| Action codec | LIVE source | Initial Request and Measurement fixed fields plus the FTM Parameters element encode/decode without allocation; malformed IE length, duplicate parameters, reserved trigger/status/bits and invalid 48-bit timestamps fail closed. |
| Requester state | LIVE source | One bounded single-burst ASAP session owns peer identity, affine session/TX generations bound to the exact private Action body, retry/deadline state, consecutive/retransmission token identity, negotiated FTM-frame count and at most `count - 1` fixed-capacity samples. |
| Timestamp result | PARTIAL | Four raw picosecond timestamps and error exponents are retained by value. Half-range wrap and responder discontinuity are checked. The exposed interval difference is explicitly uncalibrated and is not a distance. |
| Connected control | FAIL-CLOSED | Production connected control can evaluate a request. The temporary transmission is consumed by a typed S31 admission rejection before the shared TX owner, sequence state or DMA is borrowed; only non-publishable frontier evidence returns. |
| PHY enable leaf | PARTIAL | The complete reviewed `phy_set_ftm_en` one-bit leaf has readback and an affine exact-restore wrapper. An explicit probe enables, verifies and restores that field, then stops before timestamp capture. |
| Physical FTM | UNAVAILABLE | No request publication, FTM RX timestamp binding, ACK timestamp binding, calibrated RTT, distance, initiator capability advertisement or HIL claim exists. |

The static frontier keeps the initiator capability and distance result
intentionally disabled. The PHY bit is not treated as evidence that the MAC
produces FTM timestamps.

## Exact remaining hardware and evidence blockers

1. Bind the powered runtime radio owner to the reversible PHY FTM leaf without
   manufacturing a second PAC/PHY owner and define association/off-channel
   coexistence ordering.
2. Identify and review the ESP32-S31 receive timestamp captured at the start of
   the FTM preamble at the receive antenna connector (`t2`), including width,
   unit, wrap, latch/clear and frame-identity rules.
3. Identify and review the transmit timestamp for the start of the ACK preamble
   at the transmit antenna connector (`t3`), including how the automatic ACK is
   tied to the exact received Dialog Token.
4. Prove how wire TOD/TOA become responder antenna-point `t1'`/`t4'` values for
   this chip. Parsing six picosecond octets does not prove PHY group-delay,
   clock-domain or calibration corrections.
5. Recover the complete calibration contract: RX/TX antenna delays, channel and
   bandwidth dependence, temperature/revision dependence, clock-rate/PPM
   treatment, error-exponent policy and persistence across PHY reconfiguration.
6. Bind unprotected Public Action publication, ACK completion and retries to
   timestamp ownership without consuming a sequence number or retry budget
   before all FTM hardware admission checks pass.
7. Bind received Measurement bodies to trusted FCS/address/BSSID metadata and
   the exact local timestamp pair before delivering them to the portable
   requester.
8. Add controlled on-air evidence for at least one fixed-distance single-burst
   ASAP exchange, timestamp monotonicity/wrap, duplicate/retransmission, timeout
   and negative controls. Only after calibrated error bounds are established
   can capability advertisement or a distance API be considered.

Until every publication and timestamp owner above is present, the production
frontier consumes the portable request locally and reports
`RuntimePhyOwnerBinding`; it does not send an FTM frame.
