# Line audit: `libphy.a[phy_i2c.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines eleven external code functions. Every instruction,
relocation, jump-table arm and constant-table element was inspected. Nine
functions have closed strict results. `phy_i2c_init1` and
`phy_bias_reg_set` remain **BODY-AUDITED** because their abstract Rust
PHY-I2C port has no target implementation in this repository and their
blocking ROM write child is audited separately.

## `phy_get_i2c_read_mask_new`

Size `0x24`. Strict status: **MISMATCH**.

The function wraps `input - 10` to a byte. Values in the inclusive wrapped
index range `0..=99` select one of 100 little-endian halfwords; other indices
return zero. The complete nonzero mapping is:

| Input | Mask | Input | Mask |
| ---: | ---: | ---: | ---: |
| `0x0a` | `0x2000` | `0x0b` | `0x4000` |
| `0x0c` | `0x1000` | `0x10` | `0x0200` |
| `0x11` | `0x0400` | `0x61` | `0x0100` |
| `0x62` | `0x0020` | `0x63` | `0x0010` |
| `0x66` | `0x0080` | `0x67` | `0x0004` |
| `0x69` | `0x0800` | `0x6a` | `0x0040` |
| `0x6b` | `0x0008` | `0x6d` | `0x8000` |

All other accepted table entries are zero. The function itself has no MMIO,
but its result is later published as the PHY-I2C read mask.

Rust retains only the thirteen masks for blocks `0x61..=0x6d` and rejects
all other block IDs in `PhyI2cAddress::new`. It therefore omits the five
nonzero vendor entries at `0x0a`, `0x0b`, `0x0c`, `0x10`, and `0x11`.
That narrower domain can change later register data and is not a
vendor-defect exception.

## `phy_get_i2c_hostid_new`

Size `0x44`. Strict status: **MISMATCH**.

The function first derives a return value with a complete eleven-arm jump
table for blocks `0x61..=0x6b`:

- host 1 for `0x61`, `0x62`, `0x63`, `0x67`, `0x6a`, and `0x6b`;
- host 0 for `0x64`, `0x65`, `0x66`, `0x68`, and `0x69`;
- host 0 for every input outside that range.

Every arm then performs the same fresh-read RMW at `0x2010f820`:

```text
new = (old & 0xfffc000f) | 0x0003fa00
```

Rust's `PhyI2cAddress::host` has the same mapping for the accepted
`0x61..=0x6d` domain, and the HAL requests the same host-map configuration
before reads and writes. It is not a complete replacement: arbitrary vendor
inputs return host zero and still execute the RMW, while Rust rejects them.
Moreover, `configure_phy_i2c_host_map` is only a platform trait method; no
implementation in this repository proves the actual target RMW.

## `phy_i2c_init1`

Size `0x216`. Strict status: **BODY-AUDITED**.

There are no branches. The complete body performs 26 ordered
`phy_i2c_writeReg` calls:

```text
6b:01=01  6b:02=73  6b:03=ba  6b:04=88  6b:0e=f4
6b:09=02  6b:07=fd  6b:08=bb  6b:05=01  6b:06=11
6b:0c=a7  6b:0d=7a  6b:0a=08  6b:0b=04  6b:0f=81
62:00=68  62:04=a8  62:0f=phy_param[0x18e]
62:0b=44  62:15=08  63:06=00  62:0d=0a
67:02=27  66:02=70
67:18=wrapping(phy_param[0xee] + 2)
67:19=wrapping(phy_param[0xee] + 2)
```

Rust `I2cInit1Transition` preserves all 26 addresses, values and their order,
including byte wrapping for the two dynamic writes. Each operation is exposed
as a separate completion edge.

The final strict status remains open for two reasons:

- the ROM `phy_i2c_writeReg` child and Rust async replacement must be closed
  under all hardware-response histories;
- `PhyI2cMasterControl`, which must perform the actual chip-level command
  register transactions, has no implementation in this repository.

On an uncontended host that completes each command, the recorded parent
sequence matches.

## `phy_bias_reg_set`

Size `0x30`. Strict status: **BODY-AUDITED**.

The input is ignored. The function calls:

1. `phy_i2c_writeReg(0x6a, 1, 0, 0xaf)`;
2. tail-calls `phy_i2c_writeReg(0x6a, 1, 1, 0x7f)`.

`BiasRegTransition` has the same two writes and order. Its strict result has
the same open ROM-child and missing target-trait implementation dependencies
as `phy_i2c_init1`.

## `phy_i2c_enter_critical`

Size `0x02`, weak definition. Strict status: **NO-REGISTER-EFFECT**.

The complete body is one `ret`. It does not disable interrupts, acquire a
lock or touch memory.

## `phy_i2c_exit_critical`

Size `0x02`, weak definition. Strict status: **NO-REGISTER-EFFECT**.

The complete body is one `ret`. It does not restore interrupts, release a
lock or touch memory.

## `phy_i2c_init2`

Size `0x2b8`. Strict status: **NOT-PORTED**.

The function zeroes six separate 22-byte arrays, calls the no-op critical
entry, and constructs 22 pairs of parallel write commands. The first command
of each pair is:

```text
blocks = [6b x15, 62 x6, 66]
regs   = [02,03,04,0e,09,07,08,05,06,0c,0d,0a,0b,0f,01,
          00,04,0f,0b,0d,15,02]
values = [73,ba,88,f4,02,fd,bb,01,11,a7,7a,08,04,81,01,
          68,a8,param[18e],44,0a,08,70]
```

The simultaneous second command is:

```text
blocks = [67 x19, 63 x3]
regs   = [02,14,15,16,17,18,19,1c,1d,1e,1f,04,05,06,07,
          0c,0d,0e,0f,06,06,06]
values = [27,high,high,low,param[ed],aux,aux,
          param[f0],param[f0],param[f0]|40,param[f0],
          param[e9],param[e9],param[ea],param[ea],
          param[e9],param[e9],param[ea],param[ea],00,00,00]
```

Here `high = sat(param[0xed] + 6, 0x3c, 2)`,
`low = sat(param[0xed] - 2, 0x3c, 2)`, and
`aux = wrapping(param[0xee] + 2)`. The high value is deliberately computed
twice by two identical child calls.

Before publishing the pairs, the body obtains read masks for blocks `0x6b`
and `0x62`, shifts each result left by two, and writes their OR (`0x00a0` for
this target) into bits 17:4 of `0x2010f820` with a fresh RMW. It then calls
`phy_i2c_paral_write_num(..., 22, 0)`.

The ROM child emits, for every pair, full-word write commands to
`0x2010f800` and `0x2010f804`, then polls bit 25 in each word. After all 22
pairs the archive body restores the host-map image with another
`0x2010f820` RMW to `0x0003fa00` in the replaced field and calls the no-op
critical exit.

No Rust transition performs these 44 paired writes or the surrounding mask
configuration.

## `phy_get_i2c_data`

Size `0x02`. Strict status: **NO-REGISTER-EFFECT**.

The complete body is one `ret`.

## `phy_i2c_master_cmd_mem_init`

Size `0x5be`. Strict status: **MATCHED**.

The function has no explicit input. It calls the pure
`phy_encode_i2c_master(block, register, value)` and then
`phy_i2c_master_fill(index, encoded)` exactly 45 times, for indices
`0..=44`. The ROM encoder produces
`block | register << 8 | value << 16`; the fill child performs one full
32-bit store to `0x2010c000 + index * 4`.

The complete command list is:

```text
00 67:02=07   01..15 6b:01..0f =
   [01,73,ba,88,01,11,fd,bb,02,08,04,a7,7a,f4,81]
16 62:00=68   17 62:04=a8   18 62:0b=44   19 62:0d=0a
20 62:0f=param[18e]          21 62:15=08
22 66:02=70   23 67:02=27
24 67:04=param[e9]           25 67:05=param[e9]
26 67:06=param[ea]           27 67:07=param[ea]
28 67:0c=param[e9]           29 67:0d=param[e9]
30 67:0e=param[ea]           31 67:0f=param[ea]
32 67:14=high                33 67:15=high
34 67:16=low                 35 67:17=param[ed]
36 67:18=aux                 37 67:19=aux
38 67:1c=param[f0]           39 67:1d=param[f0]
40 67:1e=param[f0]|40        41 67:1f=param[f0]
42 63:06=00   43 6a:00=af   44 6a:01=7f
```

`high`, `low`, and `aux` are the same expressions defined for
`phy_i2c_init2`. Rust's `PHY_I2C_MASTER_TEMPLATE`,
`master_dynamic_values_from_snapshot`, and
`configure_i2c_master_command_memory` reproduce every encoded word and all
45 full stores in the same ascending order. The PAC rejects out-of-range
indices, but the bounded Rust loop never supplies one. There are no readiness
or failure branches in the vendor function.

## `phy_i2c_master_mem_cfg`

Size `0x20`. Strict status: **NO-REGISTER-EFFECT**.

The function writes the six-byte caller buffer image
`[0, 0, 1, 1, 0x2c, 1]` in the instruction order
offsets `0, 1, 3, 4, 2, 5`. It has no MMIO or child call.

## `phy_i2c_master_command_mem_cfg`

Size `0x2c`. Strict status: **NO-REGISTER-EFFECT**.

The function writes the caller's first buffer as
`[0, 0, 0, 1, 1, 1, 0x2c, 1]`, in the direct instruction order
offsets `3, 4, 5, 7, 0, 1, 2, 6`, then stores the 32-bit value `2` through
the second pointer. It has no MMIO or child call.

## Member conclusion

The cold-profile pieces used by Rust are substantially recovered:
`phy_i2c_init1`, bias setup and all 45 command-RAM words. Strict equivalence
is still incomplete because:

- five legacy/read-mask table inputs are rejected by Rust;
- arbitrary host-ID inputs no longer execute the vendor host-map RMW;
- the 22-pair `phy_i2c_init2` path is absent;
- chip-level `PhyI2cMasterControl` has no implementation in this repository;
- blocking ROM I2C child behavior remains a documented safety divergence.

Only the unbounded child polls qualify for possible vendor-defect exceptions.
The missing domains, missing target binding and missing `phy_i2c_init2` path
do not.
