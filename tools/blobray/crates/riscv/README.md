# RV32 function decoding and relocation interpretation

Owns the `FunctionDecoder` and `FunctionSemantics` implementations over pinned rv-asm 0.2.1 and RISC-V
relocation interpretation. It receives bytes and structural facts, never a
project, archive path or publication capability. Unsupported encodings remain
explicit gaps. Legacy backend state and orchestration are not dependencies.

Lifting returns bounded typed operations over RV32 registers. Loads, stores and
atomics describe effects without reading memory. Compressed instructions use the
pinned decoder's normalized operands. The backend declares relocation roles;
analysis validates the flowing address relationship and owns abstract states.


`RiscvExecutor` owns concrete RV32IMC register state and the iterative instruction
loop. It receives entry, stack, integer arguments, an `ExecutionMemory` port and
shared run control. It uses the same decoder and integer lift descriptions, with
separate concrete control-flow and fence handling. Unknown operands or unsupported
instructions end with an explicit gap. The backend cannot select images, acquire
memory regions, choose models or publish a verdict. See the
[concrete profile](../../next/README.md#concrete-execution-and-comparison).
