# Line audit: `libphy.a[phy_rfpll.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines four external code functions. All direct instructions,
branches and relocations were inspected. One function is closed as
**NOT-PORTED**; the other three remain **BODY-AUDITED** while their complete
ROM child traces are checked.

## `phy_chip_set_chan_misc_new`

Size `0x24`. It always:

1. calls `phy_set_chan_reg(1)`;
2. tail-calls `phy_wifi_set_tx_gain_new(original_channel, 0)`.

The original channel argument is preserved across the first child. There are
no conditional branches. Rust channel setup contains channel-register and
gain-generation work, but this exact two-child trace remains open until both
children are strictly audited.

## `phy_chip_set_chan`

Size `0x10e`. The complete direct order is:

1. call `phy_i2c_enter_critical`;
2. load signed `phy_param[0x20..0x22)` as frequency offset;
3. call `phy_chan_to_freq(original_input)` and retain the frequency;
4. when the unsigned original input is greater than `0x96b` (2411), call
   `phy_mhz2ieee` and use its zero-extended 16-bit result as channel;
5. store CBW byte to `phy_param[0x11f]`;
6. store channel halfword to `phy_param[0x11c]`;
7. store `CBW != 0` to `phy_param[0x11e]`;
8. call `phy_disable_agc()`;
9. call `phy_bbpll_cal(1)`;
10. call `phy_tsens_temp_read()`;
11. call `phy_set_channel_rfpll_freq(frequency, phy_param[0x4f],
    frequency_offset)`;
12. call `phy_chip_set_chan_misc_new(channel)`;
13. call `phy_i2c_master_mem_txcap()`;
14. call `phy_bb_cbw_chan_cfg(phy_param[0x11f])`;
15. when `phy_param[0x26] != 0`, call
    `phy_chan14_mic_cfg_new(channel == 14)`;
16. when `phy_param[0x28] != 0`, call
    `phy_11p_set(phy_param[0x28], phy_param[0x29])`;
17. call the function pointer at `g_phyFuns + 0x14`;
18. call `phy_bbpll_cal(0)`;
19. call `phy_dc_mem_clr()`;
20. call `phy_enable_agc()`;
21. tail-call `phy_i2c_exit_critical`.

Both archive critical-section definitions are no-op `ret` stubs, but the calls
and their ownership contract are still part of the vendor graph.

### Confirmed Rust mismatches

The Rust channel request contains `frequency_offset`, but a source-wide use
audit finds it only in the struct declaration and default construction. It is
not consumed by `PhyChipChannelTransition`, while the vendor always passes the
signed value to `phy_set_channel_rfpll_freq`. Every nonzero offset profile is
therefore a register-programming mismatch.

Rust also rejects channel 14 and any enabled channel-14 MIC option before
emitting transactions. The vendor executes both paths. The exact missing
channel-14 register trace is closed in
[the `phy_basic.o` audit](libphy-phy_basic.md).

The 802.11p function in this artifact only rewrites its existing two parameter
bytes, but the public feature/lifecycle remains absent. The indirect callback
and remaining child traces still require strict proof.

## `phy_chip_set_chan_offset`

Size `0x7c`. Strict status: **NOT-PORTED**.

The complete body:

1. enters the I2C critical section;
2. computes signed-16 `rounded = (input + 2) & ~3`;
3. stores it at `phy_param[0x20]`;
4. when `phy_param[0x9f] != 0`, adds
   `signed(phy_param[0xa0]) * 8` and stores the signed-16 result back;
5. calls `phy_freq_correct(1, phy_param[0x20])`;
6. calls `phy_disable_agc()`;
7. calls `phy_set_channel_rfpll_freq(phy_param[0x11c],
   phy_param[0x4f], phy_param[0x20])`;
8. calls `phy_enable_agc()`;
9. tail-calls `phy_i2c_exit_critical`.

Rust has no transition that updates and applies a runtime channel-frequency
offset. Merely carrying an unused `frequency_offset` field does not implement
this function.

## `phy_set_chanfreq`

Size `0x24`. It:

1. calls `phy_mhz2ieee(first_input)`;
2. zero-extends the low 16-bit channel result;
3. calls `phy_chip_set_chan(channel, saved_second_input)`;
4. discards the child result and returns zero.

The Rust channel constructor accepts an already-normalized frequency and
performs a corresponding conversion, but the child has known unsupported
branches and the nonzero-offset mismatch above. This wrapper cannot be marked
MATCHED until `phy_chip_set_chan` closes.

## Member conclusion

The default zero-offset, channel-1-through-13 path remains a useful profile
match. Under the required all-input criterion this member is not equivalent:

- runtime offset programming is absent;
- the offset field is ignored even by ordinary channel changes;
- channel 14/MIC is rejected instead of programmed;
- several delegated register children remain strict audit dependencies.

None of these differences is a vendor-defect exception.
