# RV32 function decoding and relocation interpretation

Owns the `FunctionDecoder` and `FunctionSemantics` implementations and RISC-V
relocation interpretation. It receives bytes and structural facts, never a
archive path or loader capability. Unsupported encodings remain
explicit gaps.

Decoding belongs to [`oer-riscv-decode`](../../../riscv/decode/README.md),
selected with every extension it decodes: ESP-IDF builds the ESP32-S31 for
`rv32imafc_zba_zbb_zbs_zcb_zcmp_zcmt`, and only the Zcmt table jumps stay
undecoded. Integer and Zcb memory forms lift to single operations;
`IntegerOp::evaluate` in the domain is the one concrete definition that
analysis and execution share. The Zcmp forms move several registers and lift
to `Unsupported`, so abstract analysis keeps a gap for them while concrete
execution runs them: pushes store the listed registers from `s11` down to `ra`
below `sp`, pops load them back, and the returning forms return through `ra`,
as the decoded flow states.

The FP register file is not modeled: single-precision loads and stores lift to
`FloatLoad`/`FloatStore` accesses through their integer base, a form with an
integer destination (`fmv.x.w`, `fclass.s`, comparisons, `fcvt.w[u].s`) lifts
to `Opaque`, and every other form to `None`. Neither executor profile runs
floating point; the Rv32imac profile decodes RV32IMAC alone, and the full
profile stops at an F form as unsupported.

The decoder, semantic and execution identities (`policy-4`, `values-8` and
`execution-13`, each over rv-asm 0.2.1) cover every behavior below; any
change to decoding, lifting or concrete execution changes its identity.

Lifting returns bounded typed operations over RV32 registers. Loads, stores and
atomics describe effects without reading memory. Compressed instructions use the
shared normalized operands, including the decoder's signed C.ANDI immediate.
The backend declares relocation roles;
analysis validates the flowing address relationship and owns abstract states.

`RiscvExecutor` owns concrete RV32 register state and the iterative instruction
loop. It receives an `ExecutionStart` with entry, stack, optional register words
and a resolved goal, an `ExecutionMemory` port and
shared run control. It uses the same decoder and integer lift descriptions, with
separate concrete control-flow and fence handling. Unknown operands or unsupported
instructions end with an explicit gap. Decoded instructions are cached per
thread, keyed by instruction bytes and profile, so a cached decode stays valid
for any address and for code that later changes. The backend cannot select images, acquire
memory regions, choose models or publish a verdict. See the
[concrete profile](../../cli/reference/execution/README.md#concrete-execution-and-comparison).

The semantic identity includes typed fence mode/predecessor/successor
sets. Analysis consumes the typed fence record; it never parses
instruction display strings. Concrete execution retains its own supported fence-mode
check and reports unsupported modes explicitly.

Execution preserves unspecified argument registers as unknown. Stack argument
placement belongs to application; the backend observes those words through memory
loads with the same initialization and permissions as other guest accesses.

LR.W, SC.W and all nine AMO.W operations use dedicated memory ports. The backend
maps aq/rl and owns arithmetic; session memory owns permissions and reservations.
Unknown or unsupported atomic accesses produce an atomic memory gap, not MMIO
substitution or a fabricated fence event.

Resolved goals stop at a fetchable symbol address or a selected x1/x5 call transfer
before the callee body; explicit tail inclusion accepts x0 transfers except canonical
returns. Premature return is `goal-not-reached`. The backend never resolves physical
symbols, infers function extents or claims an early goal executed its target body.

Execution dispatches explicit call-boundary responses through the injected memory port after observe-call goals. It applies psABI caller-saved unknown clobbers and explicit return words, preserves callee-saved/SP/gp/tp, and never resolves bindings or owns model state. Ordinary code and canonical returns retain their instruction paths.

Eligible call transfers report whether their target was indirect. Application may
validate a selected runtime interface before dispatch; typed interface gaps stop the
phase. Canonical returns and the reserved root-return sentinel bypass call dispatch.
The backend neither resolves reviewed roots nor infers pointer provenance.

A successful selected dequeue is supplied by application after modeled output;
execution stops with `observed-dequeue` at that service event. Failed/other
service events cannot complete it, and returning first remains goal-not-reached.

The executor identifies each instruction through the memory port independently of
progress callbacks. Conditional branches emit physical site/target/fallthrough and
taken choice; the session's explicit timeline capture decides whether to retain
these events. The backend does not own event storage or comparison selection.
Normal-memory and atomic effects are observed by their session owner.
