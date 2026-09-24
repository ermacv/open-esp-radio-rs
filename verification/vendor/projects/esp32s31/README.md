# ESP32-S31 vendor-analysis project

## Captured PHY research with Next

[phy_research.py](phy_research.py) exercises the current Next application using
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
cargo build --profile blobray -p blobray-next --bin blobray
python3 verification/vendor/projects/esp32s31/phy_research.py \
  --binary target/blobray/blobray --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf --linker /usr/bin/ld.lld --limit-mode watchdog \
  --output target/blobray-phy-research
```

The script uses 256 MiB working capacity, a 600-second deadline and a shared work
budget per operation. Select kernel mode only in an environment with delegated
cgroup memory control; watchdog is an explicit sampled-RSS policy. Run JSON files
are the measurement authority, not this documentation.

## Captured I2C command-memory comparison

[phy_i2c.py](phy_i2c.py) compares the authenticated archive/ROM command initializer
and its descriptor/no-op leaves with a freshly built production probe ELF. It
checks all 45 ordered command writes against independent instruction-derived
expectations on zero, mixed, lower and upper parameter profiles. Descriptor
outputs are checked byte for byte from both zero and filled initial memory.
The command bank is an explicit passive register model; analog bus transactions,
completion timing and RF behavior are outside this scenario.

```console
cargo xtask build vendor-probes --chip esp32s31
python3 verification/vendor/projects/esp32s31/phy_i2c.py \
  --binary target/blobray/blobray --library /private/libphy.a \
  --rom /private/esp32s31_rev0_rom.elf \
  --production target/verification/esp32s31-probes/riscv32imafc-unknown-none-elf/release/open-esp-radio-verification-esp32s31-probes-elf \
  --linker /usr/bin/ld.lld --nm /usr/bin/llvm-nm --limit-mode watchdog \
  --output target/blobray-phy-i2c
```

The runner owns scenario construction and independent expected-value assertions;
ordinary Next application operations own capture, linking, execution, comparison
and persistence. No provider pack or legacy engine is selected. `--nm` independently
identifies `phy_param` in the exported linked ELF; inexact source mappings are not
used to infer that address. All inputs are captured before local source copies
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
target/blobray/blobray-run --project verification/vendor/projects/esp32s31/vendor-project.toml \
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
target/blobray/blobray-run --project verification/vendor/projects/esp32s31/vendor-project.toml \
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

## Current PHY calibration gate

The `phy-calibration-leaves` suite compares four production-bound leaves against
the pinned esp-phy-lib archive. Its vendor entry authenticates both the archive
and rev0 ROM by SHA-256 before execution. Keep the original `archive` binding
for the existing investigation; the new suite uses `phy-current`.

Bind `source-artifact:phy-current` to the archive and
`source-companion:phy-current` to the ROM in the ignored run spec. The
`rust-artifact` role must point to a fresh production probe ELF.

```console
target/blobray/blobray-run --project verification/vendor/projects/esp32s31/vendor-project.toml \
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

The separate channel comparison executes `phy_get_romfunc_addr` before
`phy_chip_set_chan` in the same execution session. This retains the actual
temperature, TX-gain and RX-compensation callbacks rather than manually
selecting ROM replacements. Its temperature-prefix profile compares effects
through the first gain-publication boundary with all five valid sensor ranges;
it makes no claim about gain publication or whole-channel equivalence.
Both channel profiles separately compare committed channel, bandwidth and
temperature. This is channel state, not the enclosing RXCAL reference commit.
A stuck-readiness profile requires failure without gain or semantic publication.
The complete-root gate includes gain publication using the current archive's
actual callback and coefficient profile. It compares all ordered channel effects
without filtering gain writes; it does not qualify the enclosing RXCAL parent.

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

```console
cargo blobray registers review --project verification/vendor/projects/esp32s31/vendor-project.toml
cargo blobray registers validate --project verification/vendor/projects/esp32s31/vendor-project.toml
cargo blobray project publish --project verification/vendor/projects/esp32s31/vendor-project.toml
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
