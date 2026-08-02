# RISC-V vendor validator backend

RV32 ELF/archive decoding, relocation handling, symbolic analysis, concrete
execution, final-image auditing and Rust reference generation for the explicit
`riscv32` + `riscv-ilp32` backend pair.

Platform ABI tables and reviewed semantic summaries are injected through a
typed harness specification. The backend depends on the neutral model (which
owns the contract vocabulary through core), never on a chip or production
driver.
