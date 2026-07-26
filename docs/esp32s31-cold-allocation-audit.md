# ESP32-S31 cold Wi-Fi allocation audit

## Current primary profile

The project's default `wifi-sta` workload now selects `wifi-primary`, the
qualified static cold-init path. It links neither the `esp-alloc` crate nor a
heap arena. The application-level `wifi` feature is retained only for
allocating vendor examples such as the standalone scan and AP oracle.

The dependency and final-ELF audits reject `esp-alloc`, `EspHeap`, TLSF heap
state and heap sections in the primary image. Allocator-shaped ABI symbols
must remain because `esp-radio` and the closed archives expose those function
slots. They are not allocators: they own no storage or metadata, allocation
and non-null deallocation enter `ebreak`, and the free-space query returns
zero. The forked `esp-radio/no-heap` mode leaves its otherwise
allocation-backed fallback RX queues at const zero capacity because the
strict Rust runtime installs its own fixed-capacity RX/TX owners before radio
start.

The promoted commands need no feature argument:

```text
cargo xtask app build wifi-sta
cargo xtask app run wifi-sta --port /dev/ttyACM0
```

Use `wifi-vendor-strict-link` only as an explicit allocating A/B oracle. It
must not be combined with `wifi-primary` or
`wifi-rust-static-cold-init-hil`.

On 2026-07-25 the no-allocator primary image passed the complete final-link
audit (6,407 functions, zero no-wait/no-heap violations) and retained 22,648
bytes of CPU0 stack against the 16,384-byte gate. Hardware completed cold
initialization without entering the heap trap, then passive scan, open
authentication, association, WPA2 M1-M4, DHCP, ping, DNS, TCP, HTTP and the
strict stress run. The run transferred 5,734,400 UDP payload bytes and four
HTTP responses; TX/RX/PP ownership balanced and the allocation snapshot
remained exactly zero in every field.

## Historical allocation inventory

This inventory covers the taskless strict STA cold-start path through
`prepare_strict_runtime`. It was captured on hardware with
`hil-cold-allocation-trace`, after the direct Rust
`wifi_init_in_caller_task` replacement was enabled. The fixed trace itself
uses a fixed laboratory-only PSRAM journal and performs no allocation. The
journal is never accessed by an interrupt handler or after cold handoff, so it
does not consume the IRQ-critical SRAM arena.

The original allocating oracle run observed 115 allocations, no reallocations, 24 frees and
128,984 requested bytes. The allocation count and byte count remained
unchanged through scan, open authentication, association, WPA2 M1-M4,
network traffic, teardown and a second complete connection. The blocking
probe remained zero and no allocation ran in radio context.

After the qualified static owners and direct API boundaries documented below,
the primary image performs zero allocations, reallocations or frees and
requests zero heap bytes. It now also contains no allocator implementation or
heap arena, so this is no longer merely an unused-bootstrap-heap observation.

Addresses below are expressed as a function-relative return offset so that
the audit does not depend on application link layout. `esf_buf_alloc_dynamic`
is a ROM implementation address and is identified by its ROM symbol rather
than a link-time offset.

| Allocator source | Returning function | Offset | Size | Count | Bytes |
| --- | --- | ---: | ---: | ---: | ---: |
| `OsiCallocInternal` | `pm_funcs_init` | `+0x18` | 68 | 1 | 68 |
| `OsiCallocInternal` | `wdev_funcs_init` | `+0x34` | 1,560 | 1 | 1,560 |
| `OsiCallocInternal` | `net80211_funcs_init` | `+0x30` | 332 | 1 | 332 |
| `OsiWifiZalloc` | `esp_wifi_init_internal` | `+0xd6` | 24 | 1 | 24 |
| `OsiWifiZalloc` | `wifi_nvs_cfg_init` | `+0x46` | 4,628 | 1 | 4,628 |
| `OsiMallocInternal` | `wifi_nvs_load` | `+0x28` | 1,024 | 1 | 1,024 |
| `OsiWifiZalloc` | `trc_init` | `+0x1e` | 152 | 1 | 152 |
| `OsiWifiZalloc` | `trc_init` | `+0x32` | 152 | 1 | 152 |
| `OsiWifiZalloc` | `trc_init` | `+0x42` | 152 | 1 | 152 |
| `OsiMallocInternal` | `pp_attach` | `+0x4a` | 40 | 4 | 160 |
| `OsiWifiMalloc` | ROM `esf_buf_alloc_dynamic` | ROM | 648 | 8 | 5,184 |
| `OsiMallocInternal` | ROM `esf_buf_alloc_dynamic` | ROM | 1,748 | 32 | 55,936 |
| `OsiMallocInternal` | ROM `esf_buf_alloc_dynamic` | ROM | 788 | 2 | 1,576 |
| `OsiZallocInternal` | `wDev_Rxbuf_Init` | `+0x36` | 384 | 1 | 384 |
| `OsiMallocInternal` | `wDev_Rxbuf_Init` | `+0x108` | 1,704 | 32 | 54,528 |
| `OsiCallocInternal` | `pm_extend_tbtt_adaptive_attach` | `+0x22` | 4 | 1 | 4 |
| `OsiWifiZalloc` | `esp_wifi_set_mode` | `+0x1e` | 24 | 3 | 72 |
| direct `calloc` | `esp_supplicant_init` | `+0x26` | 108 | 1 | 108 |
| `OsiWifiZalloc` | `esp_wifi_register_mgmt_frame_internal` | `+0x22` | 24 | 1 | 24 |
| `OsiWifiZalloc` | `esp_wifi_internal_reg_rxcb` | `+0x28` | 24 | 4 | 96 |
| `OsiWifiZalloc` | `esp_wifi_set_country` | `+0x1e` | 24 | 1 | 24 |
| `OsiWifiZalloc` | `esp_wifi_set_ps` | `+0x2c` | 24 | 2 | 48 |
| `OsiWifiMalloc` | `esp_wifi_stop` | `+0x2e` | 24 | 1 | 24 |
| `OsiWifiZalloc` | `esp_wifi_set_config` | `+0x2e` | 208 | 2 | 416 |
| `OsiWifiZalloc` | `esp_wifi_set_protocols` | `+0x144` | 24 | 2 | 48 |
| `OsiWifiZalloc` | `esp_wifi_start` | `+0x1a` | 24 | 1 | 24 |
| `OsiWifiZalloc` | `wifi_create_sta` | `+0x30` | 612 | 1 | 612 |
| `OsiWifiZalloc` | `wifi_create_sta` | `+0x6e` | 1,296 | 1 | 1,296 |
| `OsiWifiZalloc` | `ieee80211_setup_ratetable` | `+0x26` | 212 | 1 | 212 |
| direct `calloc` | `pmksa_cache_init` | `+0x16` | 20 | 1 | 20 |
| `OsiWifiZalloc` | `esp_wifi_ipc_internal` | `+0x34` | 24 | 2 | 48 |
| `OsiWifiZalloc` | `esp_wifi_set_max_tx_power` | `+0x40` | 24 | 1 | 24 |
| `OsiWifiZalloc` | `esp_wifi_set_promiscuous` | `+0x4a` | 24 | 1 | 24 |

## Replacement order

The five `wDev_Rxbuf_Init` and ROM ESF pool classes account for 75 calls and
117,608 bytes, or 91.2 percent of all requested memory. They are persistent,
bounded buffer arrays and are the first replacement target.

`rust-static-rx-buffer-init` now supplies the pinned `wDev_Rxbuf_Init`
descriptor arena and payload buffers from 16-byte-aligned internal SRAM. It
accepts only the audited allocator source, exact return offsets and exact
1,704-byte payload size. The descriptor count is bounded to the qualified 32;
the pool can be expanded only after more cold heap storage has been removed.
Teardown recognizes and releases only exact pool addresses. The vendor function still
performs the finite descriptor construction and hardware list publication.

Hardware qualification of this first replacement produced the exact expected
delta:

- allocator calls: 115 to 82;
- requested heap bytes: 128,984 to 74,072;
- `wDev_Rxbuf_Init` heap sites: both absent from the trace;
- first and second scan/association/WPA2/network cycles: successful;
- post-handoff allocations, reallocations and allocation failures: zero;
- radio-context allocator calls and blocking-probe hits: zero.

The test image reduced its temporary bootstrap heap from 128 KiB to 80 KiB.
It used 72,292 bytes and retained 9,628 bytes after handoff, proving that the
new SRAM pool replaced rather than duplicated the old heap ownership.

The explicit `rust-static-esf-buffer-init` boundary supplies
the three ROM ESF classes from separate 16-byte-aligned internal-SRAM arrays.
Admission requires the fixed ECO0 implementation return address `0x2f832460`
plus the exact allocator source and size pair observed above. Each class has
its exact observed capacity, and teardown recognizes only aligned slot bases
in those arrays. An unexpected ROM revision, source, size or extra request
therefore falls back to the traced bootstrap allocator instead of aliasing
storage.

Static ESF storage reduces cold allocation from 82 calls / 74,072 requested
bytes to 40 calls / 11,376 requested bytes. The apparent reconnect-time
`g_phyFuns` corruption was not an ESF lifetime failure. A JTAG write
watchpoint stopped in the ROM `memcpy` called by the Rust WPA2 continuation:
the generated `strict_wpa2_m1_m2_with_security` poll frame was 10,288 bytes,
while the 24 KiB bootstrap heap plus the new static sections left CPU0 only
about 10 KiB of stack. Its local copy crossed `_stack_end_cpu0` and overwrote
the adjacent vendor BSS.

Reducing the cold-only heap to 16 KiB leaves 18,328 bytes of CPU0 stack while
the measured post-init heap occupancy is 9,428 bytes. A post-link audit now
rejects static-ESF images with less than 16 KiB of CPU0 stack. Two independent
cold boots, each containing scan, WPA2 association, network traffic, teardown
and a second complete connection, then passed with unchanged allocation
counters, zero blocking-probe hits and an intact PHY function-table pointer.
A subsequent six-connection run completed five teardown/reconnect boundaries
with post-link traffic after every handshake. Channel work stayed balanced,
all TX/RX owners returned to their pools and the same allocation and PHY
snapshots remained intact. The application static cold-init profile therefore
now includes the ESF boundary; its former explicit feature name remains only
as a compatibility alias.

The next qualified boundary supplies the `wifi_nvs_cfg_init` descriptor table
and `wifi_nvs_load` scratch page from separate 16-byte-aligned internal-SRAM
owners. Admission is exact: `OsiWifiZalloc`, 4,628 bytes and
`wifi_nvs_cfg_init + 0x46` for the 89-by-52-byte table; or
`OsiMallocInternal`, 1,024 bytes and `wifi_nvs_cfg_init + 0x13b6` for the
serialized load page. The first owner lives until `wifi_nvs_deinit`; the
second is released inside `wifi_nvs_load`. Every other source, size or caller
falls through to the ordinary cold allocator.

This reduces cold allocation to 38 calls / 5,724 requested bytes and the
largest remaining request to 1,560 bytes. An 8 KiB bootstrap heap measured
4,796 bytes used / 3,396 free and leaves 20,848 bytes of CPU0 stack. Two cold
boots passed; the second completed six WPA2 scan/auth/association/handshake
and post-link cycles with five teardown/reconnect boundaries, unchanged
allocation counters, balanced channel work, an intact PHY function-table
pointer and fully returned TX/RX owners. The application static cold-init
profile now includes this boundary.

The numerous remaining 24-byte entries are API command envelopes and should
disappear when the upper initialization/configuration state machine is called
directly instead of being replaced by generic allocator exceptions.

The `wdev_funcs_init` and `net80211_funcs_init` callback tables are the next
qualified persistent owners. `rust-static-function-table-storage` admits only
`OsiCallocInternal` requests of exactly 1,560 bytes returning at
`wdev_funcs_init + 0x34`, or exactly 332 bytes returning at
`net80211_funcs_init + 0x30`. Both owners are separately 16-byte aligned in
internal SRAM, claimed with one non-retrying CAS, zeroed before publication,
and recognized by exact base address during teardown. The vendor constructors
remain finite direct function-pointer stores; their corresponding deinitializers
free the same published bases.

This reduces cold allocation to 36 calls / 3,832 requested bytes and the
largest request to 1,296 bytes. The unchanged 8 KiB bootstrap heap measured
2,896 bytes used / 5,296 free. A two-cycle A/B run and a promoted six-cycle
run both completed scan, authentication, association, WPA2 M1-M4 and post-link
traffic without changing the allocation snapshot. The six-cycle run ended
with 70/70 channel switches, 55/55 TX owners and 57/57 RX owners, no queue
rejection, no allocation failure, no radio-context allocator call and an
unchanged PHY function-table pointer. The application static cold-init profile
now includes this boundary; its explicit feature name is retained only as a
compatibility alias.

The existing fixed 212-byte `ieee80211_setup_ratetable` scratch owner is now
also admitted during cold init, not only after heap lockout. The admission
still requires `OsiWifiZalloc`, the exact size and the pinned
`ieee80211_setup_ratetable + 0x26` return address. The vendor leaf serializes
use and frees the scratch before returning; the Rust release path wipes it and
clears its one-shot claim without retrying.

This removes one allocation and its matching free: the qualified two-cycle
run measured 35 allocations, 22 frees and 3,620 requested bytes, with the
largest request unchanged at 1,296 bytes. Both WPA2 connections and post-link
traffic completed with an unchanged runtime snapshot, zero allocation failure
and zero radio-context allocator calls. Heap occupancy remains 2,896 bytes
because the former heap scratch was already transient.

The next boundary supplies the two allocations made by `wifi_create_sta` or
`wifi_create_softap`. `rust-static-interface-storage` recognizes only
`OsiWifiZalloc` with these pinned caller and size pairs:

- `wifi_create_sta + 0x30` or `wifi_create_softap + 0x32`, 612 bytes, for the
  interface state;
- `wifi_create_sta + 0x6e` or `wifi_create_softap + 0x6e`, 1,296 bytes, for
  the interface PHY state.

The interface state has a 624-byte physical internal-SRAM reservation so its
612-byte logical range can start and remain 16-byte aligned. The PHY owner is
an independently aligned 1,296-byte internal-SRAM reservation. Each owner is
claimed with one non-retrying CAS, zeroed before publication and released only
for its exact base address. The blob destroy paths first clear the published
global interface pointer, then free the PHY state and finally the interface
state; the Rust release path wipes each exact owner and clears its claim.

There is deliberately one shared pair of owners. It supports the qualified STA
or softAP modes, but not simultaneous APSTA construction: an unexpected second
claim falls through to the cold allocator and is therefore visible instead of
silently aliasing live state. Strict heap-free APSTA remains unsupported until
separate per-interface lifetime evidence is available.

The two-cycle A/B qualification removed exactly two allocations and 1,908
requested bytes. The promoted six-cycle run measured 33 allocations, 22 frees,
1,712 requested bytes and a 208-byte largest request. The 8 KiB bootstrap heap
retained only 980 bytes after handoff and CPU0 retained 17,016 bytes of stack.
All six scan/authentication/association/WPA2/network cycles completed with an
unchanged allocation snapshot, zero allocation failures and zero
radio-context allocator calls. The final pool snapshot showed 56/56 TX owners
and 58/58 RX owners returned, with no queue rejection. One TX ADDBA response
timed out during the run and the asynchronous state machine recovered without
affecting association, WPA2 or post-link traffic. The application static
cold-init profile now includes this boundary; its explicit feature name is a
compatibility alias.

The remaining `esp_wifi_stop + 0x2e` command was not merely an allocation
site. The pinned public wrapper can retry stop phase one up to 500 times,
calling the registered OSI delay for 10 ms between attempts. The qualified
cold-start caller invokes it only while `g_ic + 0x1f5` is state 1, before the
radio has started; the vendor stop process maps that state to
`ESP_ERR_WIFI_NOT_STARTED` and the public wrapper converts it to success.

`rust-direct-cold-stop` replaces this narrow pre-start use with one volatile
state read. States 0 and 1 return success immediately. State 2 or greater is
rejected and counted rather than entering the vendor active-stop body. A
running radio must later be stopped by an explicit Rust asynchronous lifecycle,
not through this synchronous compatibility ABI. The final ELF audit requires
the wrapper to contain exactly one byte load, no call and no control-flow
cycle, and rejects an image which still links `esp_wifi_stop` or
`__real_esp_wifi_stop`.

Hardware observed one call in state 1, one pre-start success and zero active
rejections. The expected allocation delta was exact: 33 to 32 calls, 22 to 21
frees and 1,712 to 1,688 requested bytes. Two complete WPA2 scan,
authentication, association, handshake and post-link cycles then passed with
the allocation snapshot unchanged, 22/22 TX owners and 20/20 RX owners
returned, and no queue rejection.

The public `esp_wifi_set_mode` wrapper is another allocation-only API
envelope. It first calls `wifi_init_completed`, allocates a 24-byte request,
writes the requested mode at byte offset 8, and dispatches
`wifi_set_mode_process`. The pinned process reads that one byte and performs
the existing finite mode transition; it does not consume any other request
field.

`rust-direct-set-mode` preserves that initialization check and calls the
vendor process with a stack-resident request of exact size 24 and alignment 4.
Only byte offset 8 is initialized. The remaining bytes deliberately stay
uninitialized because the reviewed process does not read them. An initial
implementation cleared the whole request, but the compiler lowered that
clear to a ROM `memset`; the ELF audit detected the unexpected third call and
the implementation was changed to `MaybeUninit`.

The final ELF audit rejects linked `esp_wifi_set_mode` or
`__real_esp_wifi_set_mode` envelopes. It requires the direct wrapper's only
calls to be `wifi_init_completed` followed by `wifi_set_mode_process`, and
rejects a control-flow cycle. Hardware observed three calls, zero
not-initialized returns, mode 1 and result 0. The exact allocation delta was
32 to 29 calls, 21 to 18 frees and 1,688 to 1,616 requested bytes. Two
complete scan, authentication, association, WPA2 handshake and post-link
network cycles passed with no runtime allocation delta, allocation failure,
radio-context allocator call or TX/RX owner leak.

The public `esp_wifi_set_ps` wrapper has the same 24-byte allocation and ioctl
envelope. It first checks initialization and the public power-save type range
`0..=2`, then writes only byte offset 8. The pinned `wifi_set_ps_process`
reads that byte, calls `pm_set_sleep_type` once and returns success. The
reviewed power-management path contains no wait or retry loop; timer changes
use the already patched nonblocking OSI timer table.

`rust-direct-set-ps` preserves both public checks and invokes that process with
the same stack-resident `MaybeUninit` request used by direct set-mode. The ELF
audit rejects linked `esp_wifi_set_ps` or `__real_esp_wifi_set_ps` envelopes,
requires the wrapper's only calls to be `wifi_init_completed` followed by
`wifi_set_ps_process`, and proves the process contains exactly one direct call
to `pm_set_sleep_type` and no control-flow cycle.

Hardware observed two calls, zero not-initialized or invalid-argument returns,
last type 0 and result 0. The allocation delta was exact: 29 to 27 calls, 18
to 16 frees and 1,616 to 1,568 requested bytes. Two scan, authentication,
association, WPA2 handshake and post-link data cycles passed with an unchanged
runtime allocation snapshot. The first also completed ping, DNS, TCP and HTTP;
the second returned 22/22 TX and 20/20 RX owners without queue rejection.

`esp_wifi_internal_reg_rxcb` contributed four more 24-byte allocation and
ioctl envelopes. Its request contains only the interface byte at offset 8 and
the callback word at offset 12. The pinned `wifi_set_rxcb_process` dispatches
interfaces 0, 1 and 2 to `wifi_sta_reg_rxcb`, `wifi_ap_reg_rxcb` and
`wifi_nan_reg_rxcb`; each target leaf is exactly one callback-pointer store
followed by return.

`rust-direct-reg-rxcb` preserves the initialization and interface-range checks,
constructs those two request fields on stack and calls the finite dispatcher
directly. The ELF audit rejects the original public envelope, verifies the
wrapper's exact two calls, disassembles the process over its complete sized
symbol range so local assembler labels cannot hide AP or NAN branches, and
requires all three target leaves to contain exactly one store and no call or
control-flow cycle.

The allocation delta was exact: 27 to 23 calls, 16 to 12 frees and 1,568 to
1,472 requested bytes. Two complete scan, authentication, association, WPA2
handshake and post-link data cycles passed with the allocation snapshot
unchanged. The first also completed ping, DNS, TCP and HTTP. The second
returned 22/22 TX and 21/21 RX owners without queue rejection.

`esp_wifi_register_mgmt_frame_internal` was the next single 24-byte ioctl
envelope. Its public body checks `wifi_init_completed`, writes the first
argument at request word offset 12 and the second at word offset 20, and
dispatches `wifi_register_mgmt_frame`. That process is a finite publication
leaf: it loads exactly those two words, stores them at `g_ic + 0x27c` and
`g_ic + 0x280`, then returns zero. It contains no callback invocation,
critical section, wait, cycle or dynamic dispatch.

`rust-direct-reg-mgmt-frame` preserves the initialization error and builds
only the two proven request fields in a stack-resident `MaybeUninit` owner.
The ELF audit rejects the original and `__real_` envelopes, permits only
`wifi_init_completed` and `wifi_register_mgmt_frame` calls from the wrapper,
and requires the vendor process to remain exactly two loads, two stores,
return, no calls and no control-flow cycle.

Hardware produced the exact expected delta: 23 to 22 allocations, 12 to 11
frees and 1,472 to 1,448 requested bytes. Two complete passive-scan, open
authentication, association and WPA2 four-way-handshake cycles passed with
post-link data and no TX/RX rejection. The first also completed ping, DNS,
TCP and HTTP. Allocation counters remained unchanged after handoff, and the
strict whole-ELF no-wait/no-heap audit reported zero violations.

`esp_wifi_set_max_tx_power` adds one 24-byte envelope after checking
initialization, radio state at least STARTED and the public power range
8 through 84. Its request contains only the signed power byte at offset 8.
The pinned `wifi_set_max_tpw` process calls ROM `phy_set_most_tpw`, then
`hal_init_tx_pwr`. The latter has one bounded table loop: a counter starts at
zero, increments once per iteration and exits at exactly 43 entries. It then
calls the three finite TB, immediate-response and TB-RU table leaves. There
is no hardware-status poll, retry, delay or wait.

`rust-direct-set-max-tx-power` preserves all three public guards and invokes
the process with the one proven stack request byte. The ELF audit rejects the
original envelope, checks both process calls by exact address and requires
the lower table builder to retain its exact four call targets, one `0..43`
counter loop and final tail call. Linker relaxation to direct `jal` or tail
`j` is decoded explicitly rather than admitted as an unknown transfer.

Hardware produced the exact delta from 22 to 21 allocations, 11 to 10 frees
and 1,448 to 1,424 requested bytes. Two passive-scan, authentication,
association and WPA2 cycles completed with an unchanged allocation snapshot.
The first completed ping, DNS, TCP and HTTP; after the second, 21/21 TX and
18/18 RX owners had returned with no rejection.

Vendor NVS is not part of the target architecture. `nvs_enable=0` remains a
mandatory initialization invariant, and credentials, country and other
configuration belong to Rust-owned fixed state. The complete vendor
`esp_wifi_set_country` process is therefore not reused: it can reach
`wifi_nvs_set`/`wifi_nvs_commit` and, for an active radio, synchronous
stop/start leaves.

`rust-direct-set-country-nvs-free` accepts only the pre-start state and
disabled-NVS configuration. It calls the finite vendor regdomain lookup to
validate the requested channel window, then publishes the validated country
fields directly into the fixed Wi-Fi configuration owner. Its one reviewed
helper loop increments an 8-bit index through the finite regdomain table. The
wrapper cannot call the vendor country process, NVS, PHY-update, lifecycle or
ioctl paths; the ELF audit also rejects the original and `__real_` public
envelopes.

Hardware produced the exact delta from 21 to 20 allocations, 10 to 9 frees
and 1,424 to 1,400 requested bytes. Two passive-scan, authentication,
association and WPA2 cycles completed with the allocation snapshot unchanged.
The first completed ping, DNS, TCP and HTTP; the second returned 21/21 TX and
19/19 RX owners without queue rejection. The strict whole-ELF no-wait/no-heap
audit reported zero violations.

`esp_wifi_set_config` allocates a 208-byte command for every invocation. It
copies the complete 184-byte STA/AP/NAN union at offset 20, then enters
`ieee80211_ioctl`. The strict STA cold path invokes it twice, accounting for
the two largest remaining API-envelope allocations and 416 requested bytes.

`rust-direct-set-config` instead owns one zeroed, four-byte-aligned 208-byte
command on the caller stack, copies the configuration into that owner and
invokes the pinned run-to-completion `wifi_set_config_process` directly. The
wrapper retains the public initialization, interface and null-pointer guards.
Its qualified ELF has a 240-byte total stack frame, no control-flow cycle and
only `wifi_init_completed`, ROM `memset`/`memcpy` and the process call. The
original and `__real_` envelopes are rejected, as are allocator and
`ieee80211_ioctl` calls from the wrapper.

This is an allocation boundary, not the final configuration-ownership
boundary. The process still publishes into `g_wifi_nvs` and reaches the
disabled-NVS accessor leaves. Those fixed-state writes must be mapped and
moved behind the Rust radio owner before the vendor process and NVS vocabulary
can be removed from the target graph.

Hardware produced the exact delta from 20 to 18 allocations, 9 to 7 frees and
1,400 to 984 requested bytes. Two complete passive-scan, authentication,
association and WPA2 reconnect cycles passed with the allocation snapshot
unchanged, zero failures and zero radio-context allocator calls. The first
completed ping, DNS, TCP and HTTP; the second returned 21/21 TX and 19/19 RX
owners without rejection. The strict whole-ELF no-wait/no-heap audit reported
zero violations.

`esp_wifi_set_protocols` adds another 24-byte allocation and
`ieee80211_ioctl` transaction per interface update. Its process is not a
suitable direct leaf: it can synchronously stop and restart an active
interface and persists protocol fields through `wifi_nvs_get`,
`wifi_nvs_set` and `wifi_nvs_commit`.

`rust-direct-set-protocols-nvs-free` validates the S31 two-band input, reduces
the 2.4 GHz bitmap to the pinned primary PHY mode plus LR flag and publishes
those fields directly into the fixed vendor configuration and interface
state. Here `g_wifi_nvs` is only the vendor name for that in-memory
configuration block; no persistent NVS function is reachable. The pinned S31
HAL capability callback is a constant AX-enabled fact, so the wrapper encodes
that target fact and retains no indirect OSI call.

The HAL applies the same station configuration once before start and once
after start to refresh rate-control state. The wrapper therefore accepts an
active request only when all already-published protocol fields are identical;
that second call is a pure no-op. A real active-radio protocol change remains
rejected for an explicit Rust async lifecycle to sequence. A pre-start change
updates the live interface and invokes `ieee80211_protocol_attach` only when
the interface object already exists.

The final ELF audit requires a separately retained, call-free and acyclic Rust
bitmap selector. The wrapper's exact calls are `wifi_init_completed`, that
selector and `ieee80211_protocol_attach`; process, ioctl, NVS, stop/start,
`pp_post` and allocator calls are forbidden. The remaining protocol-attach
dispatcher is checked by exact PHY/HT/HE call and tail-call sets and may not
contain an internal cycle. Chained compiler return funnels are accepted only
when every transfer is an unconditional jump ending at `ret`; a unit test
still rejects an actual jump cycle.

Hardware produced the exact delta from 18 to 16 allocations, 7 to 5 frees and
984 to 936 requested bytes. Two complete passive-scan, authentication,
association and WPA2 reconnect cycles passed with an unchanged allocation
snapshot, zero failures and zero radio-context allocator calls. The first
completed ping, DNS, TCP and HTTP; both completed the Rust WPA2 handshake and
post-link data path. The strict whole-ELF no-wait/no-heap audit reported zero
violations.

The strict handoff also calls `esp_wifi_set_promiscuous(false)` once and reads
the state back before arming the ordinary AP/STA RX roots. The public setter
allocates a 24-byte ioctl command even when promiscuous mode is already
disabled. Its process cannot be admitted as a synchronous leaf: a real state
change calls `wifi_hw_start` or `wifi_hw_stop` and reconfigures the virtual
interface through `ic_set_vif`.

`rust-direct-promiscuous-idempotent` preserves the initialization guard and
reads the exact `g_ic + 0x1f7` control byte used by the public getter and
process. It succeeds only when the requested boolean already equals that
byte. Any actual transition, including a malformed control value, returns
`ESP_ERR_WIFI_STATE`; changing optional RX mode belongs to an explicit Rust
async lifecycle rather than this compatibility ABI.

The final ELF audit rejects the original and `__real_` public envelopes. The
wrapper must call only `wifi_init_completed`, contain no indirect transfer or
cycle and cannot reach the vendor process, ioctl, hardware start/stop,
`ic_set_vif`, `pp_post` or any allocator.

Hardware produced the exact delta from 16 to 15 allocations, 5 to 4 frees and
936 to 912 requested bytes. Two complete passive-scan, authentication,
association and WPA2 reconnect cycles passed with an unchanged allocation
snapshot, zero failures and zero radio-context allocator calls. The first
completed ping, DNS, TCP and HTTP; after the second, 22/22 TX and 20/20 RX
owners had returned without rejection. The strict whole-ELF no-wait/no-heap
audit reported zero violations.

The remaining two 24-byte `esp_wifi_ipc_internal + 0x34` allocations came
from the two HAL applications of `esp_wifi_set_inactive_time`. The public API
uses IPC mode 1: its request remains owned by the caller and
`wifi_ipc_process` invokes the callback before returning. The callback is
`esp_wifi_set_inactive_time_local`; it validates interface, timeout and Wi-Fi
mode, writes one timeout halfword into the live STA/AP interface and another
into `g_wifi_nvs`, then tail-calls `wifi_nvs_set`.

`rust-direct-set-inactive-time-nvs-free` preserves the initialization and
started-state guards plus the pinned timeout rules: STA requires more than
two seconds, AP more than nine, and the requested interface must be enabled
by the current STA/AP/APSTA mode. It publishes the same two RAM halfwords and
omits only the persistence tail. A null fixed configuration or interface
owner fails closed. No generic function pointer, IPC command, ioctl, NVS
operation, post, allocator or wait is entered.

The final ELF audit rejects the original public envelope, requires the wrapper
to call exactly `wifi_init_completed` and a separately retained Rust selector,
and proves that selector call-free and acyclic. Calls to
`esp_wifi_ipc_internal`, `wifi_ipc_process`, `ieee80211_ioctl`,
`wifi_nvs_set`, `wifi_nvs_commit`, `pp_post` and every allocator are forbidden
from the wrapper. The blob still contributes some of those symbols for other
linked features; symbol presence alone is not treated as execution.

Hardware produced the exact expected delta from 15 to 13 allocations, 4 to 2
frees and 912 to 864 requested bytes. The first strict WPA2 cycle completed
ping, DNS, TCP and HTTP. Teardown, a second passive scan, authentication,
association and WPA2 M1-M4 also completed with the allocation snapshot fixed
at 13/2/864, zero failures and zero radio-context allocator calls. The strict
whole-ELF no-wait/no-heap audit again reported zero violations.

The independently qualified `rust-static-pm-init-interpose` boundary is now
part of the standard static cold-init profile rather than an isolated A/B
feature. It replaces the persistent 68-byte `pm_funcs_init` allocation with
one exact-size internal-SRAM object. Initialization clears it, publishes it
through `ptr_beacon_offset_funcs` and calls only the finite 17-store
`pm_beacon_offset_funcs_init`; deinitialization withdraws the pointer without
freeing static storage.

The application audit now enables the PM publisher as an explicit strict root
for every static cold-init alias. It also verifies the exact 0x44-byte SRAM
section, init opcode sequence and two direct call targets (`memset` and the
publisher), plus the call-free pointer-clear deinitializer.

Hardware removed exactly one allocation and 68 requested bytes, producing
12 allocations, 2 observed frees and 796 requested bytes. The free count does
not change at the post-init snapshot because the vendor PM table was
persistent and had not yet reached its deinitializer there. Two complete
scan/authentication/association/WPA2 cycles passed with the snapshot fixed at
12/2/796. The first also completed ping, DNS, TCP and HTTP; all strict ELF
audits reported zero violations.

The pinned `trc_init` was the next persistent owner. It allocated three
zeroed 0x98-byte default transmit-rate-control contexts and published them in
`g_per_conn_trc[19]`, `[20]` and `[21]`. Reverse inspection recovered the
complete initialized subset: the three primary schedule pointers at offsets
0x64, 0x68 and 0x6c, P2P and legacy schedule pointers at 0x70 and 0x74, flags
0x80 at offset 0x0c, zero current/final state at 0x28/0x87 and identities
0, 1 and 2 at offset 0x85.

`rust-static-trc-init-interpose` replaces those allocations with one exact
3-by-0x98 arena in internal SRAM. Initialization first fails if any of the
three table cells is already owned, clears the arena, writes the recovered
fields with a straight-line finite sequence and publishes the three exact
pointers. Deinitialization is deliberately narrower than the vendor loop: it
accepts only those three exact publications, clears them and never scans or
frees an unknown pointer.

The final ELF audit rejects the original and `__real_` TRC constructors,
requires the exact 456-byte aligned SRAM section, proves the initializer has
no cycle and calls only `memset`, and verifies the three-pointer call-free
deinitializer. When the cold allocation journal is enabled, the same audit
requires its 2,560-byte fixed object to reside in PSRAM. This preserves
18,768 bytes of CPU0 stack without moving any rate-control or interrupt-owned
state out of SRAM.

Hardware removed exactly three allocations and 456 requested bytes, producing
9 allocations, 2 observed frees and 340 requested bytes. The first complete
scan/authentication/association/WPA2 cycle reached DHCP and completed ping,
DNS, TCP and HTTP. Teardown and a second scan/WPA2/post-link-data cycle also
completed with the snapshot fixed at 9/2/340, zero allocation failure and
zero radio-context allocator calls. The strict whole-ELF no-wait/no-heap
audit reported zero violations.

`pm_extend_tbtt_adaptive_attach` was the next persistent allocation boundary.
Reverse inspection recovered a data size of
`(interface[0x2a2] + 1) * sizeof(u32)`. The qualified STA/AP cold path has a
zero value at that halfword, so the attachment owns exactly one zeroed
32-bit word. The function publishes the interface through offset 0 of the
singleton returned by the call-free `pm_extend_tbtt_adaptive_instance`,
publishes that singleton at interface offset 0x430 and stores the allocated
data pointer at singleton offset 0x0c. The vendor deattachment frees the data
pointer and clears the two singleton fields.

`rust-static-tbtt-adaptive-interpose` now supplies that exact word from an
aligned internal-SRAM section. Attachment fails closed on a null interface,
on a nonzero audited count halfword or if either publication is already
owned. It then zeros the word and performs only the recovered publications.
Deattachment accepts only the exact static pointer, zeros it and clears the
singleton publications without entering a deallocator. This intentionally
does not generalize the boundary to an unqualified variable-sized adaptive
array.

The final ELF audit requires the exact four-byte SRAM section, rejects the
original and `__real_` attach/deattach implementations and proves the
singleton accessor is the expected call-free address materialization. The
attach wrapper must contain exactly six stores, the deattach wrapper exactly
four, both must be acyclic, and their only call target may be the singleton
accessor.

Hardware removed exactly one allocation and four requested bytes, producing
8 allocations, 2 observed frees and 336 requested bytes. The first strict
WPA2 cycle completed passive scan, authentication, association, M1-M4, DHCP,
ping, DNS, TCP and HTTP. Teardown and the second scan/authentication/
association/WPA2 cycle completed post-link traffic with the allocation
snapshot fixed at 8/2/336, zero failures, zero radio-context allocator calls
and no TX/RX queue rejection. The strict whole-ELF no-wait/no-heap audit
again reported zero violations.

The next persistent owner was the supplicant's empty PMKSA-cache header.
Reverse inspection of `libwpa_supplicant.a[pmksa_cache.c.obj]` recovered an
exact 20-byte layout: entry-list head and entry count at offsets 0 and 4,
the `wpa_sm` pointer at 8, and the free callback plus its context at 12 and
16. `pmksa_cache_init` allocated and zeroed that object before publishing the
three nonzero fields. The vendor deinitializer walked and freed entries,
cancelled and recomputed expiration timeouts and finally freed the header.

The strict WPA2 STA path does not create a vendor PMKSA entry: both qualified
connections retained an empty list, a zero count and an unchanged runtime
allocation snapshot. `rust-static-pmksa-cache-interpose` therefore replaces
only that observed empty-header case with one aligned internal-SRAM object.
Initialization requires all three input pointers and an entirely unowned
object, then performs the exact five field stores. Deinitialization accepts
only the exact static address with an empty head and zero count, clears the
same five fields and returns. An unknown or populated cache fails closed:
Rust does not call a vendor callback, timer registration/cancellation or
deallocator and does not pretend to implement PMKSA entry ownership.

The final ELF audit explicitly retains both wrappers so archive extraction or
LTO cannot silently discard this proof boundary. It requires the exact
20-byte aligned SRAM section, rejects the original and `__real_`
constructor/deinitializer and proves that each wrapper is acyclic, call-free
and contains exactly five stores. The same explicit retention is applied to
the other audited cold wrappers, preventing a build-only audit from accepting
a missing compatibility boundary.

Hardware removed exactly one direct `calloc` and 20 requested bytes,
producing 7 allocations, 2 observed frees and 316 requested bytes. The first
strict WPA2 cycle completed passive scan, authentication, association,
M1-M4, DHCP, ping, DNS, TCP and HTTP. Teardown and a second passive-scan,
authentication, association and WPA2 cycle completed post-link traffic with
the allocation snapshot fixed at 7/2/316, zero allocation failures, zero
radio-context allocator calls, zero core stalls and no TX/RX queue rejection.
The strict whole-ELF no-wait/no-heap audit inspected 6,407 functions and
reported zero violations.

The next direct allocation was the supplicant callback table constructed by
`esp_supplicant_init`. Reverse inspection of
`libwpa_supplicant.a[esp_wpa_main.c.obj]` recovered an exact 108-byte,
27-word layout. The constructor first calls `calloc(1, 0x6c)`, publishes the
result through `wpa_cb`, and installs the following nonzero callbacks:

| Word | Offset | Callback |
| ---: | ---: | --- |
| 0 | `0x00` | `wpa_attach` |
| 1 | `0x04` | `wpa_deattach` |
| 2 | `0x08` | `wpa_sta_connect` |
| 3 | `0x0c` | `wpa_sta_connected_cb` |
| 4 | `0x10` | `wpa_sta_disconnected_cb` |
| 5 | `0x14` | `wpa_sm_rx_eapol` |
| 6 | `0x18` | `wpa_sta_in_4way_handshake` |
| 7 | `0x1c` | `hostap_init` |
| 8 | `0x20` | `hostap_deinit` |
| 9 | `0x24` | `hostap_sta_join` |
| 10 | `0x28` | `wpa_ap_remove` |
| 11 | `0x2c` | `wpa_ap_get_wpa_ie` |
| 12 | `0x30` | `wpa_ap_rx_eapol` |
| 13 | `0x34` | `wpa_ap_get_peer_spp_msg` |
| 14 | `0x38` | `wpa_config_parse_string` |
| 15 | `0x3c` | `wpa_parse_wpa_ie_wrapper` |
| 17 | `0x44` | `wpa_michael_mic_failure` |
| 22 | `0x58` | `wpa_config_done` |
| 25 | `0x64` | `wpa_sta_clear_curr_pmksa` |
| 26 | `0x68` | `wpa_config_reload` |

Words 16, 18, 19, 20, 21, 23 and 24 remain zero. The constructor then calls
`eloop_init` and `esp_supplicant_common_init` before registering the table.
Its error path frees the same object and clears `wpa_cb`; normal supplicant
deinitialization unregisters and clears `wpa_cb` but does not free the table.

`rust-static-supplicant-callback-storage` admits only the exact direct
`calloc`, size 108 and pinned return site `esp_supplicant_init + 0x26`. It
claims one four-byte-aligned internal-SRAM object with a single non-retrying
CAS and zeros it before the vendor constructor performs the recovered finite
stores. A second live claim returns null instead of falling through to the
heap or aliasing state. The exact constructor failure-path `free` wipes and
releases the static object; every other allocation or free retains its
ordinary traced behavior.

The strict application checks after cold initialization that the claim is
live and `wpa_cb` is exactly the static SRAM address. The final ELF audit
requires the exact 108-byte aligned section, exactly one call edge from
`esp_supplicant_init` to `__wrap_calloc`, no edge to `__real_calloc`, and no
internal control-flow cycle. This keeps admission tied to the qualified
vendor constructor rather than turning it into a generic 108-byte allocator
exception.

Hardware removed exactly one direct `calloc` and 108 requested bytes,
producing 6 allocations, 2 observed frees and 208 requested bytes. The first
strict WPA2 cycle completed passive scan, authentication, association,
M1-M4, DHCP, ping, DNS, TCP and HTTP. The second passive-scan,
authentication, association and WPA2 cycle completed post-link traffic with
the allocation snapshot fixed at 6/2/208, zero allocation failures, zero
radio-context allocator calls, zero core stalls and no TX/RX queue rejection.
The strict whole-ELF no-wait/no-heap audit inspected 6,407 functions and
reported zero violations.

The qualified lifecycle includes radio reconnect without a full supplicant
deinitialization. Because the vendor normal deinitializer clears `wpa_cb`
without freeing this table, an eventual full `esp_supplicant_deinit` followed
by `esp_supplicant_init` intentionally fails closed today. Re-enabling that
lifecycle requires an explicit Rust reset boundary after unregister,
deinitialization and quiescence of every callback consumer; silently
reclaiming the same object earlier would permit a stale callback user to
alias the new lifetime.

The next persistent allocation boundary is the four-object `s_bars` array
constructed by `pp_attach`. Reverse inspection of `libpp.a[pp.o]` recovered
the complete finite allocation loop:

- the loop bound is exactly four;
- each iteration loads the internal-malloc callback from OSI slot `0x158`;
- each request is exactly 40 bytes;
- the indirect call returns at `pp_attach + 0x4a`, where the pointer is
  published into the next `s_bars` word;
- each successful object is zeroed for exactly 40 bytes;
- the back edge at `pp_attach + 0xbc` targets `pp_attach + 0x3c`.

If allocation fails, the constructor frees and clears only the already
published prefix before returning its error. `pp_deattach` likewise frees all
four objects and clears every `s_bars` entry, so this owner has an exact,
auditable full teardown/reinitialization lifecycle.

`rust-static-pp-bar-storage` admits only `OsiMallocInternal`, size 40 and the
pinned `pp_attach + 0x4a` return site. It supplies four separately claimed,
four-byte-aligned internal-SRAM objects and zeros each one before publication.
Each claim uses a single non-retrying CAS. The fixed four-entry search is
finite construction logic rather than a wait or polling loop. A fifth exact
request records an allocation failure and returns null instead of falling
through to the heap or aliasing an existing object.

The exact address of each static object is also recognized at release. A live
claim is cleared and wiped; a duplicate free of a pool address is consumed
without forwarding that address to the heap. All unrelated allocations and
frees retain their traced behavior. After cold initialization, the strict
application verifies that all four claims are live and that each `s_bars`
entry equals its corresponding static SRAM object.

The final ELF audit requires an exact 160-byte, four-byte-aligned
`.critical.bss.wifi_strict.pp_bars` section in internal SRAM and pins the
recovered constructor instructions, allocator slot, request size, publication
store, four-iteration bound and back-edge target. In the qualified image the
section was at `0x2f060e7c`, `s_bars` at `0x2f07fd1c` and `pp_attach` at
`0x40044834`. The strict whole-ELF no-wait/no-heap audit again inspected 6,407
functions and reported zero violations.

Hardware removed exactly four `OsiMallocInternal` calls and 160 requested
bytes, producing 2 allocations, 2 frees and 48 requested bytes. The first
strict cycle completed passive scan, authentication, association, WPA2
M1-M4, DHCP, ping, DNS, TCP and HTTP. Teardown and a second passive-scan,
authentication, association and WPA2 cycle completed post-link traffic with
the allocation snapshot fixed at 2/2/48, zero allocation failures, zero
radio-context allocator calls, zero core stalls and no TX/RX queue rejection.

The final two allocation sites were the 24-byte command envelopes in
`esp_wifi_init_internal` and `esp_wifi_start`. Both load the
`OsiWifiZalloc` callback from OSI slot `0x174`. In the final linked image their
allocator calls return at `esp_wifi_init_internal + 0xd6` and
`esp_wifi_start + 0x1a`.

These commands do not take the ordinary RTOS ioctl path in the taskless cold
profile. After `pp_create_task` has been replaced by direct publication, the
patched `_task_get_current_task` reports the fixed logical Wi-Fi identity to
the one serialized composition-root caller. `current_task_is_wifi_task`
therefore succeeds, leaving the ioctl wait flag clear. `ieee80211_ioctl`
selects the direct `ieee80211_ioctl_process` branch, runs the finite command
leaf inline and invokes the free callback before returning. It does not call
`pp_post`, create or take a semaphore, poll a status word, delay or switch a
task for either command.

`rust-static-cold-api-envelope-storage` supplies distinct four-byte-aligned
24-byte internal-SRAM objects for init and start. Admission requires the exact
`OsiWifiZalloc` source, request size and corresponding pinned return PC. Each
object has an independent single non-retrying CAS claim and is zeroed before
use. Unexpected lifetime overlap returns null and records failure instead of
falling through to the heap. The release path recognizes only the two exact
addresses, wipes a live object and consumes duplicate frees without ever
forwarding static SRAM to the captured allocator.

Per-object use and release counters prove after cold initialization that both
commands ran, both objects are no longer live and every use has exactly one
release. The final ELF audit requires the exact 48-byte, four-byte-aligned
`.critical.bss.wifi_strict.cold_api_envelopes` section in internal SRAM and
pins both allocator load/call sequences. In the qualified image the section
was at `0x2f05e6b0`, `esp_wifi_init_internal` at `0x40076f60`,
`esp_wifi_start` at `0x40077184` and `ieee80211_ioctl` at `0x40073580`.
The strict whole-ELF no-wait/no-heap audit inspected 6,407 functions and
reported zero violations.

Hardware removed exactly the final two allocations, two frees and 48
requested bytes. The resulting cold snapshot was 0 allocations, 0
reallocations, 0 frees and 0 requested bytes. The first strict cycle completed
passive scan, authentication, association, WPA2 M1-M4, DHCP, ping, DNS, TCP
and HTTP. Teardown and a second passive-scan, authentication, association and
WPA2 cycle completed post-link traffic with that all-zero snapshot unchanged,
zero allocation failures, zero radio-context allocator calls, zero core
stalls and no TX/RX queue rejection.
