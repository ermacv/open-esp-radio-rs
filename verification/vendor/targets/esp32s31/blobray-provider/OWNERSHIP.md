# ESP32-S31 facts and executable model ownership

The reviewed chip pack identifies `esp32s31-rev0-radio-v1`; the investigation
manifest identifies one vendor artifact composition. Compiled facts move from
the investigation into the chip provider only when their current evidence
establishes applicability to every investigation that may select that rev0
chip pack. Symbol spelling or a ROM-looking address is not sufficient.

## Enforced dependency direction

| Crate | Owns | Does not install |
|---|---|---|
| chip `contracts` / `knowledge` | ROM/entry ABI, crystal declaration, pointer encoding facts | runtime adapters or function traces |
| chip `models` | C/ESP-IDF executable adapters and chip harness composition | private investigation reconstructions |
| project `contracts` / `knowledge` | project ABI/entry declarations, reviewed RAM roles, `pp_post` event meaning | executable reconstruction hooks |
| project `models` | exact-body selection, polling/search/scratch/assertion reconstructions, composition with chip models | production driver behavior or qualification policy |
| project host | explicit facts + executable model selection | implicit model installation through a data pack |

The model crates depend on knowledge/contracts. Knowledge crates have no
model-provider, runtime-addon or execution-interpreter dependency. Host tests
check that direct dependency boundary. Type-only access to backend memory
classification/encoding records remains in knowledge.

`KnowledgeProviderDescriptor.execution_models` records the separately selected
implementation, including its kind, applicability description and review source.
These are implementation provenance fields, not facts proving equivalence.
`project doctor` lists both selected executable providers, so agents can see
when temporary manual reconstructions are installed. Function-level hook
applicability remains enforced in executable code.
The optional descriptor field supports neutral/legacy harnesses: `None` does
not prove arbitrary hook code is declarative. This ESP32-S31 split enforces
actual ownership through separate crates and dependency checks. Provider IDs
cannot use the reserved `models:` prefix or identity delimiters, so a fact
provider cannot impersonate an executable model in composed cache provenance.

Model revisions enter the canonical composed provider identity used by cached
stages and comparison provenance. The registry also requires the declared
model ID/revision to match the harness semantic-cache-domain suffix, preventing
model revision changes from silently reusing old function facts.

## Reusable rev0 chip provider

| Fact | Owner | Why reusable |
|---|---|---|
| Cold thirteen-slot `rom_phyFuns` table | chip contracts | Every target is an explicit ESP32-S31 rev0 ROM address and the chip pack selects that ROM revision; no archive symbol participates. |
| `none` and `esp32s31-phy-cold` entry states | chip contracts | They refer only to absence of mutable registration or the cold ROM table. |
| `ets_printf(format, ...)` diagnostic boundary | chip contracts | The reviewed rev0 ROM boundary retains only its stable format-pointer ABI; variadic payload is deliberately excluded. |
| `rtc_clk_xtal_freq_get() -> 40` semantic | chip knowledge | The chip pack establishes the fixed ESP32-S31 40 MHz crystal contract; it is independent of a vendor library body. |
| Generic C and ESP-IDF runtime interpretation | generic executable add-ons, composed by chip models | They contain no ESP32-S31 address or private blob identity; they are executable adapters, not facts. |

## Investigation overlay retained fail-closed

| Module/fact | Guard or dependency | Why it is not promoted |
|---|---|---|
| `pp_post` semantic | exact `pp.o` body plus complete relocation schema | It describes one private libpp archive member. |
| `phy_get_i2c_hostid_new` | exact linked address and body | The function is supplied by the project `archive` source, not ROM. |
| `phy_get_i2c_hostid_`, `phy_chip_i2c_readReg_org`, `phy_chip_i2c_writeReg` | exact body and absolute address | They look ROM-specific, but no separate reviewed applicability record currently proves that these body summaries are valid for every artifact composition using the chip pack. |
| RFPLL, frequency-offset and IQ-estimator reconstructions | exact linked name/address/size/body SHA-256, no archive member or relocations | Temporary executable models are bound to the reviewed ROM bytes; their absolute call sites and environment dependencies keep them project-owned. |
| `__divdi3` intrinsic | exact linked name/address/size/body SHA-256, no archive member or relocations | Body applicability is authenticated; independent chip-wide semantic applicability remains unproven. |
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
identity includes the base/overlay fact provider IDs and revisions and their
separately registered executable model IDs/revisions. A missing, mismatched or
contract-dropping overlay fails before analysis; manifest ordering is never an
override mechanism.

## Body applicability and caller facts

The five ROM reconstruction bindings use symbol-body digests extracted by
`artifact::load_code_symbol_exact` from the complete ROM whose SHA-256 is
`a52ad7513deb656a910a5740125f1cce2c7941f11ce57213b7b43aea93d5ab87`,
as recorded in `registers/evidence/vendor-rom.toml`. The extraction checked
that whole-image digest first. Only digest metadata is retained in source.
Unresolved objects, relocations, archive members, changed bytes or changed
symbol boundaries reject these models and fall back to structural analysis.
The optional `authenticated_rom_bindings_accept_reviewed_bodies_and_reject_every_byte_mutation`
test reproduces the binding check with a caller-owned `BLOBRAY_REVIEWED_ROM`
path; ordinary tests use synthetic bytes and need no vendor input.

These executable reconstructions remain temporary models, not hardware facts
or generated analysis. A body digest does not prove its caller preconditions
or the implementation of another function it calls. In particular, the former
DTM argument-zero channel range `0..=39` is no longer supplied by this overlay:
the summary hook receives neither authenticated caller evidence nor an entry
contract proving that bound. The generic analyzer must preserve unknown input
until such evidence can be carried and validated explicitly.

Provider extension validation also preserves every complete compressed-pointer
encoding fact, including its reconstruction parameters. Retaining only a
fact ID while changing the encoding is a downgrade.
