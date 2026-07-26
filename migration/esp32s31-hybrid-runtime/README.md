# ESP32-S31 stackless Wi-Fi runtime

> Migration source only. This directory is not a workspace member and must
> not be linked by an application. It preserves the complete Rust driver
> workset that previously lived beside `esp-wifi-sys`, including temporary
> blob-interposition boundaries. Source-owned pieces are being moved into
> PAC/HAL/PHY and future MAC/security crates. Each compatibility path is
> deleted here after its source-only replacement is qualified.

This experimental crate replaces the infinite vendor `ppTask` loop with a
wake-driven Rust `Future`. It does not modify the vendor archives and does not
initialize Wi-Fi peripherals.

The current prototype provides:

- the recovered ESP32-S31 `ppTask` event map;
- direct calls to the original exported run-to-completion handlers;
- the required `pp_sig_cnt[36]` receive accounting;
- a fixed-capacity lock-free queue for task and ISR producers;
- a bounded `WifiRuntimeFuture` with no RTOS task or separate C stack;
- recognition of `ppTask` in an OSI task-create callback;
- an allocation-free OS timer bridge whose callbacks run in async context;
- non-blocking normal and recursive OSI mutexes;
- a stackless Enterprise EAP dispatcher replacing `wpa2_task`;
- an async Michael MIC countermeasure continuation replacing its 10 ms sleep;
- a generic fixed-capacity owned-data channel and ISR-to-async edge signal;
- an optional owned Wi-Fi event bridge replacing blocking ESP event posting;
- critical-section/core-stall reachability counters that preserve hardware semantics;
- a typed, fixed-capacity application command channel serviced by one radio owner;
- pinned owned crypto jobs with interrupt-driven async completion and cancellation;
- an async WPA-Personal PBKDF2/PMK precomputation job;
- fixed WPA2-Personal STA/AP state, M1-M4/GTK builders, and owned crypto jobs;
- a fail-fast WPA2 I/O command boundary with aligned CCMP keys and static key storage;
- finite WPA2 retransmission schedules driven only by one-shot alarm edges;
- an owned STA EAPOL TX-done bridge replacing the stock WPA state callback;
- an owned fixed-slot STA/AP data RX channel replacing arbitrary netstack callbacks;
- an owned fixed-slot application data TX channel with fail-fast static-pool submission;
- an S31 static-pool TX and pairwise-CCMP backend with stable opaque key objects;
- a strict future wrapper that fails on observed blocking, allocation, or core-stall paths;
- allocation-free recording of unexpected blocking calls.

The binary audit and exact dispatch table are in
[`../docs/esp32s31-async-runtime-audit.md`](../docs/esp32s31-async-runtime-audit.md).
Regenerate and validate them with:

```console
cargo +stable run -p xtask --bin analyze-esp32s31 -- --write
```

## Integration boundary

This crate intentionally does not own the complete OSI table. It patches the
runtime fields of a table supplied by the hardware integration and preserves
that table's PHY, clock source, allocator, NVS, and coexistence callbacks.

On ESP32-S31, `patch_pp_runtime_callbacks` patches these fields in an existing
complete `wifi_osi_funcs_t` supplied by the hardware integration:

- semaphore create/delete/take/give and thread semaphore;
- queue send/receive/count and Wi-Fi queue create/delete;
- task create/delete/delay/current-task identity;
- millisecond-to-tick conversion and maximum task priority.
- normal/recursive mutex create/delete/lock/unlock;
- event-group create/delete/set/clear and non-waiting state checks;
- OS timer set/arm/disarm/done and monotonic time access.

The PP queue and semaphores are fixed-capacity and allocation-free. Semaphore
take never waits: an unavailable semaphore with a non-zero timeout is recorded
as a blocking-path violation. Unknown task entries and generic queues are also
rejected and recorded.

```rust,ignore
patch_pp_runtime_callbacks(&mut wifi_osi_table);

fn now_us() -> u64 {
    // Read the application's monotonic hardware/executor clock.
}

fn rearm_alarm(deadline_us: Option<u64>) {
    // Program or cancel one non-blocking hardware/executor alarm.
}

let wifi = take_wifi_runtime(
    DEFAULT_EVENT_BUDGET,
    DEFAULT_EVENT_BUDGET,
    now_us,
    rearm_alarm,
)
.expect("the Wi-Fi runtime may only be taken once");
executor.spawn(wifi);
```

The alarm interrupt must call `timer_alarm_interrupt()`. It only wakes the
future; vendor timer callbacks never execute in interrupt context. After each
poll the runtime calls `rearm_alarm` with the nearest absolute microsecond
deadline, so there is no periodic polling.

Timer callbacks execute under the logical PP identity. This makes
`esp_wifi_ipc_internal()` take its inline path instead of posting an ioctl to
`ppTask` and synchronously waiting on a semaphore.

Stock PP events 6 and 7 are not accepted after the strict proof. Event 5 is
accepted only as the token for the Rust-owned net80211 TX queue: one executor
action lends exactly one frame to the remaining output compatibility stage.
The stock shared-list producer is replaced, so its loop cannot receive an
unbounded batch. Event 6's vendor ioctl envelope
contains an arbitrary callback, heap ownership flags, an optional completion
semaphore, and PM wake/sleep bookkeeping. Configuration, mode changes,
start/stop, and any API that would post such an ioctl must finish before
`prepare_strict_runtime`. The original event-7 producer allocates an eight-byte
timer envelope; its mandatory wrapper instead claims one of sixteen fixed
slots and posts a private one-action executor event. Only the `timer_connect`
success action is completed locally. `chm_dwell` and vendor
auth/assoc/handshake/reconnect/scan/beacon/hostap recovery timers can enter
synchronous MAC teardown or channel switching and therefore fail closed;
WPA2 retry timing remains Rust-owned. An event 5 without a reserved Rust queue
token and any unexpected stock event 6 or 7 fail immediately.

Every vendor handler must still return without waiting. NVS writes, direct
delays, logging, and application-facing synchronous Wi-Fi calls need
reachability instrumentation on hardware before the runtime is complete.

For a runtime that owns application event dispatch, install a static event
bridge before Wi-Fi initialization:

```rust,ignore
static EVENTS: WifiEventBridge<16, 1536> = WifiEventBridge::new();

unsafe { patch_async_event_post(&mut wifi_osi_table, &EVENTS) };

loop {
    let event = EVENTS.receive().await;
    // `base()` and `data()` are owned slices; no vendor pointer escapes.
    handle_wifi_event(event.base(), event.id(), event.data()).await;
}
```

The callback copies the payload and never honors the blob's infinite event
post timeout. Full or oversize events are rejected instead of blocking.

Application control calls can be serialized onto the same executor stack with
`RadioCommandQueue` and `RadioOwnerFuture`. The command handler is the only
application-facing code entered under virtual Wi-Fi task identity:

```rust,ignore
static COMMANDS: RadioCommandQueue<Command, 8> = RadioCommandQueue::new();

let wifi = take_wifi_runtime(/* ... */).unwrap();
let owner = RadioOwnerFuture::new(wifi, &COMMANDS, CommandHandler, 4);
executor.spawn(owner);

// From any other future or an ISR producer:
COMMANDS.try_submit(Command::Disconnect)?;
```

Commands are transferred by value. A full queue returns the original command;
it never turns into a hidden semaphore wait or a vendor call from the producer.
The producer claim is machine-code bounded as well. LLVM currently lowers both
strong and weak Rust `compare_exchange` on RV32 to an `lr.w`/`sc.w` retry
loop. The channel therefore uses a small inline `atomic_once` leaf which emits
one reservation and one store-conditional only. Reservation loss is returned
as ordinary contention with command ownership intact; `submit` waits for the
next Rust async capacity wake before trying again. The final-ELF auditor
rejects any control-flow cycle in the live monomorphized
`RadioCommandQueue::try_submit`.

Call `disable_vendor_nvs` before `esp_wifi_init` when the application owns
persistence. `disable_dynamic_wifi_buffers` also clears the explicit dynamic
RX/TX/cache counts, but the caller must still configure adequate static pools
and target-correct static buffer types.

`InterruptSignal` converts a hardware/DMA completion interrupt into a future
without running vendor code in the ISR. `patch_critical_section_probes` counts
Wi-Fi interrupt critical sections and other-core stalls. Before strict mode it
delegates to the hardware adapter; strict mode requires the executor to run on
`wifi_task_core_id`, switches the interrupt lock to local MIE masking, rejects
wrong-hart `pp_post`, and never stalls the second core.

`patch_allocator_probes` similarly wraps all OSI allocator slots and reports
counts, sizes, failures, and calls made from radio context. Direct C allocator
references in the vendor archives must additionally pass through the GNU
linker's wrappers. In the promoted application `wifi-primary` profile there
is no captured heap implementation: every qualified cold request is resolved
to an exact fixed owner, while any unmatched OSI, direct C, or Rust allocation
reaches an `ebreak` ABI sentinel. The allocating path remains only as the
explicit `wifi-vendor-strict-link` A/B oracle. TX/RX completion, WPA2 ingress,
and key programming also require symbol interposition. Ordinary archive
definitions use LLD wrapping:

```text
-Wl,--wrap=malloc
-Wl,--wrap=calloc
-Wl,--wrap=realloc
-Wl,--wrap=free
-Wl,--wrap=pp_post
-Wl,--wrap=ppTxPkt
-Wl,--wrap=ieee80211_timer_process
-Wl,--wrap=ieee80211_mgmt_output
-Wl,--wrap=ieee80211_hostapd_beacon_txcb
-Wl,--wrap=ic_get_next_tbtt
-Wl,--wrap=ieee80211_tx_mgt_cb
-Wl,--wrap=wDev_record_ftm_data
-Wl,--wrap=pm_on_coex_schm_status_config
-Wl,--wrap=pm_set_beacon_duration
-Wl,--wrap=cnx_check_bssid_in_blacklist
-Wl,--wrap=cnx_add_to_blacklist
-Wl,--wrap=cnx_remove_from_blacklist
-Wl,--wrap=cnx_clear_blacklist
-Wl,--wrap=wDev_ftm_set_t1t4
-Wl,--wrap=wDev_isNANPktInValidSlot
-Wl,--wrap=dbg_read_tx_ppdu
-Wl,--wrap=dbg_dump_rx_ppdu
-Wl,--wrap=dbg_dump_rx_sigb
-Wl,--wrap=wifi_gpio_debug
-Wl,--wrap=wDev_SnifferRxData
-Wl,--wrap=wdev_csi_rx_process
-Wl,--wrap=wpa_sm_rx_eapol
-Wl,--wrap=wpa_ap_rx_eapol
-Wl,--wrap=hal_crypto_set_key_entry
-Wl,--wrap=ieee80211_classify
-Wl,--wrap=ieee80211_search_node
-Wl,--wrap=cnx_node_search
-Wl,--wrap=vTaskDelay
-Wl,--wrap=os_sleep
-Wl,--wrap=sleep
-Wl,--wrap=usleep
-Wl,--wrap=wifi_log
-Wl,--wrap=wifi_assert
```

The following sixteen entries (`esf_buf_alloc`, `esf_buf_recycle`,
`ieee80211_set_tx_pti`, `lmacTxDone`, `hal_mac_get_txq_state`,
`hal_mac_get_txq_complete`,
`pm_on_beacon_rx`, `pm_on_data_rx`, `pm_on_data_tx`,
`esp_test_tx_enab_statistics`, `esp_test_set_rx_error_occurs`,
`rcUpdateTxDone`, `rcUpdateAckSnr`, `wDev_AppendRxBlocks`, `wDev_DiscardFrame`, and
`wDev_ProcessRxSucData`) are ECO0 ROM exports.
Do not pass them through
LLD `--wrap`: `esp-rom-sys` defines them with absolute linker-script
assignments, and LLD would rewrite the Rust `__wrap_*` definition itself to a
ROM address. Load `esp32s31-rom-wrap-overrides.x` after all `esp-rom-sys` ROM
fragments instead. It pins each original address under `__real_*` and aliases
the public entry to Rust without modifying ROM or a vendor archive.
In strict mode the RX recycle replacement prepares at most 64 descriptors,
publishes one chain under local interrupt masking, and returns immediately.
The MAC reload completion is observed once from a Rust one-shot timer
continuation; concurrently returned chains coalesce in fixed SRAM state.
`ppTxProtoProc` uses the same late fragment but is a complete replacement, not
a delegating probe: the fragment retains the unique
`wifi_strict_pp_tx_proto_proc` Rust symbol and aliases the public ROM name
directly to it. This avoids LLD creating an absolute `__wrap_ppTxProtoProc`
symbol at the ROM address.

`ppTxPkt` is an ordinary `libpp.a[pp.o]` archive function and therefore uses
GNU `--wrap`. Its strict replacement validates the adopted STA/AP interface
and descriptor queue, calls the Rust protocol/security/rate leaves, applies
the recovered finite Rust queue mapper, and appends the frame to the
Rust-owned logical queue registry. Strict handoff adopts only the four
vendor-initialized scheduler masks and cursors; all sixteen queue heads/tails
then have one single-hart Rust owner. `ppTxPkt`, `ppMapTxQueue`,
`ppDequeueTxQ`, `ic_interface_enabled`, `lmacIsIdle`, and
`pp_process_hmac_waiting_txq` are absent from the armed runtime path. The
TX-done completion links, callback masks, and the six admitted callback
identities are also adopted into a fixed Rust SRAM registry. The remaining
two per-queue PPDU-format values are adopted as opaque PLCP inputs in the TX
scheduler state. RX callback routing and the interrupt-to-executor intrusive
queue are adopted as well. Handoff redirects the recovered
`pp_wdev_funcs+0x1dc` `lmacRxDone` slot to an internal-SRAM Rust callback after
proving the vendor queue empty. RX readiness is the Rust queue itself, not a
fallible event entry; `RadioFuture` serves it round-robin with vendor and
internal events. Consequently no strict-runtime path uses `pTxRx`,
`lmacRxDone`, or `ppDequeueRxq_Locked`. `pTxRx` remains only a cold-init and
one-shot adoption source until the corresponding initialization is ported.
The reference strict STA HIL no longer registers an Embassy task waker for
this owner. It pins the one `RadioOwnerFuture` in SRAM and supplies a custom
SRAM `RawWaker` whose only action is one `FROM_CPU_INTR2` MMIO write. The
vtable, clone/wake/drop leaves, software-interrupt entry, task pointer, and
future storage are all in internal SRAM; the final-image auditor reads the
vtable bytes and requires those exact targets. This removes the shared
Embassy transfer-stack CAS retry from the hard RX interrupt closure. The first
hardware qualification completed WPA2, 4,096 UDP datagrams and four HTTP
transfers with balanced ownership, no queue rejects and no allocation delta at
27.477 Mbit/s.

ESP32-S31 has no hardware cache-enable status register; the upstream
`cache_ll.h` explicitly requires software-maintained enable/disable state.
The HIL therefore keeps a four-bit executor gate in internal SRAM:
`CACHE_AVAILABLE`, `POLLING`, `DEFERRED`, and `TERMINATED`. A cache owner calls
the SRAM `wifi_strict_radio_try_suspend_cached_executor` leaf before disabling
cache. It succeeds only after atomically closing the gate while no poll owns
it; failure is immediate and must be retried by an async wake, never a loop.
The SRAM `wifi_strict_radio_resume_cached_executor` leaf reopens the gate and
raises one software interrupt when a wake was deferred. The interrupt entry
acquires its poll lease with one AMO and returns without touching the cached
future when the gate is closed. Both leaves and the interrupt entry are
required in SRAM by the final-image audit.

The gate's deferred path is exercised deterministically at executor startup,
and a second hardware run completed the same WPA2/UDP/HTTP workload with
4,786/4,786 TX, 691/691 RX, 20,598/20,598 PP events, no rejects or allocation
delta, and 25.801 Mbit/s. This proves the gate state machine and preserves
normal operation; it does not claim that cache was physically disabled during
that run. Every future flash/cache owner must adopt this suspend/resume
contract before cache-disabled execution is considered globally proven.

Before the strict-runtime proof is issued, both OSI and direct-C wrappers
delegate to the original allocator so vendor initialization can complete.
The management-output wrapper accepts only ordinary AP/STA association,
authentication, and probe subtypes on the home channel. It checks live
non-mesh/non-NAN and AP no-power-save invariants before entering the stock
body; rejection recycles the fixed ESF slot immediately.
The adjacent TX-PTI wrapper does not call the OSI coexistence callback. For a
validated event below 48 it performs the pinned `coex_pti_tab[event]` byte read
directly; an invalid event marks the buffer so the following management-output
gate consumes and recycles it exactly once.
The basic-HT retry path likewise no longer enters `hal_mac_tx_set_ppdu` or its
indirect `mac_tx_set_pti` callback. Rust performs the bounded queue-control,
power-table and PTI orchestration, including the terminal PTI MMIO writes, and
reproduces the complete guarded RTS-rate mapping. That stage reduced the
boundary to three audited finite PLCP/HTSIG leaves. PLCP0 and its internal
TX-protection leaf are now Rust MMIO as well. PLCP1, HTSIG, and both length
registers are also Rust-owned, leaving no binary formatting leaf. A
5,025-completion HIL run exercised 233 same-frame retries and passed the strict
WPA2/UDP/HTTP workload at 29.829 Mbit/s with every heap, blocking and delay
probe at zero.
The low-level NAN-slot hook is also interposed: non-NAN AP/STA descriptors pass
the recovered bit test, while a NAN descriptor returns false without invoking
the optional scheduler callback.
Afterwards allocation returns null immediately and `free` is recorded without
entering the heap. Heap access can be restored only after the executor and all
Wi-Fi callbacks have stopped.

After installing both probes, `AuditedFuture` can enforce the observed runtime
boundary:

```rust,ignore
patch_allocator_probes(&mut wifi_osi_table);
patch_critical_section_probes(&mut wifi_osi_table);
let audit = StrictAudit::global(StrictPolicy::heap_free_single_owner());
executor.spawn(AuditedFuture::new(owner, audit));
```

This fails closed on the first recorded blocking callback, OSI allocation,
unbalanced interrupt restore, excessive nesting, or other-core stall. The
final-ELF audit also requires the four allocator wrappers,
`__wrap_lmacTxDone`, `__wrap_hal_mac_get_txq_state`, and the strict AP-beacon,
management-completion, FTM-rejection, PS-beacon, radio-debug, and WPA2 RX
wrappers, so omitting one of the linker arguments is a build failure.
The strict AP-beacon state aliases are supplied by an additional linker
fragment. WPA2 AP also needs the local audited join boundary:

```text
-Tesp-wifi-async-runtime-esp32s31/ld/esp32s31-net80211-locals.x
-Tesp-wifi-async-runtime-esp32s31/ld/esp32s31-wpa2-ap-locals.x
-Tesp-wifi-async-runtime-esp32s31/ld/esp32s31-wpa2-sta-locals.x
-Tesp-wifi-async-runtime-esp32s31/ld/esp32s31-rom-wrap-overrides.x
```

The production path patches the stable OSI table before
`esp_wifi_init_internal`. The task-create callback recognizes the exact pinned
`ppTask` entry, returns a logical task handle, and releases the vendor startup
latch without ever calling the entry point or creating an RTOS task. Unknown
task entries fail closed. Immediately after the vendor init call,
`drain_wifi_initialization_events(budget)` runs only the already-queued finite
handlers on the caller's stack. An empty queue completes immediately; budget
exhaustion, shutdown, or an unsupported handler is an error, never a wait or a
fallback to RTOS.

The `handoff` module remains solely for comparative HIL against the original
blob. It deliberately starts the real `ppTask`, so it is not an acceptable
production configuration and must not be used to claim taskless operation.

`derive_wpa2_sta_message2` is the first composed handshake transition. It
consumes the owned M1, awaits an `AsyncWpa2StaCrypto` PTK operation, awaits the
M2 HMAC-SHA1 operation, advances the ticketed STA state, and returns a complete
owned Ethernet/EAPOL M2 plus the persistent PTK. A hardware implementation of
the crypto trait must wake from completion IRQs; the transition contains no
heap use, delay, RTOS wait, or hardware-status loop. The returned frame still
has to be moved to the radio owner and the M3/key-install/M4 composition must
be completed before an on-air WPA2 connection can be claimed.

Runtime shutdown must likewise be
exposed as an async command that completes after `RadioFuture` drains its queue,
instead of calling that blocking wrapper directly. Call `request_shutdown()`
and then await completion of the spawned `WifiRuntimeFuture`; its final poll
cancels the external alarm. A subsequent vendor deinit may call
`pp_delete_task`: the adapter rejects its duplicate event 15, selecting the
blob's non-waiting `pp_delete_task_manually` cleanup path for the private task
and queue globals.

## WPA async features

Enable `wpa-async-eap` for the Enterprise EAP worker replacement,
`wpa-async-mic` for the Michael MIC callback replacement, or `wpa-async` for
both. The final firmware link must include the corresponding linker fragments:

```text
-Tesp-wifi-async-runtime-esp32s31/ld/esp32s31-eap-locals.x
-Tesp-wifi-async-runtime-esp32s31/ld/esp32s31-wpa-locals.x
```

These fragments do not patch `libwpa_supplicant.a`. They assign external alias
names to audited local function/data sections during the normal final link.
Runtime size checks reject a different local layout, while the analyzer checks
the complete archive digest and symbol sizes.

The normal WPA `eloop_run` path is finite: it processes due timeouts and
returns. It is serviced by `WifiRuntimeFuture` through the OSI timer bridge.

With `wpa-async-eap`, the adapter recognizes the exact generic queue created by
`esp_eap_client.c.obj` (three 8-byte entries) and the `wpa2_task` entry point.
No task or receive-loop is started. Messages 0 and 1 are put onto the existing
radio queue; message 1 processes one RX node per poll and schedules a
continuation if more nodes remain. Message 2 performs only the finite teardown
inline, before vendor deinit can free `gEapSm`. The former completion-semaphore
take for messages 0 and 1 now means async queue acceptance and never waits.

With `wpa-async-mic`, install the callback after supplicant initialization and
restore it before supplicant deinitialization:

```rust,ignore
unsafe { install_async_michael_callback()? };
// Wi-Fi operation; the second MIC failure now schedules a 10 ms continuation.
unsafe { uninstall_async_michael_callback()? };
```

The replacement keeps the original WPA states, key-request call, countermeasure
flag, and 60-second stop timeout. Only the direct `os_sleep(..., 10000, ...)` is
replaced by the shared executor-driven timer.

## Strict no-wait profile

Enable `strict-no-wait` to replace PP event 22 with executor continuations. The
original `lmacProcessTxTimeout` performs a 16-us busy delay for every active TX
queue, and its discard path contains two data-dependent loops. The replacement
preserves the two sides of the settling interval, scans at most one list link,
and discards at most one MSDU per executor event. Test-statistics/logging HAL
wrappers on that path are replaced by the pinned S31 TXQ MMIO fields.

The strict basic profile requires static pools and no aggregation:

```rust,ignore
configure_static_wifi_buffers(
    &mut config,
    StaticWifiBufferConfig {
        rx: 10,
        tx: 16,
        management_rx: 5,
    },
);
disable_vendor_nvs(&mut config);
disable_frame_aggregation(&mut config);
disable_ftm(&mut config);
validate_strict_basic_config(&config)?;
```

The generated S31 defaults are not strict: TX buffers are dynamic and the
static TX count is zero. After the cold-start drain and before the executor,
`prepare_strict_runtime_before_handoff(&config)` (legacy name) sets and verifies
`WIFI_PS_NONE` and `WIFI_LOG_NONE` through the initialization OS adapter.
That preparation also routes connection-time management frames into the fixed
Rust pool before the connection request, so no heap-owned authentication,
association, or probe frame can cross the strict boundary.
`prepare_strict_runtime(&config, preparation)` then does not call a vendor
control API; it refuses to issue a proof unless the `ppTask` entry was
virtualized (or the explicitly selected legacy HIL handoff completed) and
`patch_pp_runtime_callbacks`, `patch_allocator_probes`, and
`patch_critical_section_probes` were applied to the OSI table before Wi-Fi
init. It also compares the linked allocator, ESF, timer, TX/RX, and WPA symbol
addresses with all twenty-four required `__wrap_*` functions. Issuing the proof
arms the allocator and core-stall wrappers: runtime
allocation calls return null, runtime frees never enter the heap, and a
requested other-core stall is recorded but never entered.
`allow_heap_for_wifi_teardown` remains solely for the allocating vendor oracle;
the primary profile has no heap to reopen. It and
`allow_core_stalls_for_wifi_teardown` can be called only after the strict
executor has fully stopped. The proof token is required by the S31 backend. In
strict mode,
unexpected AMPDU, power-save, BSS-color, modem-beacon, or coexistence events
fail closed instead of entering vendor handlers with reachable delay paths.

This feature is not yet a claim that every remaining PP path is strict. Run the
fail-closed relocation audit while replacing them:

```console
cargo +stable run -p xtask --bin audit-strict-esp32s31 -- --enforce
cargo +stable run -p xtask --bin audit-strict-esp32s31 -- \
    --elf path/to/final-firmware.elf --enforce
cargo +stable run -p xtask --bin audit-strict-esp32s31 -- \
    --include-static-binding-init --include-static-pm-init \
    --elf path/to/final-firmware.elf --enforce
cargo +stable run -p xtask --bin audit-state-esp32s31 -- \
    --elf path/to/final-firmware.elf \
    --write docs/esp32s31-linked-state-audit.md
```

The archive audit rejects direct heap/delay/RTOS/flash/logging paths, every
unproven register-indirect call, and every unproven backward branch. The
final-ELF audit additionally rejects forbidden symbols and vendor entries that
the strict Rust dispatcher replaced. Direct allocator references are accepted
only when the corresponding `__wrap_malloc`, `__wrap_calloc`,
`__wrap_realloc`, or `__wrap_free` symbol is linked. `lmacTxDone` must resolve
through `__wrap_lmacTxDone`; its callback bitmap, inline `ppProcTxDone`/PM tail,
and TX-queue resume are then executor continuations.
Stock initialization and bypassed WPA objects keep a few forbidden symbol
definitions in the image. The final audit permits them only while every call
site remains in its pinned pre-handoff or dormant owner: a new caller of
`esp_event_post`, libc printing, or the vendor assert immediately fails the
audit.
The optional static-binding-init roots cover `net80211_data_ptr_init` and
`wdev_data_init`. These two pinned leaves only connect fixed archive storage
such as `gChmCxt`, `g_ic`, and `TxRxCxt` to the S31 ROM ABI cells; they add no
allocation, wait, indirect call, or control-flow cycle. The state audit joins
archive relocations with the final ELF to report the exact live mutable blob
objects, ROM ABI cells, Rust strict sections, wrappers, direct aliases, and
retained `__real_*` ROM oracles. Its generated snapshot is
[`../docs/esp32s31-linked-state-audit.md`](../docs/esp32s31-linked-state-audit.md).
The ownership model, reverse-engineering evidence, completed channel-manager
slice, and next state-migration priorities are maintained in
[`../docs/esp32s31-rust-ownership-migration.md`](../docs/esp32s31-rust-ownership-migration.md).
The two leaves contain exactly 43 pointer publications: 12 net80211 bindings
and 31 PP/WDEV bindings. Forty-two still select their pinned archive backing;
`g_txop_queue_status_ptr` selects the equivalent Rust-owned three-byte TXOP
pool. `bind_static_vendor_state` exposes the audited vendor leaves as a
serialized cold-init operation and immediately replaces that one publication,
while
`bind_static_vendor_state_in_rust` performs the same two ordered groups of
volatile stores without entering either vendor body. The
`rust-static-bindings-interpose` feature exports matching
`__wrap_net80211_data_ptr_init` and `__wrap_wdev_data_init` boundaries. The
heap-free primary profile uses those Rust publishers; the vendor-publisher
path remains an explicit A/B oracle. The surrounding initialization sequence
is still temporary vendor code; before handoff,
`prepare_strict_runtime_before_handoff` independently verifies all 43 cells
against their exact fixed backing objects.
The adjacent `pm_funcs_init` leaf previously obtained one zeroed 0x44-byte
callback table through the OSI allocator, published it through
`ptr_beacon_offset_funcs`, and called `pm_beacon_offset_funcs_init`. Its
matching deinitializer freed the same pointer. The
`rust-static-pm-init-interpose` feature replaces that ownership pair with a
fixed internal-SRAM table: `__wrap_pm_funcs_init` clears and publishes the
table before calling only the separately audited 17-store callback publisher,
and `__wrap_pm_funcs_deinit` withdraws the pointer without entering `free`.
`static_pm_functions_bound` verifies the live publication before strict
handoff. This removes one cold-init allocation; it does not yet replace the
surrounding vendor initialization sequence.
Even with vendor persistence disabled, `misc_nvs_init` unconditionally
allocates and clears a 0x3c-byte settings block before observing
`nvs_enable == 0`. In that branch it performs no NVS operation; the linked
consumers only use its WPS type/status words at offsets 4 and 8. The
`rust-static-misc-nvs-init-interpose` feature replaces the init/deinit pair
with a zeroed fixed internal-SRAM block and direct `g_misc_nvs` publication.
`static_misc_nvs_bound` verifies that publication. This replacement is valid
only for the non-persistent configuration established by
`disable_vendor_nvs`.
The surrounding `wifi_init_in_caller_task` previously obtained its interrupt
lock token and two mutexes through three indirect OSI create callbacks. The
`rust-static-wifi-init-interpose` feature publishes three dedicated Rust
objects instead and directly sequences only `wifi_menuconfig_init`, the static
misc-NVS boundary, the taskless PP boundary, and `ieee80211_ioctl_init`. It
also selects the hardware-qualified static lower-MAC RX arena. The independent
`rust-static-esf-buffer-init` boundary remains HIL-only while its reconnect
lifetime is diagnosed.
The menuconfig and ioctl leaves independently pass the strict archive audit.
The two fixed mutexes fail immediately on ownership contention; they never
wait or scan the general mutex pool. `static_wifi_init_locks_bound` verifies
all three ROM-ABI publication cells.
The next `ic_create_wifi_task` leaf is only a tail call into
`pp_create_task`. That vendor envelope creates a queue and startup semaphore,
invokes the OSI task-create callback, takes the semaphore with an infinite
timeout, inserts a one-tick task delay, and deletes the semaphore. The
`rust-static-pp-task-init-interpose` feature bypasses the whole envelope:
`__wrap_pp_create_task` directly publishes the existing fixed Rust queue,
`xphyQueue`, and logical `PP_TASK_HANDLE`; it enters no OSI callback and never
calls `ppTask`. `__wrap_pp_delete_task` clears those cells only when no radio
future or queued work is live, otherwise it fails immediately. The
`static_pp_task_bound` check proves all five queue/task/semaphore publication
cells before handoff.
The ROM `is_ndpa_to_dut` HE user-info scan is retained rather than pretending
that a link wrapper can intercept a ROM-to-ROM call. Its sole backward branch
walks four-byte frame records with a counter narrowed to `u8`; including the
zero/wrap case, it executes at most 256 times and never polls hardware or
external state. Its logging call is still consumed by the mandatory
`wifi_log` wrapper.
The ROM-resident `wDev_IndicateFrame` cannot be truthfully interposed with GNU
`--wrap`, so its qualified singleton body is reproduced directly in SRAM
Rust. The event-25 continuation first follows the completed segment under a
short local interrupt mask, checks every payload, and admits only a final
marker reached in at most 64 links. Qualified status-zero/base-offset,
zero-CSI, single-descriptor STA data, association responses, beacons,
authentication frames, and guarded Action frames then bypass both the vendor
aggregate classifier and the ROM indication leaf.

The Rust leaf decodes descriptor bits 0..13 as capacity and bits 14..27 as
actual received length. It claims kind 7 from the fixed Rust large-RX pool or
kind 8 from the initialized finite small-RX free list, performs the recovered
bounded copy and descriptor stores, recycles the hardware descriptor, and
publishes the new ESF owner directly through `wifi_strict_lmac_rx_done`.
Exhaustion returns immediately and discards the input unit; it never enters a
dynamic allocator or waits. Probe Requests in the STA-only profile take their
recovered direct-discard route. Base-layout copy-mode-zero MPDUs spanning two
or more hardware descriptors are now joined by the same Rust boundary. Safe
Rust validates a maximum of 64 links and the 14-bit (`0x3fff`) ESF length,
then a finite pointer leaf copies into one of two fixed aggregate owners. Each
owner keeps its 0x90-byte intrusive header and ownership word in internal
SRAM, while its 16-KiB payload is in initialized PSRAM and is never touched by
the hard RX ISR. Reorder and network ownership use a distinct aggregate slot
ID range, so those rare owners do not inflate the ordinary kind-7 RX credit.
Copy-mode-one multi frames preserve the recovered immediate-discard behavior.
CSI, extended metadata, optional-sublength multi frames, and other
unqualified variants retain an explicit ROM fallback.

The qualifying WPA2/network HIL validated 715/715 RX units. Rust indicated
699 data and 13 management frames, including three Action frames, discarded
three Probe Requests, and observed zero indication rejects, ROM indication
fallbacks, or vendor aggregate fallbacks. The full 4,096 UDP plus four HTTP
workload balanced all 4,795 TX and 695 network RX owners. The primary
firmware also compiles out unsupported vendor benchmark-statistics calls, so
the allocation probe remained exactly zero for allocations, failed attempts,
reallocations, and frees.

The multi-descriptor image passed 274 host tests and the final 6,407-function
strict audit with 25 roots and zero violations. The configured HIL image
placed the 32-KiB aggregate payload arena at `0x50001038` in PSRAM and only
296 aggregate header/ownership bytes in ISR-visible SRAM. Its full WPA2 STA
stress run completed 4,096/4,096 UDP datagrams and 4/4 HTTP transfers at
26.343 Mbit/s, balanced 4,787/4,787 TX and 692/692 RX owners, and retained
zero allocator calls and ESF rejects. Normal Ethernet MTU traffic observed
`max_descriptors=1`, so this run proves regression and placement invariants;
the multi copy itself is presently qualified by pinned disassembly, safe host
copy-plan tests, and target code generation rather than an observed jumbo
MPDU.
`hal_mac_get_txq_state` must resolve through its wrapper as well: completion
and collision handlers receive one bitmap bit per event, while the wrapper
posts another event for a captured remainder. `hal_mac_get_txq_complete` is
also a late ROM alias: its wrapper performs only the fixed basic-HT register
decode and traps on HE, BAR, A-MPDU, or live MPLEN state before the vendor
caller can misinterpret an unsupported record. Event 23 itself is dispatched
in Rust one queue at a time and uses a direct outcome `match`. The
HIL feature exposes `lmac_tx_complete_snapshot()` so TX stress can prove the
queue-kind, TXOP/list, and descriptor invariants without allocating or changing
the selected outcome. The proven basic success/recycle path now runs in Rust
and feeds the bounded Rust TX-done continuations. Basic ACK and CTS timeout
accounting, rate fallback, retry-limit/lifetime decisions, and terminal
discard also run in Rust. The guarded one-frame `lmacRetryTxFrame` submission
leaf plus the unobserved RTS-error and generic TX-error outcomes remain strict
audit roots. It is expected to fail
until all reported roots are replaced or their exact indirect target and loop
bound are proven.

TXOP admission and release are now Rust-owned as well. The recovered vendor
state is exactly three availability bytes initialized to `[1, 1, 1]`; request
takes the first non-zero slot and stores its index at hardware-queue offset
`0x1d`, while release restores the slot and writes the sentinel `3`. Safe Rust
owns that finite transform and the persistent bytes. The ABI adapters validate
the four hardware queues and trap on invalid pointers or class values. Late
link aliases replace both `lmacRequestTxopQueue` and
`lmacReleaseTxopQueue`, including their WDEV callback-table addresses, so the
old bodies and private `g_txop_queue_status` object are absent from the primary
ELF.

The next blocking boundary is the contents of classified TX callbacks and the
remaining TX/RX completion handlers, not event 22. The strict timeout/discard
path no longer calls `lmacTxDone`: its mode-1 bitmap is advanced one known bit
per executor event, the frame is appended to the TX-done list without the
vendor callback loop, and event 16 is posted normally. Strict event 16 no
longer calls `ppProcTxDone`: its Rust continuation dequeues one frame, invokes at most one
directly classified mode-0 callback, or recycles one fixed-pool frame per event.
It rejects unknown callback bits, a registered user TX callback, fragmented or
trace descriptors, and frame types outside the strict fixed pools. The original unbounded queue
drain and its power-management tail are therefore absent from this path.

The mode-1 replacement accepts only the STA EAPOL bit and checks that its
registered pointer matches the pinned vendor symbol. Off-channel rate-probe
and AP power-save callbacks fail before invocation. It also rejects
aggregation/direct-recycle descriptors and optional TX-time recording. The
classified basic callback roots are audited separately. AP beacon completion
now only rearms the fixed timer and rejects mesh/power-save branches.
Management completion is also Rust-owned: authentication, association, probe,
and ordinary beacon subtypes complete without vendor side effects, while
disassociation, deauthentication, and action/off-channel subtypes poison the
strict dispatcher before their hidden node/key/channel state machines can run.
Those three operations need explicit executor commands before they can be
supported. Other RX/retry/connection paths still prevent certifying basic
AP/STA for the stated zero-allocation/zero-wait requirement.

The ordinary STA/AP `ieee80211_classify` leaf is also Rust-owned. Its finite
port classifies EAPOL/WAPI, STA ARP, DHCP/DNS, IPv4, IPv6, multicast, and WMM
admission-control traffic. The recovered admission graph is expressed with an
explicit three-transition/four-state bound, and the fixed-per-packet-rate descriptor bit
is written directly without entering PP/TRC code. Direct archive references
use GNU wrapping, but the ESP32-S31 ROM output path calls callback-table slot
`net80211_funcs+0x24`; strict handoff adopts that slot only when it still
contains the pinned vendor classifier or the Rust replacement, then verifies
both paths. JTAG inspection of the running final image confirmed that the ROM
slot contained `__wrap_ieee80211_classify`.

The WPA2-CCMP key-selection and header leaf is also Rust-owned. Strict handoff
adopts `net80211_funcs+0x44`, and the replacement accepts only a fixed
Rust-owned key object with the pinned CCMP layout. It chooses the pairwise or
group hardware index, advances the recovered 48-bit packet number by three,
and inserts the exact eight-byte CCMP header. It does not read the
`g_ic+0x148` software-key slots and does not call an indirect cipher callback.
Because `ieee80211_crypto_encap` is an absolute S31 ROM export, the supplied
linker override aliases that public name to a unique Rust symbol rather than
using GNU `--wrap`.

The adjacent `ieee80211_align_eb` leaf is Rust-owned as well. It reserves only
the recovered 24-byte legacy or 26-byte QoS header, validates the packed
14-bit MPDU length, moves the frame by an alignment delta of at most three
bytes, and commits the ESF pointer/length word only after validation. Its absolute S31
ROM export is routed through the same direct-alias mechanism; invalid context,
header geometry, or length traps instead of delegating after strict handoff.

## Remaining WPA scope

The active secured scope is WPA2-Personal AP/STA. WPA3 SAE is not being
reimplemented: this S31 build advertises `WIFI_ENABLE_WPA3_SAE = 0`, its
`sae.c.obj` and `esp_wpa3.c.obj` contain no usable high-level SAE engine, and
`esp_supplicant_init` leaves the net80211 SAE callback slots empty. The
net80211 glue symbols are ABI-pinned only so a future blob update cannot make
that conclusion stale silently.

The EAP leaf handlers still execute to completion and may perform synchronous
cryptography or allocation inside the vendor blob. The removed waits are the
RTOS queue receive-loop and completion semaphore. Hardware reachability tests
are still required for EAP methods and failure paths used by a product.
Accordingly `strict-no-wait` and `wpa-async-eap` are compile-time incompatible.

The pinned stock `sta_eapol_txdone_cb` slot normally resolves to `eapol_txcb`
(`0x182`), whose reachable state transitions contain `calloc/free` for eloop
timeouts and a deauthenticate path to `ets_delay_us`. Strict integration must
call `install_async_wpa2_sta_tx_done` after setup. TX completion validates the
owned QoS/optional-CCMP/LLC MPDU directly and copies only M2/M4 message kind,
replay counter, and status into a fixed channel; it never calls the vendor
connection-manager callback. The registered callback identity remains a
handoff invariant and is restored only under exclusive teardown ownership.
Installation is permitted during serialized initialization before
IRQ/executor start, and `prepare_strict_runtime` refuses to issue its proof
until it is complete.

The Rust scanner and association state machine also own reconnect candidate
selection. Four mandatory final-link wrappers therefore make the vendor
allocation-backed BSSID blacklist unreachable. Lookup always reports no
vendor entry and add/remove/clear are no-ops; reconnect policy remains in the
fixed-capacity Rust scan records.

`Wpa2Ingress` is connected directly to the pinned STA and AP RX callbacks by
`--wrap=wpa_sm_rx_eapol` and `--wrap=wpa_ap_rx_eapol`. Both callbacks validate
an exact RSN EAPOL-Key length, copy it into the global eight-frame
`OwnedEapolFrame` channel, and return immediately. `receive_wpa2_eapol` is
producer-woken and performs no timer polling; invalid input or capacity
exhaustion is counted by `rejected_wpa2_eapol` and reported to the blob as not
consumed. AP peer MAC is copied from the pinned station-object field at offset
eight. The parser exposes descriptor flags, replay counter, nonce, MIC, and key
data and classifies pairwise messages 1 through 4 plus group messages.

Before AP start, `install_async_wpa2_ap_callbacks(&rsn_ie)` replaces the
complete heap-owning AP callback group: hostap init/deinit, station join/remove,
RSN-IE lookup, and peer SPP lookup. The supplied IE must be WPA2-PSK with CCMP;
SAE/OWE, TKIP, PMF-required, PMKID, and group-management extensions fail
closed. The replacement keeps the advertised IE and eight pinned `0x28`
station objects in static Rust storage. A successful join copies an
`Associated` event into the producer-woken channel returned by
`receive_wpa2_ap_event`; removal emits `Removed`. Capacity exhaustion sends an
association failure immediately and never waits.

The association response does not enter `esp_send_assoc_resp`, its temporary
`calloc`, or the allocating ioctl wrapper. It calls only the pinned
association-response constructor, fixed management-buffer pool, descriptor
setup, and management output for subtype `0x10`/`0x30`. TX power-management
accounting is interposed as a no-op under the already verified
`WIFI_PS_NONE` invariant. Install the callbacks after `esp_supplicant_init`
created `wpa_cb`, while AP RX is still stopped, then start AP and issue the
strict-runtime proof. The stock hostap authenticator and its eloop rekey timers
are never created.

For STA, `install_async_wpa2_sta_callbacks()` replaces the connected,
disconnected, and four-way-handshake-query slots in the same `wpa_cb` table.
The two notifications copy only BSSID/reason metadata into the fixed channel
returned by `receive_wpa2_sta_link_event`; they never enter the stock
disconnect path, whose eloop timeout cancellation can end in `free`.
`set_wpa2_sta_handshake_active` makes the Rust handshake owner responsible for
the query result. Install these callbacks after `esp_supplicant_init`, before
STA RX, automatic reconnect, or the executor runtime can run.

`Wpa2StaState` and `Wpa2ApState` now consume those owned frames one at a time.
They validate role, peer, descriptor version, message order, nonces, replay
counters, and async completion tickets. The STA acknowledges a repeated M3
after completion without reinstalling PTK/GTK. The AP uses `Wpa2ApPeers<P>` for
a fixed peer limit and authorizes a station only after M2/M4 MIC completion and
pairwise-key installation. Every transition emits at most one owned
crypto/install/TX action and returns.

`start_wpa2_ap_handshake`, `complete_wpa2_ap_message2`,
`complete_wpa2_ap_pairwise_key_install`, and `complete_wpa2_ap_message4` close
the allocation-free AP orchestration boundary. They turn owned ingress into an
owned M1, pairwise-key install, MIC-protected M3, and controlled-port command,
respectively. RFC 3394 wrapping is exposed through `AsyncWpa2KeyWrap`; the
bounded software backend performs finite CPU work and the trait also permits an
interrupt-completed hardware implementation. The application must still keep
the returned install and transmit commands in FIFO order and own a fixed peer
session table.

PTK PRF-384, HMAC-SHA1 EAPOL MIC, and RFC 3394 key unwrap have fixed
`CryptoJob` constructors. EAPOL MIC input is copied with the MIC field cleared,
MIC comparison is constant-time, and the persistent 48-byte CCMP PTK wipes KCK,
KEK, and TK on drop. `CryptoFuture` reads peripheral completion status only
after an interrupt-generation change; unrelated executor polls do not poll the
hardware.

`Wpa2TxFrame` now builds exact CCMP M1-M4 frames, `Wpa2PlainKeyData` builds the
RSN IE plus GTK KDE for RFC 3394 wrapping, and `parse_gtk_key_data` validates
the corresponding plaintext after unwrap. `build_sta_action_frame` and
`build_ap_action_frame` bind transmit actions to the state machine's peer and
nonce context. Plain GTK/key-data, PTK, and aligned install-key objects wipe
their secret storage on drop.

`Wpa2IoQueue` moves complete Ethernet frames, CCMP key installs, and peer
authorization commands by value to the single radio owner. `TryWpa2Io` permits
one immediate submission attempt only and returns the complete command on
backpressure. The mandatory `hal_crypto_set_key_entry` wrapper writes the
pinned fixed key-table MMIO directly for keys up to the hardware maximum of 32
bytes, so pointer alignment can no longer select the vendor temporary
malloc/copy/free path. `AlignedCcmpKey` still keeps the Rust-owned key ABI
stable, and `StaticWpa2Keys<P>` retains a fixed number of STA/AP keys.

`Wpa2Retry` turns an original M1-M4 transmit action into a finite sequence of
generation-tagged one-shot alarms. An alarm event emits at most one
retransmission and one next absolute deadline. Cancellation invalidates an
already pending IRQ; there is no sleep, periodic tick, hardware-status read, or
catch-up loop.

On the target, `S31StaticWpa2Io<K>` bypasses `esp_wifi_internal_tx`, because
that wrapper always acquires `g_wifi_global_lock` through an OSI callback. The
strict path instead performs bounded peer lookup, one fixed-pool buffer claim,
descriptor setup, and `ieee80211_post_hmac_tx` directly. Pool exhaustion and
ordinary AP power-save transmissions fail without entering the vendor queue.
Bufferable AP ADDBA responses use a bounded management exception: a sleeping
peer transfers the nine-byte action body into one of eight Rust-owned slots,
the original ESF is recycled, and the same `RadioOwnerFuture` resumes only on a
peer-bound Active/PS-Poll/removal edge. The continuation reconstructs a fresh
fixed-pool frame, reproduces the bounded AP/action header and descriptor tail,
runs the recovered finite TIM bitmap update in Rust, and enters `ic_tx_pkt`
directly. It never calls `ieee80211_set_tim`, links into
`ieee80211_pwrsave` or `pwrsave_flushq`, re-enters
`ieee80211_mgmt_output`, or changes the peer's live power-save flag. Deferred
AP data uses the same finite Rust TIM leaf. A failed data post poisons the
backend until Wi-Fi deinit
rather than retrying an ambiguously owned buffer. A STA transmission is
rejected until the async EAPOL TX-done callback is active.
Pairwise and group CCMP installation bypass both stock allocating wrappers: the
hardware leaf receives an aligned key and net80211 receives a pinned `0xb8`
software-key object from `S31StaticKeyStorage<K>`. Before registration the
backend reads the pinned `g_ic` key slot directly, verifies that it is empty or
already points to the same static object, and then performs the exact pointer
store itself; neither `ieee80211_get_key` nor the potentially freeing
`ieee80211_set_key` remains a runtime root. AP PTK/SPP lookup uses a mandatory
finite `cnx_node_search` wrapper over the nine statically provisioned AP/BSS
node entries; the original eight-bit wraparound/assert loop is unreachable.
The adjacent `ieee80211_search_node` wrapper accepts STA/AP only and rejects
NAN before its assert loop. STA GTK metadata is reduced to pinned byte
loads/stores. The pinned `wifi_init_key` call is reduced to its exact two
constant-length fills inside that object, so it is no longer a vendor runtime
root. STA GTK ids share the chip's single hardware group slot and update
the pinned two-byte metadata mapping directly; AP GTK ids use fixed hardware
slots 8 through 11.

The pinned blob exports no independent AP controlled-port setter. The backend
therefore owns authorization in a fixed peer table: authorization succeeds only
after an AP pairwise key exists, deauthorization is immediate, and
`is_ap_peer_authorized` is the mandatory gate for every ordinary Rust-owned AP
data-channel enqueue/transmit. EAPOL ingress remains a separate pre-auth channel.

Application Ethernet traffic uses `try_send_wifi_data` and
`receive_wifi_data_tx`. Both directions own one of eight fixed 1600-byte slots;
there is no borrowed netstack lifetime and no allocation on backpressure. The
single radio owner submits a received TX guard with
`S31StaticWpa2Io::try_transmit_wifi_data`. STA uses the same one-attempt static
buffer path as EAPOL. AP additionally checks the destination against the live
controlled-port table at submission time, so a queued frame cannot retain
authorization after peer removal.

Ordinary data TX exposes one hardware credit even though the application pool
contains eight slots. The credit is reserved while copying the Ethernet frame,
committed to the exact ESF frame address after successful radio submission,
and released only by the matching hardware TX-done edge. The release wakes the
network executor. This keeps temporary vendor descriptor exhaustion out of the
radio-owner error path and provides interrupt-driven async backpressure without
polling, delay, or retry loops.

`wifi_data_tx_snapshot()` and `wifi_data_rx_snapshot()` expose cumulative
claims, queue transfers, releases, rejection reasons, current ownership, and
the occupied-slot high-water mark without allocating or locking. A quiescent
TX/RX boundary must satisfy `claimed == released + occupied` and
`enqueued == dequeued + queued`; after TX drains it additionally requires
`hardware_committed == hardware_released` and a free hardware credit.
`RadioQueue::snapshot()` and
`RadioCommandQueue::snapshot()` provide the corresponding bounded scheduler
and command-queue watermarks for load qualification.

Promiscuous/sniffer RX is outside this basic profile. Strict mode rejects event
13 unconditionally before `ppProcessRxPktHdr`: that vendor handler not only
invokes the optional `pTxRx + 0x404` callback, but also releases its payload and
envelope through the OSI heap hook.

```rust,ignore
static KEY_STORAGE: S31StaticKeyStorage<4> = S31StaticKeyStorage::new();

// patch_pp_runtime_callbacks(&mut osi) and patch_allocator_probes(&mut osi)
// must run before Wi-Fi init.
// After esp_supplicant_init, before AP start/RX:
unsafe { install_async_wpa2_ap_callbacks(ap_rsn_ie)? };
unsafe { install_async_wpa2_sta_callbacks()? };
unsafe { install_async_wpa2_sta_tx_done()? };
unsafe { install_async_wifi_data_rx()? };
// Complete the finite vendor controls after the cold-start drain.
let preparation = unsafe { prepare_strict_runtime_before_handoff(&config)? };
// The proof accepts only a virtualized ppTask (or the separate legacy HIL path).
let proof = unsafe { prepare_strict_runtime(&config, preparation)? };
// Then under the single radio owner:
let io = unsafe { S31StaticWpa2Io::new(&KEY_STORAGE, &proof)? };
let handler = Wpa2IoHandler::new(io);
```

The target backend covers static-pool submission, pairwise/group CCMP, and a
Rust-owned AP controlled-port gate. On-air taskless STA qualification now
covers scan, authentication, association, M1-M4, ADDBA, DHCP, DNS, TCP/HTTP,
and sustained UDP through the Rust TX/RX pools. This runtime evidence does not
replace the final-ELF audit: an integration must still retain every required
wrapper and SRAM section and must enforce the controlled-port gate for every
non-EAPOL frame.

The crypto callbacks embedded in `wifi_init_config_t` are replaceable but have
a strictly synchronous ABI. They can select Rust or hardware crypto, but cannot
return pending. Fully async Enterprise crypto requires moving the EAP/TLS state
machine to Rust; see
[`../docs/esp32s31-async-boundaries.md`](../docs/esp32s31-async-boundaries.md).

For Rust-owned protocol work, `InterruptCryptoEngine` provides the actual async
boundary. A pinned `CryptoJob` owns all buffers until completion; the hardware
backend arms DMA and returns, while its ISR calls only
`InterruptSignal::notify_from_isr`. Dropping the future aborts the backend and
the job wipes key, IV, input, and output storage on drop.

The expensive WPA-Personal derivation is available directly:

```rust,ignore
let mut job = WpaPskJob::wpa_psk(passphrase, ssid)?;
// Every poll performs up to 32 useful HMACs and then cooperatively requeues
// itself. It does not poll a peripheral or wait on an RTOS object.
job.derive_software::<32>().await?;
let pmk: &[u8] = job.result()?; // 32-byte PMK, ready before connect

// Execute this command through RadioOwnerFuture after supplicant init.
unsafe { install_precomputed_wpa_pmk(&job)? };
```

The software future owns fixed intermediate `U`/XOR blocks, wipes them on
completion or cancellation, and matches the standard WPA `password`/`IEEE`
test vector. An IRQ-capable SHA-1 backend may instead execute the same
`CryptoJob` through `InterruptCryptoEngine`; the S31 SHA work queue is not used
as an async substitute because its SHA-1 path recalls itself to read busy
status rather than receiving a completion interrupt.
