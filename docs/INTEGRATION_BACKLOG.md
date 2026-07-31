# Integration backlog

Verified against `hil/esp32s31/runtime/src/radio_hil.rs` on 2026-07-31
(10,400 lines).

The HIL workspace intentionally owns board clocks and boot, PSRAM/flash
placement, the executor, concrete `embassy-net`/smoltcp scenarios,
credentials, traffic generation and reporting. Reusable radio behavior must
instead live in a driver, protocol or integration crate. This file lists only
the remaining ownership violations; completed transfer history is archived in
the [2026-07-31 integration report](archive/integration/2026-07-31-esp32s31-rust-integration-audit.md).

## 1. Wi-Fi MAC TX executor

The HIL still owns the production portion of `TxStorage` and the executor
functions beginning with:

- `transmit_encoded_frame`;
- `transmit_encoded_unicast_with_retry`;
- `transmit_protected_ethernet_frame`;
- the protected Ethernet A-MPDU/A-MSDU append and transmit functions;
- the policy-neutral part of connected protected Ethernet transmission.

Move finite TX/retry/EDCA transitions into the ESP32-S31 Wi-Fi MAC crate and
keep async completion/wakeup composition in the S31 Wi-Fi/Embassy integration
crate. Synthetic payloads, matrix selection, counters and throughput reporting
remain HIL policy. Reuse the existing `TxSlot`, `HtAmpduTxStorage`,
`AmpduRetryState`, `StaTxRuntimePolicy`, `UnicastRetryState` and pinned network
lease owners; do not copy the HIL functions verbatim.

## 2. Connected Wi-Fi STA dispatcher

The connected RX loop still combines Trigger, NDPA, AddBA/DELBA, CCMP,
BlockAck and `embassy-net` routing. Split protocol classification and typed STA
actions from the executor. The generic network adapter must continue to know
nothing about ESP32-S31 registers, while the interrupt adapter must not absorb
STA protocol policy.

Authentication, association, WPA2 phase/key/deadline ownership and TX/RX
BlockAck session state already have reusable owners. Their HIL functions are
hardware/timer executors, not a reason to recreate those state machines.

## 3. Shrink the HIL surface

After the two runtime slices above move, `radio_hil.rs` should contain only:

- board/resource selection and linker-profile hooks;
- task spawning, timers, interrupt entry points and logging;
- credentials, peer addresses and traffic configuration;
- raw-MAC, UDP/iperf, rate-matrix, DCM, Trigger and power scenarios;
- synthetic packets, qualification markers and diagnostic snapshots.

Stable diagnostic register meanings move to SVD/PAC with provenance. Raw
reads may remain only when they are explicitly comparison or HIL evidence and
cannot affect runtime transitions.

## Completion gate

For each extracted slice:

1. add host tests for the finite state and ownership rules;
2. change the HIL to consume the new API and delete its duplicate logic;
3. run workspace formatting, tests and lints;
4. repeat the HIL cells named by
   [the feature ledger](ESP32S31_WIFI_FEATURE_STATUS.md) when DMA lifetime,
   interrupt ordering, target placement or protocol behavior changes.
