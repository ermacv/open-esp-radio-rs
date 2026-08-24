# Verification

Verification answers whether observed vendor behavior agrees with a named
Rust binding under an explicit contract. It is supporting evidence. Product
readiness is decided only by the repository qualification ledger.

## Evidence classes

The report keeps proof strength visible:

- `production-trace`: concrete vendor replay compared with an exact compiled
  production entry;
- `shared-core`: concrete replay reaches production logic but not the exact
  integration entry;
- `static-analysis`: lifted or generated-reference evidence without concrete
  production execution;
- `reviewed` and `hil`: explicit external evidence.

Lower-strength evidence is useful for research and implementation, but cannot
be silently promoted to production equivalence.

## Observations, dispositions and bindings

Authenticated compiled vendor/Rust artifacts, execution inputs, and recorded
observations are the source of observed behavior. The generic engine owns the
comparison verdict. A disposition cannot alter either side of the trace.

A disposition states who owns the replacement and whether the vendor function
is direct, composed, a state transition, bounded, or not yet implemented. A
binding names the compiled Rust probe and classifies it as an exact production
entry, shared production core, generated reference or verification
projection.

Suites and comparison inputs live in `verification-addon.toml`, outside the
neutral project manifest. A generic project can therefore analyze and publish
register evidence without configuring production comparison. The chip pack's
optional knowledge provider may enrich lifting, generated references, and the
reviewed ABI inventory of opaque diagnostic calls, but it cannot execute a
production comparison or return a verdict. The generic verification engine
alone compares compiled artifacts and observations.

Effect contracts define the ordered observable reads, writes, calls, state
changes and allowed normalizations. Unlisted effects fail closed. Concrete
profiles define arguments, initial memory/device state, observations and
finite-domain preconditions.

Independent `mmio-domains` form a Cartesian product. When multiple register
words are meaningful only as complete correlated states, list the concrete
case names in `mmio-image-cases` instead. Blobray then uses each named case's
entire `mmio-initial` map as one indivisible static-coverage input image; it
does not infer combinations between words from different images. Every image
must contain the same address set and still remains an ordinary executable
case. The combined argument/MMIO coverage domain is checked before it is
materialized and fails closed above 4096 cases.

Profile schema 5 requires explicit `case-execution` and
`transaction-comparison` policies. `case-execution = "independent"` gives
every case a fresh vendor and Rust execution session. `case-execution =
"stateful"` treats two or more cases, in declaration order, as phases of one
vendor session and one Rust session. Stateful comparison carries writes to
writable ELF segments, explicitly declared persistent RAM, and modeled FIFO
contents across phases. The executor stack, MMIO read responses, and device
model instances remain phase-local; the profile must state each phase's
environment rather than inventing device state.

`transaction-comparison = "state-only"` is reserved for an explicitly
projected state contract. It compares the ordered before/after bytes of the
declared vendor and Rust observations (their artifact addresses may differ)
and an optional return value. Vendor-only MMIO/calls remain in the evidence
report; the result does not claim that those transactions are equivalent or
irrelevant. Its coverage scope is reported as `concrete-state-cases`, not as
static whole-CFG coverage.

`transaction-comparison = "state-and-reviewed-calls"` keeps the same
address-independent state contract and additionally requires declared
`call-equivalences`. Use it when the reviewed claim includes a semantic edge
such as exactly one re-publication but not every platform helper call.

A stateful profile may declare ordered `vendor-setup` phases. Each phase runs
an exact linked vendor symbol in the same execution session before the first
comparison case; its completion, calls, steps and memory changes are retained
in the report. Use this for real initialization such as `wdev_data_init` or a
bounded `lmacInit` prefix. Do not replace initialized `.data`, `.bss` or ROM
interface pointers with scenario fixtures. Side-specific `vendor-mmio-reads`
and `rust-mmio-reads` model environmental reads which only one artifact
performs without forcing the other side to consume a fictitious response.
Optimized code that copies otherwise-uninitialized struct or enum padding may
use side-specific `vendor-stack-fill` and `rust-stack-fill` bytes. The declared
domain must exercise at least two distinct fills and retain identical compared
observations; stack fill is an explicit execution condition, never an inferred
value.

The document-level policy is the explicit default for every profile in that
file. A profile may override it with its own `case-execution` field when one
suite contains both finite independent-domain cases and an ordered lifecycle.

Use `persistent-memory` when both artifacts use the same explicit RAM range,
or `vendor-persistent-memory` and `rust-persistent-memory` when linked layouts
differ. A later phase's explicit RAM seed overrides the carried byte for that
phase. Zero-length and overflowing ranges fail profile validation. An
incomplete stateful phase stops execution of the later phases because their
input state is no longer known. A completed `DIFF` remains a difference and
does not erase the concrete state needed to report later phases.

`observables` compares ordered MMIO, delay, and fence events;
`observables-under-effect-contract` compares that same concrete ordered stream
through the reviewed disposition effect contract, so an explicitly declared
device-ordering fence may be present on the Rust side while every unlisted
effect still fails closed;
`observables-and-calls` also compares every named call boundary;
`observables-and-reviewed-calls` compares only explicitly listed semantic
call pairs; and `full` additionally exposes branch and ordinary RAM state.
Call-site addresses remain provenance rather than equality keys. A vendor and
Rust call may share an operation only through a reviewed `call-equivalences`
entry; symbol spelling or a semantic hint never creates equivalence.
Call arguments can be compared exactly, ignored, or restricted to explicit
`argument-indices`. The selected form is for reviewed ABI projections where
the two leaves intentionally have different scratch/live-register shapes; it
must not be used to imply equality for the omitted positions.

Known C runtime and ecosystem service leaves are opaque semantic boundaries
when an add-on supplies their signature and bounded behavior. Their calls
remain visible facts, but Blobray does not recursively reconstruct an
available implementation merely because bytes for that implementation are
present. Unknown calls remain blockers.

Every target-aware run, comparison, profile verification, and event replay
installs the diagnostic call contracts from the `TargetSpec` knowledge
provider. A neutral target installs none. Comparison reports and replay
evidence record both the provider `id@analysis_cache_revision` and the actual
contracts sorted by `(symbol, argument_count)`. The same canonical identity is
part of execution-profile evidence and replay-cache fingerprints, so changing
a provider contract cannot reuse evidence produced under the old ABI list.
Explicit scripted responses remain scenario inputs; they are not deleted or
inferred from the provider contract.

Function identity is `(source, symbol)`, not symbol spelling alone. Probe names
never imply production ownership; dispositions provide that reviewed mapping
and an upper bound on the claim, not execution truth.

## Flat verification policy

`verification-policy.toml` contains independent surfaces of three kinds:

- `review-scope`: every selected vendor function must be accounted for;
- `selected-functions`: named requirements must have acceptable evidence;
- `bounded-property`: a finite reviewed property may pass without claiming
  whole-function equivalence.

The policy does not contain product phases, HIL campaigns or an umbrella
feature hierarchy. It consumes verification suites and produces pass/fail
supporting evidence. The qualification ledger references that evidence and
remains the only readiness authority.

## Production trace versus environment model

An environment model may supply explicit external-call and device responses.
It cannot replace either compared implementation, hide an effect, or prove
that manually written driver code follows the vendor operation order.

Migration happens one function at a time:

1. compile a thin probe that invokes the real production entry;
2. replay vendor and Rust under the same explicit environment;
3. compare ordered effects and fail on unresolved behavior;
4. bind the resulting production trace in the disposition;
5. delete obsolete probe/model glue after no current suite references it.

The former ESP32-S31 `phy_chip_set_chan` self-verdict contract has been
deleted. Its retained `0x1a00` versus `0x3fa00` observation exposed a binding
defect: it compared the cold ROM callback with production even though vendor
`phy_get_romfunc_addr` replaces the slot with archive
`phy_get_i2c_hostid_new`. Entry contracts may therefore declare
source-qualified function-table targets. That is provenance for an observed
runtime table value, not a global symbol override: direct cold-ROM calls remain
ROM calls, while post-registration indirect calls resolve to the archive body.
A new generic comparison must still bind and compare the complete compiled
shipping entry. The corrected callback identity cannot be promoted into a
whole-function match.

## Reports and gates

`MATCH`, `DIFF` and `INCOMPLETE` are distinct outcomes. Missing inputs,
unresolved effects and stale evidence never count as a match. A bounded match
applies only to its declared finite precondition and cannot become a
whole-function claim.

The RISC-V executor recognizes ABI facts, not vendor meaning. Absolute ROM
call-vector symbols retain their authenticated addresses, and standard
`memcpy`, `memmove`, and `memset` leaves have bounded concrete memory effects.
An unknown vector without linked code or an explicit model is an
`INCOMPLETE`-class execution blocker with a named remedy, never guessed code.

The replacement graph connects vendor identities, production components,
compiled probes, contracts and proof results without treating a passing probe
as proof of an unrelated driver implementation.

Candidate evidence is written separately and never overwrites an accepted
baseline. Reviewers compare artifact hashes, binding identities, evidence
class and contract before accepting it.

### Reviewing an intentional difference

An observed `DIFF` is never converted to `MATCH` by changing a disposition,
baseline, or broad normalization. Review it as a new bounded claim:

1. Pin both artifact identities, the exact production entry, input scenario,
   initial device/memory state, and the first differing ordered effect.
2. Classify the difference as a production defect, an unsupported artifact
   pairing, or a proposed platform refinement. Unknown provenance remains
   `DIFF`.
3. For a proposed refinement, declare the narrow precondition and the exact
   effect difference. Every other observed effect remains comparison-visible;
   a wildcard or value-erasing normalization is invalid.
4. Record target HIL that distinguishes the two behaviors and its product
   consequence. Evidence that only shows the Rust path works does not explain
   why the vendor effect may differ.
5. Add a reviewed bounded relation only after those inputs exist, then replay
   both sides. The result may support that relation; it does not establish
   whole-function equivalence.

The obsolete `phy_chip_set_chan` `ANA_CONF2` difference must not be reviewed as
an intentional platform refinement. Cold ROM writes `0x1a00`; the actual
post-registration vendor path and production both use `0x3fa00`. The remaining
verification result is `INCOMPLETE` until the complete compiled production
boundary is compared; correcting call identity alone proves neither operation
order nor whole-function equivalence.

## Commands

```console
cargo blobray project verify --project path/to/vendor-project.toml
cargo blobray project audit bindings --project path/to/vendor-project.toml
cargo blobray project check --project path/to/vendor-project.toml
```

Focused `advanced verify ...` and `advanced execute ...` commands are backend
tools. The project commands are the normal reproducible interface.

The qualification ledger remains outside the project. Blobray owns no
ledger parser, readiness types, or policy calculation. A future frontend may
display a read-only result emitted by the independent `qualification-check`
tool, but it cannot update the ledger or turn Blobray evidence into
readiness.
