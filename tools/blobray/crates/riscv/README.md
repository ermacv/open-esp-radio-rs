# RV32 function decoding and relocation interpretation

Owns the `FunctionDecoder` and `FunctionSemantics` implementations over pinned rv-asm 0.2.1 and RISC-V
relocation interpretation. It receives bytes and structural facts, never a
project, archive path or publication capability. Unsupported encodings remain
explicit gaps. Legacy backend state and orchestration are not dependencies.

Lifting returns bounded typed operations over RV32 registers. Loads, stores and
atomics describe effects without reading memory. Compressed instructions use the
shared normalized operands. The backend corrects rv-asm 0.2.1's unsigned
C.ANDI immediate to the ISA's signed six-bit value before display, lifting or
execution; decoder policy 2, values 6 and execution 10 identify this behavior.
The [C extension](https://docs.riscv.org/reference/isa/v20260120/unpriv/c-st-ext.html)
defines this sign extension. The backend declares relocation roles;
analysis validates the flowing address relationship and owns abstract states.


`RiscvExecutor` owns concrete RV32IMAC register state and the iterative instruction
loop. It receives an `ExecutionStart` with entry, stack, optional register words
and a resolved goal, an `ExecutionMemory` port and
shared run control. It uses the same decoder and integer lift descriptions, with
separate concrete control-flow and fence handling. Unknown operands or unsupported
instructions end with an explicit gap. The backend cannot select images, acquire
memory regions, choose models or publish a verdict. See the
[concrete profile](../../next/reference/execution/README.md#concrete-execution-and-comparison).

`PointerDecoder` provides the `rv32-absolute-rela/1` profile: NONE writes nothing;
R_RISCV_32 RELA supplies absolute 32-bit symbol-plus-addend semantics. Other
relocations remain unsupported for pointer interpretation. This port grants no
loader, project, artifact or allocator authority.

The `values-6` semantic identity includes typed fence mode/predecessor/successor
sets. Static trace extraction consumes the saved fence record; it never parses
instruction display strings. Concrete execution retains its own supported fence-mode
check and reports unsupported modes explicitly.

`execution-10` preserves unspecified argument registers as unknown. Stack argument
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


`execution-10` dispatches explicit call-boundary responses through the injected memory port after observe-call goals. It applies psABI caller-saved unknown clobbers and explicit return words, preserves callee-saved/SP/gp/tp, and never resolves bindings or owns model state. Ordinary code and canonical returns retain their instruction paths.

Eligible call transfers report whether their target was indirect. Application may
validate a selected runtime interface before dispatch; typed interface gaps stop the
phase. Canonical returns and the reserved root-return sentinel bypass call dispatch.
The backend neither resolves reviewed roots nor infers pointer provenance.

A successful selected dequeue is supplied by application after modeled output;
`execution-10` stops with `observed-dequeue` at that service event. Failed/other
service events cannot complete it, and returning first remains goal-not-reached.

The executor identifies each instruction through the memory port independently of
progress callbacks. Conditional branches emit physical site/target/fallthrough and
taken choice; the session's explicit timeline capture decides whether to retain
these events. The backend does not own event storage or comparison selection.
Normal-memory and atomic effects are observed by their session owner.
