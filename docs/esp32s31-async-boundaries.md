# ESP32-S31 async boundary and heap audit

This document complements the generated symbol inventory in
`esp32s31-async-runtime-audit.md`. It is tied to the same pinned archives:

- `libpp.a`: `f863c65c3ed89cf5d2a2cbe0d6bca3b783ca35788a704bb68e13958e4b94958e`
- `libwpa_supplicant.a`: `f1c03da6047ccc5a9dca67d69d9c260ef711bea5119c9a451728c560e4eb34e3`

## Feasibility summary

| Boundary | Can be replaced without changing the blob? | Current solution |
|---|---|---|
| PP and WPA worker receive loops | Yes | Stackless bounded dispatcher |
| `ppTask` creation | Yes | Pre-init OSI interception returns a logical handle; the entry point is never called |
| OS timers | Yes | Executor alarm plus timer pool |
| Wi-Fi event posting | Yes, if the ESP event loop is replaced | Owned bounded `WifiEventBridge` |
| Interrupt-to-task notification | Yes | `InterruptSignal` or `BoundedChannel` |
| STA/AP data RX callback | Yes | Eight fixed payload slots plus a producer-woken owned channel |
| STA/AP application data TX | Yes | Eight fixed payload slots, one radio-owner submission attempt, and AP authorization at submit |
| NVS writes | Yes, by disabling vendor NVS | `disable_vendor_nvs`; application restores and persists configuration |
| Dynamic RX/TX/cache buffers | Partly | `disable_dynamic_wifi_buffers`; caller supplies adequate static pools |
| WPA2-Personal RX ownership | Yes | Fixed `Wpa2Ingress` plus validated owned EAPOL-Key frames |
| WPA2-Personal handshake control | Yes, above blob | Fixed STA/AP states, replay protection, completion tickets, and owned actions |
| WPA2 PTK/MIC/key unwrap | Yes, above blob | IRQ-driven fixed `CryptoJob` operations; no hardware-status polling before IRQ |
| WPA2 M1-M4/GTK framing | Yes, above blob | Fixed owned builders/parser, exact lengths, replay/nonce context binding |
| WPA2 TX/key command ownership | Yes, above blob | `Wpa2IoQueue`, aligned CCMP keys, fixed key table, one fail-fast backend attempt |
| WPA2 retransmission scheduling | Yes, above blob | Finite generation-tagged one-shot alarms; one action per alarm edge |
| STA EAPOL TX completion | Yes, required replacement | Rust parses the completed QoS/optional-CCMP MPDU and copies only M2/M4 metadata into a fixed channel |
| Connection blacklist | Yes, required replacement | Rust scan/reconnect policy owns candidates; allocation-backed vendor add/remove/check/clear entries are final-link no-ops |
| STA link notifications | Yes, required replacement | Connected/disconnected events use a fixed channel; the four-way query is Rust-owned state |
| S31 static TX/pairwise key backend | Partial | Static-pool TX and stable `0xb8` CCMP objects implemented; GTK/authorization fail closed |
| WPA2-Personal crypto callback table | Only synchronously | Precompute PBKDF2 asynchronously with `WpaPskJob`; replace remaining handshake transitions above this ABI |
| WPA3 SAE in this S31 blob | No usable implementation | Out of current scope; feature is disabled and registered SAE callbacks are null |
| Rust-owned async crypto | Yes | Pinned `CryptoJob` plus `InterruptCryptoEngine` and ISR signal |
| Enterprise EAP/TLS crypto inside blob | No | Requires replacement of the EAP/TLS state machine above the crypto calls |
| `malloc` inside WPA/EAP/TLS | No | Requires replacement of the allocating protocol code or a fixed arena compatibility layer |
| `ets_delay_us` inside PP leaf functions | No | Requires replacement of the containing leaf with a timer-driven continuation |
| Lifecycle waits in `eloop_destroy` | No | Serialize lifecycle and replace the complete async deinit boundary |
| WPS `eloop_register_timeout_blocking` | No | Keep WPS disabled until its control flow is replaced |

## Cryptography

The current secured target is WPA2-Personal AP/STA. WPA3 is intentionally not
implemented by this runtime: the bundled S31 supplicant has no usable SAE
engine behind the net80211 glue. Enterprise EAP is also outside the current
priority.

`wifi_init_config_t` embeds `wpa_crypto_funcs_t`. AES, SHA, HMAC, PBKDF2,
CCMP, GMAC, wrap, and unwrap callbacks can therefore be replaced with Rust or
hardware implementations without modifying an archive. Every callback has a
synchronous C ABI, however: the result buffer must be complete when the
callback returns. There is no request context, completion callback, or pending
return value.

Consequences:

1. A hardware peripheral may be used only if the callback polls it to
   completion. That removes software compute but still blocks the executor.
2. Returning after starting DMA is invalid because the blob immediately reads
   the output and often releases the input buffers.
3. WPA-Personal PBKDF2 should be performed before connection and the resulting
   PMK supplied to the connection state where possible. The remaining
   handshake hashes are short bounded operations.
4. Enterprise EAP uses the internal TLS/EAP implementation and calls
   `eap_sm_process_request` as one synchronous operation. The Wi-Fi crypto
   callback table is not an async boundary for certificate parsing, RSA, or
   TLS record processing.

`Wpa2Ingress` now removes borrowed vendor RX-buffer lifetime from the WPA2
boundary. It validates the complete EAPOL and RSN key-data lengths before a
bounded copy, owns peer/interface metadata, and hands the frame to a
producer-woken async consumer. This is transport ownership and parsing, not a
key-install or TX implementation.

The control-flow portion is now implemented separately from hardware actions.
STA/AP states accept one owned frame or one ticketed completion and return one
action. Replayed M3 never causes key reinstallation, AP peer count is a const
generic fixed table, and stale crypto completions cannot advance a newer
handshake. PTK, MIC, key wrap/unwrap, and TX frame jobs own all inputs/outputs.
The I/O queue also owns complete Ethernet frames and aligned CCMP install keys.
`S31StaticWpa2Io` now submits into the configured static PP pool and programs
pairwise hardware/software CCMP state. Its software objects live in caller
provided `'static` storage, and the existing vendor slot must be null or point
to that exact object. GTK node metadata and AP authorization remain target
integration boundaries.

The viable fully async design is a Rust WPA/EAP/TLS state machine that owns its
buffers and submits crypto commands to an async engine. A hardware completion
ISR calls `InterruptSignal::notify_from_isr`; the crypto future resumes on the
executor and only then advances the protocol state. This cannot be inserted
under an existing synchronous crypto callback.

That Rust-owned hardware boundary is now implemented by
`InterruptCryptoEngine`. Its backend starts the peripheral using a pinned
`CryptoJob`, reports hardware status without polling in a loop, and finalizes
only after `InterruptSignal` wakes the future. Cancellation calls `abort`, and
job storage is wiped on drop. `WpaPskJob` specializes this path for the
4096-iteration WPA/WPA2-Personal PBKDF2-SHA1 derivation, allowing a 32-byte PMK
to be ready before the synchronous connect path begins. The audited exported
`wpa_set_pmk` (`0x8c` bytes in the pinned archive) then copies that key from a
completed job when invoked by the single radio owner.

The S31 has no verified SHA-1 completion interrupt, so its concrete PBKDF2 path
is a Rust software future with a caller-selected HMAC budget per poll. Every
poll advances cryptographic work; it never reads a busy flag or waits for an
RTOS object. The future uses only fixed job/intermediate storage and wipes
partial output if cancelled. This is distinct from the HAL CPU SHA work queue,
whose recall mechanism polls hardware busy state and is therefore excluded
from the strict runtime.

## Critical sections and locks

There are four different mechanisms and they must not be treated alike:

- `_wifi_int_disable`/`_wifi_int_restore` protect state shared with the Wi-Fi
  ISR, including `pp_sig_cnt`. Initialization delegates to the hardware
  adapter. The strict phase requires the executor and Wi-Fi ISR on
  `wifi_task_core_id`, validates the current hart, and uses local MIE masking;
  it neither spins on nor stalls the second core.
- `wifi_api_lock` skips its mutex when `current_task_is_wifi_task()` is true.
  PP and timer callbacks already run under the virtual PP identity, so this
  bypass is active there.
- `g_wifi_global_lock`, eloop mutexes, and other OSI mutexes do not all have an
  identity bypass. The current adapter never waits for them. If contention is
  observed, silently returning failure is not generally correct because many
  blob callers ignore the return value.
- `_dport_access_stall_other_cpu_*` remains supplied by the hardware adapter
  only during initialization. After `prepare_strict_runtime` the wrapper
  records a violation and returns without entering the stall.

All public Wi-Fi operations should therefore be serialized as commands to the
single radio owner instead of calling synchronous `esp_wifi_*` APIs from
arbitrary futures. Rust-owned queues and signals use atomics and do not need
the vendor mutexes. The interrupt masks around blob-owned shared state remain
until that state is also moved to Rust.

`RadioCommandQueue<C, N>` and `RadioOwnerFuture` now implement this ownership
boundary. Producers move typed commands into a fixed-capacity channel. Only
the owner handles them under virtual Wi-Fi identity, with a finite per-poll
budget before PP/timer work is polled.

The queue claim cannot rely on Rust `compare_exchange_weak` alone: the pinned
LLVM RV32 backend lowers both weak and strong operations to an `lr.w`/`sc.w`
retry loop. `atomic_once` instead emits exactly one reservation and one
store-conditional. A failed store-conditional is reported as contention, the
producer retains the typed command, and `submit` yields until a Rust async
capacity wake. The final-image audit locates the live
`RadioCommandQueue::try_submit` monomorphization and rejects every
control-flow cycle in it, including compiler-introduced LR/SC retries.

The production initialization path prevents the task from existing at all.
Before `esp_wifi_init_internal`, the stable OSI table is patched in place. Its
task-create callback recognizes only the pinned `ppTask` entry, publishes a
logical handle and releases the startup latch without invoking the entry point.
Already queued initialization events are then dispatched synchronously with an
explicit finite budget before the next vendor API consumes their effects.
There is no RTOS task creation, event-15 retirement, delay, status poll, or
blocking semaphore. The older event-15 handoff is retained only as a
comparative HIL mode and cannot establish the production taskless invariant.

## Interrupt synchronization

`InterruptSignal` is an edge counter plus a waker. Its interrupt side performs
one atomic increment and a non-spinning wake. Vendor code is never called from
the ISR. `wait_after(generation)` turns the next edge into a future and detects
multiple/coalesced interrupts through the generation value.

`BoundedChannel<T, N>` is used when the ISR also has to transfer owned data. It
is fixed-capacity and lock-free; overflow is returned immediately. The async
consumer registers its waker before checking the queue to avoid a lost wake.

Ordinary STA/AP data is also detached from arbitrary netstack callbacks.
`install_async_wifi_data_rx` registers two finite callbacks which claim one of
eight static 1600-byte slots, copy the frame, recycle the vendor RX object, and
return immediately. `receive_wifi_data` owns the slot until its frame guard is
dropped. Oversized input or exhaustion is counted and rejected without retry.
Recycling runs under the virtual Wi-Fi task identity, so the pinned API-lock
path cannot wait.

The matching application TX channel also owns eight fixed 1600-byte slots.
Producers call `try_send_wifi_data`; the radio owner awaits
`receive_wifi_data_tx` and calls `S31StaticWpa2Io::try_transmit_wifi_data` once.
Static pool exhaustion is returned immediately. AP submission rechecks the
destination against the current Rust controlled-port table, while STA follows
the same direct peer lookup, buffer claim, descriptor setup, and PP post path.
Promiscuous/sniffer RX remains disabled: the strict event-13 dispatcher always
fails closed before entering the vendor handler, which would otherwise invoke
an optional callback and then release two heap-owned objects.

Vendor ioctl event 6 is also outside the strict runtime. Its envelope contains
an arbitrary callback, heap ownership flags, and an optional completion
semaphore, while its dispatcher enters PM wake/sleep bookkeeping around that
callback. Wi-Fi configuration, start, stop, and mode changes must complete
before `prepare_strict_runtime`; a later event 6 is rejected without invoking
or freeing the envelope.

Events 9 through 12, 28, and out-of-range values select
`pp_default_event_handler`, whose entire body is a log followed by an infinite
loop. Strict dispatch now reports the unsupported action immediately and never
enters that terminal body. The full WPA2/network stress workload produces none
of these events.

Net80211 output event 5 now owns an explicit Rust intrusive queue. The
replacement `ieee80211_post_hmac_tx` accepts only a run-to-completion call on
the strict Wi-Fi hart, an ordinary STA/AP descriptor, and the current home
channel. A Rust-owned event-token bit ensures that publication posts event 5
only on the empty-to-non-empty transition. Each event consumes that token,
removes one frame, and runs the ordinary STA/AP consumer directly in Rust.
Node lookup, Ethernet-to-802.11 encapsulation, completion-callback selection,
classification, optional CCMP selection, alignment, descriptor construction,
PTI selection, and the call to the existing `ppTxPkt` preparation boundary
all complete in that one bounded action. A follow-up token is reserved only
if another Rust-owned frame remains. A nested publication observes the
already reserved token, so it cannot create a surplus empty event. Thus
neither the stock shared-list drain nor `ieee80211_output_process` is a
runtime dependency, and the vendor `g_ic+0x1ac/+0x1b0` off-channel queue
cannot become live.
Node lookup is already constrained by the Rust STA/AP interface and node-table
owners. Classification is now a finite Rust leaf: direct references are
wrapped, and strict handoff replaces the ROM consumer's callback-table slot
`net80211_funcs+0x24` after validating its previous value. EAPOL/WAPI, STA
ARP, DHCP/DNS, IPv4/IPv6 priority, multicast, and the four-state WMM admission
graph are handled without allocation, waiting, retry, or indirect calls.
WPA2-CCMP key selection and header insertion are also Rust-owned. The strict
leaf accepts only a key object in the fixed Rust key registry, selects the
pairwise or group hardware index without reading the `g_ic` software-key
table, advances the recovered 48-bit packet number by three, and inserts the
eight-byte CCMP header without the vendor cipher-object indirect call. The
following ESF header reservation and alignment leaf is Rust-owned too. It
admits only the recovered 24-byte legacy or 26-byte QoS header, checks the
14-bit MPDU length, shifts the MPDU by an alignment delta of at most three
bytes, and publishes the packed storage word only after all arithmetic
succeeds. The live ordinary encapsulator now constructs the STA/AP address
layout, RFC 1042 LLC/SNAP prefix, QoS/no-ack policy, sequence field,
descriptor, and PTI directly. It rejects WAPI, raw, HE-prefix, NAN/mesh,
off-channel, and AP power-save states before ownership can escape. The
recovered STA EAPOL rule is explicit: EtherType `0x888e` owns descriptor
callback bit 3; ordinary STA data owns no callback; AP traffic owns callback
bit 12. The remaining lower transition is `ppTxPkt` and its already
interposed stateless preparation/queue-map leaves, not the vendor net80211
event-5 consumer.

For timers, the original producer allocates an eight-byte envelope and posts
event 7. The final-link timer wrapper replaces that producer with a sixteen-slot
fixed pool and a private executor event. Only the proven `timer_connect`
success action is completed locally. The `chm_dwell` action is rejected
because its downstream `chm_end_op` contains an arbitrary completion callback
and an OSI synchronization call.
Stock auth/assoc/handshake/reconnect/scan/beacon/hostap timeout recovery can
reach synchronous MAC deinit or channel switching and therefore fails closed
after the strict proof. WPA2 retry timing is Rust-owned and remains async.

The strict profile has a separate Rust-owned passive scan and does not call
`esp_wifi_scan_start`. `passive_scan_2_4ghz` enqueues one channel command at a
time to the radio owner, uses the executor timer-backed channel-operation
boundary for dwell completion, and awaits one wake edge per channel. Beacon
and probe-response frames are parsed before the vendor management-frame tail
and deduplicated by BSSID into a 32-entry BSS table. The caller supplies the
output slice; no vendor AP list, `Vec`, semaphore, task delay, or polling loop
is used. Table overflow is reported in the scan summary and never waits.
The pinned RX-policy jump table is not entered: its exact policy-3 and policy-0
branches are expressed as direct calls to audited finite `ic_*` leaves. Dwell
completion likewise accepts only the Rust scan callback, clears the fixed
channel-operation state directly, and makes no arbitrary indirect call.

HE20 discovery is now owned by the same bounded parser. Extension elements 35
(HE Capabilities) and 36 (HE Operation) are copied into fixed 64-byte and
32-byte record fields; an element that does not fit marks the record truncated
instead of allocating or accepting a prefix. The stateless HE parser validates
the complete element length, extracts the mandatory <=80-MHz RX/TX MCS/NSS
maps for NSS1, and records BSS color. Association-response observation exposes
the received HE element lengths, bidirectional MCS9 support, and BSS color
through atomics in `StaAssocSnapshot`.

An external channel-11 monitor capture of the pinned vendor STA recorded its
single association request and the successful response 5.17 ms later. The
request carries one 24-byte HE Capabilities extension:
`ff 16 23 03 18 9c ca 10 80 00 10 8a 1b 0d c0 1f 00 02 82 01 fd ff fd ff`.
The final four bytes advertise RX and TX NSS1 MCS0-9 and reject NSS2-8. The
vendor link reported HE20, proving that no 40-MHz capability is needed for the
qualified AP.

`hil-he-association-oracle` may append exactly that bounded element after the
HT capability, but only when scan parsing already proved that the selected AP
supports bidirectional MCS9. It is an association oracle, not a production HE
claim: peer mutation still installs only recovered HT state, the qualified TX
policy remains HT MCS7, and descriptor/completion gates continue to reject HE.
The ordinary strict profile therefore remains HT-only. HE transmission must
not be enabled until the vendor element's optional MAC/PHY claims have been
narrowed to Rust-owned behavior and its peer state, HE-SIG construction, retry
schedule, and completion layouts have each been recovered and admitted as
bounded leaves.

The first taskless HIL run reached a successful HE association, WPA2 M1-M4,
and a 32-frame BlockAck agreement with unchanged allocation and zero
blocking/delay probes. It then received no ordinary data and could not acquire
DHCP. Returning to the same image without the oracle immediately restored
protected IPv4 and passed the full UDP/HTTP stress workload. This A/B result
locates the next missing boundary after association construction: the S31 HE
node/receive state must be reproduced before the AP is allowed to treat the
station as an HE peer.

The strict RX boundary obtains the complete 14-bit MPDU length from S31
`sig_len` rather than the one-byte length of the first hardware block. It
subtracts the documented four-byte FCS before constructing the shared protocol
slice. Scan, association, power-save observation, and EAPOL classification
therefore see one identical bounded frame and cannot parse FCS bytes as an
additional information element.

## Heap and indirect calls

Setting dynamic RX, dynamic TX, and cache TX counts to zero removes only the
explicitly configurable dynamic packet-buffer modes. Adequate static buffer
counts and the correct target-specific static buffer type must be configured
by the hardware integration.

It does not remove these allocations:

- EAP request and response `wpabuf` objects;
- EAP/TLS/PEAP/TTLS method state;
- certificate, ASN.1, RSA, and bignum objects;
- eloop timeout nodes;
- WPA and net80211 temporary frames and nodes.

Direct `malloc`, `calloc`, `realloc`, and `free` references are widespread in
`libwpa_supplicant.a`. The final firmware must link with
`--wrap=malloc`, `--wrap=calloc`, `--wrap=realloc`, and `--wrap=free`.
The crate's `__wrap_*` functions delegate during initialization, then deny all
four operations after `prepare_strict_runtime`. This is a runtime tripwire, not
permission to use allocating WPA/EAP/TLS paths: a denied allocation may leave
vendor state partially mutated, so every strict root must still be statically
proven not to reach it.

`patch_allocator_probes` measures all allocator slots reached through the OSI
table, including internal and Wi-Fi-specific malloc/calloc/realloc variants.
It delegates only before the strict phase. The linker wrappers cover direct C
symbols, and the final-ELF auditor rejects a firmware missing any wrapper.

`AuditedFuture` combines the blocking, allocation, critical-section, and
other-core-stall probes into a fail-closed runtime gate. Allocation tripwires
cover OSI slots and direct C symbols, while the relocation audit must still
prove that normal strict execution never triggers either class.

The final link uses LLD `--wrap` for `pp_post`,
`ieee80211_timer_process`, and
`--wrap=ieee80211_hostapd_beacon_txcb`, and
`--wrap=ieee80211_tx_mgt_cb`, and
`--wrap=wDev_record_ftm_data`, and
`--wrap=wDev_ftm_set_t1t4`, and
`--wrap=wDev_isNANPktInValidSlot`, and
`--wrap=dbg_read_tx_ppdu`, `--wrap=dbg_dump_rx_ppdu`, and
`--wrap=dbg_dump_rx_sigb`, and
`--wrap=wifi_gpio_debug`,
`--wrap=wpa_sm_rx_eapol`, `--wrap=wpa_ap_rx_eapol`, and
`--wrap=hal_crypto_set_key_entry`, and `--wrap=wifi_log`. Every vendor success,
retry, discard, and
collision branch converges on `lmacTxDone`; its strict wrapper replaces the
inline callback bitmap, `ppProcTxDone` power-management tail, and tail-call to
`ppProcessTxQ` with one-callback/event and queue-resume continuations. The TXQ
state wrapper reads pinned MMIO without test/log hooks and exposes one
completion/collision bitmap bit per event. The original archive sections must
not remain in the final ELF.

Twelve ROM-exported entries cannot use LLD wrapping because the ROM linker
scripts assign their public symbols after `--wrap` rewriting. The late
`esp32s31-rom-wrap-overrides.x` fragment instead aliases
`ieee80211_set_tx_pti`, `esf_buf_alloc`, `esf_buf_recycle`,
`hal_mac_get_txq_state`, `hal_mac_get_txq_complete`, `lmacTxDone`,
`pm_on_beacon_rx`, `pm_on_data_rx`, `pm_on_data_tx`, `ppRecycleRxPkt`,
`wDev_DiscardFrame`, and `esp_test_tx_enab_statistics` to Rust wrappers while
pinning their
`__real_*` names to the audited ROM addresses.
The adjacent archive-only `esp_wifi_internal_free_rx_buffer` export is also a
direct public alias to its unique Rust owner, but needs no `__real_*` symbol:
pre-handoff behavior delegates through `__real_ppRecycleRxPkt`.

The AP-beacon replacement uses
`ld/esp32s31-net80211-locals.x` to name the pinned local timer/flag sections.
It only resets the send flag, reads the next TBTT, and rearms the fixed OSI
timer. Mesh, sleeping-client multicast flush, and the optional indirect hook
fail closed before invocation.

The management-completion replacement accepts ordinary authentication,
association, probe, and beacon subtypes without entering the stock callback.
Disassociation, deauthentication, and action/off-channel completions fail
closed. The stock branches behind those subtypes mutate node/key/channel state
and can eventually reach the MAC deinitialization delay; supporting them needs
an explicit bounded async command/state machine.

FTM is outside the strict basic profile. `disable_ftm` clears both capability
bits and validation rejects either bit if restored. The final-link wrapper is
defense in depth: an unexpected FTM action frame records a strict failure
instead of entering `wDev_record_ftm_data_local -> ets_delay_us(50)`.
The TX-side optional T1/T4 hook records the same failure without calling its
registered callback. GPIO tracing and TX test statistics are unconditional
no-ops, so their callback/time-query paths cannot enter the strict graph.
The NAN valid-slot hook retains its recovered descriptor-kind test: ordinary
AP/STA frames return true, while NAN frames return false without entering the
registered scheduler callback.

The strict `WIFI_PS_NONE` profile also replaces `pm_on_beacon_rx`,
`pm_on_data_rx`, `pm_on_coex_schm_status_config`, and
`pm_set_beacon_duration` with no-ops. PP/net80211 performs ordinary beacon/data
parsing, delivery, and the independent RX rate update outside these hooks. The
removed tails are limited to power-save/mesh bookkeeping: the beacon tail
contains the TIM-to-radio-shutdown delay path, the data tail reaches
modem-sleep OSI timers and Wi-Fi API locks, the coexistence status bridge
queries taskless-uninitialized connectionless-PM state before entering an OSI
lock/timer path, and the duration setter's first-sample path invokes two
optional beacon-offset callbacks. Direct calls and saved vendor function-table
pointers are redirected by mandatory final-link interposition.

Hardware qualification with the data-RX alias completed WPA2,
DHCP/DNS/TCP/HTTP, 4,096/4,096 UDP datagrams, and 4/4 HTTP transfers at
25.710 Mbit/s. Allocation remained unchanged after takeover and all blocking,
task-delay, direct-delay, and queue-rejection probes remained zero. The strict
graph first fell from 95 to 74 violations. Interposing the PM-only beacon
duration setter and proving that `rc_get_trc` removes one bit from a u32 peer
bitmap per iteration then removes the last three `ppRxProtoProc` violations.
The resulting image passed the same complete workload at 24.820 Mbit/s with
unchanged allocation and zero blocking/delay/rejection probes. Its final-ELF
audit reports 71 remaining violations and rejects any residual call to the
interposed vendor duration setter.

The three verbose PPDU/SIG-B decoders are also no-op wrappers under the verified
`WIFI_LOG_NONE` policy. This removes their formatting loops and direct
`puts`/`putchar` leaves from ordinary TX completion and RX success without
changing descriptor or retry state.
The `wifi_log` dispatcher itself is interposed as a no-op as well, so error and
diagnostic branches cannot re-enter a formatter or an arbitrary logging sink.

The STA/AP WPA2 RX entry points are Rust-owned as well. Their wrappers copy one
validated EAPOL-Key packet and peer identity into a global eight-slot channel;
they never call the stock supplicant/authenticator. Queue full and invalid
frames return “not consumed” immediately and increment a rejection counter.
The saved WPA function-table addresses are linker-verified against both Rust
wrappers before the strict proof is issued.

The remaining STA runtime notification slots are patched after
`esp_supplicant_init` and before RX starts. Connected/disconnected callbacks
copy bounded metadata into a fixed channel, and the four-way-handshake query
reads an explicit Rust-owned atomic flag. This removes the stock disconnect
edge through `wpa_sm_notify_disassoc -> eloop_cancel_timeout -> free` from the
active strict graph. `Wpa2IoCommand::ResetStaLink` now provides the bounded
local teardown half of reconnect: after the application closes its network
link and awaits the fixed TX ownership drain, the radio owner preflights every
software-key pointer, closes the controlled port, clears both STA hardware
keys, wipes/releases their static objects, stops the Rust BlockAck session,
and removes the static node/association facts. Completion is published through
one generation-counted async signal. A foreign key pointer or any live
management/A-MPDU owner rejects the command before mutation. This supports
Rust-owned reassociation without entering the vendor disconnect path; emitting
a standards-level deauthentication frame and automatic reconnect policy remain
separate work.

Indirect calls are also part of the protocol ABI: EAP method `process`
pointers, WPA crypto tables, callbacks, eloop timeouts, and PP registered
handlers. They cannot all be removed while retaining the vendor protocol
implementation. New Rust-facing paths should use enums and typed bounded
channels; the remaining indirect calls must be confined to audited vendor
dispatch boundaries.

## Event, logging, and NVS policy

`wifi_event_post` passes `UINT32_MAX` to `_event_post`. `WifiEventBridge` copies
both the event-base name and payload before returning, then transfers ownership
through a bounded channel. It deliberately replaces the ESP event loop, so its
async consumer owns application event dispatch.

The S31 `libprintf.a` formats radio logs into an 80-byte stack buffer and calls
`__esp_radio_printf`. Without the `sys-logs` feature the Rust symbol is a no-op,
so the default path does not block or allocate. Enabling `sys-logs` delegates
to the selected logger and is only async-safe if that logger is a nonblocking
ring-buffer sink.

`disable_vendor_nvs` makes the blob's Wi-Fi NVS paths no-ops. The application
must load configuration before Wi-Fi operation and persist changed settings in
an async storage task. A deferred wrapper around only `_nvs_commit` is not
sufficient because setters and blob operations may themselves enter flash.

## Remaining replacements

1. Route every application-facing Wi-Fi event through `WifiEventBridge` and
   define an explicit overflow policy per event class.
2. Define the product's command enum and response channels on top of the
   implemented `RadioCommandQueue`; never call vendor APIs from producers.
3. Keep WPS disabled and make deinit an explicit async state machine.
4. Replace remaining TX-stop/deinit leaves containing `ets_delay_us` or hardware
   polling with timer/interrupt continuations.
5. For strict heap-free Enterprise support, replace EAP/TLS with fixed protocol
   buffers on the implemented async crypto boundary. There is no ABI-only shortcut.

## Historical strict no-wait gate

The former `audit-strict-esp32s31` built a direct call graph from RISC-V
relocations in all S31 archives. Its roots were the PP handlers called by the Rust
dispatcher plus the vendor leaves called by Rust replacements. Direct heap,
delay, RTOS wait, NVS, logging, abort, and core-stall symbols are forbidden.
An unresolved `jalr`/`jr` is also rejected until its OSI slot or callback is
classified explicitly. Control-flow cycles are rejected until a fixed bound
is proven; the auditor builds an intra-function CFG so a backwards layout edge
without a path back is not mistaken for a loop. This catches polling loops and
data-dependent queue drains that a symbol-only import inventory misses.

The first implemented replacement is PP event 22. With `strict-no-wait`, the
16-us `lmacDisableTransmit` settling delay is split across an executor alarm.
The linked-list scan and MSDU drain from `lmacDiscardMSDU` are also split so a
continuation touches at most one link or one MSDU. The replacement reads and
clears the pinned S31 TXQ interrupt MMIO directly, avoiding the vendor HAL's
optional test/log callbacks. The audit pins the original
`lmacProcessTxTimeout` (`0x62`), `lmacDisableTransmit` (`0xae`),
`lmacDiscardFrameExchangeSequence` (`0xd6`), and `lmacDiscardMSDU` (`0xd2`)
sizes in addition to the complete archive digest.

Strict initialization must call `configure_static_wifi_buffers`,
`disable_vendor_nvs`, `disable_frame_aggregation`, `disable_ftm`, and
`validate_strict_basic_config`. The archive defaults select dynamic TX/RX and
provide no static TX pool. After driver init and before executor start,
`prepare_strict_runtime` sets and verifies both `WIFI_PS_NONE` and
`WIFI_LOG_NONE`, verifies that `patch_pp_runtime_callbacks` replaced the RTOS
OSI table and that the allocator plus critical-section probes were installed
before init, verifies all sixteen final-link wrapper addresses, then arms runtime
heap and core-stall guards and returns the proof
required by `S31StaticWpa2Io`. Allocation callbacks return null, free callbacks
do not enter the heap, and core-stall callbacks return immediately until
explicit post-executor teardown; strict dispatch rejects
unexpected aggregation, PM, BSS-color, modem-beacon, and coexistence events.

Pre-handoff preparation switches connection-time management allocation to the
fixed Rust pool. Authentication, association, and probe frames therefore
cannot remain as heap-owned objects when the runtime heap gate is armed.

PP event 16 is also replaced. The original `ppProcTxDone` drains the complete
linked list, iterates callback bitmaps, and ends in power management. The Rust
state machine performs one classified mode-0 callback per continuation, while
callback-free data may load and recycle a fixed prefix of four frames in one
executor dispatch. It verifies the callback-table pointer before the direct
call and fails closed on unknown bits, user TX callbacks, fragment/trace
descriptors, and frame types outside the strict fixed pools.
The Wi-Fi-only build has the compile-time coexistence feature disabled, making
the registered `_coex_wifi_release` target an exact no-op; strict recycle omits
the `pp_coex_tx_release` classifier and its indirect OSI-table tail. The
remaining pinned recycle leaf is `esf_buf_recycle` (`0x156`), together with the
four basic STA/AP mode-0 callback sizes.

The strict timeout/discard path also replaces the `lmacTxDone` mode-1 bitmap
loop with one classified callback bit per executor event before it appends the
frame to event 16. Final-link interposition routes the other vendor
TX-completion calls through the same state machine. The replacement accepts
only the pinned STA EAPOL mode-1 callback. Off-channel connection-probe and AP
power-save callbacks fail before invocation, as do direct-recycle/aggregation
descriptors and optional TX-time recording.

The strict gate still intentionally fails. Confirmed remaining paths include:

- the stock WPA2-Personal `eapol_txcb` target reaching `calloc/free` through
  eloop timeouts and `ets_delay_us` through deauthentication if strict
  integration fails to install the provided async TX-done callback first;
- basic retry submission now enters only the pinned finite PLCP/HTSIG and
  terminal PHY/MMIO leaves; event-23 RTS/generic-error outcomes use the bounded
  Rust retry/discard classifier; the separate collision event also performs
  one Rust retry per executor action, while connection-management state
  transitions are not yet reconstructed;
- explicit disassociation, deauthentication, and off-channel action completion
  is not implemented; the Rust management wrapper rejects those subtypes before
  the stock channel-change and `hal_mac_deinit -> ets_delay_us` branches;
- allocator and OSI callbacks reached through unresolved indirect calls;
- remaining callback chains and TX/RX queue drains with unproven backward
  branches.

### TX A-MPDU boundary

The successful Rust-owned ADDBA exchange is only protocol negotiation; it does
not make the vendor aggregation scheduler safe to enable. The pinned S31
`ieee80211_ampdu_request` allocates a `0x78`-byte per-TID object and owns an OS
timer. Those two responsibilities are now replaced by `TxBlockAckSession` and
the executor timer pool, but the stock data path would still enter a separate
stateful subsystem:

- `ppCalTxAMPDULength` moves frames between linked lists in `pTxRx`, pauses the
  hardware TXQ, and can call `ppAssembleAMPDU` repeatedly;
- `ppAssembleAMPDU` mutates every descriptor and contains a fatal logging path
  followed by a non-returning loop;
- `lmacEndFrameExchangeSequence` reads the 64-bit hardware BlockAck, updates a
  vendor bitmap, and selects recycle, BAR resend, regression, or resort paths;
- `ppRecycleAmpdu`, `ppRegressAmpdu`, and `ppResortTxAMPDU` walk and relink the
  aggregate chain, and can post more PP work.

Consequently `ic_ampdu_op` is not a sufficient stateless enable switch. The
strict runtime keeps the vendor operational bit clear until all aggregate
descriptor completion and timeout ownership has moved to Rust.

`TxAmpduBatch` is the first half of that replacement. It owns up to 32 fixed TX
slot indices, assigns consecutive 12-bit sequence numbers, consumes a 64-bit
BlockAck including sequence wrap, and returns exactly one acknowledged/retry
result per executor step. It owns no raw frame pointer and has no allocator,
timer, lock, polling operation, or variable-length drain. The remaining work is
to construct the hardware descriptor chain from those slots and replace the
aggregate branches of TX completion/timeout before enabling the negotiated
agreement.

The basic-HT assembly boundary is now independently reproduced. A temporary
HIL-only final-link wrapper called the unmodified ROM `ppAssembleAMPDU` as an
oracle and copied its input/output chain into statically allocated SRAM. It is
not part of the strict runtime and does not justify starting `ppTask`. On an
HT20 association, one captured six-MPDU aggregate had these exact facts:

- each payload metadata word carried MPDU length `0x612` (1,554 bytes), two
  bytes of four-byte alignment padding, and zero empty delimiters;
- the aggregate length was therefore `5 * (4 + 1554 + 2) + (4 + 1554) = 9358`;
- only the first descriptor changed from `0x00042009` to `0x004c2009`, its
  first remaining-length field changed from `0x05f8` to `9358 - 34 = 9324`,
  and the first payload retry-header bit was cleared;
- only the final buffer descriptor gained `0x40000000`; intermediate frame,
  descriptor, and buffer state was unchanged;
- the frame chain uses `frame+0x30`, while the tail buffer of one MPDU uses
  `buffer+0x08` to point to the *first* buffer at `next_frame+0x04`. This
  distinction matters for future scatter/gather frames where `frame+0x04`
  and the tail at `frame+0x08` need not be equal.

A second HIL capture wrapped `hal_mac_tx_set_ppdu` for the same selected
two-MPDU chain. Its aggregate length was 3118, `frame+0x24` carried QoS
sequences `0x014/0x015`, HT-SIG was `0x8f0c2e07`, the data-length register was
`0x70400c2e`, and the length-control register was `0x00400244`. These values
confirm both the sequence metadata offset and every field programmed by the
strict aggregate-submit leaf. The oracle-only vendor/RTOS throughput run was
about 53.4 Mbit/s UDP; it is a format baseline, not part of the final runtime.

A third oracle interposed the ROM `ppResortTxAMPDU` only for one partial
BlockAck. The aggregate covered sequences `0x020..=0x034`; bitmap
`0x001f7fff` acknowledged every MPDU except `0x02f`. The blob detached every
old frame/buffer link. Acknowledged descriptors changed `0x00042009` to
`0x00442009` and queue word `0x00a00304` to `0x01a00304`. The missing MPDU's
MAC frame control changed only from `0x4188` to `0x4988` (IEEE 802.11 Retry),
then it became the head of a new aggregate containing newly queued frames.
Its payload metadata and CCMP-ready ownership were otherwise retained.
`basic_ht_ampdu_completion` and the SRAM-only
`apply_basic_ht_ampdu_completion` reproduce just these bounded per-frame
markers after restoration; neither enters the stock resort/recycle graph.

`HtAmpduLengthAccumulator`, `prepare_basic_ht_ampdu_chain`, and
`assemble_basic_ht_ampdu` now reproduce those rules with a maximum of 32
static frame pointers. The target build places both raw chain operations in
`.rwtext.wifi_strict.*`; the only compiler-generated calls in the larger
ownership constructor are bounded `memcpy`/`memset` operations over its fixed
32-entry arrays. `decode_ht_block_ack_registers` and `read_ht_block_ack`
reproduce the separate three-load `hal_mac_tx_get_blockack` leaf: the 12-bit
starting sequence comes from bits 4..15, the control nibble from bits 16..19,
and the two adjacent registers form the 64-bit bitmap.

This still does not enable A-MPDU in strict mode. Frames currently pass from
`ieee80211_post_hmac_tx` into the vendor PP software scheduler one at a time.
The recovered initial hardware-submit branch is now available as
`submit_basic_ht_ampdu`, but is deliberately not connected to that scheduler.
It accepts only the validated result of `prepare_basic_ht_ampdu_chain`, writes
the idle queue state and programs PLCP0, PLCP1, HT-SIG, length, protection,
power, PTI, EDCA, and queue enable without `GetAccess`, allocation, callback,
event post, wait, or vendor scheduler traversal. The aggregate HT-SIG differs
from an ordinary MPDU in two independent ways: it uses the full aggregate
length and sets bit 3 of the high HT-SIG byte. The two `pTxRx` formatting bytes
initialized by `ppCalTxAMPDULength` are carried as the recovered constant
`0x01/0x01`, so this leaf does not depend on live vendor aggregation state.
The entry point and every Rust helper reachable from it are emitted in
`.rwtext.wifi_strict.*`; its mutable backoff seed is in critical SRAM.

The leaf is still not connected to ordinary data submission. Its completion
side is now installed: strict event 23 recognizes the owned aggregate, reads
the fixed BlockAck registers before clearing the hardware edge, takes the
queue's unique owner token, validates and detaches both chains, and schedules
one private executor continuation. Every continuation mutates at most four
acknowledged/retry MPDUs. Acknowledged frames are admitted to the ordinary
TX-done list only when their raw callback mask is zero, then one event 16
publishes that finite prefix; missing frames remain in a fixed 32-entry SRAM
retry handoff. Management, EAPOL, and AP callback-bearing descriptors cannot
enter the batched path. No call to `ppResortTxAMPDU`, linked-list drain,
allocation, wait, or rate-control callback is made. The remaining boundary is
to connect that retry handoff and the ordinary prepared-frame stream to a Rust
aggregation scheduler.

The opt-in `hil-ampdu-intercept` feature now provides that connection for
hardware qualification only. Its final-link `ppMapTxQueue` wrapper no longer
calls the real mapper. Before ADDBA it admits only the complete hardware-
observed management, EAPOL, Action and fixed-rate QoS state table and applies
the recovered descriptor-byte treatment directly. After ADDBA it retains only
guarded basic-HT QoS data in a fixed 32-pointer SRAM queue and returns `3`,
which makes `ppTxPkt` skip every vendor queue insertion. Unknown mapper states
trap; there is no vendor fallback. A private executor event moves at most four
retained retries or assembles/submits one aggregate of at most 20 MPDUs;
completion schedules the next event rather than recursing. The 20-frame
laboratory cap guarantees the S31 `0x7fff` aggregate-length limit for 1600-byte
static TX slots.

The normal HIL throughput profile also separates diagnostics by cost.
`hil-vendor-tx` retains counters and EAPOL failure evidence, while the
descriptor flight recorder, complete data-frame snapshots, and their
per-transition atomic writes require the explicit
`hil-tx-deep-telemetry` feature. The hardware-submit register snapshot and
detailed successful-completion invariant recorder are also deep-only; the
ordinary profile does not read those diagnostic MMIO registers for every
PPDU. Aggregate size, aggregate bytes, retained queue high-water, and bounded
submit/completion cadence are counted in fixed atomics and remain available
without the deep recorder.

The cadence recorder uses `mcycle` at three owned boundaries: hardware submit,
completion-edge decode, and completion handoff. It stores service,
edge-to-handoff, and handoff-to-next-submit sums in 256-cycle units, plus
sample counts and maxima. Maximum updates get one compare/exchange attempt and
never spin. Missing BlockAck bits and the resulting retry submissions are
counted separately. A completion continuation transfers at most four detached
retry frames per executor action before rescheduling. This fixed quantum
preserves finite execution and ownership while avoiding one private event for
each missing bit in a partial BlockAck; it neither waits nor recursively
submits.

The first `ppTxPkt` preparation leaf, `ppTxProtoProc`, is also replaced by an
SRAM-resident stateless Rust transformation. Its complete recovered decision
tree depends only on two MAC-header bytes and two existing descriptor words,
and host tests cover every branch. The final ELF aliases `ppTxProtoProc`
directly to the Rust entry because applying GNU `--wrap` to this ECO0 ROM
export would otherwise replace the wrapper symbol itself with the ROM address.
The reachable plaintext and WPA2-CCMP branches of `ppProcTxSecFrame` are
likewise replaced by an SRAM-resident Rust leaf. It accepts only the exact
observed management, EAPOL, Action and protected-QoS layouts, validates the
complete transformation before its first write, and then preserves the vendor
write order. The pinned CCMP branch is stateless: selector `3` contributes an
eight-byte MIC plus four-byte FCS trailer, while the common path reserves the
eight-byte metadata prefix. It does not encrypt, allocate, wait, or access a
global; hardware encryption remains selected by the unchanged `0x304`
descriptor word. Rate-control words are deliberately outside this leaf's
input because the pinned implementation never reads them; the mapper and
scheduler validate those fields at their own boundary. Likewise, the lower
layout bits remain opaque buffer identity; this leaf checks only that the
`0x2000` headroom-applied bit was not already set. Any unqualified security
state traps before mutation. The pinned `rcGetSched` boundary is now reduced
to two measured stateless
branches: fixed primary HT and an SRAM-owned 12-byte legacy secondary schedule.
The final-link wrapper traps on every other adaptive/PHY override and never
delegates to vendor rate control.

The enclosing `libpp.a[pp.o]::ppTxPkt` function is now replaced as well.
The implementation was recovered from the pinned RV32 disassembly, including
the complete user-priority-to-hardware-queue decision tree and the intrusive
tail-queue offsets. The armed path validates the Rust-adopted STA/AP interface,
executes only the Rust protocol, security, rate, and mapper transformations,
stores the MAC time, and publishes one frame into the selected logical queue.
It reads hardware-queue idle state directly from the adopted LMAC instance and
posts one stackless executor event; it never calls `ic_interface_enabled`,
`lmacIsIdle`, the cached-HMAC queue consumer, or the vendor mapper.

The replacement was exercised on ESP32-S31 through passive scan, open
authentication, HT20/WMM association, WPA2 M1-M4, DHCP, ping, DNS, TCP, HTTP,
ADDBA, and the combined UDP/HTTP stress workload. All 4,786 submitted TX frames
and all 690 RX frames returned their static credits; the PP queue finished with
21,284 pushes and pops, zero rejects, and zero queued entries. The allocation
snapshot remained unchanged after strict handoff. The run delivered 4,096 of
4,096 UDP datagrams plus four HTTP transfers at 23.799 Mbit/s. During this run
the static ESF slots exposed layout values `0x2000`, `0x2008`, and `0x2010`;
the pinned mapper tests bit `0x2000`, while the lower bits identify the reused
slot and must not be interpreted as mapper state.

The logical TX scheduler has since been separated from the mixed 1,044-byte
`pTxRx` object. At strict handoff Rust requires all sixteen vendor logical
queues to be empty and idle, validates their intrusive empty-tail invariant,
and adopts the four initialized scheduler masks and cursors. Producer append,
hardware selection, dequeue, error requeue, and timeout-chain requeue then use
one fixed SRAM `StrictTxQueueState`; no armed path calls `ppDequeueTxQ` or
mutates the vendor logical queue links.

The first hardware stress run after that split completed WPA2/DHCP/ping/DNS/
TCP/HTTP/ADDBA, 4,096/4,096 UDP datagrams, and 4/4 HTTP transfers. It released
4,787/4,787 TX and 692/692 RX owners, drained 21,371/21,371 PP events with zero
rejects, and preserved the allocation snapshot. The measured UDP payload rate
was 17.514 Mbit/s; this run qualifies ownership and correctness, not a new
throughput ceiling.

The TX-done slice is now Rust-owned as well. Handoff requires the vendor
completion list to be empty and validates its tail-link invariant, then copies
the mode-0/mode-1 masks and only the six callback slots admitted by the strict
profile. All later completion append, dequeue, callback filtering, and callback
identity checks use a fixed SRAM `StrictTxDoneRegistry`; `txdone.rs` does not
touch `pTxRx` after handoff. A hardware stress run completed WPA2, DHCP,
ping/DNS/TCP/HTTP, ADDBA, 4,096/4,096 UDP datagrams, and 4/4 HTTP transfers. It
released 4,786/4,786 TX and 692/692 RX owners, drained 21,295/21,295 PP events,
and preserved the allocation snapshot. The measured correctness-run rate was
19.517 Mbit/s.

The two remaining TX-format bytes per logical queue are now adopted into the
same `StrictTxQueueState`. Their individual bit meanings are still unknown, so
the Rust field names deliberately describe only their observed use as opaque
PLCP length/data inputs. `lmac.rs` no longer references `pTxRx`. A repeated
hardware stress run again completed WPA2/ADDBA/network traffic, 4,096/4,096 UDP
datagrams and 4/4 HTTP transfers, with 4,786/4,786 TX and 692/692 RX releases,
21,320/21,320 PP events, no allocation delta, and 18.466 Mbit/s. RX is now the
only live strict-runtime `pTxRx` slice.

RX callback routing no longer belongs to that slice. Strict handoff adopts the
STA callback and the presence of the pinned AP callback into immutable SRAM,
while rejecting any unknown AP callback or enabled NAN callback. Ordinary RX
and A-MPDU expiry therefore do not inspect the callback words at
`pTxRx+0x3f8..+0x400`. Hardware qualification passed WPA2 and the complete
strict stress workload with 4,786/4,786 TX, 692/692 RX and 21,295/21,295 PP
ownership transitions, no allocation delta, and 20.931 Mbit/s. The remaining
RX dependency is the intrusive ISR-to-executor queue plus the protocol and
recycle leaves.

That intrusive queue has now moved behind an explicit Rust ownership edge.
The recovered `wdev_funcs_init` table stores `lmacRxDone` at
`pp_wdev_funcs+0x1dc`; after proving the vendor queue empty under a bounded
local interrupt mask, handoff replaces that slot with an internal-SRAM Rust
callback. The ISR and executor share only a fixed intrusive FIFO, serialized
by the Wi-Fi hart's local interrupt mask. The final ELF forbids any call to
the ROM producer or `ppDequeueRxq_Locked`.

RX continuation does not consume capacity in either event queue. Queue
non-emptiness is the durable readiness condition, the ISR only wakes on its
empty edge, and `RadioFuture` selects RX/vendor/internal work round-robin.
This removes the possible lost-wakeup case caused by a rejected continuation
event without adding a poll loop, delay, retry, or RTOS context switch. The
hardware qualification run completed the full WPA2/network stress workload
with 4,786/4,786 TX, 691/691 RX, 20,591/20,591 PP events, no allocation delta,
and 19.634 Mbit/s. `pTxRx` is now read only during one-shot adoption; the
remaining PP protocol and recycle leaves are replaced below.

RX packet recycling is now a direct Rust ownership transfer. The pinned
`libpp.a[pp.o]::ppRecycleRxPkt` reference body is exactly fourteen bytes: it
loads the buffer descriptor from `frame+0x04`, restores its data view from the
original RX-control pointer at `frame+0x10`, then tail-calls
`esf_buf_recycle`. `rx.rs` calls the Rust recycler directly; the public ROM
symbol is late-aliased to the same unique internal-SRAM function for any
remaining ROM/archive caller. Strict mode admits only an outstanding frame
from the fixed management, large-RX, or qualified static ESF pools, restores
the view once, and releases that owner without PP state, allocation, an OSI
primitive, a delay, or a retry loop. Pre-handoff delegation retains only the
pinned ROM entry at `0x2f800f98`.
The ESP32-S31 WPA2 stress qualification completed 4,096/4,096 UDP datagrams
and 4/4 HTTP transfers with 4,786/4,786 TX, 692/692 RX, and
20,693/20,693 PP ownership transitions. The measured UDP payload rate was
26.532 Mbit/s. `strict_ok=true` additionally proves that the fixed ESF
rejection counter was unchanged across the run; all allocation, blocking,
task-delay, and direct-delay counters remained zero.

The adjacent `ppRxProtoProc` leaf is now Rust-owned as well. The pinned
reference is exactly 0x154 bytes. Under the mandatory `WIFI_PS_NONE` policy,
its data and beacon branches have no remaining effect because
`pm_on_data_rx`, `pm_set_beacon_duration`, and `pm_on_beacon_rx` are verified
no-ops. Rust reproduces the observable tail: RX-control bits 4 through 6 select
rate-control route 0, 1, 2, or no route; the selected context is stored at
`packet+0x2c`; and the finite `rcUpdateRxDone` leaf applies the RSSI average.

The original lookup through `rc_get_trc` is also removed. Rust reads the three
route bitmaps from the explicitly named 28-byte `trc_ctl` backing and scans at
most the 22 pointer publications in the fixed `g_per_conn_trc` ROM-ABI table.
Each candidate must be non-null and match the six-byte receiver address at
context offset `0x21`; a bitmap outside the 22-slot range fails closed. This
is a bounded data search, not polling for external progress. The table remains
an explicit compatibility boundary until its per-peer publishers move to
Rust-owned state.

The resulting ESP32-S31 run completed WPA2, 4,096/4,096 UDP datagrams and 4/4
HTTP transfers at 25.033 Mbit/s. TX ownership balanced at 4,786/4,786, RX at
692/692, and PP at 20,589/20,589. ESF rejection remained 0 to 0, with zero
allocation, blocking, task-delay, direct-delay, or queue-rejection probes.
The final ELF contains no call to `ppRxProtoProc` or `rc_get_trc`.

The remaining 0x66-byte `rcUpdateRxDone` tail is now Rust-owned too. Its two
guard flags, wrapping calibration addition from `wDevCtrl+0x2e`, signed
`(previous + sample) / 2` stage, and signed
`(3 * previous + intermediate) / 4` stage are reproduced in a pure
host-tested function. The target adapter performs only the four bounded byte
loads and two byte stores. A second hardware qualification completed
4,096/4,096 UDP and 4/4 HTTP transfers at 25.759 Mbit/s with balanced
4,786/4,786 TX, 691/691 RX, and 20,611/20,611 PP ownership. ESF rejection
remained 0 to 0 and all allocation, blocking, delay, and queue-rejection
probes remained zero. No PP receive-protocol or receive-rate vendor leaf
remains on the strict RX path.

The public network-buffer release leaf is now Rust-owned too.
`libpp.a[if_hwctrl.o]::esp_wifi_internal_free_rx_buffer` is exactly eight
bytes and only tail-calls `ppRecycleRxPkt` with the unchanged ESF pointer.
The late linker fragment publishes
`wifi_strict_esp_wifi_internal_free_rx_buffer` under that public name, so the
archive object is not extracted. The strict data-RX callback calls the unique
Rust function directly; it consumes the same fixed-pool owner used by
`ppRecycleRxPkt`, with no mutex, queue operation, allocation, wait, or hidden
state. Cold calls still delegate through the pinned real PP recycler before
strict handoff.

The hardware qualification completed WPA2, 4,096/4,096 UDP datagrams and 4/4
HTTP transfers at 25.535 Mbit/s. TX ownership balanced at 4,786/4,786, RX at
690/690, and PP at 20,623/20,623. ESF rejection stayed 0 to 0; all allocation
and queue-rejection probes remained zero. The final audit reports 6,407
functions and zero violations and rejects any instruction that calls the old
public release leaf.

The adjacent `wDev_DiscardFrame` ownership transition is now Rust-owned.
The pinned 0x20-byte reference body only detaches the completed descriptor
prefix: it retains `wDevCtrl.head`, reads and clears `tail.next`, publishes
that next descriptor as the new head, and transfers
`(old_head, tail, count)` to `wDev_AppendRxBlocks`. Rust represents the
detached prefix as a non-`Copy` token and performs the head publication under
one finite local Wi-Fi interrupt mask before consuming the token into the
fixed asynchronous recycler. It introduces no allocation, poll, wait, delay,
RTOS primitive, task handoff, or new static object.

Because the ESP32-S31 ROM script exports `wDev_DiscardFrame` absolutely at
`0x2f8010c8`, GNU `--wrap` would bind the generated wrapper name back to the
ROM address. The late override fragment instead keeps that address only as
the cold `__real_wDev_DiscardFrame` oracle and asserts that the public symbol
equals the unique SRAM function `wifi_strict_wdev_discard_frame`. The final
audit rejects the old leaf as a call target and proves the Rust entry is code
in internal SRAM.

The qualified hardware run completed taskless cold init, passive scan, WPA2,
DHCP and post-link checks, then 4,096/4,096 UDP datagrams and 4/4 HTTP
transfers at 23.778 Mbit/s. TX ownership balanced at 4,786/4,786, RX at
692/692, and PP at 20,691/20,691, with zero ESF rejection or allocation
delta. The complete static-binding, static-PM and runtime audit reports 6,407
functions and zero violations.

The Rust outer successful-RX walk now also preserves the exact aggregate ABI.
The pinned 0x150-byte `wdevProcessRxSucDataAll` body tests the completion
marker on its current descriptor and passes that same descriptor in `a0` to
`wDev_ProcessRxSucData` at `+0x102`; it does not pass the first descriptor of
the unit. The inner 0x6a0-byte routine reloads the first descriptor from
`wDevCtrl.head` and retains the argument as the unit tail for indication or
discard. The earlier Rust `unit_head` interpretation was therefore unsafe for
multi-descriptor units and has been removed. A non-`Copy`
`CompletedRxUnit { tail, count }` now crosses the boundary exactly once.

The corrected HIL image completed the full WPA2/network stress workload at
26.829 Mbit/s with 4,786/4,786 TX and 690/690 network RX ownership, zero
allocation delta, and no queue or ESF rejection. Its WDEV probe validated
702/702 units and recycler completions, but observed
`max_descriptors=1`. Consequently the singleton runtime path is
hardware-qualified; the multi-descriptor contract remains a pinned
disassembly proof until traffic that produces such a unit is captured.

Every call now also crosses a unique SRAM Rust metadata boundary before the
remaining ROM aggregate. The safe `decode_rx_metadata_layout` function
reproduces the pinned 0x146-byte `get_sublen_offset` result from a fixed
44-byte prefix and one explicit read of MAC register `0x2010_4098` bit 23.
It bounds the computed status-byte offset by the descriptor length and records
its finite layout/status/route class in fixed Rust-owned SRAM. The public
absolute ROM name is late-aliased to that boundary; the original
`0x2f8010f4` address remains reachable only through
`__real_wDev_ProcessRxSucData`.

The first common routes no longer enter the pinned 0x6a0-byte body. For a
status-zero, base-offset STA data frame, or an ordinary association-response,
beacon, or authentication management frame, with
promiscuous/error-dump/CSI modes disabled, Rust publishes the recovered
`wDevCtrl+0x40/+0x44/+0x45` fields, copies the two metadata nibbles, derives
the exact copy/aggregate arguments, and enters the Rust single-descriptor
indication leaf. Probe
Requests in the STA-only profile instead take the recovered direct discard
path: strict preparation proves the optional observation callback null, the
interface registry proves AP absent, and the unit is consumed into the Rust
asynchronous recycler. Action frames are admitted only after the quiescent
handoff proves `wDevCtrl+0x31` interface bit two (NAN) and
`g_wifi_menuconfig+0x40` bit `0x04` (FTM) both clear. That proof is copied into
one byte of Rust-owned SRAM, so the RX hot path does not consult either hidden
C global. Other classes still delegate explicitly, so the aggregate remains
in the strict root graph and the ROM indication leaf remains only for
multi-descriptor, CSI, or otherwise unqualified indication variants.

The singleton indication boundary now also accepts a rounded optional
sublength when CSI/extended metadata is absent. A safe `SingleRxCopyPlan`
proves the two source ranges and the compacted published length before the
unsafe ESF leaf performs its two finite copies. The pinned kind selection is
reproduced exactly: copy-mode-one frames use kind 8 only through 500 bytes and
otherwise use fixed kind 7; an exhausted kind-8 pool falls through immediately
to kind 7 without recording a false allocation failure. Multi-descriptor units
still require a distinct aggregate owner because the ROM constructs one
contiguous MPDU larger than the current 1700-byte ESF slot; treating the
hardware descriptor chain as multiple network frames would violate both the
ESF ABI and BlockAck reorder ownership.

The management HIL measurement produced subtype bitmap `0x2912`: association
response (1), probe request (4), beacon (8), authentication (11), and action
(13). After the Action port, the full WPA2/network stress run decoded 708/708
base-layout, status-zero STA units. Rust indicated 694 data and 13 management
routes, including 2 Action frames, discarded one Probe Request, and observed
zero ROM aggregate fallbacks. It completed scan, authentication, association,
the four-way handshake, DHCP, 4,096/4,096 UDP datagrams, and 4/4 HTTP transfers
at 24.798 Mbit/s with balanced 4,786/4,786 TX and 690/690 network RX ownership,
zero allocations, and zero rejections. The host-tested aggregate decoder
reported only flag value zero. Control, AP/NAN, optional metadata and error
classes remain unqualified and keep an explicit fallback.

The qualified singleton indication body is now Rust-owned as well. The
base-layout/zero-CSI precondition makes the ROM copy split at `0x38` one
contiguous bounded copy. Kind 7 claims the fixed Rust large-RX pool; kind 8
claims only the finite initialized small-RX free list. Neither path can enter
the public allocator or wait for capacity. The stateless population leaf
writes the recovered ESF buffer length, timestamp, rate, channel, and
single/aggregate flags, then preserves the ROM ownership order by recycling
the hardware descriptor before publishing the ESF object to the Rust RX
queue. Descriptor bits 0..13 are validated as capacity and bits 14..27 are
decoded independently as received length.

The final HIL run validated 715/715 units. Rust indicated 699 data plus 13
management frames, including three Action frames, and directly discarded
three STA Probe Requests. Both indication rejection counters, the ROM
indication fallback, and the vendor aggregate fallback remained zero. The
complete WPA2/DHCP/UDP/HTTP workload balanced 4,795 TX and 695 network RX
owners. The primary profile also compiles out unsupported vendor benchmark
statistics; the runtime allocation probe consequently remained exactly zero
from cold takeover through stress, rather than merely unchanged during the
measurement window.

Consumer authority is now distinct from that ISR publication view.
The one-way `RadioResources` claim creates a zero-sized, non-cloneable
`RxExecutorCapability` and moves it into the sole runtime dispatcher. Every
strict RX dequeue requires a mutable borrow of this capability. Cold
initialization has no such token and fails closed on an unexpected RX event;
the ISR can only append and wake. The readiness probe remains global but is
read-only and cannot dequeue or recycle a packet.

The strict STA HIL now supplies the dedicated fixed single-task interrupt
executor/waker. `RadioOwnerFuture` has one static SRAM address and its custom
`RawWaker` only raises `FROM_CPU_INTR2` with one write plus readback. The
vtable and all four waker leaves are in SRAM, so the hardware RX callback no
longer reaches `embassy_executor::raw::waker::wake`, its flash vtable, or its
shared transfer-stack CAS retry. The final-image audit resolves the four
vtable words back to the required SRAM symbols instead of accepting an
unproven indirect call. `FROM_CPU_INTR2` is a low-priority async bottom half.
Its first hardware stress run completed WPA2, 4,096/4,096 UDP datagrams and
4/4 HTTP transfers with 4,786/4,786 TX, 694/694 RX, 20,601/20,601 PP events,
no reject or allocation delta, and 27.477 Mbit/s.

The waker handoff has an additional priority-inversion rule. Every producer
publishes durable readiness before calling `WakerCell::wake`, and the sole
consumer registers before checking that readiness. If the radio bottom half
preempts a lower-priority producer while the producer owns the waker's short
atomic lock, registration sets the pending bit and returns immediately. It
must not wake the same software interrupt: doing so would repeatedly re-enter
the bottom half and prevent the producer from ever releasing the lock. The
published ready state, an older registered waker, or the pending bit preserves
the edge after the producer resumes.

This failure was isolated by JTAG after an intermittent stress stall produced
about 2.5 million software-interrupt entries and wake-by-reference calls while
processing only about 350 real events. After the bounded contention return was
installed, six consecutive cold-start runs completed scan, WPA2, DHCP,
ping/DNS/TCP/HTTP and 4,096 UDP datagrams plus four HTTP transfers. Every run
balanced approximately 4,787 TX owners and 692 RX owners, drained about 20,600
PP events, and recorded no allocation or queue rejection. JTAG observed one
real registration contention in the final passing run, directly exercising
the repaired path rather than merely failing to reproduce the race.

There is no S31 hardware register from which this bottom half can infer cache
availability. Espressif's upstream `cache_ll.h` declares
`CACHE_LL_ENABLE_DISABLE_STATE_SW` and requires software-maintained state.
Accordingly the HIL now puts `CACHE_AVAILABLE`, `POLLING`, `DEFERRED`, and
`TERMINATED` in one internal-SRAM atomic byte. The SRAM
`wifi_strict_radio_try_suspend_cached_executor` leaf closes the gate with one
AMO and succeeds only if no poll is active. Failure is immediate: the cache
owner must arrange an async retry and must not spin. A wake arriving while
closed leaves durable `DEFERRED` readiness; the SRAM
`wifi_strict_radio_resume_cached_executor` leaf reopens the gate and raises
one software interrupt when that bit was observed. The interrupt itself first
claims `POLLING`, returns before dereferencing the cached future when the gate
is closed, and traps on re-entry or future termination. There is no
compare/exchange retry in this protocol.

Executor launch deliberately starts with the gate closed and raises the first
software interrupt before reopening it, so either scheduling order exercises
the deferred protocol or the already-pending edge. Hardware qualification
then passed scan, WPA2, DHCP, ping, DNS, TCP, HTTP, ADDBA, 4,096/4,096 UDP
datagrams and 4/4 HTTP transfers with 4,786/4,786 TX, 691/691 RX,
20,598/20,598 PP events, no rejects or allocation delta, and 25.801 Mbit/s.
The run validates the state machine but did not physically disable cache.
Every future flash/cache owner must acquire and release these leaves around
its cache-off interval; until all such owners are audited, cache-disabled
execution is a local executor guarantee rather than a global firmware proof.

The post-ADDBA mapper also has a bounded stale-completion guard. A late frame
object whose first buffer has already been detached cannot be inspected,
queued, or safely recycled, so exactly one pointer may be quarantined and
reported as consumed without dereferencing it. Repeated calls for that same
pointer are idempotent; a second distinct detached pointer still traps instead
of concealing pool corruption. Rust continuations use a dedicated 64-entry
static SRAM queue instead of competing with vendor `pp_post` producers. The
single executor alternates that queue with the PP queue inside one bounded
poll. Same-hart internal producers are serialized by a bounded local interrupt
mask; each queue still makes one CAS attempt and never spins. This feature may
only be run in the current Wi-Fi-only image where Bluetooth/802.15.4
coexistence is not started.

`BasicHtAmpduChain` now is that reversible ownership token. Besides the public
first/last/count/length summary, it privately retains all 32 validated frame
pointers, each exact 12-bit QoS sequence, and the exact pre-assembly scalar
values. `TxAmpduBatch::push_sequence` accepts those already assigned values
without assuming a consecutive retry aggregate and rejects duplicate slot or
sequence ownership. The SRAM-only
`restore_basic_ht_ampdu_chain` validates every frame link and every tail-buffer
link plus the aggregate first/tail markers before its first write, then removes
both chains and restores the original payload word, descriptor words,
remaining length, timestamp, and tail flags. It deliberately performs no
recycle or retry; those decisions remain executor continuations.

The token is deliberately non-`Copy`: `submit_basic_ht_ampdu` transfers it
into one of four fixed SRAM owner slots immediately before enabling hardware.
An occupied slot fails fast, and a failed enable removes the just-installed
owner. Thus completion can recover the exact chain without a heap object,
lookup allocation, lock, or RTOS queue.

The strict basic-HT completion path also replaces `hal_mac_get_txq_complete`.
The original `0x81e`-byte body performs the required fixed MMIO decode first,
then enters HE MPLEN list maintenance, connection-state locks, formatters, and
debug logging. The replacement reproduces the two six/eight-byte completion
records for both ordinary basic-HT MPDUs and Rust-owned A-MPDUs. Aggregate
BlockAck remains a separate three-load leaf. It traps after recording a strict
failure if it observes HE, BAR, or live MPLEN state. A trap is required because the pinned vendor
caller discards the callee's return value and would otherwise interpret a
returned error as a completion record. Rust now owns the basic completion
outcome state machine, including a four-MPDU bounded aggregate-disposition
quantum, while rejecting those unrelated tails.
The independent `hal_mac_tx_get_blockack` leaf is only `0x3e` bytes, contains
fixed MMIO loads/stores and no calls or cycles, and is the active Rust A-MPDU
completion input.

Strict event 23 no longer enters `lmacProcessTxComplete`. Rust selects one bit
from the completion bitmap, decodes one fixed completion record, copies the six
recovered queue-state fields, clears that queue's completion bit, and uses a
direct `match` for success, RTS error, CTS timeout, TX error, or ACK timeout.
The stock outer loop, test hook, formatter, and indirect outcome jump table are
therefore absent. Hardware stress over 5,012 completions proved that the 4,790
successes used only queue zero/kind three, with zero TXOP ownership, no linked
MPDU, and no aggregate descriptor state. The strict success path now performs
the recovered short/optional-long state updates and basic MPDU recycle count in
Rust, then enters the existing bounded Rust TX-done continuations directly.
ACK and CTS timeout processing now has the same strict basic-HT boundary. Rust
updates the short/long queue and descriptor counters, applies the recovered
rate fallback, evaluates the rate-control and MIB retry limits, performs the
non-aggregate lifetime check from the MAC clock, and either prepares one retry
or enters the existing one-frame-per-event discard continuation. It rejects
TXOP, linked, HE, BAR, A-MPDU, aborted, and trigger/MU states before mutation.
The narrow submission body implements the pinned non-HE `rcGetRate` behavior
as either one direct per-peer selection or at most four cumulative fallback
table entries. It then performs the status-three descriptor transition,
long-frame classification, lifetime timeout, bounded contention backoff, EDCA
configuration, and basic queue enable entirely in Rust/MMIO. Rust reproduces
the complete guarded `mac_tx_get_rts_rate` mapping, PLCP0 and PLCP1 words,
TX-protection registers,
bounded RTS/data power-table reads, queue PPDU-control write,
`coex_pti_tab[1]` priority clamp, and both PTI-register updates. This removes
`hal_mac_tx_set_ppdu`, its indirect `mac_tx_set_pti` OSI callback,
`hal_set_tx_pti`, `mac_tx_get_rts_rate`, `mac_tx_set_plcp0`, and its internal
`hal_he_set_tx_protection` leaf, plus `mac_tx_set_plcp1`, `mac_tx_set_htsig`,
and `mac_tx_set_len`, in addition to
`lmacTxFrame`, ROM
`lmacSetTxFrame`, `ppProcessLifeTime`, the OSI random callback, the common EDCA
helper, and the common TXQ-enable helper from the retry path. In particular,
the unsupported-type `wifi_log` plus infinite loop in `ppProcessLifeTime` is no
longer reachable. Off-channel, NAN, FTM, HE-TB, test, and assertion branches
fail before hardware mutation. HE and aggregate descriptors are rejected
before selection, so the nested `rcGetSMPDURate` logic is outside this profile.
`ppCalFrameTimes` is also absent: its controlling bit is rejected before
mutation and the Rust selector changes only the rate byte.

A 5,026-completion hardware stress run exercised 235 ACK timeouts through this
Rust submission wrapper. All retries retained the same basic-HT frame,
queue-zero/kind-three ownership, no TXOP, and no linked MPDU. The complete
WPA2/DHCP/DNS/TCP/HTTP/UDP test passed at 27.263 Mbit/s with 4/4 HTTP transfers
and zero post-takeover allocation, blocking callbacks, task delays, direct
delays, or queue rejection.

The following PPDU-boundary HIL run exercised the Rust-owned formatting branch
over 5,016 completions: 4,791 successes, 215 ACK timeouts, ten CTS timeouts,
and 225 same-frame retries. WPA2/DHCP/DNS/TCP/HTTP plus 4,096/4,096 UDP
datagrams passed at 28.670 Mbit/s. All application and PP queues had zero
rejects, the allocation snapshot remained unchanged, and every blocking,
task-delay, and direct-delay probe remained zero. A separate graph audit of
the then-five remaining formatting leaves found no indirect call or control-flow
cycle.

After moving the terminal PTI register programming into Rust, another run
passed at 29.932 Mbit/s over 5,024 completions. Its 232 ACK timeouts and three
CTS timeouts exercised 235 same-frame retries; 4,096/4,096 UDP datagrams and
4/4 HTTP transfers completed with zero queue rejection, allocation change,
blocking callback, task delay, or direct delay. Four binary leaves remain in
the basic-HT formatting branch.

Replacing the finite RTS-rate selector with its exhaustive Rust mapping then
passed the same workload at 28.745 Mbit/s over 4,964 completions. Its 171 ACK
timeouts and three CTS timeouts exercised 174 same-frame retries. All 4,096
UDP datagrams and 4/4 HTTP transfers completed with zero queue rejection,
allocation change, blocking callback, task delay, or direct delay. The final
ELF has no call from the Rust submission path to `mac_tx_get_rts_rate`; the
three remaining binary formatting leaves have a zero-violation graph audit.

The subsequent PLCP0 replacement passed at 27.918 Mbit/s over 5,159
completions. It exercised 318 ACK timeouts and 49 CTS timeouts, producing 367
same-frame retries without a changed or detached frame. All 4,096 UDP
datagrams and 4/4 HTTP transfers completed with zero queue rejection,
allocation change, blocking callback, task delay, or direct delay. The final
ELF has no call from the Rust submission path to either `mac_tx_set_plcp0` or
`hal_he_set_tx_protection`; two audited binary formatting leaves remain.

Moving the PLCP1 format, rate, descriptor, and protection-bit packing into
Rust then passed at 28.786 Mbit/s over 4,996 completions. Its 200 ACK timeouts
and four CTS timeouts exercised 204 same-frame retries. All 4,096 UDP
datagrams and 4/4 HTTP transfers completed with zero queue rejection,
allocation change, blocking callback, task delay, or direct delay. The final
ELF has no call from the Rust submission path to `mac_tx_set_plcp1`; only the
audited `mac_tx_set_htsig` binary formatting leaf remains.

The final HTSIG/length replacement passed at 29.829 Mbit/s over 5,025
completions. Its 230 ACK timeouts and three CTS timeouts exercised 233
same-frame retries. All 4,096 UDP datagrams and 4/4 HTTP transfers completed
with zero queue rejection, allocation change, blocking callback, task delay,
or direct delay. Final-ELF disassembly confirms that strict basic-HT retry
formatting contains no `mac_tx_set_plcp0`, `mac_tx_set_plcp1`,
`mac_tx_set_htsig`, or `mac_tx_set_len` call.

The outer TX software-queue action is now Rust-owned as well. A complete HIL
oracle around the pinned `ppProcessTxQ` body observed 5,486 queue-zero calls
during the strict workload: 4,793 submitted exactly one frame and 693 found no
frame. Events one through four were never posted. Every submitted frame used
logical and hardware queue zero, queue kind three, status one after submission,
no linked successor, and the already qualified basic non-HE/non-A-MPDU layout.
The replacement accepts only that profile, removes at most one pointer from
the Rust-owned logical queue, validates the peer and descriptor before mutation, and
calls the existing finite Rust basic-frame submit path. An error before
hardware ownership restores the pointer at the queue head; exhaustion and a
busy hardware queue return immediately. Events one through four fail closed.

Hardware qualification of the replacement completed WPA2 association,
DHCP/DNS/TCP/HTTP, 4,096/4,096 UDP datagrams, and 4/4 HTTP transfers at
25.520 Mbit/s. It handled 5,485 queue actions and submitted 4,792 frames with
zero unexpected outcomes, allocation changes, blocking callbacks, task delays,
direct delays, or queue rejection. Final-ELF enforcement requires the Rust
action to reside in internal SRAM and rejects any instruction that calls the
absolute ROM `ppProcessTxQ` symbol. The absolute export may remain in the symbol
table because it is supplied by the ROM linker script; symbol presence alone
does not make it reachable.

The `hil-vendor-tx` build records allocation-free before/after snapshots for
success and retry outcomes, including queue kind, status, retry counters,
TXOP/list state, and descriptor flags. This remains the HIL oracle for future
aggregate enablement. RTS-error and generic TX-error outcomes were not observed
in ordinary stress, but no longer enter their vendor handlers: recovered
collision-class response codes use the bounded Rust retry path, the key-error
code and unknown diagnostic/interface codes discard one frame through the
existing continuation, and the radio owner remains alive.

PP event 24 no longer calls `lmacProcessCollisions_task`. The existing TXQ
state wrapper exposes one collision bitmap bit and reposts any remainder. Rust
then validates the queue's live one-frame basic-HT ownership and disabled MPLEN
state, disables and acknowledges that hardware queue, and enters the same
bounded collision retry body used by event 23. The stock bitmap drain,
assertions, MPLEN linked-list cleanup, diagnostics, and vendor outcome graph are
not reachable.

For WPA2 specifically, `hal_crypto_set_key_entry` is replaced at final link.
The Rust wrapper reproduces the pinned fixed key-table register writes for keys
up to 32 bytes and performs no temporary allocation irrespective of pointer
alignment. The surrounding `wDev_Insert_KeyEntry` bookkeeping and finite
`hal_crypto_enable` call remain. This is not enough to call the stock higher
key wrappers: AP and non-delete STA paths allocate a persistent opaque
software-key object. `S31StaticWpa2Io` instead
reproduces the pinned pairwise/group object in stable `S31StaticKeyStorage` and
reads the `g_ic` key slot directly, rejecting a foreign pointer before either
hardware or metadata mutation. It stores the verified static pointer itself;
the getter and potentially freeing vendor setter are no longer roots. AP
PTK/SPP lookup and STA GTK metadata use `cnx_node_search` plus the recovered
bounded byte loads/stores.
Its initialization reproduces the exact bounded `wifi_init_key` layout fills
directly in Rust-owned storage, so no vendor initializer remains on that path.
The final link replaces `esf_buf_alloc/recycle`. Strict data TX uses the
already initialized vendor kind-1 free list under local MIE masking, while
management kinds 2 through 4 use eight Rust-owned 1600-byte slots. Exhaustion
returns immediately; no OSI mutex, malloc, free, or dynamic buffer fallback is
called. A final-link management-output gate admits only ordinary association,
authentication, and probe subtypes on the home channel. It rechecks live
non-mesh/non-NAN and AP no-power-save invariants before the stock body; a
rejected frame is returned to the fixed ESF pool.
The measured AP ADDBA-response exception no longer enters
`ieee80211_pwrsave`: when its peer is asleep, the gate copies only the peer,
association generation, and nine-byte action body into an eight-slot
Rust-owned table, recycles the original ESF immediately, and registers the
radio owner on that peer's RX-derived Active/PS-Poll/removal edge. A ready
continuation reconstructs a fresh fixed-pool management frame, restores the
pinned callback bit 13, reproduces the finite AP/action header and descriptor
stores in Rust, and enters `ic_tx_pkt` directly. It neither changes the live
node power-save bit nor re-enters `ieee80211_mgmt_output`. Repeated requests for
one peer update the owned dialog body instead of growing a linked queue. TIM
publication is another finite Rust leaf: the AID at node offset `0x26` selects
a bit in the virtual bitmap at `g_ic + 0x1b7`, while `g_ic + 0x1b6` bit zero
mirrors the BSS/self-node state. The same leaf is shared by deferred AP data;
no strict Rust TX path calls the vendor `ieee80211_set_tim`.
The neighboring `ieee80211_set_tx_pti` wrapper replaces its OSI-table call with
the exact bounded success operation from the pinned coexistence archive: one
volatile byte read from exported `coex_pti_tab[48]` and two descriptor stores.
An out-of-range event is handed to the management-output gate for one-time
recycle instead of continuing with an invalid descriptor.
`S31StaticWpa2Io` no longer
calls `esp_wifi_internal_tx`, because its
global OSI lock can wait; it enters the bounded peer/static-buffer/post leaves
directly and fails immediately on exhaustion. A sleeping unicast peer retains
one owned command behind its Rust Active/PS-Poll/removal future. Group frames
move to a 16-element fixed ESF queue (eight per pseudo-peer), set TIM in Rust,
and are released by the transmitted-beacon DTIM edge with More Data on every
non-final MPDU. None of these paths polls peer state, invokes the vendor
power-save queue, or waits synchronously. STA GTK maps
all logical ids to the pinned hardware group slot one and
uses the finite `ieee80211_set_sta_gtk_index` byte stores; AP GTK uses hardware
slots 8 through 11. Since the blob has no independent controlled-port setter,
AP authorization lives in a fixed Rust peer table and must gate every ordinary
data-channel operation; EAPOL uses a separate pre-auth path. TXQ bitmap
processing, the common TX-done tail, beacon completion, ordinary management
completion, basic retry accounting/discard, and retry rate selection are
classified. Basic retry scheduling, PPDU orchestration, PTI selection, and
queue enable are Rust-owned; the terminal PLCP/HTSIG/PHY leaves and connection
paths are not yet fully Rust-owned. Key installation additionally
requires no live RX fragment from the old key, because the stock cleanup path
can call `wifi_log`.

Peer readiness is an association-generation capability, not a MAC-only flag.
RX publishes Active/PS-Poll only for the current fixed AP association, removal
is published before that generation is released, and every retained unicast
owner compares its captured generation before retry. A missing bounded event
slot remains Pending; it is never interpreted as Active or Removed.

The same ownership rule applies to local STA teardown. The command is accepted
only after data TX reports zero queued/occupied/hardware-credit owners. It
deletes hardware slots 4 and 1 through the recovered finite `ic_del_key` leaf,
clears only software pointers proven to reference `S31StaticKeyStorage`, and
volatile-wipes both key objects. The Rust TX BlockAck retry budget is
per-association while its diagnostics remain cumulative. When the laboratory
A-MPDU intercept is present, teardown additionally requires its fixed frame
array and direct/coalesce state to be empty before disabling the intercept.

The strict WPA2 AP boundary now patches the complete callback group before AP
start. Stock `hostap_init/deinit`, station join/remove, RSN lookup, and peer-SPP
lookup are replaced together, so the heap-backed authenticator and its eloop
rekey timers are never created. Rust retains one validated WPA2-PSK/CCMP RSN IE
and eight stable `0x28` station slots. Association events are copied into a
fixed wake-driven channel; the response uses only the pinned subtype
`0x10`/`0x30` constructor, fixed management pool, and management output. The
stock allocating `esp_send_assoc_resp` and ioctl envelope are bypassed.

Use `--elf final.elf --enforce` after linking. Presence of a forbidden symbol,
a missing direct-heap wrapper, or a replaced vendor entry is a build failure;
`--verbose` prints every archive path.

RX fallback accounting now assigns every compatibility call one of seventeen
fixed reasons. The first measurement isolated all 692 post-link fallbacks to
a non-null `g_wdev_csi_rx`, while the same units had zero CSI metadata. This
was a false dependency: pinned `wDev_IndicateFrame+0xf4..0x10a` loads the
ten-bit CSI length from metadata bytes `0x26..0x27` into `s5`, and the
`wdev_csi_rx_process` call at `+0x298` is skipped when `s5` is zero. Because
the Rust copy plan already requires `csi_length == Some(0)`, callback
registration cannot affect any admitted frame. The hot path no longer reads
that global; nonzero CSI metadata still fails closed before pointer ownership
changes.

Hardware qualification of the corrected boundary routed 709/709 RX units
through Rust with zero vendor or indication fallback. The complete taskless
WPA2/network stress completed 4,096/4,096 UDP datagrams and 4/4 HTTP transfers
at 30.332 Mbit/s, balanced all 4,787 TX owners, and retained zero allocation,
ESF rejection, blocking, task-delay, and direct-delay observations.

## Open standalone legacy performance baseline (2026-07-29)

The fully open `open-radio-phy-prelude-hil` path now has a deliberately narrow
performance control before HT is enabled. `LegacyRate` owns the exact
non-monotonic S31 hardware codes recovered from the sibling esp-wifi-sys
header, `_oracles/libpp.a`, and the ROM `phy_rate_to_index` routine. The HIL
accepts `OPEN_RADIO_LEGACY_RATE_MBIT=6|9|12|18|24|36|48|54`; the default is
legacy OFDM 54 Mbit/s. Management and EAPOL remain at 1 Mbit/s. HT MCS values
cannot enter this descriptor path accidentally.

The first live run used `psram-code-psram-data --open-radio-hil`, channel 6,
20 MHz, WPA2, and the nearby FRITZ access point. Scan observed both HT and HE,
but the intentionally legacy association negotiated neither HT nor WMM. Open
PHY initialization, scan, authentication, association, WPA2, protected ARP,
DHCP, and the Embassy UDP sink all completed without vendor Wi-Fi code. DHCP
assigned `192.168.178.138`.

The UDP RX sink on port 4323 measured these preliminary host-to-device samples
with 1,400-byte datagrams:

| Host offered | Open sink payload | Socket errors | Network queue drops |
| ---: | ---: | ---: | ---: |
| 10.5 Mbit/s | 10.511 Mbit/s | 0 | 0 |
| 21.0 Mbit/s | 18.273 Mbit/s | 0 | 0 |
| 31.5 Mbit/s | 17.952 Mbit/s | 0 | 0 |

SOURCE: live serial records
`OPEN_RADIO_PHY_HIL result=BENCH stage=udp-rx` and the matching iperf2 client
summaries from the 2026-07-29 ESP32-S31 rev0 HIL. These figures establish a
legacy single-MPDU baseline, not an HT capability claim. The plateau occurs
with `dropped=0`, so the next comparison must sweep OFDM rates and inspect the
RX MAC/PHY duplicate/retry boundary before enabling the already recovered HT
PLCP and A-MPDU machinery.

That controlled OFDM sweep exposed a concrete standalone-TX transcription
error. With the same PSRAM/PSRAM image, channel-11 HT40- WPA2 peer, power
limit and 20-Hz ICMP load, fixed 6M and 24M each delivered 200/200 packets and
48M delivered 199/200. The former 54M image repeatedly lost 25 to 39 percent.
Complete `_oracles/libpp.a[hal_mac_tx.o]::{mac_tx_get_rts_rate,
mac_tx_set_len}` and the promoted migration `tx_rate.rs` prove that
`LENGTH_CONTROL` must carry basic rate 24M for both 48M and 54M; the standalone
path had incorrectly copied the data rate instead. After restoring the exact
selector and deriving the RTS power bytes from that basic rate, 54M delivered
197/200 and then 495/500 with 1.975-ms and 2.353-ms mean RTT. Linux decoded the
station at 54.0 Mbit/s. This removes the large legacy loss but does not hide
the remaining approximately one-percent retry/loss tail.

That tail led to the next complete-blob boundary. `libpp.a[trc.o]::rcGetRate`
does not repeat the selected data rate after every ACK timeout: it walks four
`(rate, count)` pairs using the descriptor's accumulated retry counters. The
54M 802.11g record sends `54M x2, 48M x2, 6M x3, 5.5M x25`. The promoted
migration had the same bounded four-entry selector, while the standalone app
reused 54M on all four permitted long-retry attempts. The app now asks the
driver's safe Rust schedule projection for each attempt; the original encoded
MPDU, Sequence Control and CCMP packet number remain owned and reused across
those retries. Hardware qualification of this correction follows separately.

## Open HE20 receive-rate boundary (2026-07-29)

The promoted vendor-exact one-stream HE capability negotiates successfully
with the same FRITZ peer. Hardware RX-control, decoded from the pinned
`esp_wifi_rxctrl_t` layout, reported format 4 (HE SU), rate field 11 and
HE-SIGA1 `0x00405b4b`. The HE-SIGA1 standard fields decode to MCS9, 20 MHz,
BSS color 27 and the longer GI/LTF selection. A direct post-CCMP,
post-decapsulation sink measured 61.230 to 65.450 Mbit/s with 1,200-byte UDP
payloads. Moving the descriptor, extraction, decapsulation and benchmark
leaves into internal SRAM removed `MAC_INT_RAW` bit `0x200` under load but did
not change that PHY-limited plateau.

One destructive capability experiment changed only the NSS1 RX/TX map from
MCS0-9 (`0xfffd`) to MCS0-11 (`0xfffe`). The AP accepted association and WPA2
messages 1 through 4, but after key installation the station received no
addressed protected frame (`addressed_protected=0`) and timed out. The
experimental claim was therefore removed; the vendor MCS9 image remains the
only qualified HE20 capability.

SOURCE[HIL_OPEN_HE20_RX_RATE_2026_07_29]: ESP32-S31 rev0,
`psram-code-psram-data --open-radio-hil`, serial `udp-rx-direct`,
RX-control histogram and WPA2 protected-RX counters. The old migration source
at commit parent of `f233006`,
`migration/esp32s31-hybrid-runtime/src/sta_link.rs`, independently labels the
captured vendor map as one-stream MCS0-9. Reaching 100–120 Mbit/s of useful
traffic therefore requires a qualified 40-MHz peer/channel (or a new
A-MSDU-capable receive path); it must not be obtained by claiming MCS11.

The subsequent all-channel active scan observed four 2.4-GHz BSSs. Every
record decoded to `ht40=None`, including the target FRITZ BSS on channel 6.
Its complete HT Operation IE begins `3d 16 06 08`: the secondary-channel
offset is zero and STA channel-width permission is clear, so selecting CBW40
against that BSS would violate its advertised operation. This is an
infrastructure boundary, not evidence that the recovered S31 CBW40 routine is
wrong.

The vendor HE constructor also confirms that HE40 is not the route used by
this firmware. Complete
`_oracles/libnet80211.a[ieee80211_he.o]::ieee80211_add_hecap` writes zero to
complete HE Capabilities IE byte 9 (first HE PHY Capabilities byte, Channel
Width Set) on both its STA branches: instructions `0x86` and `0xb2` in the
special branch, and `0x200` in the ordinary branch. The same ordinary branch
builds the captured MCS0-9 map at complete bytes 20 through 23. Consequently,
the open performance policy prefers the separately recovered HT40/short-GI
capability (150-Mbit/s one-stream PHY) whenever the peer's HT Capabilities and
HT Operation IEs permit a secondary channel; otherwise it retains the
qualified HE20/MCS9 mode.

SOURCE[HIL_OPEN_40MHZ_AVAILABILITY_2026_07_29]: complete 13-channel open scan
on ESP32-S31 rev0, plus instruction-exact disassembly of the pinned
`ieee80211_add_hecap` object and the IEEE-defined HT/HE capability fields.

## Open HT40 receive qualification (2026-07-29)

A controlled Linux/mac80211 AP supplied the missing 40-MHz peer boundary:
primary channel 11, secondary channel below, WPA2-PSK/CCMP and WMM. The open
scan decoded the complete HT Operation geometry as
`ht40=Some(Below)`. The channel transition consequently selected center
frequency 2452 MHz and S31 CBW value 3. Authentication, association, the
four-way handshake, pairwise/group hardware-key installation, protected ARP,
and the connected receive loop all ran without vendor Wi-Fi initialization or
vendor MAC/PHY calls.

The association capability needed one correction beyond the promoted HT20
image. Capability `0x0062` advertises static SMPS and was not accepted by the
controlled HT40 AP. The vendor constructor does not manufacture static SMPS:
complete `_oracles/libnet80211.a[ieee80211_ht.o]::
ieee80211_add_htcap_body` preserves the base capability at node offset
`0x14c`, then independently adds supported channel width (`+0x4e`), SGI20
(`+0x8e`) and SGI40 (`+0xaa`). Advertising SMPS disabled with capability
`0x006e` produced a status-zero association and WMM negotiation. WPA message
3 additionally required the RSNXE saved by the open scan to be passed into
GTK key-data validation; no empty synthetic RSNXE is used.

The qualified `psram-code-psram-data --open-radio-hil` run used 4,608-byte RX
DMA buffers, large enough for the negotiated 3,839-byte A-MSDU class plus the
S31 public header and trailer. The AP's RX A-MPDU request was accepted with a
16-frame hardware window. An iperf2 sender offered 180 Mbit/s in 1,472-byte
UDP datagrams; the host radio actually delivered 86,240 datagrams in
10.0055 seconds. The open direct sink received 86,238 datagrams and
126,942,336 payload bytes in 10.008449 seconds:

| Measurement | Result |
| --- | ---: |
| Open sink payload | 101.468 Mbit/s |
| Datagrams received/sent | 86,238 / 86,240 |
| Linux AP negotiated TX rate | 150.0 Mbit/s, MCS7, 40 MHz, short GI |
| Linux AP TX retries / failures | 2 / 0 |
| S31 RX control | format 2, rate 11 (maximum 11) |

This qualifies the requested 100–120-Mbit/s useful receive regime for the
fully open driver. The result is a direct post-CCMP/post-decapsulation sink;
it does not claim that the full Embassy socket path or device-to-host TX path
has the same ceiling.

The larger buffer is not a substitute for a general chained-unit owner. The
pre-promotion migration implementation at the parent of commit `f233006`,
`migration/esp32s31-hybrid-runtime/src/wdev.rs::process_rx_success`, records
that descriptor word-0 bit 30 marks the final descriptor of a complete RX
unit and that one unit can span multiple descriptors. The live ring must
retain this invariant if a future capability or buffer profile again permits
split units.

SOURCE[HIL_OPEN_HT40_RX_2026_07_29]: ESP32-S31 rev0 serial records
`sta-channel-select`, `sta-assoc-response`, `wpa2-protected-rx`,
`rx-addba-active`, `udp-rx-direct`, and `udp-rx-phy`; matching iperf2 output
and `iw dev wlan0 station dump`; pinned `libnet80211.a[ieee80211_ht.o]`;
promoted migration parent of `f233006`.

## Handshake RX-ring and controlled-port retry boundary (2026-07-29)

The open STA handshake must retain one live RX-ring owner across each bounded
wait. Repeatedly preparing a cold zero-terminated list at descriptor zero is
not a valid rearm operation after the walker has consumed a list:
`RX_LAST_DESCRIPTOR` retains the accepted tail. Before the correction,
authentication and association waits repeatedly stopped after exactly 64 or
32 received descriptors. With `RxRingLive::recycle_completed_half`, one
authentication wait processed 132 frames before its protocol retry, and an
association response was accepted at frame 33. Those observations cross both
old terminal frontiers and therefore distinguish a live append from a lucky
cold restart.

The recovered implementation matches complete rev0 ROM
`wDev_AppendRxBlocks`: detach a CPU-owned completed half, repair its
descriptor/buffer sentinel state, rotate the next head beyond the retained
tail, publish the accepted last descriptor and ring the RX reload doorbell.
The Rust ownership boundary is
`open-esp-radio-mac-esp32s31::rx::{RxRingStopped,RxRingLive}`; the application
only binds its static descriptor and buffer storage to that owner.

EAPOL M4 TX completion is also not the controlled-port boundary. It proves
that the AP acknowledged the EAPOL MPDU, while the promoted vendor STA path
delivers its connected event separately from the EAPOL callback. A 10-ms
post-M4 settling interval removed the observed burst of four immediate
status-5 protected-ARP transmissions.

Loss of the subsequent ordinary ARP transaction must not roll the WPA state
machine back to message 2. A bounded ARP retry now allocates a new 802.11
sequence number and a new CCMP packet number while retaining the installed
PTK/GTK. Five consecutive cold resets all reached IP-ready. The fifth run
exercised the retry boundary: ARP attempt 1 returned MAC status 5, attempt 2
returned status 0, its protected response was accepted, and no EAPOL message
2 was retransmitted.

SOURCE[HIL_OPEN_STA_HANDSHAKE_LIVE_RING_2026_07_29]: ESP32-S31 rev0,
`psram-code-psram-data --open-radio-hil`, five cold-reset serial records;
`crates/open-esp-radio-mac-esp32s31/src/rx.rs`; complete rev0 ROM
`wDev_AppendRxBlocks` and `hal_mac_rx_set_last_desc`; promoted migration
parent of `f233006`,
`migration/esp32s31-hybrid-runtime/src/wdev.rs::{prepare_rx_recycle_chain,
wDev_AppendRxBlocks}` and the separate STA EAPOL/connected callbacks; Linux
hostapd authorization state and `iw dev wlan0 station dump`.

The remaining authentication retry is before parsing or WPA. On a failed
first attempt, the directed request completes with hardware status zero, but
no addressed Authentication frame exists even in the raw DMA buffers. On the
following protocol attempt the response is the first completed descriptor,
with word zero `0xc0181200`, frame control `0x00b0`, and zero RX/internal
status bytes. Three destructive A/B checks did not change this boundary:
adding 100 us after RX enable, running an otherwise empty RX stop/rearm epoch,
and selecting genuinely different initial 12-bit transmit sequences. The
latter used seeds `1228`, `1558`, `1118`, `1062`, and `2689`; all five runs
still required the second Authentication request.

The unlocalized hardware difference is therefore retained as
`UNKNOWN[first-directed-TX-to-RX-turnaround]`, not assigned to a speculative
register. The open state machine performs the IEEE-level retry. An earlier
diagnostic policy bounded the first wait to 100 ms and later attempts to the
migration default of 500 ms. Five cold resets with that policy all reached
IP-ready, and the image retained 102.143 Mbit/s of useful HT40 UDP receive
throughput (86,805 of 86,807 offered 1,472-byte datagrams). These durations
were useful experiments, but they are not the vendor STA policy.

Complete pinned
`_oracles/libnet80211.a[ieee80211_sta.o]::ieee80211_sta_new_state` resolves
the production timer exactly. The ordinary non-mesh Authentication branch and
the Association branch each arm `0x3e8`, or 1,000 ms. Complete
`_oracles/libnet80211.a[wl_chm.o]::chm_phy_change_channel` performs
`pm_wake_up`, `ic_mac_deinit`, `phy_change_channel(channel, 1, 0, cbw)`,
`hal_mac_set_csi_cbw`, `ic_mac_init`, and `pm_wake_done`; neither this path
nor the complete Authentication-output path waits for a beacon before sending
the request. The open state machine now uses the exact 1,000-ms response
timeout.

Five reset-to-DHCP HIL repetitions with the exact timeout all reached
IP-ready, while only one of five received the first Authentication response.
This series also used the complete blob PTI rule: active Probe Request event 5
and Authentication/Association event 6 each map to packet PTI 1, while
`mac_tx_set_pti` selects unsigned `min(packet_pti, event_one_pti)`, producing
scheduler 1, PTI vector `0x00111110`, and Authentication queue configurations
`0x120003ff..0x120033ff`. In the other four runs, the first TX completion
reported status zero, the live RX ring received 92 to 142 unrelated frames
during the full second, and no raw Authentication response was present;
attempt two then succeeded. This rules out a merely late response, a stopped
RX walker, and the former PTI mismatch. It does not yet distinguish an
immediate TX-to-RX turnaround loss from the AP declining to produce the first
response.

The same A/B exposed an independent entropy error in the application
composition root. Plain `esp_hal::rng::Rng` produced the same cold-reset
stream, so it was unsuitable for the WPA supplicant nonce. The HIL binary and
the relocated runtime now retain the unique ESP32-S31 `RNG` peripheral in an
LP `TrngSource` owner and pass an entropy-qualified `Trng` capability into the
radio workload. The driver crates do not acquire an unrelated RNG peripheral;
nonce construction remains an explicit application dependency.

SOURCE[HIL_OPEN_STA_AUTH_TURNAROUND_AND_TRNG_2026_07_29]: raw and decoded
ESP32-S31 RX-descriptor serial records; five-run fixed-sequence,
TRNG-sequence, RX-prime, and 100-us RX-settle A/B series; five-run
100-ms-retry series and matching iperf2/`udp-rx-direct` record; five-run exact
1,000-ms Authentication-timeout series; complete pinned
`libnet80211.a[ieee80211_sta.o,wl_chm.o,ieee80211_output.o]`; promoted
`sta_link.rs::{authenticate_open,initialize_static_node}` experimental
three-by-500-ms policy; `esp-hal/src/rng/{mod.rs,trng.rs}` and the existing
ESP32-S31 `trng_kat` ownership qualification.
