# Revision-zero ROM PBus core audit

This page applies the complete instruction standard from
[the audit method](../audit-method.md) to the 17 small ROM functions that own
PBus command publication, packed-result reads, mode changes and clock-force
fields. Addresses refer to `_oracles/esp32s31_rev0_rom.elf`.

Audit baseline: 2026-07-30.

## Reproduction

```console
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f824102 --stop-address=0x2f824602 \
  _oracles/esp32s31_rev0_rom.elf
llvm-objdump -d --no-show-raw-insn \
  --start-address=0x2f827bb0 --stop-address=0x2f827d16 \
  _oracles/esp32s31_rev0_rom.elf
dd if=_oracles/esp32s31_rev0_rom.elf bs=1 \
  skip=$((0x55910)) count=44 status=none | xxd -g4
```

The final command covers the two jump tables at ROM addresses `0x2f84d910`
and `0x2f84d924`. The ELF `.rodata` section starts at virtual address
`0x2f848000` and file offset `0x50000`.

## Result

| Function | Address | Size | Status | Strict result |
| --- | ---: | ---: | --- | --- |
| `phy_pbus_force_mode` | `0x2f824102` | `0x90` | MISMATCH | Rust has the two initial RMWs, but the zero-input tail is caller-composed and some reached owners omit it or use a one-microsecond pulse |
| `phy_pbus_rd_addr` | `0x2f824192` | `0x5c` | NO-REGISTER-EFFECT | Pure selector/path-to-address jump table |
| `phy_pbus_rd_shift` | `0x2f8241ee` | `0x3a` | NO-REGISTER-EFFECT | Pure selector/path-to-shift jump table |
| `phy_pbus_force_test` | `0x2f824228` | `0x42` | MISMATCH | Command image and completion clear match, but Rust invents a pre-publication busy read and rejection path |
| `phy_pbus_rd` | `0x2f82426a` | `0x3c` | MISMATCH | Rust reads the wrong physical word for selector zero and rejects vendor fallback selectors |
| `phy_pbus_debugmode` | `0x2f8242a6` | `0x06` | MATCHED | Exact nonzero tail-call branch of `phy_pbus_force_mode` |
| `phy_pbus_workmode` | `0x2f8242ac` | `0x06` | MISMATCH | Exact zero tail-call reaches the incompletely composed Rust work-mode behaviour |
| `phy_pbus_set_rxgain` | `0x2f8242b2` | `0x5c` | MISMATCH | Three values/order are represented, but each Rust command inherits the extra busy read and new failure path |
| `phy_pbus_xpd_rx_off` | `0x2f82430e` | `0x26` | MISMATCH | Three command tuples match; force-test transaction traces do not |
| `phy_pbus_xpd_rx_on` | `0x2f824334` | `0x62` | MISMATCH | Seven command tuples and parameter byte match; force-test transaction traces do not |
| `phy_pbus_xpd_tx_off` | `0x2f824396` | `0x3a` | MISMATCH | Five command tuples match; force-test transaction traces do not |
| `phy_pbus_set_dco` | `0x2f8243d0` | `0x3e` | MISMATCH | Four halfword loads and tuples match; force-test transaction traces do not |
| `phy_pbus_xpd_tx_on` | `0x2f82440e` | `0x7c` | BODY-AUDITED | Complete body and command order recorded; the fixed eight-byte ROM object at `0x2f8472d0` is outside the ELF's materialized sections |
| `phy_pbus_clear_reg` | `0x2f824572` | `0x90` | MISMATCH | Twelve tuples and work-mode timing match, but all commands inherit the force-test mismatch |
| `phy_force_txrx_off` | `0x2f827bb0` | `0x66` | MATCHED | Both Boolean branches preserve two fresh RMWs and two one-microsecond delays |
| `phy_set_txclk_en` | `0x2f827cd2` | `0x24` | MATCHED | Exact fresh-read replacement of bits 17:16 |
| `phy_set_rxclk_en` | `0x2f827cf6` | `0x20` | MATCHED | Exact fresh-read replacement of bits 15:14 |

The mismatches above are normal parity findings. The unbounded vendor
force-test wait remains a documented robustness defect, but it does not
justify the additional successful-path pre-publication register read.

## `phy_pbus_force_mode`

Input zero and nonzero are the only behaviour classes.

For a nonzero input, ROM:

1. freshly reads `0x2010088c`, clears bits 25:0 with `0xfc000000`, and writes
   it;
2. freshly reads `0x20100884`, sets bit 0, and writes it.

For zero, ROM:

1. freshly reads `0x20100884`, clears bit 0, and writes it;
2. freshly reads `0x2010088c`, sets bit 26, and writes it;
3. reads `0x20109c18` and returns immediately when bit 1 is clear;
4. otherwise delays one microsecond;
5. freshly reads `0x2010702c`, replaces bits 27:0 with `0x32000000`, and
   writes it;
6. freshly reads the same word, sets bit 27, and writes it;
7. delays two microseconds;
8. freshly reads the same word, clears bit 27, and writes it.

HAL `configure_debug_mode` reproduces the nonzero branch. HAL
`configure_work_mode` reproduces the first two zero-branch writes and the
condition read, then returns that condition to an outer transition. This split
can be exact, and `PhyPbusClearTransition` and the XTAL-duty restore transition
do schedule the `1 µs`, set, `2 µs`, clear tail. It is not a complete generic
replacement: the RX saturation owner discards the condition, while RX-gain
and shared TX-calibration owners use `1 µs` for the second delay.

## Packed PBus read tables

The address jump table contains:

```text
selector 0 -> 0x2f8241de
selector 1 -> 0x2f8241a8
selector 2 -> 0x2f8241b2
selector 3 -> 0x2f8241c2
selector 4 -> 0x2f8241c8
```

The shift jump table contains:

```text
selector 0 -> 0x2f824204
selector 1 -> 0x2f824210
selector 2 -> 0x2f82421c
selector 3 -> 0x2f824210
selector 4 -> 0x2f82421c
selector 5 -> 0x2f824218
```

Expanding both bodies gives the complete vendor map:

| Selector | Path equals 1 | Vendor address | Shift |
| ---: | --- | ---: | ---: |
| 0 | no | `0x201008a0` | 9 |
| 0 | yes | `0x201008a0` | 18 |
| 1 | no | `0x20100894` | 0 |
| 1 | yes | `0x20100894` | 9 |
| 2 | no | `0x2010089c` | 18 |
| 2 | yes | `0x20100898` | 0 |
| 3 | no | `0x2010089c` | 0 |
| 3 | yes | `0x2010089c` | 9 |
| 4 | no | `0x201008a4` | 18 |
| 4 | yes | `0x201008a0` | 0 |
| 5 | either | `0x201008a4` | 9 |
| greater than 5 | either | `0x201008a4` | 0 |

`phy_pbus_rd` calls the address helper, calls the shift helper, performs one
32-bit load through the returned address, masks nine bits, shifts, and returns
the zero-extended halfword.

Rust `read_pbus_result` matches selectors 1 through 5. For selector zero it
uses `READ_RESULT_4` at `0x201008a4`; ROM uses `0x201008a0`. The recovered SVD
and generated PAC carry the same incorrect selector-zero claim. Rust also
returns `None` for selectors above five, whereas ROM reads the low nine bits
of `0x201008a4`.

## `phy_pbus_force_test`

ROM freshly reads `0x20100884` and composes:

```text
(old & 0xfffe0001)
| (((test << 6) | (selector << 2) | (path << 15)) & 0x0001fffc)
| 0x00000002
```

It writes the command, repeatedly reads `0x20100890` until bit 31 clears,
freshly reads `0x20100884`, clears bit 1, and writes it. There is no range
check, pre-publication busy sample, retry bound or error return.

The PAC command-image function is instruction-exact, including the overlap
from the low eleven bits of a `u16` test image. The Rust HAL first reads
`0x20100890`; if busy it publishes nothing and returns `Busy`. It additionally
rejects selector/path values outside its typed field domain. After publication
the async binding response-indexes repeated busy samples and clears bit 1 on
completion.

The bounded wait is the safe replacement for
`VENDOR-ROBUSTNESS-001`. The extra ready-state read before every successful
publication is nevertheless an invented register transaction under the
strict audit rule, and busy-at-entry produces a different command stream.

## Command-composition helpers

The following lists are in exact call order. Each tuple is
`(selector, path, value)`.

`phy_pbus_set_rxgain(value)` reads `phy_param_rom[2]` and emits:

1. `(1, 2, ((value << 6) & 0x1c0) | ((value >> 4) & 0x3f))`;
2. `(0, 1, ((value >> 12) & 0x38) | ((value >> 12) & 7) | 0x40)`;
3. `(0, 2, phy_param_rom[2])`.

`phy_pbus_xpd_rx_off()` emits `(0,1,0)`, `(1,1,0)`, `(1,2,0)`.

`phy_pbus_xpd_rx_on(value)` emits `(4,1,0)`, `(4,2,1)`, `(5,1,0)`,
`(0,1,0x40)`, `(0,2,phy_param_rom[2])`, `(1,1,0x189)`,
`(1,2,(u16)value)`.

`phy_pbus_xpd_tx_off()` emits `(4,1,0)`, `(5,1,0)`, `(1,1,0)`,
`(1,2,0)`, `(0,1,0)`.

`phy_pbus_set_dco(pointer)` uses four unsigned halfword loads and emits
`(2,1,h0)`, `(3,1,h1)`, `(2,2,h2)`, `(3,2,h3)`.

`phy_pbus_xpd_tx_on(first, second)` copies two words from fixed ROM address
`0x2f8472d0` to a stack-local four-halfword DCO object, then emits
`(0,1,0x80)`, `(0,2,0)`, `(4,2,0)`, `(1,1,0x7c)`, calls
`phy_pbus_set_dco` on that object, and emits `(1,2,(u16)second)`,
`(4,1,0x0b)`, `(5,1,(u16)(first + 0x1c0))`. The input additions and
narrowing are wrapping RV32 operations. The referenced constant lies in the
gap before the ELF's materialized `.rodata` section, so its four halfword
values cannot be certified from this pinned container alone.

Rust transitions contain the same tuple expansions for their represented
profiles, including explicit `phy_param_rom[2]` inputs. They all publish
through the mismatching force-test binding, so tuple equality is not complete
register-trace equality.

## `phy_pbus_clear_reg`

After debug mode, ROM emits these twelve commands:

```text
(4,1,0), (4,2,0), (5,1,0), (5,2,0),
(0,1,0), (0,2,0), (1,1,0), (1,2,0),
(2,1,0x100), (3,1,0x100), (2,2,0x100), (3,2,0x100)
```

It then tail-calls `phy_pbus_workmode`. `PhyPbusClearTransition` preserves the
twelve tuples and the complete conditional `1 µs`/set/`2 µs`/clear work-mode
tail. Its only register-trace difference on responsive hardware is inherited:
each force command has the additional Rust pre-publication busy read. Its
typed timeout outcome also replaces vendor nontermination.

## Force and clock leaves

`phy_force_txrx_off(nonzero)` replaces bits 11:8 of `0x20100890` with 8,
delays one microsecond, freshly reads and replaces them with 10, then delays
one microsecond. Input zero uses 2 and then 0 with the same delays.
`PhyRegisterTransition` plus HAL `configure_force_txrx` preserve both fresh
RMWs and timer edges.

`phy_set_txclk_en` freshly reads `0x20100890`, clears bits 17:16, sets them
both for nonzero input, and writes once. `phy_set_rxclk_en` does the same for
bits 15:14. The typed Rust Boolean preserves the complete zero/nonzero
behaviour classes and both HAL leaves use one fresh PAC RMW.
