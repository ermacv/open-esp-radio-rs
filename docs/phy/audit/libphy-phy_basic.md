# Line audit: `libphy.a[phy_basic.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines one external code function:
`phy_chan14_mic_cfg_new`, size `0x46`.

## `phy_chan14_mic_cfg_new`

Strict status: **NOT-PORTED**.

### Complete instruction behaviour

Input `a0` is treated as enabled only when it equals exactly one. Every other
value takes the disabled branch.

Enabled branch:

1. load the 32-bit word at `0x20107400`;
2. AND with `0xffff9fff`, clearing bits 14:13;
3. OR with `0x00002000`, setting bits 14:13 to binary `01`;
4. write the 32-bit result to `0x20107400`;
5. signed-load `phy_param[0x24]`;
6. tail-call `phy_set_most_tpw_new(value)`.

Disabled branch:

1. load the 32-bit word at `0x20107400`;
2. OR with `0x00006000`, setting bits 14:13 to binary `11`;
3. write the 32-bit result to `0x20107400`;
4. signed-load `phy_param[0x06]`;
5. tail-call `phy_set_most_tpw_new(value)`.

There are no other instructions, loops, delays, returns or relocations. The
tail-call child stores its argument back to `phy_param[0x06]`, loads the
halfword at `phy_param[0x11c]`, and invokes
`phy_wifi_set_tx_gain_new(value, 0)`. The child gain-generation and
gain-memory trace remains a separate strict audit item.

### Register identity

`0x20107400` is in the recovered `BBTX` window. The PAC already exposes it as
`PHY_BASEBAND_CONFIG_ORACLE.BASEBAND_INIT_7400`, with the recovered two-bit
field at bits 14:13. The identity is therefore available; the missing
behaviour is at the HAL/PHY composition layer.

### ROM relationship

ROM `phy_chan14_mic_cfg` at `0x2f826144`, size `0x42`, has the same direct
register branch and parameter selection. It tail-calls ROM
`phy_set_most_tpw` instead of the target-specific archive child.

ROM `phy_chan14_mic_enable` at `0x2f826186`, size `0x26`, stores the enable byte
at `phy_param[0x26]`. Disable immediately delegates to
`phy_chan14_mic_cfg(0)`. Enable clamps its second argument to at most `0x30`,
stores it at `phy_param[0x24]`, and returns without touching the BBTX register.

### Rust comparison

`PhyChipChannelRequest::validate` currently:

- returns `Channel14MicEnabled` whenever `channel_14_mic_enabled` is true;
- rejects normalized channel 14 as `UnsupportedChannel(14)`;
- emits none of the vendor register or gain-publication operations.

Rust does initialize bits 14:13 of the same register to `11` as part of
`phy_bb_reg_init`, but that one-time baseband initialization is not a
replacement for the runtime enabled/disabled branch above.

Therefore:

- register address availability: **present in PAC**;
- direct vendor RMW trace: **absent**;
- channel-14 maximum-power parameter update: **absent**;
- required TX-gain regeneration: **absent**;
- complete function parity: **NOT-PORTED / REGISTER TRACE MISMATCH**.

This is not a vendor-defect exception.
