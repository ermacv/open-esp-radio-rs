# Line audit: `libphy.a[phy_feature.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines eight external code functions. All eight complete bodies and
relocations were inspected.

## Function results

| Function | Size | Strict status | Direct register effect |
| --- | ---: | --- | --- |
| `phy_set_most_tpw_new` | `0x1a` | NOT-PORTED | indirect TX-gain-memory regeneration |
| `phy_get_adc_rand` | `0x170` | NOT-PORTED | six direct RMW locations plus clock/PBus children |
| `phy_internal_delay` | `0x04` | NO-REGISTER-EFFECT | none; returns zero |
| `phy_ftm_comp` | `0x1e` | NO-REGISTER-EFFECT | none; pure parameter lookup |
| `phy_11p_set` | `0x12` | NOT-PORTED | none immediately; changes later channel behaviour |
| `phy_freq_mem_backup` | `0x02` | NO-REGISTER-EFFECT | none; single `ret` |
| `phy_set_rate` | `0x40` | NOT-PORTED | two PHY-I2C masked writes |
| `phy_get_rx_freq` | `0x5e` | NO-REGISTER-EFFECT | none; pure packed-field arithmetic |

## `phy_set_most_tpw_new`

The body:

1. stores the low input byte to `phy_param[0x06]`;
2. loads the halfword at `phy_param[0x11c]`;
3. tail-calls `phy_wifi_set_tx_gain_new(loaded_halfword, 0)`.

Rust can calculate and publish Wi-Fi TX-gain tables during channel setup, but
it has no runtime owner that preserves this setter's parameter mutation and
mandatory regeneration trace. It is therefore not ported as a function.

## `phy_get_adc_rand`

The function first calls `phy_param_addr(phy_param)`, calls
`phy_get_romfuncs()`, and stores the returned pointer in `rom_phyFuns`. This
happens for both input branches.

For nonzero input, the direct 32-bit RMW trace is:

| Order | Address | Operation |
| ---: | ---: | --- |
| 1 | `0x20704184` | OR `0x003c0000` |
| 2 | `0x207040f0` | OR `0x10000000` |
| 3 | `0x20109c04` | OR `0x80000000` |
| 4 | `0x20109c14` | OR `0x0000e400` |
| 5 | `0x20109c0c` | replace bits 15:12 with `0x4` |
| 6 | `0x20109c0c` | fresh read, replace bits 27:24 with `0x4` |
| 7 | `0x20109c0c` | fresh read, replace bits 31:28 with `0x4` |
| 8 | `0x20100894` | OR `0x00100000` |

Between orders 7 and 8 it calls `phy_set_rxclk_en(1)`. It then calls, in exact
order:

1. `phy_pbus_debugmode()`;
2. `phy_pbus_force_test(4, 1, 0)`;
3. `phy_pbus_force_test(4, 2, 1)`;
4. `phy_pbus_force_test(5, 1, 0)`;
5. `phy_pbus_force_test(0, 1, 0)`;
6. `phy_pbus_force_test(0, 2, 0)`;
7. `phy_pbus_force_test(1, 1, 0x189)`;
8. `phy_pbus_force_test(1, 2, 0x100)`;
9. `phy_pbus_set_dco([0x100, 0x100, 0x100, 0x100])`.

Zero input instead calls only:

1. `phy_pbus_xpd_rx_off()`;
2. `phy_pbus_workmode()`;
3. `phy_set_rxclk_en(0)`.

The direct enable configuration is not restored by the zero branch. There is
no equivalent Rust parent. Some PBus and clock leaves exist, but the ordered
feature, PMU/platform operations and ADC register identities are not composed.

## `phy_internal_delay`

The complete body is `li a0, 0; ret`. It performs no delay despite its name and
has no register or global-state effect.

## `phy_ftm_comp`

The signed byte at `phy_param[0x11f]` selects:

| Parameter | Return |
| ---: | ---: |
| 0 | `0xfd` |
| 1 | `0xc7` |
| any other signed value | `0x97` |

This pure helper is absent from Rust. Its input byte is retained in the Rust
parameter image, but that does not implement the function.

## `phy_11p_set`

The body stores its first input byte to `phy_param[0x28]` and its second input
byte to `phy_param[0x29]`, then returns. It performs no immediate MMIO, but the
state controls a later branch in `phy_chip_set_chan`.

Rust preserves equivalent bytes inside `PhyChipChannelParameters` but has no
setter entry point and does not execute the later 802.11p channel branch.
This is therefore not ported.

## `phy_freq_mem_backup`

The complete body is a single `ret`. It has no register, memory or return-value
contract beyond returning to its caller.

## `phy_set_rate`

The function always performs two masked writes, in order:

1. block `0x6b`, host 1, register `0x07`, bits 3:0 receive zero;
2. block `0x6b`, host 1, register `0x0a`, bits 5:3 receive one when the unsigned
   input is at least 8, otherwise zero.

Both calls use `phy_i2c_writeReg_Mask`. The generic Rust PHY-I2C engine can
represent these commands, and cold initialization writes full initial values
to both registers, but there is no runtime `phy_set_rate` owner. The two-write
trace is not ported.

## `phy_get_rx_freq`

This is a pure signed packed-field transform:

- for selector 0 through 7, sign-extend the low 15 bits of the low input
  halfword and divide by `-48`;
- for selectors above 7, shift the input right by 15, sign-extend the selected
  15-bit value, multiply by `-5`, then divide by `128`;
- all results are truncated with RISC-V signed division semantics and
  sign-extended back from 16 bits.

No equivalent Rust helper was found. It has no direct or transitive register
effect.

## Member conclusion

No function in this member is fully matched by an owned Rust entry. The
important missing register traces are ADC-random analog setup, runtime rate
selection and maximum-power-triggered gain regeneration. The 802.11p setter
also feeds a missing channel branch.

None of these differences qualifies as a vendor-defect exception.
