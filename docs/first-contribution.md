# Make a first contribution without hardware

This tutorial exercises existing production policy and a synthetic research
fixture. You need Rust basics, Linux for Blobray, Git and access to public Cargo
dependencies. No board, stand file or private vendor binary is needed.

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
cargo test -p oer-ieee80211-sta machine::tests --locked --offline
```

Confirm a nonzero executed-test count and successful results. Read the
[station's contract](../crates/protocols/ieee80211/sta/README.md), the
[station machine](../crates/protocols/ieee80211/sta/src/machine.rs) and its
[tests](../crates/protocols/ieee80211/sta/src/machine/tests.rs), which play the
port and run the real [channel arbiter](../crates/protocols/ieee80211/arbiter/README.md).

Start with `a_station_alone_scans_joins_an_open_network_and_leaves_it`: the
station leases channel 1, then channel 6, where the beacon arrives, and joins
there. Then inspect `beside_an_access_point_the_scan_goes_in_absences_after_its_tbtts`:
beside an access point the visit of channel 6 waits for the absence after the
access point's TBTT and the port returns before its next beacon. The tickets,
buffers and places in this fixture are test tokens; production tokens carry
resource authority.

For a small exercise, change the access point's beacon interval in that test
locally and update the expected instants from the arbiter's absence rule. Run
the focused test, then the whole selector. Inspect the diff and restore just
your exercise edits when finished. This explores a timing boundary; adding the
same case without a missing contract is not automatically a useful upstream
contribution.

The tests establish portable ordering and token return. They do not execute
channel tuning, DMA or RF hardware.

## Try synthetic binary discovery

```console
cargo test --manifest-path tools/blobray/Cargo.toml -p blobray-cli --test library every_library_function_reports_its_masked_and_unresolved_accesses --locked --offline
```

This selector runs one test in the
[library register-access fixture](../tools/blobray/cli/tests/library.rs).
It builds a small RV32 archive of two identical members in memory, analyzes
every function in process and reports the memory addresses they access. Read
the assertions to distinguish what instructions reveal from what a register
model declares.

This is an application regression scenario, not a CLI session and not real
ESP32-S31 evidence. See the [Blobray task map](../tools/blobray/README.md#choose-a-task).

### Read the observations

Keep the test source open while following these checkpoints. The assertions
are the expected results; the normal test output reports pass/fail rather than
printing a research report. The fixture is synthetic, so its values describe
this exercise only.

| Checkpoint in the test | Expected observation | What you have learned |
| --- | --- | --- |
| `summary.functions` and `unresolved_addresses` | Two analyzed functions and at least one unresolved address | Analysis can finish while selected facts remain unknown |
| The distinct `symbol.object` values | One occurrence per archive member | Identical members stay distinct occurrences of one archive |
| `RegisterAccess::Observation` masks | Word accesses at `0x20000`, read-selection and write-replacement masks | Instructions reveal accesses and manipulated bits; they do not name a physical register |

The fixture also loads through an unknown input pointer. Do not interpret
that unresolved address as absence of another register access. The masks
describe this synthetic instruction sequence, not reviewed ESP32-S31 fields.

Before reading each assertion, predict whether it concerns an observation or
an identity. Then check your answer against the table. In particular, explain
why the observed word access at `0x20000` does **not** generate a PAC:
production publication consumes a separately reviewed hardware model and API
policy.

### Match the exercise to operator commands

The fixture calls `blobray_application::library::register_accesses`. The
operator command runs the same operation over real libraries:

```console
cargo blobray --help
cargo blobray register-accesses --help
```

Its input contract is [library register accesses](../tools/blobray/cli/reference/registers-data/README.md#library-register-accesses).

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
vendor comparisons have additional prerequisites.

Follow [CONTRIBUTING](../CONTRIBUTING.md) for checks and PR preparation.
Neither tutorial claims vendor equivalence.
