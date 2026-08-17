# Binary analysis and semantic IR

The analysis pipeline has one primary purpose: turn machine code into a
reviewable description of behavior without pretending that uncertain facts
are known. Authenticated artifact bytes, ABI/load mapping and provenance are
the authoritative observed source. Persistent linked IR is the canonical
derived representation shared by navigation, semantic analyses, pseudo-code
and executable reference generation.

## What the IR records

For each selected function the IR retains:

- exact instruction and control-flow provenance;
- natural-loop regions, latches, exits, nesting, and irreducible CFG regions;
- direct calls and resolved indirect targets;
- arguments, return-value provenance and branch guards;
- MMIO reads, writes and read-modify-write operations;
- global and pointed-to memory objects;
- table instances, slots and reviewed layouts;
- semantic actions such as delay, event dispatch or queue operations;
- completeness blockers where resolution stopped.

IR and facts from disassembly are not automatically semantic or hardware
truth. Stable names,
table layouts, RTOS operations and callback meaning come from reviewed
interface/function packs or a target provider. Unknown targets and ambiguous
memory remain explicit blockers.

## Human-readable pseudo-Rust

The pseudo-Rust view is a first-class output. It is intended to answer “what
does this vendor function appear to do?” and must preserve the relationships
that matter during a port:

```text
fn phy_chip_set_chan(channel, cbw) {
    call g_phyFuns[PHY_DISABLE_AGC](false);
    write32(MODEM.FREQ_CTRL, ...);
    delay_us(10);
    while read32(MODEM.STATUS).ready == 0 { ... }
    state.current_channel = channel;
}
```

This is explanatory pseudo-code, not production driver source. It may name
reviewed global tables, pointer slots, callbacks, RTOS primitives and MMIO
fields while retaining an `unknown(...)` expression where evidence is
incomplete. It should never fill a gap with a plausible implementation.

The compact view folds CFG-backed natural loops instead of printing bounded
unrolling. A narrowly recognized affine induction pattern may be rendered as
`for`/`step_by`, but the IR marks its trip count as a structural candidate and
never as an execution or termination proof. Unknown loops remain
`loop_at(...)`; multiple-entry cycles remain `irreducible_cfg_region(...)`.
`--full` retains the lossless instruction and basic-block view.

The RV32 backend may lift a reviewed subset of floating-point instructions as
raw-bit symbolic value flow. This is enough to retain relationships such as
`fcvt.s.w -> fsub.s -> fdiv.s -> fmadd.s -> fcvt.w.s` in memory-write
expressions and loop pseudo-code. It is deliberately not host floating-point
execution: the instruction rounding mode is retained on every node, dynamic
rounding remains explicit, and executable-reference generation fails closed
until a reviewed architectural FP environment and exception model exist.
Unsupported FP operations remain ordinary decode blockers.

The Blobray no longer generates a production-driver candidate. Driver code
is written and reviewed in `driver/`.

## Executable reference from the same IR

When every reachable effect is resolved, the same IR may emit a small
executable Rust reference used as an independent verification oracle. This
output is mechanically generated and disposable. It is not a second behavior
model and it is not production code.

Generation fails closed when a call, memory object, MMIO operation, return
value or required semantic action is unresolved. The reference is compiled
and its trace is checked against the lifted vendor trace before it is used.
Keeping pseudo-code and executable reference on one derived IR prevents the
two views from silently describing different functions. Each emitted effect
still links back to artifact provenance; success only establishes a statement
about the modeled/observed artifact, not hardware correctness.

## Indirect calls and platform semantics

Resolution follows evidence in layers:

1. relocation and symbol facts;
2. constant propagation and control-flow facts;
3. reviewed global-pointer and table layouts;
4. reviewed interface slots and ABI contracts;
5. target-provider semantics for ROM and RTOS boundaries.

The optional C add-on treats exact, standardized library identities as opaque
semantic boundaries. It does not lift their implementation bodies merely
because bytes are present in an archive. Fixed-arity string/memory contracts
retain typed arguments and known return shape. Variadic functions remain
explicitly unresolved until a variadic ABI/effect contract exists; a fixed
arity must not be invented for `sprintf` or printf-style payloads. Target
providers may separately classify exact logging hooks as diagnostic sinks and
record only the reviewed stable arguments.

For a global function table, the IR records both the load path and the selected
slot. For RTOS or callback boundaries it records the concrete callsite plus
the reviewed semantic action. A friendly label alone is insufficient: the
slot address, guard and provenance remain available for inspection.

The generated navigation index also joins calls across separately generated IR
profiles by exact project symbol and source-qualified function identity. One
candidate is reported as `unique`, multiple candidates as `ambiguous`, and no
candidate as `unresolved`. This is a navigation association only:
`linker_resolution_claim` remains false and no callee body is transitively
inlined across profiles.

## Completeness

“Readable” and “complete” are different properties. A function can have useful
pseudo-Rust while remaining incomplete for executable generation or proof.
The UI and machine reports therefore carry completeness blockers rather than
collapsing them into a confidence score.

Typical blockers are an unresolved indirect target, unknown initial memory,
an unmodelled device read, a missing external ABI, or an address outside the
reviewed MMIO map.
