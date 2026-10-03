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
transfer.

A function's bound is its frame or, if deeper, a callee's bound below the
`sp` of the transfer that reaches it: the analysis's depth at that site, or the
whole frame at a site it did not reach. The bound never underestimates the
code it reaches.

Bounds fail closed. A root has a number only when nothing it reaches is
unresolved; otherwise `Bound::reasons` counts the unresolved sites and
functions by `Reason`, and `Reason::closed_by` names the stage of the stack
analysis that resolves each: indirect calls through a stack slot
(`core::hint::black_box`), through a loaded pointer or another register,
indirect jumps out of a function, transfers outside the image (ROM) or into
a function's middle, functions without a frame, and recursion. The partial
path over what is resolved is diagnostic, never a bound.

```console
cargo test -p oer-riscv-stack
```
