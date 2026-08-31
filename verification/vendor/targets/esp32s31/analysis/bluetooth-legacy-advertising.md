# ESP32-S31 legacy advertising admission boundary

This review defines the smallest hardware contract needed after the portable
`ADV_NONCONN_IND` encoder. It does not reproduce the vendor advertising
driver. The first open role is deliberately restricted to LE 1M, standalone
always-awake operation, one non-connectable PDU on primary channels 37, 38 and
39, no receive window, no scan response and no coexistence policy.

## Evidence and role map

Current behavior comes only from the linked ESP32-S31 controller built from
[`esp32s31-bt-lib@7f20740dd66ee774ffce5db0b55507892551aa31`](https://github.com/espressif/esp32s31-bt-lib/tree/7f20740dd66ee774ffce5db0b55507892551aa31).
The public named archive at
[`esp32s31-bt-lib@31c30949541a5d3abd4043a1cb66d55aa55577dd`](https://github.com/espressif/esp32s31-bt-lib/tree/31c30949541a5d3abd4043a1cb66d55aa55577dd)
is used only to recover roles. Stable prologues, object offsets, call order and
callee relationships establish these correspondences; old bodies do not
supply current behavior or layout.

| Current linked body | Reviewed role |
| --- | --- |
| `r_sym_ble_MrW1ZaJZqsHzi3wwRhsn` | build a legacy primary-channel PDU |
| `r_sym_ble_c9Zmr2aWmPOsZoITHDe9` | checked legacy-PDU builder wrapper |
| `r_sym_ble_fxKAT8in6cXLv0gLB2W5` | reset the advertising link-state graph |
| `r_sym_ble_fiiv8hEPVagnOAVa7EM2` | reset an advertising role |
| `r_sym_ble_cfILxLFIWftw22I0zQab` | allocate the private advertising graph |
| `r_sym_ble_8UzoZYkzYu9MXbM1vyWN` | start an advertising role |
| `r_sym_ble_mqh4OXzoN59kvnkKFMA1` | schedule its first primary event |
| `r_sym_ble_77zgK6v8rbStzf0ReBjv` | schedule its next primary event |
| `r_sym_ble_pfcv0QVYrNS6KoQetydw` | recycle a completed scheduler item |

The shared finished-list ingress and scheduler-list consumer are already
reviewed in `bluetooth-interrupt-runtime.md`. The focused Blobray scope
`ble-legacy-nonconnectable-admission` connects the exact packet, link-state and
first-event producers plus recycle to those two asynchronous roots. The wider
`ble-legacy-nonconnectable-advertising` scope retains broad role allocation,
reset, start and recurrence research.

## Existing open implementation

The portable Link Layer validates address kind, advertising data, non-empty
primary channel maps and interval bounds. It encodes an exact bounded
`ADV_NONCONN_IND` and retains generation, event and channel identity through
an affine lifecycle. The S31 pre-admission owner adds only the channel's
frequency projection and supports exact cancellation. It cannot publish a
scheduler head or claim that the PDU is in flight.

This is the correct stopping point. DTM packet state cannot be reused merely
because both roles eventually use the common scheduler. Advertising has a
different private producer graph and recurrence policy.

## Minimum production admission contract

One advertising transmission may enter production only when current-artifact
evidence closes the following connected edges:

1. **Packet producer:** one owned packet/header image contains the encoded PDU,
   declared length, advertising access address `0x8e89bed6`, CRC initialization
   `0x555555`, selected primary-channel frequency and matching whitening seed.
2. **Graph binding:** the link-state and scheduler item point to exactly that
   packet storage, and every hardware-followed pointer has a bounded typed
   encoding, alignment rule and lifetime.
3. **Publication:** a single affine operation orders all SRAM writes before
   publishing one common-scheduler list head and issuing `RUN`. CPU mutation is
   impossible while the owner is hardware-visible.
4. **Terminal observation:** the interrupt/finished-list path identifies the
   same item, fences hardware writes, unlinks it from both hardware and
   software lists and returns CPU ownership exactly once.
5. **Result and recurrence:** status has a finite success/failure disposition;
   successful completion advances the exact portable channel/event identity,
   while failure either returns a retryable pre-publication owner or a retained
   fail-stop owner. The first slice needs no RX or scan-response handling.

Unknown private fields do not block the first driver merely because they lack
vendor names. They do block admission when their value participates in packet
selection, timing, ownership, launch or completion. A reviewed whole producer
image with controlled inputs is sufficient; guessing individual bit names is
not required.

## Current blockers and non-blockers

The remaining blocker is SRAM causality across producer, graph publication and
completion. A hardware-frontier query currently asks for one project-local
assertion at `0x2010199c/16`, but the reviewed chip model already identifies it
as `BLE_HW_CTE_RING_CONTROL.RING_CONTROL`. It is reached through generic CTE
support and is not used by the restricted no-CTE transmission, so it is not an
admission blocker. Broad register discovery is therefore not the critical
path. Diagnostic logging, extended advertising, periodic advertising, scan
responses, RX buffers, sleep-enabled wake and coexistence are also not
blockers for the restricted first transmission.

The next review should therefore trace data backward from the first-event
scheduler item and forward from its recycle callback. It should not attempt to
make every reachable advertising function semantically complete.

```console
tools/blobray/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  project research next \
  --scope ble-legacy-nonconnectable-admission \
  --focus all --strategy frontier --limit 30

tools/blobray/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  inspect function ble-controller:r_sym_ble_mqh4OXzoN59kvnkKFMA1 \
  --full --details

tools/blobray/scripts/run-limited \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/targets/esp32s31/local.toml \
  inspect function ble-controller:r_sym_ble_pfcv0QVYrNS6KoQetydw \
  --full --details
```

The admission gate is closed when a Rust-owned static graph can implement all
five edges without exposing raw SRAM images above the private memory codec.
At that point work should return immediately to the controller/LL actor; full
vendor-scope completion is neither required nor desirable.
