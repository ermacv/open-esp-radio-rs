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
optional knowledge provider may enrich lifting and generated references, but
it cannot execute a production comparison or return a verdict. The generic
verification engine alone compares compiled artifacts and observations.

Effect contracts define the ordered observable reads, writes, calls, state
changes and allowed normalizations. Unlisted effects fail closed. Concrete
profiles define arguments, initial memory/device state, observations and
finite-domain preconditions.

Profile schema 4 requires an explicit `transaction-comparison` policy.
`observables` compares ordered MMIO, delay, and fence events;
`observables-and-calls` also compares every named call boundary;
`observables-and-reviewed-calls` compares only explicitly listed semantic
call pairs; and `full` additionally exposes branch and ordinary RAM state.
Call-site addresses remain provenance rather than equality keys. A vendor and
Rust call may share an operation only through a reviewed `call-equivalences`
entry; symbol spelling or a semantic hint never creates equivalence.

Known C runtime and ecosystem service leaves are opaque semantic boundaries
when an add-on supplies their signature and bounded behavior. Their calls
remain visible facts, but Workbench does not recursively reconstruct an
available implementation merely because bytes for that implementation are
present. Unknown calls remain blockers.

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
deleted. Its retained observations expose a real production-verification gap:
the vendor ROM path uses the recovered `0x1a00` analog-I²C host configuration,
while production uses the newer recovered `0x3fa00` configuration. A new
generic comparison must bind the compiled shipping entry and classify that
difference from target provenance and HIL evidence before it can be accepted
or fixed. It must not be normalized into a whole-function match.

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

The current `phy_chip_set_chan` difference is deliberately stopped before
step 4: vendor writes `I2C_ANA_MST.ANA_CONF2 = 0x1a00`, while production writes
`0x3fa00`. Until target provenance and discriminating HIL classify it, the
qualification ledger keeps
`channel-production-trace-difference-unreviewed` open.

## Commands

```console
cargo vendor-binary-workbench project verify --project path/to/vendor-project.toml
cargo vendor-binary-workbench project audit bindings --project path/to/vendor-project.toml
cargo vendor-binary-workbench project check --project path/to/vendor-project.toml
```

Focused `advanced verify ...` and `advanced execute ...` commands are backend
tools. The project commands are the normal reproducible interface.

The qualification ledger remains outside the project. Workbench owns no
ledger parser, readiness types, or policy calculation. A future frontend may
display a read-only result emitted by the independent `qualification-check`
tool, but it cannot update the ledger or turn Workbench evidence into
readiness.
