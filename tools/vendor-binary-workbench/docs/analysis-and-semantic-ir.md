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

The Workbench no longer generates a production-driver candidate. Driver code
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

For a global function table, the IR records both the load path and the selected
slot. For RTOS or callback boundaries it records the concrete callsite plus
the reviewed semantic action. A friendly label alone is insufficient: the
slot address, guard and provenance remain available for inspection.

## Completeness

“Readable” and “complete” are different properties. A function can have useful
pseudo-Rust while remaining incomplete for executable generation or proof.
The UI and machine reports therefore carry completeness blockers rather than
collapsing them into a confidence score.

Typical blockers are an unresolved indirect target, unknown initial memory,
an unmodelled device read, a missing external ABI, or an address outside the
reviewed MMIO map.
