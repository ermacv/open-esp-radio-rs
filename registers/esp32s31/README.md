# ESP32-S31 radio register ownership

The editable hardware source is the schema-3
[`model/device.toml`](model/device.toml) and its schema-2 peripheral fragments.
The reviewed assertion overlay in [`model/reviewed.toml`](model/reviewed.toml)
retains explicit applicability and evidence for the IEEE 802.15.4 event-status
identity, access and W1C semantics. Both production generation and vendor
analysis read this model; neither owns a second copy.

| Path | Owner and purpose |
| --- | --- |
| `model/device.toml`, `model/peripherals/` | Reviewed hardware register geometry and semantics |
| `model/memory.toml` | MMIO regions, including platform-owned regions outside radio publication |
| `model/reviewed.toml` | Typed reviewed assertions with their applicability and evidence |
| `policy/api.toml` | Schema-5 production PAC ownership partitions and typed transactions |
| `policy/ownership.toml` | Schema-1 shared publication scope of named MMIO ranges |
| `policy/lints.toml` | Reviewed register-model lint policy shared by publication and the investigation |
| `evidence/` | Source identities, provenance, reviewed confidence and supporting records |
| `upstream/platform-radio-deps.svd` | Reviewed upstream platform PAC input for analysis |
| `published/radio.svd` | Generated portable CMSIS-SVD representation |
| `published/radio.bindings.toml` | Generated binding index |
| `publication/registers.toml` | Source-only publication composition |

The generic generator is
[`tools/registers/model`](../../tools/registers/model/README.md).
Its other two checked outputs are
[`pac/raw/src/lib.rs`](../../crates/hardware/esp32s31/pac/raw/src/lib.rs) and
[`pac/src/generated.rs`](../../crates/hardware/esp32s31/pac/src/generated.rs).
Handwritten runtime ownership and safe hardware access remain in the
[closed PAC](../../crates/hardware/esp32s31/pac/README.md), not in publication tooling.

## Publication and investigation are separate compositions

The [source-only project](publication/README.md) selects the reusable chip
provider, model, reviewed assertions, ownership policy and PAC API. It requires
no private artifacts. It explicitly selects the shared lint pack and source evidence catalogs,
without selecting the investigation's executable reconstructions or authenticating
private vendor artifacts.

The [vendor investigation](../../verification/esp32s31/README.md)
selects those additional inputs explicitly and authenticates artifact-specific
facts in its caller-provided run context. Sharing a publication scope does not
inherit that context or promote a model-only check into comparison evidence.
The common scope is selected through `[registers].ownership-policy`. Its schema-1
pack contains only `owned-ranges`; combining it with an inline `owned-ranges`
list is rejected. A standalone project may instead select an inline scope.
Neither spelling has merge or override precedence.

From the repository root:

```console
cargo registers validate --manifest registers/esp32s31/publication/registers.toml
cargo registers generate --check --manifest registers/esp32s31/publication/registers.toml
```

Omit `--check` only when publishing an intentionally reviewed source change.
Do not edit generated outputs directly. These leaf commands preserve reviewed
model/API validation and independently check each configured output.
`project publish` remains the investigation workflow requiring structural review
scopes; an absent review is not silently replaced by model-only publication.

## Words shared with ESP-HAL

Several owned ranges are single fields of 32-bit words that ESP-HAL also
writes through the upstream `esp-pacs` chain. The two chains do not share a
lock, so a word both write at runtime would lose updates. A shared bit
therefore has one runtime owner.

| Word | ESP-HAL writes | Radio writes | Why it is safe |
| --- | --- | --- | --- |
| `HP_SYS_CLKRST.REF_160M_CTRL0` | Clock-tree divider and gate | Nothing; it requests `PlatformClock::Pll160m` | Not published here |
| `MODEM_LPCON.CLK_CONF` | Every gate, reference-counted under one lock: analog-I2C master, coexistence, low-power timer; the Wi-Fi power gate in `esp_hal::init` | Nothing; it requests `AnalogI2cMaster`, `ModemCoexistence` and `ModemLowPowerTimer` | Not published here |
| `PMU.HP_ACTIVE_HP_CK_POWER` | MPLL power when the MPLL reference count crosses zero | Front-end baseband power | The adopted PSRAM mapping holds a permanent MPLL reference through ESP-HAL's clock tree (`Psram::from_existing_mapping`), so the count never crosses zero; the radio also holds a `PlatformClock::Mpll` reference while powered |
| `PMU.IMM_MODEM_ICG`, `PMU.IMM_SLEEP_SYSCLK` | Trigger-bit writes in `esp_hal::init` | Trigger-bit writes | Both sides write whole words; no read-modify-write |

ESP-HAL writes the following owned words only inside `esp_hal::init`, which
completes before any radio route or the radio arbiter exists. Their later
radio read-modify-writes therefore have no concurrent ESP-HAL writer:

- `MODEM_SYSCON.CLK_CONF_POWER_ST` and `MODEM_LPCON.CLK_CONF_POWER_ST`, whose
  modem clock state maps ESP-HAL ORs in;
- `MODEM_LPCON.WIFI_LP_CLK_CONF` and `MODEM_32K_CLK_CONF`, set by its Wi-Fi
  low-power clock selection;
- `PMU.HP_ACTIVE_ICG_MODEM` and `PMU.ANA_PERI_PWR_CTRL`;
- `LP_AON_CLKRST.ROOT_CLK_CONF`, whose slow and fast clock selectors the
  radio only reads.

An image must not start a radio before `esp_hal::init` returns, and a change
to ESP-HAL that writes one of these words at runtime reopens the race.

## Upstream and evidence boundaries

`upstream/platform-radio-deps.svd` describes official-PAC registers reached by
vendor radio code. It is pinned to the workspace's `esp-pacs` revision and is
not an input to the radio PAC generator. It creates no runtime peripheral owner.
The reviewed common-PHY `TICK_CONF` carveout is in the radio model; adjacent
platform-owned `MODEM_LPCON` registers remain in the upstream analysis catalog.
See the [PAC provenance map](../../crates/hardware/esp32s31/pac/README.md).

Source provenance catalogs retain their exact source IDs, revisions and hashes.
Reserved fields remain absent. A register or field whose meaning is not
established ends in `_OPAQUE` and is reviewed with `naming = "opaque"`; see
[name origin](../../tools/registers/model/README.md). Neighboring-chip
similarity is not sufficient evidence for an ESP32-S31 address or bit. Reviewed
source assertions, vendor comparison and dated hardware observations retain
their distinct strength. Publication verifies consistency and reproducibility;
[qualification](../../qualification/README.md) determines readiness.
