# Static RV32 images

`oer-riscv-program` reads one borrowed static RV32 ELF executable for the
program model of [`oer-riscv-model`](../model/README.md): its ELF
floating-point ABI (`abi`), its validated load segments and the
`ProgramView` that serves file-backed read-only constants as `ImageMemory`
to the [analysis](../analysis/README.md), its code symbols
(`code_symbols`, `code_symbol_at`) and its executable sections. It performs
no loading, relocation or execution, and has no filesystem access.

`execution_segments` lends validated static RV32 ELF segment bytes to a caller's
admitted loader. It shares `ProgramView` validation with static image research,
including overlapping mappings, permissions, dynamic/TLS rejection and input
capacity. Borrowed bytes expire after the callback; application owns any copied
mutable memory. This port performs no relocation, model selection or execution.

`executable_sections` lends validated static RV32 executable sections independently
of function symbols. Missing section coverage fails the final-image audit instead
of being interpreted as a clean empty program.

Function analysis and final-image audit share mapping-symbol validation and
admitted interval storage. Only local zero-sized `$d`/`$x` markers authorize data
intervals. ISA-qualified `$xrv32…` markers also end data intervals; other XLENs,
conflicting, misaligned code or out-of-section markers fail closed.
