# ESP32-S31 HT Duplicate MCS32 source frontier

Date: 2026-08-24

This is a source-frontier record, not a hardware qualification claim. No
qualification-ledger result changes, vendor binary, private oracle output or
disassembly dump are attached. It records only the boundary established by
the reviewed source, SVD/register descriptions and dated open HIL records
already present in this repository.

## Reviewed facts

- `WIFI_MAC_TX_QUEUE_VECTOR.HT_SIGNAL%s` documents the complete ordinary
  `mac_tx_set_htsig` formatter only for non-HE raw rates 16 through 35. Its
  concrete captures cover ordinary HT MCS0 and MCS7 in the relevant width/GI
  combinations; none identifies MCS32.
- The production ordinary `HtRate` domain is one-spatial-stream MCS0 through
  MCS7. Its typed descriptor code, HT-SIG, DATA_LENGTH, protection rate,
  calibrated-power lookup and retry methods are therefore evidence-bounded
  ordinary-format operations. The distinct `HtDuplicateRate` has none of
  those raw-code escape hatches.
- `WIFI_MAC_TX_POWER_INIT` establishes that `hal_init_tx_pwr` consumes 43
  two-byte PHY results. Cardinality alone does not identify an MCS32 lookup
  index or queue power image, so no extra table entry is assigned by analogy.
- The reviewed TX-retry profiles exercise ordinary schedules and ordinary
  timeout/error transitions. They do not prove that a retry keeps duplicate
  mode selected.
- The public S31 RX prefix owns a five-bit summary rate and a separate
  format-specific signal word. For an HT PPDU the latter exposes the seven-bit
  HT-SIG MCS field, CBW, aggregation and short-GI bits. Source normalization can
  therefore distinguish HT40/MCS32 from ordinary HT and retain a non-HT40
  MCS32 mismatch. There is no dated controlled MCS32 RX cell proving that the
  S31 actually decodes and delivers such a PPDU.

## Maintained boundary

| Boundary | Status | Maintained behavior |
| --- | --- | --- |
| Peer capability | LIVE | Complete peer HT Capabilities parsing retains the independent RX MCS32 bit and admits it only with peer HT40 support; scan state reaches the connected STA owner. |
| Local advertisement | LIVE, fail-closed | STA Association Request, AP Beacon and AP Association Response builders keep the local RX MCS32 bit clear and advertise only the implemented equal MCS0..MCS7 sets. |
| RX normalization | PARTIAL | HT-SIG MCS32 plus CBW40 is typed separately; invalid width is an explicit diagnostic. Actual on-air S31 MCS32 reception is unqualified and is not advertised. |
| STA/AP request ownership | PARTIAL | An explicit fixed request reaches the common selection frontier without replacing ordinary data or aggregate fallback rates. |
| Physical TX | UNAVAILABLE | No MCS32 plan is constructible. Selection reports the exact source gaps and the independent qualification gap before descriptor or queue publication. |

The independent request result contains an `HtDuplicateTxPlan`, not a
`TxPhyRate`. Consequently even a future admitted duplicate plan cannot be
silently routed through the ordinary legacy/HT/HE formatter.

## Minimum missing TX formatter oracle

One controlled, synchronous vendor MCS32 submission for a known PSDU length,
HT40 channel and fixed long GI must retain all six independently consumable
facts below. A separate short-GI submission is required before short GI can be
admitted.

1. The complete descriptor selector: format/rate byte and every flag that
   selects duplicate mode.
2. The resulting PLCP0, PLCP1 and packed HT-SIG queue words.
3. DATA_LENGTH, LENGTH_CONTROL and the calculation/enforcement of the request's
   maximum PPDU duration.
4. The complete protection choice and queue image, including RTS/basic rate.
5. The calibrated target-power lookup input and the resulting primary and
   alternate data/protection power fields.
6. A second snapshot after an induced failed attempt showing the retry
   transition and proving that every retry retains duplicate mode.

The capture must bind the descriptor inputs to the synchronous queue image;
an idle register dump or a 43-entry power-table observation cannot substitute
for that binding. These are construction fields. They are deliberately kept
separate from the final qualification gate: after a source-reconstructed image
exists, the production path still requires a controlled peer to decode the
MCS32 PPDU and return the expected ACK or BlockAck for that exact image.

## Minimum missing RX evidence

A controlled peer must transmit a known HT40 MCS32 MPDU to the S31. The open
RX owner must deliver the exact payload with valid FCS and publish coherent HT
format, seven-bit MCS32 and CBW40 metadata. A second malformed or non-HT40
vector, if the peer can generate one, may validate rejection diagnostics but
cannot replace the successful receive cell. Synthetic prefix bytes prove only
normalization and do not qualify reception.

Until those TX formatter and on-air cells exist, TX stays fail-closed and the
local MCS32 capability bit stays clear.
