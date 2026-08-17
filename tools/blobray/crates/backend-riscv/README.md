# RISC-V vendor-binary backend

RV32 ELF/archive decoding, relocation handling, symbolic analysis, concrete
execution, final-image auditing and Rust reference generation for the explicit
`riscv32` + `riscv-ilp32` backend pair.

Platform ABI tables and reviewed semantic summaries are injected through a
typed harness specification. The backend depends on the analysis model and
`open-radio-vendor-contracts`, never on a chip or production driver.
