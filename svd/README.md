# ESP32-S31 radio register source

`esp32s31-radio.svd` is the editable machine-readable source for the recovered
radio clock, reset and power PAC. Run:

```console
tools/generate-esp32s31-radio-pac.py
```

The generated file is
`crates/open-esp-radio-pac-esp32s31/src/power.rs`. The source-only audit runs
the generator with `--check`, so a direct edit of generated Rust fails CI.

## Evidence policy

Descriptions use `SOURCE[...]` and `CONFIDENCE[...]` tags. Confidence has the
following meaning:

- `exact-s31-layout`: field name, offset and width come from an ESP32-S31
  register structure or SVD;
- `instruction-exact-semantics-unknown`: the complete ROM/blob instruction
  body proves the address, mask and operation, but not the hardware meaning;
- `register-exact-fields-unknown`: the S31 structure names the register while
  its internal bit layout is still unknown;
- `mixed-per-field`: individual fields in the register have different evidence.

Unknown and reserved fields are omitted. An `OPAQUE` or `UNKNOWN` name is
intentional: it records usable instruction-level evidence without converting a
neighboring-chip similarity into an S31 fact.

## Recovered sources

| Source ID | Basis |
|---|---|
| `S31_MODEM_SYSCON_STRUCT` | Pinned `esp-wifi-sys` commit `2585f278`, S31 `modem_syscon_struct.h`, SHA-256 recorded in the SVD |
| `S31_MODEM_LPCON_STRUCT` | Same commit, S31 `modem_lpcon_struct.h`, SHA-256 recorded in the SVD |
| `S31_PMU_HEADERS` | Same commit, S31 `pmu_reg.h` and `pmu_struct.h` |
| `S31_ESP_PACS_SVD` | Local `esp-pacs` commit `f823dd9d`, ESP32-S31 generated SVD |
| `ROM_REV0_PHY_OPEN_FE_BB_CLK` | Complete 0x38-byte rev0 ROM body at `0x2f823ec0` |
| `BLOB_LIBPHY_PHY_CLOSE_FE_BB_CLK` | Complete 0x20-byte `libphy.a[phy_init.o]` body |
| `BLOB_LIBPHY_PHY_BB_INIT` | Complete 0x16a-byte `phy_bb_init` body and relocation graph |

The public ESP-IDF ESP32-C5/C61 register headers are only cross-chip
validation. They are not accepted as the sole basis for an S31 address or bit.
