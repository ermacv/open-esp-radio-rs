# Make a first contribution without hardware

This tutorial exercises existing production policy and a synthetic research
fixture. You need Rust basics, Linux for Blobray, Git and access to public Cargo
dependencies. No board, lab configuration or private vendor binary is needed.
Read [the end-to-end explanation](binary-to-station.md) alongside this exercise.

## Prepare the checkout

Run from the repository root. Install Rust through rustup; the checkout's
`rust-toolchain.toml` selects the compiler and components. Native host tools need
a C linker and build utilities (on Debian/Ubuntu, `build-essential` and
`pkg-config`). The full documentation checkpoint also needs the configured
RISC-V target and `llvm-tools-preview`; those are separate from this host exercise.

```console
rustup show active-toolchain
cargo fetch --locked
```

Fetching populates the dependency cache from public sources. The following
commands deliberately use `--offline`; an empty cache is a setup error, not a
test failure or a reason to change the lockfile.

## Follow one scan and its error return

```console
cargo test -p oer-wifi-sta scan::tests --locked --offline
```

The current selector runs six tests. Confirm a nonzero executed-test count and
successful results. Read the [policy contract](../crates/protocols/ieee80211/sta/README.md),
[`scan.rs`](../crates/protocols/ieee80211/sta/src/scan.rs) and its
[tests](../crates/protocols/ieee80211/sta/src/scan/tests.rs).

Start with `selected_candidate_follows_the_complete_ordered_channel_plan`:
the fake backend sees channels `[1, 6, 11]` in order before candidate selection.
Then inspect `channel_failure_returns_the_exact_owner_and_stops_the_plan`:
failure on channel 6 returns the backend's current owner, starts two channels,
completes one and never visits channel 11. The numeric owner in this fixture is
a test token; production owners carry resource authority.

For a small exercise, change that test locally to fail on channel 11 and update
its expected owner, error, progress and event sequence from the fake backend's
behavior. Run the focused test, then the whole scan selector. Inspect the diff
and restore just your exercise edits when finished. This explores a failure
boundary; adding the same case without a missing contract is not automatically
a useful upstream contribution.

The tests establish portable ordering and owner return. They do not execute
channel tuning, DMA or RF hardware. Follow the
[runtime-to-PHY explanation](binary-to-station.md#production-layers-during-a-scan)
to locate those responsibilities.

## Try synthetic binary discovery, review and export

```console
cargo test -p blobray-next --test functions register_discovery_review_conflicts_and_source_free_export_share_scope --locked --offline
```

This selector runs one test in the
[existing register research fixture](../tools/blobray/next/tests/functions/registers.rs).
It constructs a small RV32 ELF in temporary storage, discovers MMIO observations,
proposes and reviews a physical declaration, checks conflicts and exports saved
results after the original source is removed. Read the assertions to distinguish
observed access widths from reviewed physical widths. Temporary inputs and
outputs are owned and cleaned up by the test.

This is an application regression scenario, not a CLI session and not real
ESP32-S31 evidence. Its test harness does not require a delegated cgroup.
For later CLI investigations choose a resource backend explicitly; where cgroup
delegation is unavailable use `--limit-mode watchdog`. See the
[Blobray task map](../tools/blobray/README.md#choose-a-task).

### Read the observations before the declaration

Keep the test source open while following these checkpoints. The assertions
are the expected results; the normal test output reports pass/fail rather than
printing a research report. The fixture is synthetic, so its names and values
describe this exercise only.

| Checkpoint in the test | Expected observation | What you have learned |
| --- | --- | --- |
| `analyze(&f)` and the first register query | One selected analysis, no declarations, and at least one unresolved address | Analysis can finish while selected facts remain unknown |
| `RegisterRecord::Address` and `Observation` assertions | Word accesses at `0x20000`, read-selection and write-replacement masks | Instructions reveal accesses and manipulated bits; they do not name a physical register |
| `KnowledgeClaim::MmioRegister` | A proposed four-byte `CONTROL` register with a `MODE` field | The test author supplies an interpretation explicitly |
| Review with `ReviewDecision::Accept` | Accepted bindings, including a byte access at `0x20001` contained in the wider register | Instruction width and physical register width are different facts |
| Propose a two-byte declaration for the same subject | A visible conflict; accepting it fails | Review cannot silently overwrite conflicting meaning |
| Query with `knowledge: None` | No declarations | A query does not implicitly select the current knowledge head |
| Remove original files, export, backup and restore | Identical saved query and exported bytes | Retained capture and review support source-free reading |

The fixture also loads through an unknown input pointer. Do not interpret
that unresolved address as absence of another register access. The masks
describe this synthetic instruction sequence, not reviewed ESP32-S31 fields.

Before reading each assertion, predict whether it concerns an observation,
an interpretation or preservation. Then check your answer against the table.
In particular, explain why accepting `CONTROL` does **not** generate a PAC:
production publication consumes a separately reviewed hardware model and API
policy. Continue with the [real channel example](channel-walkthrough.md#2-find-the-accepted-hardware-meaning)
to see that boundary in the repository.

### Match the exercise to operator commands

The fixture invokes the application API for analysis and review, and CLI helpers
for export and backup/restore. In an operator session, the corresponding command
families are:

| Exercise step | Operator entry point | Detailed input contract |
| --- | --- | --- |
| Capture and select code | `init`, `import`, `inventory`, `select` | [Capture and selection](../tools/blobray/next/reference/capture-images/README.md) |
| Analyze a function | `analyze-function` | [Function analysis](../tools/blobray/next/reference/analysis/README.md#function-analysis-contract) |
| Inspect candidates | `registers` | [Saved register research](../tools/blobray/next/reference/registers-data/README.md#saved-register-research) |
| Propose and review | `knowledge propose-register`, `knowledge accept` | [Knowledge and review](../tools/blobray/next/reference/knowledge-review/README.md) |
| Preserve the investigation | `backup`, `restore` | [Preservation](../tools/blobray/next/reference/knowledge-review/README.md#knowledge-and-preservation) |

For command discovery without changing a project, run from the repository root:

```console
cargo blobray --help
cargo blobray registers --help
cargo blobray knowledge --help
```

For an actual investigation, supply the exact selectors and requests documented
by those references. Temporary fixture IDs are not reusable operator inputs.

## Turn understanding into a contribution

Pick a documented limitation or missing behavior check from the
[capability map](../qualification/README.md#everyday-status-and-next-work).
Useful work without hardware includes portable policy regressions, codecs,
documentation, synthetic analysis fixtures and host tooling. Check the owner
and existing tests before changing behavior.

Choose a bounded first contribution: explain a confusing handoff with source
links, add a missing failure-path assertion to an existing policy test, or
reproduce an analysis gap with a synthetic fixture. State the missing behavior
or explanation before proposing a change. Board-dependent RF behavior and
vendor comparisons have additional prerequisites; the [hardware route](station-hardware.md)
names them separately.

Follow [CONTRIBUTING](../CONTRIBUTING.md) for checks and PR preparation.
Continue with [the hardware route](station-hardware.md) when you have the
required artifacts or board; neither tutorial claims vendor equivalence.
