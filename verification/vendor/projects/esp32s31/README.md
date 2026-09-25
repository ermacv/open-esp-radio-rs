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

## All PHY comparison scenarios

`all` runs every stage-11/12 PHY comparison scenario (`gain`, `i2c`, `channel`,
`rx-gain`, `tx-dc`, `tracking`) in sequence under one budget, each in its own
directory below `--output`. It requires every optional input, so no obligation
is left unmet, stops at the first failure and prints each scenario's duration.
With `--index <path>` it then writes the native evidence index qualification
reads: every claimed vendor root with its production entry, compared cases and
retained executions, the input identities and the digests of the sources the
verdicts depend on. Regenerate
`evidence/scenario-evidence.json` after any production, probe, scenario or
Blobray change; until then qualification treats the index as stale. See
[vendor verification](../../../../docs/verification-and-qualification.md#vendor-verification-path).

```console
cargo xtask vendor-scenario all --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf \
  --production target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/open-esp-radio-verification-esp32s31-probes-elf \
  --sdk /private/bootloader.elf --phy-sdk /private/phy_tracking_reference.elf \
  --rftest /private/librftest.a \
  --linker /usr/bin/ld.lld --output target/blobray-research/all --limit-mode watchdog \
  --index verification/vendor/projects/esp32s31/evidence/scenario-evidence.json
```

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
  --production target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/oer-verification-esp32s31-probes-elf \
  --linker /usr/bin/ld.lld --limit-mode watchdog \
  --sdk /private/bootloader.elf --phy-sdk /private/phy_tracking_reference.elf \
  --output target/blobray-phy-i2c
```

`--sdk` enables the calibration leaves and the PBus/DCODE prefix. `--phy-sdk`
requires `--sdk` and adds RFPLL. Without them the command-memory, transport and
call-boundary scenarios still run. Each missing obligation is recorded in
`unmet-obligations.request.json`, and the scenario exits with status 2.

The scenario owns construction and independent expected-value assertions;
Blobray operations own capture and linking, and Blobray's in-process
verification owns execution and comparison. The scenario
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
exhausted event capacity must fail with a resource limit. There is no hardware
claim.

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
relation selects read-operation returns and a reviewed effect contract per
vendor/production pair: production performs a pre-issue readiness check, so the
contract ignores analog I2C port reads, and every other read, write, fence and
delay compares exactly, including the read-mask and host-map accesses. Void
returns are explicitly excluded.

Exhausted scripted replies must produce INCOMPLETE without retained-value
fallback. An out-of-field write must produce DIFF: captured code spills the
value into an adjacent bit while the typed production path clips it. Limited
completion edges and the shipping reset polling bound must leave pending-model
INCOMPLETE on timeout; they cannot become MATCH merely because a CPU returned.
This establishes the selected software
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
register input must produce INCOMPLETE, and exhausted event capacity must fail
with a resource limit. These leaves do not
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

The DCODE relation compares each of the eight selected final bytes and every
effect under a reviewed contract. Production performs extra I2C busy prechecks,
so the contract ignores reads of the two command ports; every other read, write,
fence and requested delay compares exactly. The runner independently checks the
four frequency programs, NRX values, forty CKGEN commands, sample consumption and
exact output `[0,63,31,32,32,31,63,0]`. The DCODE and PBus matrices are one
request each, with a stack fill per case. A comparison without the contract
retains the expected polling DIFF.

Stuck channel readiness, stuck I2C and a read/modify/write whose halves individually
fit but jointly exhaust the production budget must return their specific failure,
leave all eight initialized output bytes unchanged and stop issuing commands at
the failed boundary. Changed samples produce DIFF, an unknown parameter pointer
produces INCOMPLETE, and insufficient event capacity fails with a resource
limit. This is a software prefix comparison under the
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

Search comparison selects the low return; maintenance omits the vendor's void
return. Both compare every effect under a reviewed contract that ignores analog
I2C port reads; every other read, write, fence and delay compares exactly. For
nonzero maintenance only, the vendor samples channel status once more
immediately before reading frequency control: the installed layout query, whose
count and exact value the runner checks. The maintenance contract lets
production omit that one read; production owns that fixed layout. Search and
maintenance are one request each, with a stack fill per case. A comparison
without the contract retains the polling DIFF. Changed capacitor input, unknown callback pointer, unmapped diagnostics and
event exhaustion have explicit DIFF/INCOMPLETE/resource-limit checks. Software
observations do not establish hardware/RF or grant qualification.

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
  --production target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/oer-verification-esp32s31-probes-elf \
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
  semantic inputs are modeled.
- The channel, RX-gain and TX-DC roots publish their committed `phy_param`
  fields through a reviewed output projection: Blobray compares each field's
  final vendor bytes with its production output location, and output padding
  is not claimed.
- The channel, RX-gain and TX-DC roots compare their effects under a reviewed
  effect contract ([`contracts.rs`](scenarios/src/contracts.rs)). The contract
  is a typed value reviewed through git; each root's relation selects it by the
  digest of its canonical encoding, and the scenario supplies it with the
  in-process comparison. Blobray
  evaluates it with `unclassified: required`: analog I2C transport reads,
  read-mask and host-map writes, and the single-microsecond wait before a
  transport or declared status read are ignored plumbing; every other MMIO,
  fence and delay effect compares exactly, in order and value. Because every
  read compares, both sides necessarily consume the same never-written
  registers. Each verdict carries the `reviewed-effect-refinement` claim.
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
must fail with a resource limit. These are software gain-child comparisons, not RF,
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
- event exhaustion, which fails with a resource limit.

## Inputs and probes

Private vendor artifacts are explicit scenario arguments; they are captured into
ignored scenario storage and never enter the repository. Never commit vendor
binaries, disassembly dumps, unreviewed extraction artifacts or private paths.
Necessary recovered hardware tables belong in production source with provenance
and consuming-code verification, as defined by the
[source policy](../../../../docs/source-policy.md).

Build the Rust probe images with `cargo xtask build vendor-probes --chip esp32s31`;
`--list-roles` lists them without building. The scenarios consume the
`rust-artifact` production probe ELF. Normal builds use Cargo's parallelism; set
`OPEN_RADIO_ANALYSIS_BUILD_JOBS` only to impose an explicit local resource limit.

The open ESP-IDF IEEE 802.15.4 controller is ported from source and requires no
controller archive or ELF. Its closed baseband and coexistence functions have no
native vendor comparison.

## Coverage decisions

Every claimed vendor root reports the coverage of its closure: the code
statically reachable from the root through direct transfers and the observed
targets of executed indirect ones, excluding call models. Each uncovered block
or branch direction is either excluded by a reviewed decision in
[`coverage.rs`](scenarios/src/coverage.rs), with its reason, or listed as
untriaged in the evidence index. Decisions currently exclude vendor runtime
helpers (diagnostic formatting, compiler arithmetic and copy helpers, prologue
millicode) as whole functions. A decision on a closure function that is fully
covered fails its scenario. Untriaged locations are pending work: each becomes a
follow-up case or a reviewed decision.

## Mutation run

`vendor-scenario mutants` asks whether the scenarios notice a single-point
defect in a named region of the production PHY they compare. A run is always
targeted: each `--target FILE[:START-END]` selects a source file, optionally a
line range, and only mutants there run. A mutant costs an incremental LTO probe
rebuild and a full rerun of every scenario reaching it, including the unchanged
vendor side, so a whole-crate run takes hours and is not a routine check.

```console
cargo xtask vendor-scenario mutants --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf --linker /usr/bin/ld.lld \
  --sdk /private/bootloader.elf --phy-sdk /private/phy_tracking_reference.elf \
  --rftest /private/librftest.a --output target/verification/mutants \
  --limit-mode watchdog --workers 4 \
  --target crates/hardware/esp32s31/phy/src/analog/temperature.rs:130-160 \
  --scenario channel
```

Tracked files must equal `HEAD`: each worker checks out a detached worktree of
`HEAD` below `--output` with its own Cargo target directory, reused by later
runs. A baseline run of every scenario records, through `--reach`, the
production probe instructions its retained executions reached; the probe's
debug line information maps them to source lines of `--scope` (default the
production PHY crate), including inlined frames. Mutants apply only to those
lines of the targets: an integer literal or scalar integer constant plus one, a comparison
replaced by its boundary neighbor or negation, a negated branch condition,
`min`/`max` exchanged and two adjacent register writes exchanged. Repeated
`--scenario` options restrict the baseline and every mutant to those scenarios,
so a check that new cases kill known survivors runs only those cases. A surviving mutant
matching a reviewed decision in [`mutation.rs`](scenarios/src/mutation.rs) (file,
trimmed source line and replacement, with its reason) is reported as `REVIEWED`;
a decision on a mutant the scenarios now kill is reported as `STALE-DECISION`. Each mutant is
rebuilt and runs the scenarios whose baseline reached its lines; the first
failing scenario kills it. A mutant whose executable sections equal the
baseline's is equivalent and does not run; one that does not build is unviable.
Each finished mutant is appended to `journal-<commit>.jsonl` below `--output`;
a rerun at the same commit keeps those results and runs only the rest.
`mutants.json` below `--output` lists every mutant with its scenarios, outcome
and duration; surviving mutants are printed and become follow-up cases or
reviewed decisions.

## Contract ownership

| Input | Responsibility |
| --- | --- |
| `scenarios/` | Typed comparison scenarios, their expected verdicts and claim tables |
| `evidence/` | Committed native evidence index consumed by qualification |
| `probes/` | Compiled calls into production code for comparison |
| `reference/` | Human-readable pinned source/artifact contracts |

The [technical references](reference/README.md) describe Bluetooth Controller,
DTM, advertising, scanning, connection and IEEE 802.15.4 boundaries. They do not
serve as run results or operational readiness declarations.

## Register publication

Register review and publication belong to the independent
[register tool](../../../../tools/registers/README.md):

```console
cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check
```

Discovery observations feed the shared reviewed register model. Publication
produces SVD, raw PAC, closed PAC API and binding index from explicitly selected
inputs; observations do not invent hardware semantics. The source-only
composition validates the same publication outputs without private artifacts.

## Verification boundary

A scenario compares vendor execution with a compiled production probe entry and
checks each expected `MATCH`, `DIFF` or `INCOMPLETE`. Only a root that compared
with `MATCH` in every retained execution selecting it becomes an index entry.
The [qualification evaluator](../../../../qualification/README.md) is the readiness
authority; the index supplies vendor evidence only while its source digests are
current. For CLI concepts, see [Blobray](../../../../tools/blobray/README.md).

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
own supervised setup operations, authenticated input capture, in-process comparison,
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

Setup results (the imported revision, its inventory, the probe catalog, linked
images and data exports) are memoized in [`setup-cache`](scenarios/src/setup_cache.rs)
below the scenario output, keyed by the Blobray executable, the authenticated input
bytes and the operation's request. A warm run creates no Blobray project; a miss
creates it once and checks that its revision equals the cached one.

The generated requests use the Next execution format and run through Blobray's
in-process verification over the authenticated input bytes and the exported
linked image: no project, content store or journal participates, and records stay
in memory. A run keeps only its results: claim verdicts, case counts, coverage
and input/source digests. The combined I2C route also checks
[compiled call boundaries](scenarios/src/harness_edges.rs) alongside positive and
negative PHY comparisons.

Run preparation-only regressions without private inputs:

```console
cargo test --manifest-path tools/blobray/Cargo.toml -p oer-esp32s31-vendor-scenarios
```
