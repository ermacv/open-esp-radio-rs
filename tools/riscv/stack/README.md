# RV32 stack bounds

`oer-riscv-stack` computes worst-case stack bounds of a static RV32 image from
its machine code, over the program model of
[`oer-riscv-program`](../program/README.md),
[`oer-riscv-analysis`](../analysis/README.md) and
[`oer-riscv-lift`](../lift/README.md).

Every defined code symbol is a function; a symbol without a size extends to
the next one of its section. Its frame is the compiler's `.stack_sizes`
record, cross-checked with the deepest entry-relative `sp` the value analysis
observes under the psABI's preserved `sp` (a deeper observation is an error);
without a record, the observed depth counts only when the function's
control-flow graph is complete. Its transfers are every direct call and
out-of-function jump of a linear sweep over its whole extent, which also
reaches code behind jump tables the graph does not expand, plus the indirect
transfers whose target the value analysis resolves; an indirect jump whose
targets the analysis finds inside the function is a jump table, not a
transfer. A call or jump through `table[index]`, whose base is a constant or a
register the value analysis knows at the transfer, reaches each entry of the
table when its length is exact: the index's bounds check on the path to the
dispatch (`bltu limit, index` or `bgeu index, limit` against a constant) or
mask (`andi`), or the size of a data object the base starts in an unwritable
section, such as esp-hal's `__EXTERNAL_INTERRUPTS` handler table, against
which the language bounds-checks the index. Entries of a jump inside its own
function are already in the sweep. A table of unknown length, such as a
`match` on an enum whose range no check states, stays unresolved: reading
until the first word outside the function could stop short of an entry that
leaves it.

A function's bound is its frame or, if deeper, a callee's bound below the
`sp` of the transfer that reaches it: the analysis's depth at that site, or the
whole frame at a site it did not reach. The bound never underestimates the
code it reaches.

Bounds fail closed. A root has a number only when nothing it reaches is
unresolved; otherwise `Bound::reasons` counts the unresolved sites and
functions by `Reason`, and `Reason::closed_by` names the stage of the stack
analysis that resolves each: indirect calls through a stack slot
(`core::hint::black_box`), through a loaded pointer or another register,
indirect tail calls through a pointer, transfers outside the image (ROM) or into
a function's middle, functions without a frame, and recursion. The partial
path over what is resolved is diagnostic, never a bound.

```console
cargo test -p oer-riscv-stack
```
