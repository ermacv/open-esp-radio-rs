# RISC-V vendor-binary backend

RV32 ELF/archive decoding, relocation handling, symbolic analysis, concrete
execution, final-image auditing and Rust reference generation for the explicit
`riscv32` + `riscv-ilp32` backend pair.

Platform ABI tables and reviewed semantic summaries are injected through a
typed harness specification. The backend depends on the analysis model and
`open-radio-vendor-contracts`, never on a chip or production driver.
Reviewed memory facts are declared in `analysis-model`; this backend retains
its public re-exports for existing consumers. Pointer-layout recognition and
all instruction/ABI semantics remain here.

`ExecutableImage::load_entry` accepts a linked ELF or a regular static archive.
For an archive, it links the selected entry and its dependencies into a temporary
RV32 analysis image using the installed Rust toolchain's `rust-lld`.
`BLOBRAY_RISCV_LINKER` can select a GNU-compatible RV32 linker instead.
The analysis link disables relaxation and retains relocations. Missing callees
remain unresolved, and missing data definitions poison their relocation sites;
reaching either cannot establish equivalence. Unrelated unreachable definitions
do not prevent execution. Link errors (including conflicting definitions and
unsupported relocations) are reported explicitly.

`advanced execute run` and `advanced execute compare` use this entry loader.
Reports identify the original archive and its hash. Archive code addresses use
an analysis placement starting at `0x40000000`; they are not firmware addresses
or evidence of final linker placement. Authentic firmware ELF images remain
necessary when the behavior depends on final placement or runtime initialization.

Concrete `Scenario::arguments` contains RV32 integer ABI words. The first eight
use `a0` through `a7`; up to 64 additional words occupy the executor's private
stack starting at the 16-byte-aligned entry SP, following the
[RISC-V integer calling convention](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_integer_calling_convention).
The caller supplies scalar widening and any multiword padding. This is not an
automatic aggregate, variadic or floating-point ABI classifier. Stack argument
storage is private to each invocation; conflicting explicit byte seeds are
rejected and uninitialized stack bytes retain the configured poison/fill policy.
Call observations continue to record the eight integer argument registers.
