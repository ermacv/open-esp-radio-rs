# ESP32-S31 HIL target

- [System and PHY watchdogs](watchdogs.md)
- [RX delivery observations](rx-delivery.md)
- [Copy benchmarks and stack measurements](memory.md)
- [Host setup and fixture installation](../../host/README.md)
- [Wire and evidence contracts](../../protocol/README.md)

This workspace selects the shared board boot and memory profile and owns Embassy
executors, the owned Xarxa/Embassy network stack, UART transport and HIL
workloads. Effective dependency locks are archived beside each image. The
[network implementation guide](../../../docs/network-implementations.md)
explains the stack's crates and source policy.

The Bluetooth images reach the radio only through HCI.
`runtime/src/bluetooth.rs` starts the shared radio with
`oer_esp32s31_radio_system::start`, which spawns the coexistence schedule,
starts the production Bluetooth client on it, runs the radio runner, the HCI
Controller service and its own periodic PHY tracking task, which reports a
tracking failure as `reason=phy-tracking`, and serves the typed console. The `bluetooth-dtm` image (`bluetooth-hil`) serves the
`bluetooth-dtm-*` scenarios: `bluetooth/dtm.rs` resets the Controller and turns
each DTM operation into LE Transmitter Test v1 (channel 0, 37-byte PRBS9), LE
Receiver Test v1 (channel 0), LE Test End or HCI Reset, reporting the Test End
packet count. The `bluetooth-gatt` image serves `bluetooth-trouble-gatt`:
`bluetooth/gatt.rs` runs the Trouble Host and the plaintext GATT application
over the Host end of the transport. Both images use the development Controller
identity Core 5.4, company `0xffff`, subversion 1, and a 500-ppm sleep-clock
bound. The `bluetooth-secure-gatt` image serves the `bluetooth-trouble-secure-gatt*`
scenarios: `bluetooth/secure.rs` keeps one RAM bond store across Host epochs.
Each epoch runs the Trouble Host and the secure application beside the radio
runner and the HCI service. A requested restart resets the Host through HCI,
retires the drained Host end of the transport, stops the Controller on the
radio system, checks that the old Host end reports the transport closed and
starts the next Controller epoch with a fresh Controller core. An application
failure closes the Controller without restarting; an unconfirmed Reset keeps
every owner.

The `wifi-ble-coex` feature adds a Bluetooth LE client to the Wi-Fi image on
the same shared radio. After the Wi-Fi image starts the radio system with
`oer_esp32s31_radio_system::start`, which spawns the coexistence schedule,
and spawns its own PHY tracking task,
`bluetooth/shared.rs` starts the Controller on the radio's Bluetooth
partition and runs the radio runner, the HCI service, the Trouble Host and the
plaintext GATT application on their own tasks; it adds no radio-system task.
The Bluetooth entropy service owns the SoC random source that the Wi-Fi client
also reads. The Wi-Fi console answers `bluetooth::GetGatt` with the
application's observations and advertises `bluetooth_gatt`. The host drives it
with the ATT echo load in `oer-hil-family-bluetooth`'s
`workload::bluetooth::coexistence`: one connection writes and reads back the
application value for the measured interval, then disconnects gracefully.

`runtime/src/product_hil/network` owns stack setup, IPv4 configuration, socket
API bindings and diagnostic wrappers. All implementations use the same traffic
workers and public production radio constructor. Radio behaviour belongs in
`crates/`.

`cargo hil run diagnostic-tx-protection-air` runs a twelve-second HT40
bidirectional UDP workload at 4 Mbit/s per direction, with AP and independent
air captures. It uses the ordinary production protection policy; the scenario
does not force RTS or CTS. The delivery verdict is not a protection verdict.
Inspect `independent-openwrt-air.pcap` when the configured dedicated observer
is available, and `independent-air.pcapng` for the laptop monitor. A CTS carries
only a receiver address: attribution to a CTS-to-self exchange also requires
its relationship to the following target transmission. Missing frames or PHY
timestamps limit what the capture can establish about Duration and SIFS.

A standalone HT AP scenario selects a runtime scheduling policy with its
`scheduler` field, `rr-ht-response24` or `deficit-ht-response24`; a comparison
runs two scenario files that differ only in it, and the second can replay the
first's firmware with `--firmware-from`. Initialization carries the selected
policy and the scenario snapshot retains it. Omitting the field preserves the
ordinary unmetered RR default. The shared experiment uses a 3000-us quantum,
100-us minimum and HT aggregate admission against the issued grant. Terminal
cost is modelled PPDU time plus 10-us SIFS and a 32-byte OFDM24 response per
unicast publication; group publications add no response. The same response
envelope deliberately applies to ordinary ACK and aggregate BlockAck exchanges.
It is not measured response PHY, airtime, CCA waiting, protection or hidden MAC
retry time. Unknown PPDU work or saturated receipts fail closed.

This comparison requires `owned-xarxa` and a standalone HT20/HT40 AP workload.
Other network images reject the policy during initialization; simultaneous
STA+AP is rejected while it is selected. The fixture configuration owns the
channel; set its AP channel to 13 with HT40-below for the channel-13 comparison.
Changing policy requires a fresh initialization, not a firmware rebuild. The
driver owns candidate selection, grants and cancellation; HIL supplies only
the explicit comparison model and workload.

`diagnostic-ap-balanced-airtime` offers 130 Mbit/s to each of two AP clients for
one 12-second window. It accepts the same `scheduler` comparison.
This workload does not force different peer PHY rates: AP downlink selection
uses each station's advertised receive capabilities. An OpenWrt transmit MCS
mask controls the opposite direction and is not evidence of downlink asymmetry.
Multi-flow UDP uses one socket per flow slot, with independent pacing and
sequence counters. Source ports are 4324 and 4325; UDP 4325 is distinct from the
TCP service on that port. Both sockets are bound before readiness, and each host
receiver confirms its own reverse path before Start. The OpenWrt fixture forwards
both UDP ports. A pending publication skips only its own flow for the current
poll. The producer parks on socket events or the earliest future pacing/session
deadline; it yields after a bounded quantum of successful publications.
Terminal send errors stop that flow and remain in its transport evidence.

Per-socket storage is separate, while stack packet pools and driver admission
remain shared resources of the selected integration. Their pressure can still
limit both flows. Offered rates alone do not prove continuous driver backlog
or airtime fairness between unequal peers.

With a scheduling comparison enabled, `wifi::AirtimePeer` records and a final
`wifi::AirtimeReport` precede the correlated AP stop event. They count the whole AP
epoch, including its ordinary traffic and teardown, rather than only the UDP
window. The target accumulates at most eight distinct association/group keys;
it retains old generations and reports dropped events or saturation explicitly.
No per-packet diagnostic text is emitted. The runner requires complete records
and reconciliation of reservation counts and budgets, with no outstanding work
at successful stop. A negative balance is permitted: it is modelled service
debt, not leaked packet ownership. The raw typed records remain in `protocol.jsonl`.

The owned Embassy/Xarxa composition enables
`auto-icmp-echo-reply` explicitly for the HIL ping workload because these
dependencies disable default features. Echo response is independent of DHCP,
UDP/TCP sockets and the radio adapter.

- `runtime`: role-neutral control plane and runtime-dispatched workloads;
- `telemetry`: HIL-only diagnostic observers;

[Shared platform](../../../platform/esp32s31/README.md) owns the board profile,
bootstrap, linker scripts and stage-two entry used by HIL and examples.

Diagnostic AP evidence retains `first_rx_protocol_rejection` for each AP epoch.
It identifies the exact data/replay/fragmentation failure or internal
reorder/output-capacity refusal, with radio monotonic time and available
transmitter, frame-control, sequence-control, TID and CCMP PN/key-ID metadata.
Retained reorder frames report their own metadata at the time of rejection.
No payload or key material is stored; subsequent failures do not overwrite the
first record. The aggregate counter retains its existing scope. The typed stop event
carries the record independently of diagnostic text delivery. A null record
means no recorded reason, not proof that an aggregate rejection count is zero.

Diagnostic AP stop evidence also carries `tx_retention`: counts of admitted
software owners dropped by active, unicast power-save or group power-save
storage exhaustion. These counters span the AP epoch and survive traffic
window resets. They are separate from on-air delivery failures. The AP gate
reports these specific losses before evaluating aggregate delivery evidence.
Without a driver observer this field is absent (`null`), rather than zero.

Images with `driver-observation` include `OAMPW`: terminal aggregate exchanges,
actual software publications, submitted PSDU bytes and MPDUs including selective
retries, nominal data serialization time, unestimated publications and saturated
receipts. It uses the same frozen observation window as other aggregate text.
An exchange contributes at its terminal completion/abort edge, so an in-flight
exchange crossing the window boundary is not split between windows. Ordinary
MPDU TX/fallback is excluded. These fields are diagnostic text, not qualification
criteria or a typed traffic-result extension. `nominal_data_us` is not measured
airtime; its scope is defined in the [egress contract](../../../docs/wifi-egress.md).
Delivery and exchange wall time remain separate counters.
`OTXW kind=ampdu|ordinary` reports separate terminal work banks: publications,
PSDU bytes, MPDUs, modelled PPDU microseconds, unknown PPDU publications and
saturated receipts. Ordinary includes connected-STA service and AP network
MPDUs, including fallback and buffered/group release; it is not a census of
all management/control or autonomous hardware responses. Both banks use the
same frozen observation interval. `ppdu_us` is incomplete when `unknown_ppdus`
is nonzero and never claims measured airtime. ACK/BlockAck, protection and
contention belong to the explicit [cost model](../../../docs/wifi-egress.md),
not implicit multipliers in these counters.
`OTXC kind=ampdu|ordinary` uses the same terminal window to report summed
programmed AIFSN, selected backoff slots and publications without contention
evidence. The counts exclude SIFS and do not measure waiting, CCA freezes or
internal hardware retries. They inherit the corresponding `OTXW` saturation
flag. A zero backoff with zero unknown publications is a known selection.
The `diagnostic-tx-architecture` image omits this observer; use a scenario with
`diagnostic-task-poll` to collect the aggregate work receipt.

`OTXR scope=station` reports ordinary-MPDU terminal outcomes in the same frozen
window: observed/missing reports, successes, successes after CTS-timeout retries,
CTS/ACK/collision re-publications, terminal CTS failures and hardware timeouts.
It includes connected-STA network traffic and aggregate fallback, excluding AP,
management and autonomous hardware responses. Retry counters omit terminal
failures that were not re-published and do not count hidden hardware retries.
These diagnostic counters wrap at u32 and do not qualify an on-air protection
exchange; pair them with the exact source snapshot and independent capture.

`performance` contains no driver observer or scheduler instrumentation.
`diagnostic-station-exit` adds only the Wi-Fi system's diagnostics to it:
the connected station's exit evidence (beacon observation, RX DMA state and
control state at disconnect) without the executor timer and PHY registration
observers of `correctness`, so saturated traffic keeps close to production
timing. Only diagnostic-tagged scenarios may select it, and it admits the air
observers of the correctness image.
RF calibration details are retained only by the PHY `registration-diagnostics`
feature, selected by HIL `driver-observation`. Ordinary role owners keep the
compact registration result instead of carrying RF measurements through every
role transition. Diagnostic images log the retained PLL results after startup.
`correctness` adds value-only driver observations, while the `diagnostic-*`
images add one explicitly named hot-path probe. `diagnostic-mac-irq` and `diagnostic-tx-wait`
extend the SRAM-closed ISR graph to sample publication-to-IRQ and
IRQ-to-bottom-half latency. UDP/TCP RX/TX/bidirectional
workers remain runtime-selected; STA/AP roles and workload direction do not
change image identity. The task-poll diagnostic accepts both data-plane
placements and retains the ordinary 16-KiB CPU1 owner stack in either case.

The task-poll overlay also counts per-interface stack/driver transfers and
polls without a transfer. For single-flow UDP TX, post-workload `hil-net`
records separate send-future poll residence, suspension and explicit workload
pacing. Cancellation at the session deadline retains the last blocked send.
These extra measurements affect diagnostic throughput. A poll without a
transfer can still do internal protocol work; suspension includes executor
scheduling latency and is not CPU time. The measurements do not timestamp
individual packets inside the radio queue. The MAC IRQ image emits `hil-irq`
entry classification for UDP TX, including interrupts with no pending status,
alongside its existing sampled publication/IRQ/service timings. The interval
ends with the traffic window; summaries are printed after traffic.

MAC IRQ diagnostic TX workloads also emit `hil-tx-ingress` RX progress from
before traffic through the final diagnostic snapshot. These records separate hardware
completion, protocol processing, reorder release and network publication;
`pool_exhausted` counts the subset of publication drops caused by allocation
failure. Incoming ARP replies are required for UDP egress to an unresolved
peer, so RX diagnostics remain relevant in a TX-only workload. The host saves
`delivery-progress.json` before burst qualification, including for zero or
partial delivery. Socket acceptance is separate from host reception, and
unavailable diagnostic counters remain unknown rather than zero.

UDP RX uses a continuous observation window for both single- and multi-flow
sessions. The first data packet starts the configured duration, with at most
two seconds of startup allowance; without a packet, the window starts when
that allowance expires. Neither socket errors, 750-ms silence nor an early
terminal marker ends the window. A separate 750-ms terminal grace excludes
late payload from throughput. Every flow uses the full duration as denominator,
including zero-delivery flows.

The reliable `ORX_WINDOW` record reports startup delay, socket errors, late
datagrams and delayed deadline service. `ORX_SILENCE` reports silence gaps,
including the offset of the longest gap and the trailing gap; `ORX_SOCKET`
identifies the last receive error when errors occurred.
Silence means no data reached the UDP consumer; it does not identify an RF,
stack or driver cause. `ORX_POOL` reports shared Xarxa allocation refusals;
`ORX_RESOURCES` reports released-Embassy RX/TX free and queued slots plus cumulative
`rx_queue_full` publication refusals. Both records name the selected STA/AP
interface; unavailable monitors remain absent rather than reporting zero. These observations can be
read even when the radio executor is blocked. Slot snapshots include neither
held tokens nor a claim of atomic observation across all queues. RX task-poll
observation closes at the window boundary, before terminal grace and reporting.

UDP TX task-poll intervals close at the end of the measured workload, before
report output. Aggregate evidence closes at the final diagnostic snapshot;
text and structured reports share the same frozen aggregate snapshot. Waiting
for diagnostic output capacity therefore cannot extend these intervals.

`udp-tx-ht40-mac-wait-diagnostic` uses `diagnostic-tx-wait` to investigate slow
transmissions, including busy-channel conditions. It has no idle-channel
admission limit or throughput floor; a pass means the diagnostic workload
completed, not that the link met a performance target. The separate
`udp-tx-ht40-mac-irq-diagnostic` retains its idle-channel limit and throughput
floor. Publication-to-IRQ samples include hardware waiting and interrupt
entry latency; IRQ-to-service samples measure the subsequent executor handoff.
The wait image combines MAC IRQ and task-poll observations with an explicitly
intrusive `tx-wait-probe` feature. While an aggregate remains published, the
station owner adds observation deadlines at 5, 10, 20 and 40 ms after each
publication. These deadlines preserve the ordinary completion/abort policy;
late wakes skip missed sample points. Each observation reads the typed queue
snapshot before consuming a completion and records both publication age and
observation-deadline lateness. A pending completion can therefore be identified
before normal service acknowledges it.

`hil-tx-wait` records retain the first eight observations in each observed-age
band: below 10 ms, 10–20 ms, 20–40 ms and at least 40 ms (upper bounds excluded).
Independent budgets prevent common short waits from hiding rare long waits.
Additional samples in a full band are counted as dropped. Output is grouped
by age band; timestamps recover chronological order across bands.
The producer uses an atomic append-only buffer;
text is emitted after UDP TX. Queue CCA fields are control settings, not a live
channel-busy measurement. Timer lateness includes executor and interrupt delays
and does not by itself measure how long interrupts were disabled. Ordinary
MAC IRQ, correctness and performance images do not enable this probe.
The shared `mac_active` field retains the reviewed MAC activity encoding;
it does not identify CCA/NAV or timestamp the beginning of a transmission.

`diagnostic-rx-clock` (scenario `diagnostic-rx-clock`, station ICMP) carries
the intrusive `rx-clock-probe` feature and reports the `telemetry/rx-clock`
property. At the first executor handoff of every 32nd received frame the
runtime logs `rx_clock_frame` with the frame's receive timestamp from the
RX-control prefix (a Wi-Fi MAC local-time reading) and the monotonic handoff
time; every 100 ms the image logs `rx_clock_pair` with a MAC local-time reading
between two monotonic readings. The pairs relate the two clocks, so the
counter's unit and drift and each frame's age at handoff follow from the
console log. Product images do not enable the probe.
Queue and activity fields are sequential reads, not an atomic hardware capture.
Hang/panic fields are cumulative hardware counters from the reviewed RX
statistics decoder, not live receiver-state or channel-busy measurements.
RX MPDU, signal, end, FCS-error and abort counters record receiver progress;
their deltas use wrapping 16-bit subtraction. They are global to the MAC,
not attributed to the queued aggregate or exclusively to the connected BSS.

Large socket buffers, task arenas and ordinary task stacks live in PSRAM.
DMA-visible storage, dedicated trap/interrupt stacks, critical data and ISR
text remain in internal SRAM. Every build audits
placement and bounds each stack from the function that runs on it: CPU0's
`runtime_main`, CPU1's `runtime_cpu1_psram_main` and the bootstrap's
`_start_rust`, each within its storage less its reserve (16 KiB on CPU0, 4 KiB
on CPU1), plus the interrupt stacks. A task stack's bound may be partial,
with its unresolved sites in `runtime-stack.txt`. The separate compiler move
limit is 4 KiB. `stack.toml` extends the platform's policy with CPU1's stack;
runtime evidence independently enforces the same absolute per-core headroom.
Each core arms a hardware
write watchpoint on the bottom word of its task stack when no debugger owns
the watchpoint. Fatal CPU exceptions report the hart, faulting instruction,
fault address and saved return address through the ROM console. Watchpoints
and stack painting complement the static bounds, which do not follow every
indirect call.

`data_plane` is selected by the startup command, not by rebuilding. Every
repository scenario selects the production `split-radio-network` topology: it
retains radio and RX protocol on CPU0 and moves only `embassy-net` plus sockets
to CPU1. The protocol still names the CPU0-local composition so a deliberately
constructed external diagnostic can isolate placement, but it is not a catalog
scenario or a default. This is an ownership-preserving executor placement, not
a second radio datapath. Upstream devices retain bounded RX/TX queues and
use Xarxa's global packet pool. Pool exhaustion drops and accounts RX frames
because upstream has no public pool-release notification. One RX drain is
bounded by the endpoint queue depth so a continuously refilling radio cannot
monopolize the network executor. The shared radio still owns the final SRAM
TX allocation and scheduling.

Initialization ends at `WifiIdle`. Credentials arrive only with a STA/AP role
request; scan and monitor do not require a temporary STA. Permanent STA and
AP `embassy-net` devices retain distinct IP/link/RX state while sharing one
tagged physical TX fabric. Role transitions publish link state instead of
reconstructing either device. The target never owns scenario criteria,
expected hashes or stored lab secrets.

Session admission reads functional station link and Block-Ack state from the
production status owner. Diagnostic aggregate counters are observations only
and never authorize traffic or change role behaviour.

## Application boundaries

The `runtime` binary is the HIL application of the shared stage-two boot
contract. `product_hil` owns the radio/network composition
and its persistent observation resources; its `traffic`, `ieee802154` and
`rx_qualification` children own workload and observation duties. The value-only
`rx_statistics` child owns RX counter deltas and wire-evidence conversion.
`network` owns storage for the selected stack/driver, per-interface IPv4
configuration and HIL-only checksum-cost policy. Shared socket workloads use
the stack's UDP and TCP bindings. UDP RX retains 16 datagrams: Xarxa
selects this depth through its configuration feature. Xarxa packet-pool capacity remains a
separate limit from the socket queue depth.
TX pacing is a workload policy, not a claimed socket queue capacity. TCP buffer
sizes remain application-owned.

`console` retains the coupled UART/session admission, logger serialization
and emergency writer lifecycle. Its `radio` child supplies the product-facing
startup, session and Wi-Fi completion endpoints; memory-only images exclude
that child while retaining the common command admission rules.
The asynchronous USB owner waits for writer release; cancelling that wait
releases its priority. Emergency output remains nonblocking. Diagnostic images
with `driver-observation` reserve 32 text records for synchronous role-transition
bursts; other images reserve eight. Both queues remain bounded SRAM storage.
Overflow warnings distinguish queue exhaustion, immediate-writer contention
and USB write errors, and retain the first queue rejection. Required reports
await admission outside measured work; ordinary radio logging never waits for
USB capacity. Extra diagnostic queue storage costs 9,408 SRAM bytes and does
not change radio pools. Arbitrary sustained logging can still overflow and is
reported, rather than treated as complete diagnostic evidence.

The `diagnostic-memory-benchmark` image excludes `product_hil` and its radio
and network tasks at compile time. It keeps the shared two-core boot,
executors, stack placement and HIL console; CPU1 publishes its spawner but
does not start a network task. The memory task alone claims AXI-GDMA and its
dedicated buffers. Image key advertisement belongs to `oer-hil-image-keys`,
so reporting memory support does not retain the product owner graph. Its
4,096-byte maximum payload describes the per-frame benchmark command policy,
independently of the product TCP buffer size.

HIL retains its `stack.toml` (CPU1's stack) and diagnostic observers.
Board initialization, relocation and interrupt-stack mechanics belong to the
shared platform; application images use the same mechanism through `cargo xtask
build firmware`. A hardware scenario verdict remains a separate HIL responsibility.

## Station AP availability

A successful station-start request admits the production station service; it
does not imply association. With the AP initially absent, HIL requires this
admission followed by generation-zero no-candidate exhaustion after three
attempts, no connected edge, and a fresh control response. Retry policy and
owner disposition remain in the production service.

```console
cargo hil doctor station-ap-loss
cargo hil plan --tag ap-availability
cargo hil run-all --tag ap-availability
```

The selected scenarios distinguish recovery with fresh ICMP exchange, prolonged
absence after a connection, and initial absence before any station start. The
last two use the production three-attempt policy and verify that control still
responds. See [scenario semantics](../../scenarios/README.md#ap-availability)
and the [focused capability program](../../../qualification/targets/esp32s31/wifi-ap-availability.toml).
Fresh runs capture build sources; explicitly include reviewed untracked inputs
with `--source-include` as described in the [host guide](../../host/README.md).

## Protocol families per image

Each image enables only the HIL protocol families it serves
([radio families](../../protocol/README.md#radio-families)), so its build, and
the evidence bound to its sources, reads only their protocol modules: the
Bluetooth images the `bluetooth` family, the radio-free `system-watchdog`,
`diagnostic-usb-jtag-off` and `system-panic-reset` images the `system` family, and the Wi-Fi images the `wifi` family, each with
the shared core. The IEEE 802.15.4 images and the memory benchmark image are
built on the Wi-Fi runtime (`product_hil`, its console and `oer-hil-agent`'s
`wifi` modules), so they also compile the `wifi` family and about thirty
Wi-Fi runtime files: a Wi-Fi source change stales their evidence, while an
IEEE 802.15.4 change leaves Wi-Fi evidence valid. Separating them needs an
IEEE 802.15.4 console and entry of their own, as the Bluetooth and system
images have.

## Event trace

Every image links one `oer-trace` trace in RTC fast memory beside the
post-mortem record (`runtime/src/trace.rs`): 512 entries, and two 1024-word
snapshot slots only with the `trace-snapshots` feature. The hang watchdog and
the agent's panic hook, which the platform's panic entry calls through its
`panic-hook` feature, freeze it before the reset; every image but
`system-panic-reset` has that hook (the agent's `panic-hook` feature, which
each image's base feature enables), and that image runs the product panic
entry alone, so the `system-panic-reset` scenario exercises the product's
record-and-reset path. The hook, the exception entry and the hang watchdog
format nothing: they record the post-mortem fault and the hart's trap
registers, the vector it took and the sources pending and routed to it
(`agent/src/fatal.rs`) in RTC fast memory and reset, and the next boot prints
`OPEN_RADIO_HIL runtime=PANIC boot=previous` or `runtime=EXCEPTION
boot=previous` with that state before anything else. A panic message with
arguments is not kept, only a static one. The next boot also holds the frozen
trace for the host, which pages it out through the
[trace commands](../../protocol/diagnostics.md#event-trace). The
`station-exit-evidence` feature also enables the station runtime's own trace
events. With an IEEE 802.15.4 image, the `ieee802154-trace` feature records
the MAC engine, runtime and transmit power-sequence events of
[`oer-ieee802154-trace`](../../../crates/hardware/ieee802154/trace/README.md). `diagnostic-ieee802154-radio-trace`
builds with it and serves the scenarios against the reference peer, so a
failed one's post-mortem holds the last MAC events before the failure:
whether a lost frame never reached the radio, was aborted (with its reason)
or was handled late. `diagnostic-ieee802154-radio` builds without it: the
trace reads the power registers on every transmission and spends interrupt
time, and the vendor calibration comparison boots that image as the
product's radio.

## Local dependency overrides

Three git-pinned dependencies of the firmware can be replaced by a local
checkout for one build or run. The variable names the checkout's root:

| Variable | Checkout | Packages patched |
| --- | --- | --- |
| `ESP_HAL_ROOT` | esp-hal | `esp-hal`, `esp-sync`, `esp-bootloader-esp-idf` |
| `EMBASSY_ROOT` | embassy | `embassy-net` |
| `OPEN_RADIO_XARXA_ROOT` | xarxa | `xarxa`, `xarxa-driver` |

The runtime build patches them in and resolves a private copy of its lock
file; the bootstrap takes only the esp-hal override, the one dependency it has.
A run's source snapshot archives each override checkout as its own source
role, so an untracked file there is named as `--source-include xarxa:<path>`.
An override build binds evidence only when the checkout's commit is the pinned
revision in `hil/targets/esp32s31/Cargo.toml`; otherwise it is an experiment.

To find the dependency commit that changed a scenario's outcome, bisect in the
dependency's clone and build this checkout against each step. A build failure
skips the step (exit 125) instead of marking it bad:

```console
git clone https://github.com/ermacv/xarxa.git ~/dev/xarxa
cd ~/dev/xarxa
git bisect start <bad-rev> <good-rev>
git bisect run sh -c 'cd ~/dev/<checkout> &&
  OPEN_RADIO_XARXA_ROOT=~/dev/xarxa cargo hil image build performance || exit 125;
  OPEN_RADIO_XARXA_ROOT=~/dev/xarxa cargo hil run <scenario>'
git bisect reset
```

Each step takes its own lease. The same recipe bisects esp-hal or embassy with
their variable.
