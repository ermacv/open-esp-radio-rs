# Line audit: `libphy.a[phy_api.o]`

Artifact:
`_oracles/libphy.a`, SHA-256
`51497819736295c9b33d6775495dade4c6fb39db887edfe095608c670d9ae223`.

The member defines 36 external code functions. Every direct instruction and
relocation was inspected. Most register behaviour is delegated to ROM or other
archive children, so these entries remain **BODY-AUDITED** until each child has
a strict register-trace proof.

## Direct tail-call wrappers

Each function below consists only of `auipc` plus `jr` with the named call
relocation. It forwards its input registers and return value unchanged.

| Public wrapper | Delegated child |
| --- | --- |
| `RFChannelSel` | `phy_chip_set_chan` |
| `phy_rx_rifs_en` | `phy_wifi_rifs_mode_en` |
| `bb_wdt_rst_enable` | `phy_bb_wdt_rst_enable` |
| `bb_wdt_int_enable` | `phy_bb_wdt_int_enable` |
| `bb_wdt_timeout_clear` | `phy_bb_wdt_timeout_clear` |
| `bb_wdt_get_status` | `phy_bb_wdt_get_status` |
| `tx_pwctrl_background` | `phy_tx_pwctrl_background` |
| `ant_dft_cfg` | `phy_ant_dft_cfg` |
| `ant_wifitx_cfg` | `phy_ant_wifitx_cfg` |
| `ant_wifirx_cfg` | `phy_ant_wifirx_cfg` |
| `ant_bttx_cfg` | `phy_ant_bttx_cfg` |
| `ant_btrx_cfg` | `phy_ant_btrx_cfg` |
| `esp_tx_state_out` | `phy_tx_state_out` |
| `set_cca` | `phy_set_cca` |
| `set_rx_sense` | `phy_set_rx_sense` |
| `read_hw_noisefloor` | `phy_read_hw_noisefloor` |
| `rx_gain_force` | `phy_rx_gain_force` |
| `tx_state_set` | `phy_tx_state_set` |
| `bt_track_pll_cap` | `phy_bt_track_pll_cap` |
| `phy_set_chan_misc` | `phy_chip_set_chan_misc_new` |

The Rust repository contains some related finite leaves—for example hardware
noise-floor decoding, cold antenna initialization and channel setup—but this
does not close these wrapper rows. Each delegated child's complete input domain
and register trace must first be audited.

## Composite wrappers

### `phy_change_channel`

Size `0x16`. It replaces `a1` with the original fourth argument in `a3`, calls
`phy_set_chanfreq(a0, a3)`, discards the child's return value and returns zero.
No other direct side effect exists.

### `ant_tx_cfg`

Size `0x22`. It preserves the original first argument, calls
`phy_ant_wifitx_cfg(a0, a0)`, restores the original first argument, then
tail-calls `phy_ant_bttx_cfg(a0)`. Both Wi-Fi and BT antenna children are
mandatory and ordered.

### `ant_rx_cfg`

Size `0x30`. It preserves the original three arguments, calls
`phy_ant_wifirx_cfg(a0, a1, a2)`, restores all three, then tail-calls
`phy_ant_btrx_cfg(a0, a1, a2)`. Both children are mandatory and ordered.

### `phy_xpd_tsens`

Size `0x30`. It reads `phy_param[0x195]`. When that byte is zero, it first
calls `phy_set_tsens_power(0)`. Both branches then store one to
`phy_param[0x195]` and return. A nonzero previous byte causes no register call.

The Rust temperature state machine owns sensor power operations for sampling,
but this one-shot parameter latch has not yet been proved equivalent.

## Parameter-only functions

| Function | Size | Complete body |
| --- | ---: | --- |
| `phy_current_level_set` | `0x0a` | store low input byte to `phy_param[0x2c]` |
| `phy_bt_power_track` | `0x0a` | store low input byte to `phy_param[0x0b]` |
| `phy_ble_set_chan_base` | `0x0a` | store low input byte to `phy_param[0x193]` |
| `phy_init_param_set` | `0x0c` | store `input & 1` to `phy_param[0x196]` |
| `phy_track_temp_debug` | `0x12` | store input bytes to `phy_param[0x1b0]` and `[0x1b1]` |

These functions have no immediate MMIO. Their state can affect later
register-producing functions, so their Rust equivalence will be closed with
the relevant lifecycle and tracking audits. The current cold Rust image
mentions `phy_init_param_set(1)` but deliberately keeps byte `0x196` at zero;
that is already an observable mismatch for that input, not a vendor defect.

## Constant-return functions

| Function | Return |
| --- | ---: |
| `phy_get_rf_cal_version` | `100` |
| `phy_get_rfdata_num` | `524` |

Neither function has a register or memory side effect.

## No-op entry points

Each complete body is a single `ret`:

- `phy_bbpll_en_usb`;
- `noise_check_loop`;
- `set_bb_wdg`;
- `phy_pwdet_always_en`;
- `phy_pwdet_onetime_en`.

They perform no direct or transitive operation. Their incoming `a0` value is
not replaced, so callers must not infer a defined return value unless the
external ABI declares the function `void`.

## Current Rust boundary

No register parity is claimed merely from these wrappers. The following
classes remain:

- channel wrappers: existing Rust path is narrower than the vendor input
  domain;
- watchdog, CCA, sensitivity, RIFS, TX-state, RX-force and tracking wrappers:
  runtime entry graphs are absent or not yet strictly compared;
- antenna wrappers: cold antenna initialization does not replace the public
  Wi-Fi-plus-BT composite operations;
- noise-floor wrapper: a promising PAC implementation exists, but the ROM
  child proof is still required;
- parameter setters: several bytes exist in Rust snapshots, but setter and
  downstream trace equivalence is not established.

No vendor-defect exception was used for this member.
