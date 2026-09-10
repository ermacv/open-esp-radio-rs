# ESP32-S31 executable reconstruction provider

This crate owns temporary handwritten control flow, intrinsic interpretation,
body applicability checks, and composition with the reusable chip runtime
model provider. Declarative semantic meaning, RAM classifications and ABI
contracts remain in sibling knowledge/contracts crates; dependency direction
is models → knowledge, with no dependency in reverse.

`PROVIDER.kind = ManualReconstruction` makes this implementation's nature
explicit to the host and `project doctor`. Its applicability/evidence fields
are provenance metadata, not accepted facts or an equivalence verdict. Hooks
must independently check the exact body and required context before returning
a reconstruction. A mismatch preserves structural analysis fallback.

RFPLL polling/search, IQ polling, I2C operation sequences, scratch lifetime and
assertion branching remain temporary executable models. Moving them here does
not make them generated analysis. Replacing them with generic instruction
reconstruction is a separate backend task; their current exact-body guards,
entry checks and regression tests are retained during that transition.

The host registers this provider separately from declarative knowledge.
Its revision participates in project cache identity, and registry validation
requires the same model ID/revision in the function-analysis harness domain.
The base model provider is included in both composition identities.

Run source-only regressions with:

```console
cargo test -p open-radio-vendor-models-esp32s31 --lib
```

The optional authenticated-ROM mutation test additionally requires a
caller-owned `BLOBRAY_REVIEWED_ROM` path and checks the reviewed whole-image
SHA before reading any body. See [`../OWNERSHIP.md`](../OWNERSHIP.md) for
applicability and the reason no DTM caller-domain bound is supplied.

## RFPLL comparison

`phy_i2c::Rfpll` models the reviewed PHY-I2C command port and an analog register
bank. Both host ports access the same bank. Each read command for capacitor
status consumes one caller-supplied sample; repeated completion polling does
not consume another sample. Command completion can be delayed independently.
Unknown commands, unknown reads, an overwritten pending command and unused
samples are explicit errors or incomplete coverage. The model does not choose
capacitors or calculate corrections.

`tests/phy_rfpll.rs` executes the authentic archive/ROM and a compiled
production probe through Blobray. Archive and ROM hashes are checked before
execution; the probe hash is emitted with the result. The comparison boundary
is deliberately narrower than full timing equivalence:

- Search: ordered I2C commands, returned signed correction and requested
  five-microsecond settling delays. Stack padding and completion latency vary.
- Frequency control: the admitted `phy_rfpll_cap_track_new` branch and the
  production `maintain` child with a zero measured correction. Ordered
  frequency MMIO, both readbacks, I2C commands and settling delays are compared.
  A stateful register model retains control writes; readback scripts must be
  consumed. The real archive's weak grant functions execute unchanged.
- Failure containment: the production child must terminate on I2C timeout
  without restoring hardware frequency control. The still-pending peripheral
  is reported as incomplete, not as successful hardware completion.
- Nonzero memory correction: every frequency-memory read/write and channel
  restoration are compared for positive and negative corrections, including
  arithmetic underflow/overflow. The scenario binds the layout installed by
  production cold init and permits the vendor's one additional layout query.
  Other frequency MMIO stays ordered and exact; all 85 result samples must be
  consumed. Channel numbers and explicit frequencies exercise restoration.
- Thermal child: threshold boundaries, both drift directions, debug overrides,
  skipped/executed outcomes and the resulting reference are compared with the
  compiled production `track` entry. Busy admission and result flags remain
  vendor characterizations: OER requires an admitted physical owner rather
  than copying the busy byte. The registered parent still disables RFPLL;
  these comparisons do not qualify its full scheduling/calibration graph.

The RX calibration comparisons are described in the project
[calibration boundary](../../README.md#current-phy-calibration-gate).
Their production executors are shared with runtime maintenance; probe wrappers
supply only isolated capabilities and semantic inputs. Output storage starts
with a sentinel so a failed child cannot be mistaken for published coefficients.

The channel comparison retains callback bodies in the analysis link and executes
the archive's callback installer in the same session. It uses explicitly seeded
temperature and BBPLL-control models; BBPLL read/write effects are not filtered
as I2C transport. The temperature-prefix test excludes TX gain and later channel
effects explicitly. The complete-root test remains a failing `DIFF` boundary
until production's ROM TX-gain coefficients are reconciled with the current
archive callback. Do not replace that callback or suppress the full-root failure
when reporting channel or RXCAL equivalence.

The complete channel diagnostic reports every differing case and effect location
before failing. It compares the committed channel/width/temperature separately
from MMIO. `tx_gain.rs` compares the real gain publisher against the production
channel binding using synthetic caller-owned input components, and characterizes
the two vendor callbacks' distinct additive/subtractive inputs. The latter is
a vendor ABI characterization, not a production equivalence test. Neither test
supplies a vendor calibration table to production or establishes RF quality.

Production I2C completion waits of one microsecond are reported separately;
ROM busy polling and those waits are excluded from the command projection.
This does not establish timing parity, PLL lock, coexistence safety, other
memory layouts, physical validity of out-of-range corrections, or equivalence
of the registered parent's policy and temperature state semantics.

Build the [production probe](../../probes/README.md), then build the private-input
test executable:

```console
cargo test -p open-radio-vendor-models-esp32s31 --test phy_rfpll \
  --profile blobray --no-run
```

Run the executable path printed by Cargo through the resource limiter:

```console
BLOBRAY_LIMIT_BACKEND=watchdog \
BLOBRAY_BINARY="<absolute test executable path printed by Cargo>" \
OER_PHY_ARCHIVE="<absolute path to the pinned libphy.a>" \
OER_PHY_ROM="<absolute path to the pinned rev0 ROM ELF>" \
OER_PHY_PROBE="<absolute path to the newly built probe ELF>" \
target/blobray/blobray-run --ignored --nocapture
```

The watchdog backend preserves the explicit input environment while enforcing
resource limits. Tests are ignored in normal workspace runs because these
inputs are private. Store generated output under the project's ignored target
directory; neither vendor binaries nor comparison reports belong in Git.

`tests/phy_rfpll/graph.rs` executes the authenticated current
`phy_param_track_tot` and `phy_cal_param_track` bodies with explicitly declared
no-effect child responses. It verifies parent call order, ABI arguments,
RX/shared-TX reference publication and separate grant brackets. The calibration
fixture supplies a restoration callback in `g_phyFuns`; it does not verify the
callback selected by vendor startup. Grant calls here execute the archive's
weak bodies; strong `libcoexist.a` linking and hardware grant effects are outside
this projection. These are vendor-boundary characterizations, not full
vendor/production hardware equivalence. The compiled production transitions
have separate ownership/order/partial-failure regression tests in PHY.

## Linked COEX protection characterization

`tests/coex_grant.rs` accepts a coex-enabled ESP-IDF ELF and the authenticated
rev0 ROM. It executes adapter registration and ROM callback-table installation,
then the strong PHY protection hooks through the linked request and ROM release.
The real allocator and logger are modeled at their OS boundary; timer MMIO is
modeled as R/W bus storage with explicit clock inputs. The test checks callback
identity, repeated initialization, request/release bus order and software status
access. It does not model RF arbitration, timer advancement, grant latency or
concurrent clients and is not vendor/production equivalence.

Build with `cargo test -p open-radio-vendor-models-esp32s31 --test coex_grant
--profile blobray --no-run`, then pass the printed executable to `blobray-run`:

```console
BLOBRAY_LIMIT_BACKEND=watchdog \
BLOBRAY_BINARY="<absolute test executable path printed by Cargo>" \
OER_COEX_ELF="<absolute linked vendor ELF path>" \
OER_COEX_ELF_SHA256="<reviewed SHA256 of that ELF>" \
OER_PHY_ROM="<absolute path to the pinned rev0 ROM ELF>" \
target/blobray/blobray-run --ignored --nocapture
```

The ELF hash is required to identify the exact build. It does not authenticate
the source revision by itself: retain the IDF/submodule revisions, archive hashes,
sdkconfig and linker map with the ignored run evidence. Strong hook selection
must be checked in that map; a coex-disabled build may retain weak PHY hooks.
