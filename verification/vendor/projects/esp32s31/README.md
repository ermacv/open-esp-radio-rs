# ESP32-S31 vendor-analysis project

## Captured PHY research with Next

The `research` scenario ([`research.rs`](scenarios/src/research.rs)) exercises the current Next application using
explicit private inputs and its built-in resource supervisor. It authenticates
the PHY and ROM artifacts, imports PHY/ROM, deletes the source copies, analyzes functions,
links the I2C command initializer with exact ROM companions, verifies composed
write effects and repeated phase measurements, reports extent coverage/storage,
reviews independently hashed table bytes and an instruction
constant, exports provenance, then moves and restores the project. Research runs
retain phase diagnostics; the scenario does not qualify hardware or interpret
unresolved pointers. It independently checks the four ROM address alternatives in
`tsf_hal_set_tbtt_rf_ctrl_disable` and the unresolved mutable-table callback in
`phy_get_i2c_mst0_mask`, including source-free reading/export after restore. This
callback has no reviewed binding; its unknown target cannot acquire callee effects.
Outputs must stay in ignored storage.

```console
cargo xtask vendor-scenario research --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf --linker /usr/bin/ld.lld --limit-mode watchdog \
  --output target/blobray-phy-research
```

Each operation's working capacity, deadline and work budget are the scenario's
`--working-memory-mib`, `--timeout-secs` and `--max-work-units` arguments. Select kernel mode only in an environment with delegated
cgroup memory control; watchdog is an explicit sampled-RSS policy. Run JSON files
are the measurement authority, not this documentation.

## Captured I2C command-memory comparison

The `i2c` scenario ([`i2c.rs`](scenarios/src/i2c.rs)) compares the authenticated archive/ROM command initializer
and its descriptor/no-op leaves with a freshly built production probe ELF. It
checks all 45 ordered command writes against independent instruction-derived
expectations on zero, mixed, lower and upper parameter profiles. Descriptor
outputs are checked byte for byte from both zero and filled initial memory.
Command-memory writes use an explicit passive register model. The runner also
executes the [transport scenarios](scenarios/src/i2c_transport.rs) below with bounded
packed-command responses; neither model claims physical timing or RF behavior.

```console
cargo xtask build vendor-probes --chip esp32s31
cargo xtask vendor-scenario i2c --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf \
  --production target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/open-esp-radio-verification-esp32s31-probes-elf \
  --linker /usr/bin/ld.lld --limit-mode watchdog \
  --sdk /private/bootloader.elf --phy-sdk /private/phy_tracking_reference.elf \
  --output target/blobray-phy-i2c
```

`--sdk` enables the calibration leaves and the PBus/DCODE prefix. `--phy-sdk`
requires `--sdk` and adds RFPLL. Without them the command-memory, transport and
call-boundary scenarios still run. Each missing obligation is recorded in
`unmet-obligations.request.json`, and the scenario exits with status 2.

The scenario owns construction and independent expected-value assertions;
ordinary Next application operations own capture, linking, execution, comparison
and persistence. No provider pack or legacy engine is selected. The scenario
reads `phy_param` directly from the exported linked ELF's symbol table;
inexact source mappings are not used to infer that address. All inputs are captured before local source copies
are removed. The pinned archive/object/table/ROM hashes authenticate vendor inputs;
the exact production ELF digest identifies the freshly compiled replacement.

An explicit setup entry copies scenario parameters into writable captured memory.
Warm execution then uses the shipping HAL/PAC command-memory transaction on one
side and the linked archive root plus captured ROM callees on the other. A retained
guest entry shim supplies zero values for integer callee-saved registers on both
sides. This is a declared concrete input: the executor still stops on unknown
register copies/spills and never initializes them implicitly. The shim may only
be entered as an isolated root because it destroys its caller's saved registers.

The relation compares all MMIO/fence/delay events and selected final descriptor
bytes. Void returns, setup addresses, internal branches and stack transactions are
excluded explicitly. MATCH establishes only this software relation under these
inputs. Changed replacement input must produce DIFF; an unknown argument must
produce INCOMPLETE; a missing ROM companion must stop at its unmapped fetch;
exhausted event capacity must publish no execution. The runner
also checks moved and restored projects and exact replay of positive and negative
evidence. Whole-project backup preserves the full closure; the exported table alone
does not. Operation JSON records carry phase/resource diagnostics. There is no
hardware or kernel-enforcement claim from a watchdog run.

### Captured I2C transport

The same runner links captured byte/field read/write, host selection and reset
paths with their exact ROM callbacks. An explicit guest interface table points to
captured code; it supplies no replacement callback semantics. The production
probe calls the shipping cold-I2C transaction state machine, host configuration
and bounded wake-reset helper. Its finite harness only supplies arguments,
capabilities and completion edges. A retained entry shim supplies all eight
integer arguments and explicit callee-saved register values.

The generic [packed-command model](../../../../tools/blobray/next/reference/execution/README.md#packed-command-bank)
owns two ports over one seeded bank. Scenario declarations select register
geometry, initial values, optional scripted replies and busy poll counts. These
are environmental assumptions. Bank writes commit on a ready read; pending
commands and unused scripted samples prevent a complete closed result. Reset
aborts the selected pending command without restoring bank values or samples.

The positive matrix checks thirteen host selectors, byte and masked read/write
on both hosts with immediate and delayed completion, and idle/busy reset. Each
of its forty cases checks independent expected command writes and applicable
read returns, not just equality between implementations. Cases use independent
cold instances and are batched to fit the ordinary request-size limit. The
relation selects all writes, fences and delays, plus read-operation returns.
Raw MMIO reads remain retained but are excluded from equality because production
performs a pre-issue readiness check. Void returns are explicitly excluded.

Exhausted scripted replies must produce INCOMPLETE without retained-value
fallback. An out-of-field write must produce DIFF: captured code spills the
value into an adjacent bit while the typed production path clips it. Limited
completion edges and the shipping reset polling bound must leave pending-model
INCOMPLETE on timeout; they cannot become MATCH merely because a CPU returned.
All positive and negative artifacts are checked after source removal and
backup/restore, including exact replay. This establishes the selected software
relation under declared peripheral responses, not analog-bus or RF qualification.

### Current calibration leaves

`--sdk /private/bootloader.elf` includes the native
[four-leaf matrix](scenarios/src/calibration_leaves.rs). Its eleven independent cold cases
exercise TX-gain restore, forced signed digital gains, temperature-to-power and
post-init AGC with complementary retained register inputs. All MMIO reads/writes
remain selected, with independently checked ordered writes and temperature
returns. AGC executes its captured ROM saturation-gain child. The guest probe
constructs a validation owner and calls existing shipping HAL/PHY leaves; no
production capability is fabricated by seeding a pointer.

The extra SDK bootloader ELF is a pinned static symbol companion (SHA-256
`e5e2929ae216e324dac3efd13cf1e05146dfcc4ea64098a1fead74b8ac453195`).
AGC shares one archive `.iram1` section with unrelated functions. Their retained
relocations require the physical SDK clock symbol in addition to ROM
symbols. The runner records all these explicit symbol bindings and captures the
SDK before deleting source copies. It does not execute SDK bodies or install
responses for them: the selected roots use only their retained image, ROM and
probe mappings. Entering an unmapped SDK dependency would be an execution gap.
The comparison does not cover the other functions co-located in `.iram1`.

Restore comparison requires the explicitly enabled domain. Temperature conversion
uses the current archive policy: positive Wi-Fi delta divides by eight, negative
by three; Bluetooth uses five/four. Selecting a similar ROM function would change
that policy. Void returns are excluded, while temperature returns are compared.
Changed temperature must produce DIFF, an unknown argument buffer or absent
register input must produce INCOMPLETE, and exhausted event capacity must publish
no result. The combined run retains these cases with the I2C evidence and checks
their exact replay after source removal and backup/restore. These leaves do not
establish a complete calibration or physical timing claim.

### PBus and DCODE prefix

`--sdk` also runs
[the native prefix matrix](scenarios/src/calibration_prefix.rs). PBus covers 24 combinations
of retained values, work-mode settling, immediate/delayed readiness and stack
fills. It compares all MMIO observations and requested delays, with independent
expected command/acknowledgment writes. Three stuck-command positions execute the
production timeout policy and check that unfinished commands are not acknowledged
and work mode is not restored afterward. Finite read runs encode the long busy
prefix without allocating one declaration element per response.

DCODE covers four crystal selectors, two stack/analog fills and two readiness
delays. A setup phase executes captured ROM `memcpy` to initialize the retained
parameter buffer. A small guest ABI shim calls the captured PBus and DCODE bodies
in order; the production wrapper executes its actual parent transitions and both
children. ROM I2C callbacks select captured archive code. The peripheral bank
supplies retained CKGEN values and eight finite samples; delay responses expose
requested microseconds only. No model replaces a calibration algorithm.

The native DCODE relation compares every MMIO write, fence, requested delay and
each of the eight selected final bytes. Production performs extra I2C busy
prechecks, so raw reads are retained but excluded from that native relation. The
runner additionally compares the complete ordered read/write/delay/fence stream
except reads of the two command ports. It also independently checks the four
frequency programs, NRX values, forty CKGEN commands, sample consumption and exact
output `[0,63,31,32,32,31,63,0]`. These extra runner assertions are not part of the
native MATCH claim. An unfiltered native comparison retains the expected polling
DIFF; no blanket peripheral aperture exclusion is used by the independent check.

Stuck channel readiness, stuck I2C and a read/modify/write whose halves individually
fit but jointly exhaust the production budget must return their specific failure,
leave all eight initialized output bytes unchanged and stop issuing commands at
the failed boundary. Changed samples produce DIFF, an unknown parameter pointer
produces INCOMPLETE, and insufficient event capacity publishes no execution.
The combined runner retains all these results for source-free move,
backup/restore and exact replay. This is a software prefix comparison under the
declared environment, not complete RX calibration or hardware qualification.

### RFPLL search and frequency maintenance

`--phy-sdk PATH` runs
[the native RFPLL matrix](scenarios/src/rfpll.rs). The additional linked SDK firmware
must have SHA-256 `ea4197a4e8d40fe43f5b1590132fab7365b2b1034dfa61f498743778002b07d9`.
It contributes the exact static `phy_printf` definition needed by the linked
archive section. It is captured and retained but is not an execution companion:
its TLS and overlapping code/data load segments exceed the current loader
profile. Diagnostics are disabled in positive cases; enabling them must stop at
an unmapped instruction fetch. No stub or successful diagnostic model is used.

Search covers nine finite status/capacitor profiles, two stack fills and two busy
prefixes (36 cases), including signed underflow and the 511-capacitor boundary.
The ROM capacitor helpers and archive I2C callbacks execute captured code. The
compiled wrappers acquire the production radio capability and invoke shipping
search/maintenance owners. Finite lock-status samples are peripheral inputs;
no model supplies the search result. Independent assertions require the exact
signed result, command sequence and requested five-microsecond settles.

The production delay adapter executes a guest save/restore shim around
`open_phy_trace_delay_event`; the model binds that inner requested-time edge.
Integer caller registers are preserved by captured instructions, avoiding
unspecified call clobbers in inactive coroutine fields. Entry explicitly seeds
unused temporaries as well as saved registers. The modeled ROM delay retains the
ordinary external-call ABI. No general unknown-value handling is relaxed. ROM
`memcpy` may use unaligned normal-memory words; the recorded execution environment
explicitly admits those accesses while MMIO and atomics remain aligned.

Maintenance includes eight zero-delta cases and forty positive/negative,
underflow/overflow cases across channels 1, 13, 14, 2412 and 2484. Setup executes
captured ROM `memcpy` to initialize the archive parameter object. The measured
phase retains those bytes and executes real weak grant functions. Nonzero
corrections read and rewrite all 85 frequency-memory entries, preserve unrelated
bits and restore the selected channel. An independent transaction oracle checks
all frequency and SDM reads/writes, including signed narrowing and high bits;
requested delays must remain exactly two microseconds followed by the search
settles. The stuck-command case returns the production timeout, leaves the first
command pending and must not restore hardware frequency control.

Native search comparison selects low return, all MMIO writes, fences and delays;
maintenance selects those events without the vendor's void return. Raw reads are
retained. The runner additionally compares the ordered frequency envelope:
transport-aperture reads and its two mapping-control writes are excluded, while
command-port writes and every other read/write/fence/delay remain. For nonzero
maintenance only, the vendor's second frequency-control read is the installed
layout query; its count and exact value are checked before excluding that one
observation. Production owns that fixed layout. These additional assertions are
not included in the native MATCH claim; an unfiltered polling comparison retains
DIFF. Changed capacitor input, unknown callback pointer, unmapped diagnostics and
event exhaustion have explicit DIFF/INCOMPLETE/no-publication checks. The combined
runner retains the entire matrix for source-free backup, move, restore and exact
replay. Software observations do not establish hardware/RF or grant qualification.

### Wi-Fi and Bluetooth gain arithmetic/publication

The `gain` scenario of the [typed scenario package](scenarios/src/gain.rs)
executes captured archive callbacks and ROM children against the compiled
production arithmetic and publishers. Build and validate the probe catalog
with `cargo xtask build vendor-probes --chip esp32s31`. Then run from the
repository root; `xtask` builds `blobray` and the scenarios and supplies
`--binary`:

```console
cargo xtask vendor-scenario gain --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf \
  --production target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/open-esp-radio-verification-esp32s31-probes-elf \
  --linker /usr/bin/ld.lld --rftest /private/librftest.a \
  --output target/blobray-research/gain --limit-mode watchdog
```

Requests and evidence are Blobray's own serde types, and CLI documents decode
through `blobray_next_host::wire`. `cargo test --manifest-path tools/blobray/Cargo.toml -p oer-esp32s31-vendor-scenarios`
checks the harness, the evidence interpretation and the oracles. Those tests
need no private inputs.

Each scenario passes its Blobray budget explicitly. `--limit-mode` is required;
`--timeout-secs` (600), `--working-memory-mib` (256) and `--max-work-units`
(2,000,000,000) set each operation's deadline and capacities. Every finite matrix
is one execution request, because Blobray retains requests by identity rather
than inside 64 KiB control records. Shared guest addresses and analog command
encodings are named in [`layout.rs`](scenarios/src/layout.rs). The full gain
scenario with `--rftest` completes in about half a minute on an otherwise idle
host.

Scenarios hold vendor knowledge, peripheral inputs and independent
expectations; Blobray mechanisms supply the rest:

- ROM companions are proposed by `propose-companions` from the ROM, then from
  any supplied SDK firmware, and copied into the retained link request; no
  scenario keeps a companion list.
- Requested-delay models answer every call. Delay counts are evidence, and the
  scenarios compare delay sequences instead of declaring budgets.
- Where a scenario uses the radio aperture, every radio register without an
  explicit model is retained storage that starts with the fill pattern. Only
  semantic inputs are modeled, and both sides must read the same never-written
  registers.
- ROM storage addresses name their ROM symbols (`rom_phyFuns`,
  `phy_param_rom`, `g_phyFuns_instance`) and are checked against the captured
  ROM inventory at session start.

Archive and ROM identities are the same pinned inputs as the I2C runner; no SDK
companion is needed. The gain object and its complete 216-byte coefficient
section must match independently extracted SHA-256 identities before comparison.
The callback's actual three `memcpy` source ranges are checked against those
captured bytes, and its real ROM kernel arguments include signed narrowing and
stack ABI words. The entry adapter supplies sixteen explicit words and initializes
saved registers; it supplies no gain algorithm or callback result.

The finite matrix contains 150 callback/kernel input combinations, 360 Wi-Fi
production arithmetic combinations, 24 complete Wi-Fi publication combinations
and 120 Bluetooth combinations. Another 72 calculation cases select every one of
the 18 Wi-Fi and 18 Bluetooth coefficient thresholds exactly; the original matrix
alone does not select every interval in the production path. All counts include
both declared stack fills. Every
Wi-Fi calculation checks all 160 output bytes; Bluetooth checks all 80. An
independent interval-selection oracle consumes the authenticated coefficients.
Separate vendor-only observations retain the distinction between current additive
Wi-Fi adjustment and the older ROM callback's subtractive attenuation and tables;
these observations do not assert equivalence of the two profiles.

Bluetooth executes the real `phy_get_romfunc_addr` installer and follows the
installed callback during full publication. The explicit ROM pointer names
captured writable BSS storage; no synthetic callback table supplies the result.
The pure production calculation borrows ordinary arrays, while complete Wi-Fi/BT
publishers acquire the shipping radio capability. A retained index register and
three passive data ports model only peripheral storage. Independent instruction-
derived expectations require all 161 Wi-Fi or 81 Bluetooth MMIO events, including
bank selection, index wrap and preserved control bits. No MMIO events are excluded
from publication comparison; void return values and physical call addresses are
not equivalence criteria. Raw call observations remain retained.

The runner batches bounded native requests, each with explicit cold/warm phases.
Selected RAM and every MMIO/fence/delay event form the native comparison relation.
Unknown curve data and missing callback installation must be INCOMPLETE. A changed
caller-supplied coefficient at the direct ROM boundary must produce DIFF and a new
execution identity; this is a vendor-boundary characterization. Event exhaustion
must publish no execution. Input copies are removed before linking; all retained
positive and negative runs reopen after move/backup/restore and replay with exactly
their original identities. These are software gain-child comparisons, not RF,
whole-TXCAL, Wi-Fi or BT/154 protocol qualification.

### Gain state and RF-test power producer

The same scenario characterizes captured calibration storage and the RF-test
power policy ([`gain_state.rs`](scenarios/src/gain_state.rs)). These are vendor-only executions: no
production storage or RF power API exists, and none is inferred.

Storage always runs, because it needs only the pinned archive and ROM. The
captured image's startup adjustment byte must be zero. For both stack fills and
adjustments 0, 1, 31, 127, 128 and 255, warm phases execute the real backup,
destruction of live state, recovery, parameter registration, curve/base
isolation, restored-adjustment consumption and consumption after moving the
value to the base. Independent expectations check the 532-byte backup image,
the exact registration writes and every 160-byte gain output against the
coefficient oracle. No hardware event occurs.

`--rftest` supplies `librftest.a` (SHA-256
`547786cd684eb9cd8902955176e9a9a7f113d8faa3f415e12108ed261f55a11e`, same
`b88e4b76` revision). Its `set_rate_power_index` and `mac_power_set` are linked
as additional roots; no power or gain callee is substituted. The real callback
installer runs, then fifteen policy rows per fill check rounding, saturation,
signed-byte wrap, the adjustment byte, conditional `phy_wifi_set_tx_gain_new`
publication with all MMIO events, and both MAC index writes. Without `--rftest`
every other case still runs, the unmet producer obligation is written to
`unmet-obligations.request.json`, and the runner exits with status 2 instead of passing.

Negative cases are:

- corrupted saved adjustments on both sides propagating to a DIFF;
- recovery from an unknown cache, which is INCOMPLETE;
- a missing callback installation, which is INCOMPLETE and publishes neither gain
  nor MAC state;
- an unknown policy input, which stops at the exact byte read and blocks later
  phases;
- event exhaustion, which publishes no execution.

All retained runs join the source-free move/backup/restore/replay set.

## Legacy configuration reference

The configuration and command vocabulary below describe retained legacy inputs;
they are not supported commands or a compatibility path of `cargo blobray` Next.

This directory is the reviewed ESP32-S31 configuration for Vendor Binary
Blobray. `vendor-project.toml` is the entry point. The target host links the
generic Blobray with ESP32-S31 knowledge contracts. Target addresses and
driver dependencies do not enter the generic package, and target code cannot
return a comparison verdict.

Reusable chip identity and compiled knowledge are composed from
`verification/vendor/chips/esp32s31/`; reusable ESP-IDF vocabulary comes from
`verification/vendor/knowledge/espressif/`. The shared
[register owner](../../../../registers/esp32s31/README.md) supplies hardware
geometry, reviewed assertions, MMIO publication scope and production PAC API.
This investigation owns artifact-specific profiles and scopes, compiled
reconstructions, reviewed interfaces, comparison policy, dispositions and
verification inputs. Adding an accepted register name or W1C fact changes the
reviewed register model or assertion overlay with its original applicability.

For register-only changes, the [source-only publication project](../../../../registers/esp32s31/publication/README.md)
checks the same SVD, PAC and bindings without selecting binary investigation
facts or requiring private artifacts. The full investigation retains exact-input
authentication. The [revision state](revisions/README.md) describes its machine
snapshot schema and compatibility constraints.

Comparison suites in [verification-addon.toml](verification-addon.toml) declare
`model-mechanisms` from [model-inputs.json](model-inputs.json). That registry
separates target/ABI, shared radio geometry, Wi-Fi and Bluetooth geometry, PHY
I2C and calibration models, and Bluetooth fail-stop summaries. Its implementation
paths are captured by the composed host at build time; hardware contract paths
must identify inputs actually loaded by the project. Update the declarations
when a suite starts depending on another mechanism. Shared dispatcher and ABI
files remain shared dependencies; source file boundaries determine the finest
implementation scope. Rendering code and research in other mechanisms are
outside the selected identity.

## Local inputs

`local.toml` is ignored and contains machine-local paths to authenticated
vendor artifacts and compiled Rust probes. Initialize it from
`local.example.toml` or with:

```console
cargo blobray project inputs init \
  --project verification/vendor/projects/esp32s31/vendor-project.toml \
  --bind source-artifact:rom=/path/to/esp32s31_rev0_rom.elf \
  --bind source-inventory:archive=/path/to/libphy.a \
  --bind source-inventory:libpp=/path/to/libpp.a \
  --bind source-inventory:libnet80211=/path/to/libnet80211.a \
  --bind rust-artifact=/path/to/rust-trace-probes.elf
```

Never commit vendor binaries, disassembly dumps, unreviewed extraction
artifacts or private paths. Necessary recovered hardware tables belong in
production source with provenance and consuming-code verification, as defined
by the [source policy](../../../../docs/source-policy.md).
`project files` lists every required role.

The open ESP-IDF IEEE 802.15.4 controller is ported from source. Its public
`esp_ieee802154_` family has an explicit binary-coverage exclusion and requires
no controller archive or ELF. The `ieee802154-btbb`, `ieee802154-zb` and
`ieee802154-coex` profiles retain binary analysis of the closed baseband and
coexistence functions. This exclusion does not claim controller qualification.

Only artifacts bound in the run spec participate in analysis. Archives are not
discovered automatically. A `source-inventory` binding preserves original
symbols and provenance; it does not link that archive into a `source-artifact`
ELF. Cross-archive execution requires a composed linked image. Static IR also
accepts ordered primary archive sets and records unresolved or ambiguous
references. Project-wide symbol associations alone are not linker resolution.

Build the three Rust comparison inputs with
`cargo xtask build vendor-probes --chip esp32s31`. Its `--list-roles`
option lists the declared roles without building. Normal builds use Cargo's
parallelism; set `OPEN_RADIO_ANALYSIS_BUILD_JOBS` only to impose an explicit
local resource limit. Bind the resulting ELF files in `local.toml`; declaring
a role does not authenticate an artifact or produce a comparison verdict.

## Normal workflow

`cargo blobray` selects the new captured-input application. See its
[operator reference](../../../../tools/blobray/next/README.md) for import,
image preparation, research and concrete comparison. Build production probe
ELFs with the command above, capture them alongside vendor inputs, and supply
an explicit native comparison request. Results remain limited to its declared
MMIO/fence/return relation.

The project/profile/provider files below retain reviewed research and required
machine provenance. Their broader verification suite, service models and legacy
command examples are not implemented by the new primary CLI. They are not
loaded through a compatibility adapter. SVD/PAC publication instead uses the
[independent source composition](../../../../registers/esp32s31/publication/README.md).

This project selects `generated/findings/capability-context.json` through
`[interfaces.capability-context]`. It is a disposable projection generated by
`project analyze`, not reviewed ESP32-S31 knowledge: sparse facts remain in the
reviewed packs. `project research next` verifies the projection's digest before
using its unresolved interface observations and capability links. A missing or
stale projection produces explicitly partial prioritization with no live
fallback; rerun the limited `project analyze` command above.

## Ordinary TX ownership edges

`libpp-tx-retry` is informational research into vendor retry/rate policy and
ACK-timeout handling. Matching software retry counters, rate choices or Embassy
dispatch is not a completion requirement. Production retry policy is tested
against its own contract; hardware completion decoding, Retry-bit publication
and DMA ownership remain hardware obligations. Reviewed ABI projections do not
supply qualification-eligible production traces.

`ordinary-tx-ownership` is a completion gate for three finite queue operations:
the `hal_mac_txq_enable` publication prefix before `GetAccess`, selector-two
completion acknowledgement, and the complete queue-disable leaf. It exercises
compiled production PAC entries for queues zero through three and three retained
register images. Publication preserves the production device fences as explicit
effect-contract additions. It does not compare the rest of vendor enable's
access/HE bookkeeping or the enclosing DMA lifecycle.

Bind `source-artifact:libpp` and `source-inventory:libpp` to the authenticated
archive named in the suite, and `rust-artifact` to a fresh production probe ELF:

```console
tools/blobray/target/blobray/blobray-run --project verification/vendor/projects/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/projects/esp32s31/local.toml \
  project verify --suite ordinary-tx-ownership
```

The selected-suite report is saved below `generated/reports/verification-suites/`;
it does not replace the complete evidence index consumed by qualification.
The [DMA owner contract](../../../../crates/hardware/esp32s31/driver/ieee80211/dma/README.md)
connects these operations to buffer retention, host regressions and the remaining
result-decoding, descriptor, abort and physical-release limits.

## TX protection control

The reviewed `WDEVTXQ_CONF2` argument order identifies **SW_RTS at bit 31** and
**SW_CTS at bit 30**. The source identity and exact call argument positions live
in `BLOB_LIBPP_TX_PROTECTION_ARGUMENTS` in the
[register evidence](../../../../registers/esp32s31/evidence/vendor-libpp.toml).
`hal_he_set_tx_protection` controls RTS, despite its generic name. Its software
request update preserves CTS and minimum MPDU spacing. Descriptor-bound
production preparation separately replaces both request flags with the explicit
PPDU mode while the queue is idle, before publication.

The `tx-protection-control` completion suite compares compiled production
`configure_rts` and `disable_he_rts_threshold` with their complete vendor
functions. The finite domain covers all four queues, RTS clear/set, absent
threshold publication, zero and maximal byte thresholds, low-u16 truncation,
and retained images with independent RTS and CTS requests. Disabling the HE
threshold updates all four queues in logical order and retains threshold bytes.
All observed MMIO effects are compared. The private-input transition test also
carries each implementation's own writes through enable, disable, clear and
republish sequences. These are register-transaction properties; the
PHY-specific duration-to-byte conversion and on-air exchange are outside them.
Use the same authenticated `libpp` and fresh `rust-artifact` bindings as above:

```console
tools/blobray/target/blobray/blobray-run --project verification/vendor/projects/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/projects/esp32s31/local.toml \
  project verify --suite tx-protection-control
```

Cold HE initialization separately clears CTS while preserving RTS and spacing.
The private-input
[`tx_protection` test](blobray-provider/models/tests/tx_protection.rs) compares
its four ordered queue RMWs with the real `hal_he_init` parent. It observes only
those queue effects, with six earlier children supplied as explicit out-of-scope
call responses, and stops before broadcast-RU setup. This is not a comparison of
the entire HE initializer. Build the test with `--profile blobray`, then run its
executable through `blobray-run` with `BLOBRAY_BINARY`,
`BLOBRAY_LIMIT_BACKEND=watchdog`, `OER_TX_ARCHIVE` and `OER_TX_PROBE` set to the
built test, authenticated archive and fresh production probe respectively.

Ordinary protection-required transmission remains closed by policy. Explicit
descriptor-bound RTS/CTS and CTS-to-self requests exist, with bounded HT40
air-capture provenance. That scope does not establish the generated CTS receiver
address, NAV duration, protected-MPDU sequencing and completion/abort contract
across the admitted rates and retry transitions; register comparisons alone
cannot qualify them.
The [ordinary DMA contract](../../../../crates/hardware/esp32s31/driver/ieee80211/dma/README.md)
and qualification catalog retain that admission limit.

## Legacy PHY calibration gate

The `phy-calibration-leaves` suite compares four production-bound leaves against
the pinned esp-phy-lib archive. Its vendor entry authenticates both the archive
and rev0 ROM by SHA-256 before execution. Keep the original `archive` binding
for the existing investigation; the new suite uses `phy-current`.

Bind `source-artifact:phy-current` to the archive and
`source-companion:phy-current` to the ROM in the ignored run spec. The
`rust-artifact` role must point to a fresh production probe ELF.

```console
tools/blobray/target/blobray/blobray-run --project verification/vendor/projects/esp32s31/vendor-project.toml \
  --run-spec verification/vendor/projects/esp32s31/local.toml \
  project verify --suite phy-calibration-leaves
```

This completion gate requires every selected function to match. Its finite
profiles cover TX-gain restore, forced digital gain, temperature-to-power and
post-init AGC; they do not qualify complete calibration, coexistence or hardware
timing. Temperature tracking follows the archive policy, including divisor eight
for a positive Wi-Fi temperature delta; the ROM primitive uses divisor five and
is not a direct-equivalence target for this computation. A selected-suite run publishes its report and replaces only its rows in the
compact evidence index. Other completed suites remain usable. An interrupted
rerun leaves its suite incomplete; immutable index history retains older results.
`project verify --suite <suite> --check` checks the selected suite's current rows.

The private-input `phy_rfpll` integration tests also execute complete production
children at the MMIO/I2C boundary. The calibration tests compare PBus clearing
against the current archive with its authenticated ROM companion, without
substituting child function completions. They cover both work-mode settle
branches, retained register contents and delayed PBus readiness. Their comparison
excludes only readiness-wait delays immediately preceding a status read; all
register effects and work-mode settle delays remain ordered. A separate stuck
PBus case checks that the production executor returns failure without clearing
the unfinished transaction or restoring work mode. These tests require fresh
probes and execution through `blobray-run`; they are not part of the four-leaf
suite and do not establish complete RXCAL/TXCAL equivalence or hardware timing.

The D-code comparison executes the production PBus/D-code prefix and both ROM
children with matching peripheral inputs. It checks channel-switch MMIO, NRX
updates, CKGEN I2C commands, explicit settle delays and eight measured output
bytes. I2C host selection/configuration and readiness polling are outside the
command-level projection. Readiness-wait delays are excluded only directly
before a bus status read. The test covers crystal selectors, distinct retained
analog bits, stack fills and delayed I2C completion. Channel-ready and I2C
failure cases require no partial output publication. These are modeled
peripheral responses, not proof of physical channel readiness or RF quality.


The RX-gain comparison executes complete `phy_set_rx_gain_table` and the
production `rx_gain_init` executor, including nested ROM DC measurements and
both gain-memory publishers. It compares ordered hardware effects and projected
per-gain/base/fine DC coefficients plus the two bank limits. Profiles exercise
DC/table guards, retained register values, signed estimator inputs, delayed
I2C completion and work-mode settling. Shared-bank baseband DC is compared
independently from Wi-Fi corrections. Unknown analog reads and unresolved data
relocations fail closed; the archive's missing ROM data symbols are resolved by
the linker against the authenticated companion, preserving relocation addends.
A stuck-channel profile requires failure without output or gain-table publication.

These RX-gain profiles do not execute the enclosing calibration parent's
working-channel restoration, temperature callback, MAC baseband restoration or
semantic commit. The separate combined-calibration comparison below exercises
these boundaries with actual children; parent call-order tests with modeled
children alone do not close that gap. Constant
estimator inputs exercise arithmetic/search paths, not an analog convergence or
RF-quality model. No hardware timing or maintenance-grant claim follows from a
matching trace.

The `channel` scenario of the [typed scenario package](scenarios/src/channel.rs)
compares channel restoration. Inputs are the same as for the gain scenario, without an RF-test archive:

```console
cargo xtask vendor-scenario channel --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf \
  --production target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/open-esp-radio-verification-esp32s31-probes-elf \
  --linker /usr/bin/ld.lld --output target/blobray-research/channel --limit-mode watchdog
```

Each case runs three phases in one execution session:

1. copy zeroed parameters;
2. run the real `phy_get_romfunc_addr` installer;
3. execute `phy_chip_set_chan`.

The installer supplies the archive's own temperature, TX-gain and
RX-compensation callbacks, so no ROM replacement is selected by hand. The
production side runs `open_phy_channel_trace_state`.

**Peripheral inputs.** The models declare these inputs explicitly:

- the sensor DAC and PLL analog cells;
- the temperature code;
- channel readiness;
- the cleared work mode and the transport read-mask and host-map words.

Every other radio register is retained storage that starts with the fill
pattern (the radio aperture described above). Both sides must read the same
never-written registers.

**Relation.** The native relation compares ordered MMIO writes. A runner-side
comparison then reviews every non-transport read and every requested delay. It
excludes only three things:

- transport-port reads;
- read-mask and host-map accesses;
- the single-microsecond delay before a transport read.

Requested delays are observed, not budgeted: both sides report every delay,
and the comparison keeps them ordered. Production additionally waits before the
transport read of each analog command, and an out-of-range sensor sample adds
the DAC read-modify-write of the ROM reselection.

**Full-root evidence.** These cases cover channels 1, 6, 11 and 13 at both
bandwidths and both fills. They require:

- identical ordered effects;
- nonempty TX gain publication;
- a committed channel, bandwidth and temperature that match both an independent
  ROM `phy_tsens_attribute`/`phy_code_to_temp` oracle and the production output.

**Temperature-prefix evidence.** These cases cover all five sensor DAC windows
at codes 0, 64, 100 and 255. They are retained as separate `prefix-*`
executions. Their ordered-effect claim ends at the first gain-bank read, with
exactly one sensor sample before it. They make no whole-channel claim.

**Stuck readiness.** The production-only case must return failure without
writing gain data or semantic output, after sampling readiness.

**Negative cases:**

- a changed production channel is a DIFF;
- omitted callback installation leaves the vendor INCOMPLETE;
- an undersized event capacity publishes nothing.

All runs replay after move/backup/restore. Stuck readiness runs production's
full bounded poll, which dominates the run's roughly one-minute duration.

This is channel state, not the enclosing RXCAL reference commit, and not RF
qualification.

Gain publication has an independent complete-body comparison using identical
synthetic DC rows, baseband settings, RF settings and digital adjustments.
It exercises bank-index wrap and retained command-register bits without copying
vendor tables into fixtures. Matching this publisher does not qualify gain
selection or RF power. The current callback also consumes an additive adjustment
where the ROM callback consumes subtractive attenuation; the characterization
executes both real bodies, including signed-byte narrowing.
The gain-state characterization executes actual calibration backup and recovery
in one session, destroys live state between them, and passes the recovered
adjustment through init-parameter loading and gain calculation. Synthetic state
exercises the complete serialized payload and untouched envelope bytes without
copying vendor calibration data. This establishes preservation and consumption,
not cache validation, full-calibration generation of the adjustment, its physical
units, or production support for vendor calibration storage.

Gain calculation is checked separately against the actual current archive
callback with synthetic channel curves and signed correction boundaries.
Production owns the reviewed current coefficients with explicit provenance. A distinct current-archive test
executes its callback, observes its coefficient-copy sources and gain-kernel
arguments, then executes the real ROM kernel directly with those inputs.
Oracle tables are read from the supplied private archive, never from duplicate
fixtures. These checks separate arithmetic from profile selection; neither
substitutes ROM coefficients in the complete current-archive channel gate.

The BT/154 gain comparison executes the current callback and complete 16-entry
publication against compiled production calculation and publication. It varies
calibration inputs, independent Wi-Fi fields and bank-index wrap. This is a
shared PHY child comparison, not qualification of a Bluetooth/154 protocol
runtime or the enclosing TXCAL transaction.

TX-DC/PWDET comparison executes `phy_txdc_cal_pwdet_init` and its actual search,
PBus and SAR children against the production executor used by runtime tracking.
Cases vary Wi-Fi/BT selection, initial DC rows, constant and alternating SAR
streams, tone clearing and work-mode settling. They compare DC outputs and
ordered hardware control effects, retaining all writes and readiness reads.
Only production PBus readiness-wait delays and three unused read-only SAR
result words from the general ROM reader are excluded. Independent fault
cases require typed PBus/SAR failure without coefficient publication; the SAR
observation limit is distinct from a timer deadline or global execution budget.
The actual TX-DC root preserves the independently seeded Wi-Fi gain adjustment;
its producer elsewhere in calibration/configuration is not established by this
comparison.

The combined `phy_cal_param_track` comparison uses the actual current archive
and ROM, including callback installation, DCODE, RX calibration, channel and
temperature restoration, both TX-DC forms, gain publication and cleanup. Its
production entry uses the same state-owned calibration transition and target
executor as maintenance. The `validation-probes` bridge only constructs
ordinary semantic fixtures and selects that child; it cannot mint registered
state or physical access. The isolated probe provides its own validation HAL
owner. No calibration child call is replaced by a modeled completion.

Profiles cover channel 13/HT40, all client selections, default/debug thermal
boundaries, and a restored temperature that creates or removes TX demand.
Ordered effects, temperature references and RX/TX banks are compared. A
production-only stuck-SAR profile fails after completed RX restoration and
checks that the combined transaction retains its earlier semantic state.
Synthetic measurements, valid DAC inputs and default weak grant hooks limit
the claim to software behavior. Projections reuse the documented child I2C,
PBus, estimator-poll and unused-SAR-read exclusions; they never remove control
writes or gain publication.

The full `phy_param_track_tot` baseline comparison also executes the real BT
power, Wi-Fi I2C/power and final temperature children with RFPLL explicitly
disabled to isolate those effects; calibration is enabled and diagnostics are
disabled. It compares
shared power-cache state, per-class gain bases, retained additive adjustment and
I2C band in addition to calibration state and ordered effects. Signed thermal
boundaries, both power thresholds and nonzero retained adjustment inputs are
covered. The parent failure profile rejects normal-owner recovery after TXCAL
failure, retains completed power children and rejects partial calibration
publication. It does not manufacture successful child completions or a registered
PHY owner. A separate RFPLL-enabled registered-policy profile executes thermal
gates and zero/positive/negative frequency-memory corrections inside this same
parent, comparing the RFPLL reference commit. Its projection omits the single
installed-layout query already documented for isolated frequency-memory
comparison, retaining all memory/control transactions. The TXCAL failure profile
also checks retention of an earlier completed RFPLL reference. Registered
production RFPLL uses the recovered thermal predicate. External COEX grant
hooks, elapsed timing and RF performance remain separate gates.

The optional RF-test power producer is characterized by `phy_rfpll/gain_producer.rs`.
It authenticates `OER_PHY_RFTEST` (`librftest.a`, SHA-256
`547786cd684eb9cd8902955176e9a9a7f113d8faa3f415e12108ed261f55a11e`, same
`b88e4b76` source revision), along with the PHY archive and ROM. Host `ar` with
MRI support combines unchanged members in a temporary archive; actual vendor
callback registration, target-power lookup, `set_rate_power_index`, conditional
gain publication and MAC power writes execute through Blobray. No power or gain
callee is substituted. The profile tests rounding, saturation-branch selection
and signed-byte wrap, and is a vendor characterization, not a production API
comparison. The [tracking contract](../../../../crates/hardware/esp32s31/phy/src/tracking/README.md)
describes why this optional test-power policy is distinct from runtime calibration.


## Contract ownership

| Input | Responsibility |
| --- | --- |
| `vendor-project.toml`, `target.toml` | Investigation composition and architecture/ABI selection |
| Chip provider | Reusable chip identity, ABI and reviewed chip facts |
| Project provider | Exact-body models, authenticated applicability and comparison host composition |
| `profiles/`, `replays/`, `dispositions/`, verification policy | Declared inputs, production bindings and comparison claim limits |
| `functions/`, `interfaces/`, semantic and capability packs | Reviewed interpretation and stable identities |
| `reference/` | Human-readable pinned source/artifact contracts |
| `probes/` | Compiled calls into production code for comparison |
| `revisions/` | Immutable machine snapshot and review state |

The [provider ownership contract](blobray-provider/OWNERSHIP.md) defines
dependency direction and cache/applicability identity. The
[technical references](reference/README.md) describe Bluetooth Controller,
DTM, advertising, scanning, connection and IEEE 802.15.4 boundaries. They do not
serve as run results or operational readiness declarations.

## Register publication

Register review and publication now belong to the independent
[register tool](../../../../tools/registers/README.md). The legacy
`cargo blobray registers review/validate` and `project publish` command grammar
is unavailable in current Blobray. For source-owned publication use:

```console
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
```

Discovery observations feed the shared reviewed register model. Publication
produces SVD, raw PAC, closed PAC API and binding index from explicitly selected
inputs; observations do not invent hardware semantics. The source-only
composition validates the same publication outputs without private artifacts.

## Verification boundary

`verification-policy.toml` selects independent review scopes, function
requirements and bounded properties. Dispositions identify exact production
entries, shared production core or projections and set a maximum comparison
claim. They do not supply execution truth. Only generic comparison of the
declared compiled production boundary can establish its observed relation.

The qualification evaluator is the readiness authority. HIL provenance in a
disposition is a reference to supporting evidence, not a substitute for an
authenticated run or a qualification result.

For CLI concepts and schemas, see [Blobray](../../../../tools/blobray/README.md).

The PHY research scenario also checks the eleven absolute relocation entries in
`phy_i2c.o` `.rodata` against independently established physical symbol indices
and a captured-byte digest. It reviews that pointer layout, exports it and checks
identical output after restore. These local code-label references do not establish
callback ABI or new function boundaries.

For the ROM `phy_get_i2c_mst0_mask` callback, the scenario checks the independently
read global-pointer address `0x2f07fc3c` and slot `+8`. It reviews only that physical
load path, leaving signature and semantic binding unknown, then compares the
selected interface query and JSON export after source removal and backup/restore.
No callback model, resolved callee or hardware assertion follows from this review.

## Shared Next scenario preparation

[`harness.rs`](scenarios/src/harness.rs) and [`session.rs`](scenarios/src/session.rs)
own supervised invocation, authenticated input capture, request diagnostics,
exact symbol selection and the shared memory/phase/comparison builders. The I2C, transport and calibration scenarios keep their independent
expected values and explicit peripheral assumptions. The research scenario uses
the same runner and capture operations.

The comparison runner exports `.blobray.probes` from the captured production ELF
through native `inventory` and `data` operations. Every declared entry must resolve;
selected production calls must be catalog members. Named scalar arguments are
lowered with signed range checks. Reference parameters are explicit guest
addresses, not host pointers or inferred private layouts. Unsupported argument
types require an explicit adapter. `Buffer` declarations derive size and alignment for references to primitive
fixed arrays, including nested arrays, and generate memory regions automatically.
Addresses, byte seeds, lifetime, reset, models and observation relations remain
scenario choices. Opaque owner/layout types require explicit adapters. Word padding requires
both an explicit count and fill value; unknown memory stays unknown.

The generated requests use the existing Next execution format. Replay reads the
retained request and captured bytes, including the catalog; it does not regenerate
requests from scenario code. The combined I2C route also checks
[compiled call boundaries](scenarios/src/harness_edges.rs), then restores and replays their
evidence alongside positive and negative PHY comparisons.

Run preparation-only regressions without private inputs:

```console
cargo test --manifest-path tools/blobray/Cargo.toml -p oer-esp32s31-vendor-scenarios
```
