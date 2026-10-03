# RV32 instruction decoding

`oer-riscv-decode` turns instruction bytes into a typed instruction and its
length (2 or 4 bytes), and prints it as assembler text. It has no I/O and no
ELF knowledge and builds for the host and `no_std`; its only dependency is the
pinned rv-asm 0.2.1, which is itself `no_std` and dependency-free. Blobray's
RV32 backend decodes through it.

`decode(bytes, extensions)` returns `None` for an encoding outside the selected
`Extensions`, a reserved encoding, an encoding longer than 32 bits and a slice
shorter than the instruction. `Extensions::ALL` is the ESP32-S31's
`rv32imafc_zba_zbb_zbs_zcb_zcmp` without Zcmt; `Extensions::RV32IMAC` is the
base with M, A and C.

rv-asm decodes RV32I with M, A and C (`Instruction::Base`). The crate corrects
its unsigned C.ANDI immediate to the ISA's signed six-bit value, as the
[C extension](https://docs.riscv.org/reference/isa/v20260120/unpriv/c-st-ext.html)
defines it. The `extensions` module decodes the forms rv-asm lacks
(`Instruction::Extension`): the Zba, Zbb and Zbs integer forms, the Zcb loads,
stores and arithmetic, and the Zcmp `cm.push`, `cm.pop`, `cm.popret`,
`cm.popretz`, `cm.mvsa01` and `cm.mva01s`. It classifies the Zcmp and Zcb
encoding spaces before rv-asm, which would read the Zcmp space as the
D-extension C.FSDSP. `c.sext.b`, `c.zext.h` and `c.sext.h` also need Zbb, and
`c.mul` M. Zcmt table jumps need the `jvt` CSR and stay undecoded.

The `float` module decodes the single-precision F extension
(`Instruction::Float`): `flw`/`fsw` with `C.FLW`, `C.FSW`, `C.FLWSP` and
`C.FSWSP`, the fused multiply-adds and every OP-FP form. Other formats (D, Q,
Zfh), `C.FLD`/`C.FSD` and the reserved rounding modes 5 and 6 stay undecoded.

The text is rv-asm's for base forms, which prints compressed forms expanded and
uses its aliases (`li`, `mv`, `ret`, `j`, `nop`), and the assembler's for the
other forms. It is versioned with the crate, not a parsing interface: consumers
read the typed instruction.

```console
cargo test -p oer-riscv-decode
```
