# Compiled ESP32-S31 knowledge ownership

The reviewed chip pack identifies `esp32s31-rev0-radio-v1`; the investigation
manifest identifies one vendor artifact composition. Compiled facts move from
the investigation into the chip provider only when their current evidence
establishes applicability to every investigation that may select that rev0
chip pack. Symbol spelling or a ROM-looking address is not sufficient.

## Reusable rev0 chip provider

| Fact | Owner | Why reusable |
|---|---|---|
| Cold thirteen-slot `rom_phyFuns` table | chip contracts | Every target is an explicit ESP32-S31 rev0 ROM address and the chip pack selects that ROM revision; no archive symbol participates. |
| `none` and `esp32s31-phy-cold` entry states | chip contracts | They refer only to absence of mutable registration or the cold ROM table. |
| `ets_printf(format, ...)` diagnostic boundary | chip contracts | The reviewed rev0 ROM boundary retains only its stable format-pointer ABI; variadic payload is deliberately excluded. |
| `rtc_clk_xtal_freq_get() -> 40` semantic | chip knowledge | The chip pack establishes the fixed ESP32-S31 40 MHz crystal contract; it is independent of a vendor library body. |
| Generic C and ESP-IDF semantics | generic add-ons, composed by chip knowledge | They contain no ESP32-S31 address or private blob identity. |

## Investigation overlay retained fail-closed

| Module/fact | Guard or dependency | Why it is not promoted |
|---|---|---|
| `pp_post` semantic | exact `pp.o` body plus complete relocation schema | It describes one private libpp archive member. |
| `phy_get_i2c_hostid_new` | exact linked address and body | The function is supplied by the project `archive` source, not ROM. |
| `phy_get_i2c_hostid_`, `phy_chip_i2c_readReg_org`, `phy_chip_i2c_writeReg` | exact body and absolute address | They look ROM-specific, but no separate reviewed applicability record currently proves that these body summaries are valid for every artifact composition using the chip pack. |
| RFPLL, frequency-offset and IQ-estimator summaries | exact name/address/size plus absolute internal call sites, messages or MMIO sequence | Size/address identity is weaker than authenticated body applicability; retain locally until independent rev0 ROM evidence is recorded. |
| `__divdi3` intrinsic | exact name/address/size, no body digest | The summary has no independent chip applicability proof and therefore stays out of the reusable provider. |
| Registered PHY table | eleven `SourceSymbol { source = "archive", ... }` targets and linked mutable pointer symbols | Its meaning depends on this investigation's archive lineage and registration lifecycle. |
| Wi-Fi OSI v9, coex adapter v2 and runtime callback v1 models | versioned foreign keys selected by the project's reviewed interface tables | ABI layout/version belongs to the supplied SDK/blob lineage. |
| `wifi_log`, `wifi_assert`, `phy_printf`, `pp_printf`, `net80211_printf`, `coexist_printf` | private vendor-library symbols and argument contracts | The current evidence is project ABI evidence, not chip ROM evidence. |
| Verification profiles, suites, dispositions and policies | compiled vendor/Rust artifacts and bounded comparison claims | Verification-harness composition is always project-owned. |

## Composition invariant

`chip.toml` selects `esp32s31-rev0-chip-knowledge-v1`. The project selects
`esp32s31-radio-knowledge-v1`, whose compiled descriptor explicitly
`extends = "esp32s31-rev0-chip-knowledge-v1"`. Blobray accepts the pair only
when both descriptors are installed, the base is a reusable root, and the
overlay contract set is a complete superset of the base. The effective cache
identity includes both provider IDs and revisions. A missing, mismatched or
contract-dropping overlay fails before analysis; manifest ordering is never an
override mechanism.
