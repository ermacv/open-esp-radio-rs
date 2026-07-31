# Line audit: `libphy.a[phy_rx_gain.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines six external code functions. Every instruction, relocation,
branch, loop and stack/table initializer was inspected. All six functions have
closed strict results: one has no register effect, one is not ported, and four
have non-defect mismatches in the Rust implementation.

## `phy_get_rxbb_dc_new`

Size `0x2e`. Strict status: **NO-REGISTER-EFFECT**.

The four inputs are a two-halfword base, a table of two-halfword adjustments,
a two-halfword output and an unsigned index. The body:

1. compares the original full-width index with `5`;
2. uses `index & 0xff` when it is at most `5`, otherwise uses `5`;
3. reads the selected two halfwords at `table + index * 4`;
4. writes two wrapping 16-bit sums to the output.

It has no MMIO and no child call. Rust embeds the same clamp and wrapping
halfword additions in `phy_generated_rx_gain_memory_entry` for the only
reached caller domain, where the index is the population count of encoded
bits 4 through 9. Rust does not expose the standalone raw-pointer ABI, but
this leaf itself has no direct or transitive register transaction.

## `phy_wr_rx_gain_mem_new`

Size `0x1c6`. Strict status: **MISMATCH**.

The five inputs are the bank selector, byte-sized entry count, encoded-record
pointer, bank-specific DC table and two-halfword DC base. Bank zero uses
memory indices starting at zero; every nonzero bank value starts at `0x50`.
The count is added to that base and truncated to a byte. The loop increments
the byte index until it wraps or reaches the base, so the vendor accepts every
count in `0..=255`.

The complete hardware order is:

1. `phy_pbus_debugmode()`;
2. `phy_pbus_xpd_rx_on(0)`;
3. `phy_set_rxclk_en(1)`;
4. `phy_set_txclk_en(1)`;
5. one `phy_write_gain_mem` call per input record;
6. `phy_set_rxclk_en(0)`;
7. `phy_set_txclk_en(0)`;
8. `phy_pbus_xpd_rx_on(0)`;
9. `phy_pbus_workmode()`.

For each record, the body calls
`phy_rfrx_gain_index_new(bank, record >> 12)`, uses
`(gain_index * 4) & 0x1fc` for the two DC-table halfwords, counts the six
bits 4 through 9, and calls
`phy_get_rxbb_dc_new(dc_base, phy_param + 0x1e0, ..., count)`.

Bank zero uses those adjusted DC halfwords, `phy_param[0xd4..0xd5]`, and
fixed mixer digital gain `7`. A nonzero bank replaces both adjusted
halfwords with `0x100`, replaces the auxiliary value with zero, and calls
`phy_bt_rx_mx_dgain(gain_index)`. The complete three-word packing is:

```text
word0 = (dc_i << 31)
      | (dc_q << 13)
      | (index_dc_q << 22)
      | (auxiliary & 0x1fff)

word1 = (((record >> 4) & 0x7f) << 20)
      | ((record & 7) << 17)
      | (index_dc_i << 8)
      | (dc_i >> 1)
      | (mixer_digital_gain << 29)

word2 = (phy_param[2] >> 6)
      | (((record >> 15) & 7) << 5)
      | (((record >> 12) & 7) << 2)
```

`phy_generated_rx_gain_memory_entry` reproduces this packing for the two
fixed generated banks, and the gain-memory HAL preserves the complete ROM
`phy_write_gain_mem` transaction. Strict parity nevertheless fails:

- Rust fixes the records, counts and bank domain instead of accepting the
  complete vendor argument space;
- most importantly, `PhyRxGainPublishTransition` delays only `1 µs` after
  setting the PBus work-mode pulse. Complete ROM `phy_pbus_force_mode(0)`,
  reached by `phy_pbus_workmode`, delays `2 µs` before clearing that pulse.

The second item changes the reached cold-Wi-Fi hardware trace twice, once for
each published bank. The lower PAC/HAL pulse operations are correct; the
upper RX-gain transition supplies the wrong second delay. This is not a
vendor defect.

## `phy_rxiq_cal_init`

Size `0x198`. Strict status: **MISMATCH**.

The second input is unused. The first input enables an optional PBus branch,
and every nonzero third input selects an early return that deliberately skips
hardware cleanup.

The common prefix is:

1. `phy_set_channel_rfpll_freq(0x985, phy_param[0x4f], 0)`;
2. `phy_set_txcap_reg(phy_param + 0xdc, 6)`;
3. store halfword `6` at `phy_param + 0x11c`;
4. set bit 14 and then bit 15 of `0x20100890` through two fresh-read RMWs;
5. `phy_pbus_debugmode()`;
6. `phy_pbus_xpd_rx_on(0)`;
7. `phy_loopback_mode_en(1)`.

When the first input is nonzero, the body additionally calls
`phy_pbus_rd(1, 1)`, ORs bit 1 into the returned halfword, and calls
`phy_pbus_force_test(1, 1, value)`.

The calibration prefix then performs four separately ordered RMWs:

1. set bit 29 of `0x20100438`;
2. set bit 13 of `0x20100c0c`;
3. clear bits 31:30 of `0x20100438`;
4. clear bits 15:14 of `0x20100c0c`.

It calls
`phy_set_rx_gain_cal_iq_new(0, 0x80, phy_param + 0xd4, 0)`, then converts
exactly four halfwords at `0xd4`, `0xd6`, `0xd8`, and `0xda` with:

```text
converted = ((value >> 1) & 0x1f80) | (value & 0x007f)
```

For a zero third input, cleanup is:

1. set bit 30 of `0x20100438`;
2. set bit 14 of `0x20100c0c`;
3. clear bit 29 of `0x20100438`;
4. clear bit 15 of `0x20100890`;
5. `phy_loopback_mode_en(0)`;
6. `phy_pbus_xpd_tx_off()`;
7. tail-call `phy_pbus_workmode()`.

For a nonzero third input the function returns immediately after coefficient
conversion and leaves the calibration setup active.

`PhyRxIqInitTransition` preserves the common `(first = 0, third = 0)` cold
profile, including the four prefix RMWs, coefficient conversion and cleanup.
It intentionally has no representation of the nonzero first-input PBus
read/force branch or the nonzero third-input early-return branch. Those are
vendor-supported register traces, so the complete function is a strict
mismatch rather than a match or vendor-defect exception.

## `phy_rx_table_init`

Size `0x7c`. Strict status: **MISMATCH**.

The first state mutation is a halfword store:

```text
*(u16 *)(phy_param + 0x120) = 0x4f4f
```

It therefore sets both `phy_param[0x120]` and `phy_param[0x121]` to `0x4f`.
The body then calls `phy_write_gain_mem` exactly 79 times, with indices
`0..=78` and these three words:

```text
word0 = 0x40200000
word1 = 0x02010080 | (phy_param[2] << 29)
word2 = 0x000000fc | (phy_param[2] >> 6)
```

The suffix calls `phy_reg_init`, `phy_bb_agc_reg_update`, and tail-calls
`phy_enable_agc`, in that order.

Rust reproduces the 79 entries and child-call order, but
`PhyColdState::prepare_rx_table_init` writes only byte `0x120`. It captures
the old byte `0x121` and passes that stale value into the subsequent Rust
`phy_reg_init` owner. If the old byte was not already `0x4f`, the AGC
register images differ from the vendor. The missing byte store is not a
vendor defect.

## `phy_set_rx_gain_table`

Size `0x28a`. Strict status: **MISMATCH**.

The function's nominal arguments are not consumed after entry. It copies the
two fixed base tables and constructs the fixed advance/threshold tables used
by the two `phy_gen_rx_gain_table` calls:

```text
Wi-Fi base      = [0040,0041,0043,006e,0078,0079,007b,007f]
Wi-Fi advance   = [8,8,10,8,5,7,6,0]
Wi-Fi threshold = [3,5,3,9,12,12,12,12]

shared base      = [0040,0041,0042,0043,006e,0078,0079,007b,027f,017f,007f]
shared advance   = [6,5,5,5,7,5,7,7,5,4,0]
shared threshold = [0,0,0,0,0,0,0,0,0,0,0]
```

Before testing either software guard, the body reads and saves
`0x20100434`. It reads the guard word at `phy_param + 0xa4`.

If bit `0x200` is clear, table generation is required. If bit `0x80` is also
clear, the body:

1. performs a fresh-read RMW clearing bits 23:22 of `0x20100434`;
2. calls `phy_set_rx_gain_cal_dc_new(1, 0, param+0x1b4, param+0x1b4)`;
3. calls `phy_set_rx_gain_cal_dc_new(0, 0, param+0x14e, param+0x16e)`;
4. performs a fresh-read RMW restoring bits 23:22 from the original saved
   register image;
5. sets bit `0x80` in the word at `phy_param + 0xa4`.

The body extracts bit 8 of the halfword at `phy_param + 0x10` as the
generator's diagnostic argument. It generates the Wi-Fi table with maximum
gain `0x1c` and eight base entries, clamps the returned last index to `0x4f`,
stores it at `phy_param[0x121]`, and publishes `last_index + 1` entries to
bank zero. It then generates the shared table with maximum gain `0x12` and
eleven base entries, clamps/stores its last index at `phy_param[0x120]`, and
publishes the second bank. Finally it sets bit `0x200` in the guard word and
copies the temperature halfword at `phy_param[0..=1]` to
`phy_param[0x190..=0x191]`.

Whether or not generation ran, the tail:

1. replaces bits 14:8 of `0x2010702c` with
   `(phy_param[0x121] << 8) & 0x7f00`;
2. clamps that byte to `0x4c` and replaces bits 24:18 of `0x2010713c`;
3. calls `phy_iq_corr_enable()`.

The fixed Rust table generation and final two limit RMWs match their vendor
arithmetic. Complete root parity fails in several independent ways:

- `PhyColdState::rx_gain_init_parameters` reads the guard word from
  `phy_param + 0xb4`, sixteen bytes after the vendor word at `+0xa4`;
- `apply_rx_gain_init_outcome` sets the corresponding completion bits at
  `+0xb4/+0xb5`, also sixteen bytes late;
- when Rust believes DC calibration is already complete, it omits the
  vendor's unconditional initial read of `0x20100434`;
- both nested `phy_wr_rx_gain_mem_new` paths inherit the wrong `1 µs`
  work-mode pulse delay described above.

The first two defects also make later calls take different guarded paths.
None is justified by a vendor defect.

## `phy_rx_table_track`

Size `0xc0`. Strict status: **NOT-PORTED**.

The function computes signed
`phy_param_s16[0x190] - phy_param_s16[0]`, calls `phy_abs_temp`, and returns
without MMIO when the result is at most `40`. Above that threshold it saves
the channel halfword at `0x11c` and signed CBW byte at `0x11f`. A nonzero
input additionally prints the old and new temperatures.

The register/state sequence above the threshold is:

1. clear bits 1:0 of `0x20109c18`;
2. clear bit `0x200` in the guard word at `phy_param + 0xa4`;
3. call `phy_set_rx_gain_table(0x985, 0)`;
4. call `phy_mac_enable_bb()`;
5. copy the current temperature halfword to `phy_param + 0x190`;
6. tail-call `phy_chip_set_chan(saved_channel, sign_extended_saved_cbw)`.

No Rust runtime transition owns this threshold-triggered RX-table
regeneration, baseband re-enable and channel-retune operation. The open crate
contains cold table generation only.

## Member conclusion

The fixed table arithmetic and most cold-path register leaves are present,
but the member is not register-equivalent:

- RX-table initialization fails to set the vendor's second `0x4f` parameter
  byte;
- RX-gain guard state is read and written at the wrong parameter offset;
- an unconditional register read is missing on cached paths;
- both RX-gain bank publications shorten a required pulse delay;
- two RXIQ argument-controlled register traces are omitted;
- runtime temperature-driven RX-table tracking is absent.

These findings are implementation defects or missing functionality in the
open code. No `phy_rx_gain.o` difference qualifies as a vendor-defect
exception.
