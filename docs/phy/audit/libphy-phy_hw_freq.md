# Line audit: `libphy.a[phy_hw_freq.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines seven external code functions. Every instruction, branch,
jump-table arm and relocation in the member was inspected. The register
children used to interpret the bodies were checked in the pinned rev0 ROM ELF.
All seven functions have a closed strict result: one
**NO-REGISTER-EFFECT**, two **NOT-PORTED**, and four **MISMATCH**.

## `phy_freq_offset_set`

Size `0x02`. Strict status: **NO-REGISTER-EFFECT**.

The complete function is one compressed `ret`. It reads and writes no register
or memory. No Rust replacement is needed for register parity.

## `phy_freq_get_i2c_data`

Size `0x208`. Strict status: **MISMATCH**.

The function receives four output arrays and an eight-bit count. Its complete
hardware prefix is:

1. masked-write `1` to bit 6 of PHY-I2C `0x62:0x0b`;
2. byte-read `0x62:0x0b`;
3. byte-read `0x63:0`;
4. byte-read `0x67:3`.

It derives:

- `sdm_clear = i2c[0x63:0] & 0xf7`;
- `sdm_set = i2c[0x63:0] | 0x08`;
- `fe_selected = i2c[0x67:3] | (phy_param[0x1af] << 2)`, truncated
  to a byte;
- `fe_cleared = i2c[0x67:3] & 0xfb`;
- the final three-byte data word
  `[fe_selected, fe_cleared, fe_selected]`.

There is no `& 1` before shifting `phy_param[0x1af]`. The vendor consumes the
entire byte modulo eight bits.

The loop zero-extends its index on every iteration, stops at the caller's
count, and emits the following four parallel descriptors:

| Index | Block | Register | Encoded index | Nonzero data |
| ---: | ---: | ---: | ---: | --- |
| 0 | `0x62` | `1` | `0x20` | none |
| 1 | `0x62` | `2` | `0x21` | none |
| 2 | `0x63` | `0` | `0x10` | word = `sdm_clear` |
| 3 | `0x63` | `6` | `0x22` | none |
| 4 | `0x63` | `5` | `0x23` | none |
| 5 | `0x63` | `4` | `0x24` | none |
| 6 | `0x63` | `3` | `0x25` | none |
| 7 | `0x63` | `0` | `0x11` | word = `sdm_set` |
| 8 | `0x62` | `0x0b` | `0x12` | word = captured `0x62:0x0b` |
| 9 | `0x61` | `0x0a` | `0x26` | none |
| 10 | `0x67` | `3` | `0x00` | three-byte front-end word |

For every requested index greater than 10, all four corresponding output
elements are explicitly zeroed.

Rust `PhyFrequencyI2cTransition` reproduces the prefix and these eleven
descriptors when the count is exactly 11 and `phy_param[0x1af]` is 0 or 1.
It is not an all-input replacement:

- it has no count input and always materializes exactly eleven descriptors;
- it models `phy_param[0x1af]` as `bool`;
- `PhyColdState::channel_frequency_control` converts every nonzero byte to
  `true`;
- Rust consequently uses only `front_end | 4`, whereas the vendor uses
  `front_end | ((raw_byte << 2) & 0xff)`.

For example, raw parameter byte `2` sets bit 3 in the vendor data but bit 2 in
Rust. This is a register-memory data mismatch, not a vendor-defect exception.
The default cold image contains `1`, so that particular profile does not
expose the defect.

## `phy_freq_i2c_data_write`

Size `0x32`. Strict status: **MISMATCH**.

The body allocates three eleven-byte descriptor arrays and eleven 32-bit data
words, then performs exactly:

1. `phy_freq_get_i2c_data(block, register, encoded, data, 11)`;
2. `phy_freq_i2c_write_set(block, register, encoded, data, 11,
   original_input)`.

Rust explicitly implements only `phy_freq_i2c_data_write(1)`. That is the
value used by `phy_set_chan_freq_hw_init`, but the vendor function forwards
every other input too. In ROM `phy_freq_i2c_write_set`, zero suppresses all
frequency-memory writes while still publishing number addresses; nonzero
enables the memory writes. Rust exposes no corresponding mode input and
always writes the memory records. The raw-byte mismatch from
`phy_freq_get_i2c_data` is inherited as well.

## `phy_bt_txpwr_freq`

Size `0x84`. Strict status: **NOT-PORTED**.

The function calls `phy_get_freq_mem_param(2)`, takes its high byte as the
base and its middle byte as the stride, and iterates over all 85 indices
`0..=84`. On every iteration it:

1. reloads byte 1 from the caller's table;
2. calls `phy_bt_chan_pwr_interp(table, index)`;
3. computes the 16-bit address
   `base + index * stride + 6` with `phy_get_freq_mem_addr`;
4. computes signed-eight-bit
   `table[1] - interpolated_power`;
5. calls `phy_freq_i2c_mem_write(address, signed_delta, 1)`.

The ROM write child updates the frequency-memory address at `0x2010001c`,
writes the mode/data word to `0x2010002c`, and pulses bit 20 of
`0x2010001c`. Rust has no BT transmit-power frequency-table operation
corresponding to this 85-write trace.

## `phy_get_rf_freq_cap`

Size `0x78`. Strict status: **NOT-PORTED**.

For original arguments `(frequency, offset, sdm_buffer, output)`, the complete
body:

1. calls `phy_set_rfpll_freq(phy_param[0x4f], frequency, offset,
   sdm_buffer)`;
2. reads bits 7:0 from PHY-I2C `0x62:5` and stores the byte to `output[0]`;
3. reads bit 2 from `0x62:7`;
4. reads bits 5:0 from `0x62:6`;
5. stores `0x80 | (bit_2 << 6) | low_six_bits` to `output[1]`.

The first child programs and calibrates RFPLL before the three reads. Rust has
RFPLL calibration machinery used by other parents, but no callable
replacement with this argument and two-byte output contract. The complete
function is therefore absent.

## `phy_get_rf_freq_init`

Size `0x1d8`. Strict status: **MISMATCH**.

The input contract is `(count, signed_frequency_offset)`. The function first
tests `phy_param[0xa4] & 0x20`; when set, it returns without hardware access.
Otherwise the exact prefix is:

1. `phy_write_pll_cap(200)`;
2. masked-write bit 7 of PHY-I2C `0x62:2` to zero;
3. `phy_set_rfpll_freq(phy_param[0x4f], 0x985, offset, sdm)`;
4. masked-write bits 5:0 of `0x62:2` to `0x3f`;
5. masked-write bit 7 of `0x62:2` to one;
6. `phy_set_rfpll_freq(..., 0x960, offset, sdm)`, then read the low
   capacitor endpoint;
7. `phy_set_rfpll_freq(..., 0x9a0, offset, sdm)`, then read the high
   capacitor endpoint;
8. byte-read `0x63:6` and retain its upper five bits.

The loop starts at frequency code `0x960`. For every index before the caller's
count it:

1. calls the pure ROM `phy_rfpll_set_freq(code, crystal, offset, sdm)`;
2. computes signed `low_cap + (accumulator / 64)`;
3. saturates the result to `0..=511`;
4. increments the index and then adds `high_cap - low_cap` to the
   accumulator;
5. calls `phy_get_xtal_duty(code)`;
6. packs the saturated capacitor, five SDM bytes, and duty into three words;
7. calls `phy_wr_rf_freq_mem(index_before_increment, words)`, producing
   three mode-7 writes.

At loop completion it fresh-reads the word at `phy_param[0xa4]`, ORs bit 5,
stores it, and returns.

For the parent's fixed `(85, 0)` call, Rust
`PhyChannelFrequencyInitTransition` and `PhyFrequencyTableTransition`
preserve this order, the three calibration frequencies, all 85 records and
all 255 memory writes. The stateless record arithmetic also matches the
vendor loop: signed truncating `/ 64`, saturation, SDM packing and capacitor
bit 8 in bit 6 of the second byte.

It is nevertheless not an all-input replacement:

- Rust hard-codes 85 entries rather than accepting `count`;
- Rust hard-codes offset zero in this parent;
- the lower RFPLL request represents its offset as `u8`, whereas the vendor
  arithmetic receives and uses a signed value.

The complete RFPLL child trace is audited separately; the mismatches above
already close this function as **MISMATCH**.

## `phy_set_chan_freq_hw_init`

Size `0x28`. Strict status: **MISMATCH**.

Both explicit arguments are ignored. The direct body always executes:

1. `phy_freq_reg_init()` using the incoming ABI values still present in
   `a0/a1`;
2. `phy_get_rf_freq_init(85, 0)`;
3. tail-call `phy_freq_i2c_data_write(1)`.

`phy_freq_reg_init` performs three ordered RMWs at `0x2010001c`, followed by
full stores `0x19800249` to `0x20100024` and `0x25824e58` to
`0x20100028`. Its hidden `phy_param[0x193]` branch replaces incoming
`(a0, a1)` by `(0, 2)` when nonzero. The ordinary cold parent supplies
`(2, 4)`, producing register-mode byte `0x42`; the override produces `0x20`.
The PAC leaf `initialize_frequency_registers` matches all five register
updates and both branch images.

Rust's aggregate implements the normal fixed child calls and the warm
`phy_param[0xa4]` branch. It still inherits the raw
`phy_param[0x1af]`-to-`bool` mismatch in the final descriptor data, so the
complete parent differs for legal vendor state such as a parameter byte of
`2`. The default image's value `1` remains only a profile-scoped match.

## Member conclusion

The existing Rust implementation is strong evidence for the current cold
Wi-Fi profile, especially the frequency-register setup and the 85-entry RF
table. It is not a complete behavioral port of this member:

- BT transmit-power frequency memory is absent;
- RF-cap acquisition is absent as a standalone operation;
- general descriptor counts and write-enable inputs are not represented;
- general table counts and signed offsets are not represented;
- the full vendor front-end parameter byte is incorrectly narrowed to a
  Boolean.

None of these differences is classified as a vendor error.
