"""Write the ESP32-C5 register project files (IEEE802154_MAC first stage)."""
import json
import os
import re

ROOT = "registers/esp32c5"
IDF = "4d59230ddff16327812782151ef0afef202dc6d7"
RAW = f"https://raw.githubusercontent.com/espressif/esp-idf/{IDF}/components"
SHA = {
    "struct": "cd2f8f9bc6718a285275b88a4c3785ec366d3c4bb159dfdfe50c7ed30e4bd5cc",
    "reg": "28051ca940282680697130f980126e6bfa5a804672b2ffbf6c2a1fe5b3d84269",
    "reg_base": "b71b182aedcd6b4bb4d4878f5387b5541cd4c3ff511a17ad3c2a8e46cb660233",
    "common_ll": "ba4ce294b402df311f25c4d0ce9cb33449e3eb41993aff94a25df5a66142d471",
    "ll": "c369a2418ddd61cc65d536ab288ceb0fa78970abd91106be0b2a0f4ee3ad9cd1",
    "libbtbb": "9cbaf5bce18e6190dfab3213d2ecc65e2a5dd8bbde48bf5218dbbf47cc23da11",
}
RENAME = {
    "esp-idf-7b9cc1ac-ieee802154-common-ll": "esp-idf-4d59230d-ieee802154-common-ll",
    "BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET": "C5_BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET",
}


def w(path, text):
    path = os.path.join(ROOT, path)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    open(path, "w").write(text)


def q(s):
    return json.dumps(s, ensure_ascii=False)


def scalar(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if isinstance(v, int):
        return str(v) if v < 256 else f"0x{v:x}"
    if isinstance(v, str):
        return q(v)
    if isinstance(v, list) and all(not isinstance(x, dict) for x in v):
        return "[" + ", ".join(scalar(x) for x in v) + "]"
    raise TypeError(v)


def table(name, d):
    out = [f"\n[[{name}]]"]
    subs = []
    for k, v in d.items():
        if isinstance(v, list) and v and isinstance(v[0], dict):
            subs.append((k, v))
        else:
            out.append(f"{k} = {scalar(v)}")
    for k, items in subs:
        for item in items:
            out.append(table(f"{name}.{k}", item).rstrip("\n"))
    return "\n".join(out) + "\n"


def fix(obj):
    if isinstance(obj, dict):
        return {k: fix(v) for k, v in obj.items()}
    if isinstance(obj, list):
        return [fix(v) for v in obj]
    if isinstance(obj, str):
        obj = RENAME.get(obj, obj)
        return obj.replace("ESP32-S31", "ESP32-C5")
    return obj


# --- policy/api.toml -------------------------------------------------------
api = fix(json.load(open("target/c5-research/api-802154.json")))
api["ownership-partitions"] = [{
    "name": "Ieee802154Peripherals",
    "member": "ieee802154",
    "description": "IEEE 802.15.4 MAC registers owned by the IEEE 802.15.4 hardware lifecycle.",
    "peripherals": ["IEEE802154_MAC"],
}]
delays = {
    "Ieee802154TxOnDelay": ("Delay40", 40, "Literal ten-bit TXON_DELAY image written by ieee802154_txon_delay_set."),
    "Ieee802154TxOffDelay": ("Delay0", 0, "Six-bit TXOFF_DELAY image cleared to zero by ieee802154_txon_delay_set."),
    "Ieee802154RxOnDelay": ("Delay50", 50, "Literal eleven-bit RXON_DELAY image written by ieee802154_txon_delay_set."),
    "Ieee802154TxRxSwitchDelay": ("Delay122", 122, "Literal ten-bit TXRX_SWITCH_DELAY image written by ieee802154_txon_delay_set."),
}
for d in api["enum-domains"]:
    name, value, desc = delays[d["name"]]
    d["values"] = [{"name": name, "value": value, "description": desc,
                    "sources": ["C5_BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET"]}]
for d in api["bounded-domains"]:
    if d["name"] == "Ieee802154TxPowerCode":
        d["description"] = d["description"].replace("eight-bit", "five-bit")
        d["max"] = 31
for m in api["masked-register-modifies"]:
    if m["name"] == "set_ieee802154_tx_power_code":
        m["preserve-mask"] = 0xFFFF_FFE0
        m["input-mask"] = 0x1F
order = ["ownership-partitions", "w1c-register-snapshots", "enum-domains", "bounded-domains",
         "opaque-domains", "zero-based-field-writes", "full-register-writes",
         "fixed-register-images", "full-register-reads", "masked-register-modifies"]
assert set(order) == set(api), set(api) ^ set(order)
text = ("# Reviewed ESP32-C5 closed-PAC domains and safe transactions; no discovery facts or hardware metadata.\n"
        "schema = 5\n\n[options]\ndevice-access = true\nallow-clippy-empty-docs = true\n")
seen = set()
for k in order:
    for item in api[k]:
        key = (k, item.get("name"))
        if key in seen:
            continue
        seen.add(key)
        text += table(k, item)
assert "S31" not in text and "7b9cc1ac" not in text.lower(), [l for l in text.splitlines() if "S31" in l or "7b9cc1ac" in l.lower()]
w("policy/api.toml", text)

# --- policy/ownership.toml, lints.toml ---------------------------------------
w("policy/ownership.toml", '# Reviewed MMIO publication scope shared by explicit project compositions.\nschema = 1\nowned-ranges = [\n    "modem-ieee802154",\n]\n')
w("policy/lints.toml", open("registers/esp32s31/policy/lints.toml").read())

# --- model ------------------------------------------------------------------
w("model/memory.toml", """schema = 1
default-address-space = "cpu"

[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"

[[regions]]
name = "modem-ieee802154"
address-space = "cpu"
kind = "mmio"
start = 0x600A3000
end-exclusive = 0x600A4000
permissions = "rw"
""")
w("model/device.toml", """schema = 3
chip = "esp32c5"
address-space = "cpu"
fragments = [
    "peripherals/ieee802154-mac.toml",
]

[device]
name = "ESP32C5_RADIO"
version = "0.1"
description = \"\"\"
ESP32-C5 modem/radio registers owned by the radio drivers. This first stage
    publishes the IEEE 802.15.4 MAC aperture only. Unknown fields are omitted.\"\"\"
vendor = "Espressif"
vendor-id = "ESP"
series = "ESP32-C5"
license-text = "MIT OR Apache-2.0"
address-unit-bits = 8
width = 32
svd-schema = "1.3"
svd-schema-location = "CMSIS-SVD.xsd"
""")
s31_device = open("registers/esp32s31/model/device.toml").read()
defaults = s31_device[s31_device.index("[device.register-defaults]"):]
w("model/device.toml", open(os.path.join(ROOT, "model/device.toml")).read() + "\n" + defaults)

w("model/reviewed.toml", f"""# Project-wide sparse reviewed facts for the ESP32-C5 radio. This file grows
# only when a reviewer accepts new investigation-specific meaning.
schema = 2
id = "esp32c5-radio-project-facts"

[classification]
provenance = "reviewed"
accuracy = "exact"
completeness = "partial"

[applies-to]
ecosystems = ["esp-idf"]
chips = ["esp32c5"]
chip-revisions = ["rev100"]
artifact-lineages = ["esp32c5-radio"]

[[assertions]]
id = "ieee802154.event-status.identity"
subject = "register:esp32c5/cpu/0x600a3064/32"
kind = "register-identity"
value = "IEEE802154_MAC.EVENT_STATUS"

[[assertions.evidence]]
source = "esp-idf-4d59230d-ieee802154-common-ll"
locator = "ieee802154_ll_get_events"

[[assertions]]
id = "ieee802154.event-status.access"
subject = "register:esp32c5/cpu/0x600a3064/32"
kind = "register-access"
value = "read-write"

[[assertions.evidence]]
source = "esp-idf-4d59230d-ieee802154-common-ll"
locator = "ieee802154_ll_get_events and ieee802154_ll_clear_events"

[[assertions]]
id = "ieee802154.event-status.write-semantics"
subject = "register:esp32c5/cpu/0x600a3064/32"
kind = "hardware-write-semantics"
value = "w1c"
note = "The pinned public LL reads the thirteen-bit event image and clears a requested mask by writing back the intersection of current status and that mask; its ISR samples the complete image and immediately clears that same image. This is the source-level W1C contract ported by the generated affine snapshot transaction. No ESP32-C5 hardware observation is recorded yet."

[[assertions.evidence]]
source = "esp-idf-4d59230d-ieee802154-common-ll"
locator = "ieee802154_ll_clear_events"
""")

# --- evidence ---------------------------------------------------------------
w("evidence/policy.toml", open("registers/esp32s31/evidence/policy.toml").read())
w("evidence/platform.toml", f"""# Reviewed register evidence: platform.
schema = 1

[[sources]]
id = "C5_REG_BASE"
description = "Official ESP-IDF commit {IDF}, {RAW}/soc/esp32c5/register/soc/reg_base.h, sha256 {SHA['reg_base']}. It names DR_REG_MODEM0_BASE as 0x600A0000 and IEEE802154_REG_BASE as 0x600A3000."

[[sources]]
id = "ESP_IDF_4D59230D_C5_IEEE802154_STRUCT"
description = "Official ESP-IDF commit {IDF}, {RAW}/soc/esp32c5/register/soc/ieee802154_struct.h, sha256 {SHA['struct']}. The public volatile esp_ieee802154_t declaration fixes register and field geometry through offset 0x184, but its TODO ZB-93 says the file still awaits generation from IEEE802154.csv; it therefore supports a selective partial model. Against the ESP32-S31 struct at the same commit it declares a seven-bit frequency code, a five-bit power code, thirteen events, four-bit TX/RX and ACK PTI fields at bits 3:0 and 7:4, no receive current-channel index and no upper-half diagnostic counters."

[[sources]]
id = "ESP_IDF_4D59230D_C5_IEEE802154_REG"
description = "Official ESP-IDF commit {IDF}, {RAW}/soc/esp32c5/register/soc/ieee802154_reg.h, sha256 {SHA['reg']}. The public register macros independently restate the selected offsets and masks with the same TODO ZB-93; they are corroboration for selected geometry only."

[[sources]]
id = "esp-idf-4d59230d-ieee802154-common-ll"
description = "Official ESP-IDF commit {IDF}, {RAW}/esp_hal_ieee802154/include/hal/ieee802154_common_ll.h, sha256 {SHA['common_ll']}. Public inline accessors prove the selected software read/write directions, command/event values and field use; they do not establish reset values or semantics for untouched fields. The ESP32-C5 extension {RAW}/esp_hal_ieee802154/esp32c5/include/hal/ieee802154_ll.h, sha256 {SHA['ll']}, adds no register accessor."
""")
w("evidence/vendor-radio-libraries.toml", f"""# Reviewed register evidence: vendor radio libraries.
schema = 1

[[sources]]
id = "C5_BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET"
description = "ESP32-C5 libbtbb.a of espressif/esp-phy-lib 20f1db053a0e6cb9f1c09d255c43bf42483041d0, sha256 {SHA['libbtbb']}; complete bt_bb_v2.o ieee802154_txon_delay_set body, size 0x42. It performs four ordered fresh-read RMW operations on the IEEE 802.15.4 MAC block and no other access: 0x600A3104 bits 9:0 become 40, 0x600A3114 bits 9:0 become 122, 0x600A3110 bits 10:0 become 50 and 0x600A310c bits 5:0 are cleared, each preserving every other bit."
""")

# --- publication -------------------------------------------------------------
w("publication/registers.toml", """schema = 1
model = "../model/device.toml"
reviewed = ["../model/reviewed.toml"]
memory = "../model/memory.toml"
ownership = "../policy/ownership.toml"
api = "../policy/api.toml"
lints = "../policy/lints.toml"
evidence = [
    "../evidence/policy.toml",
    "../evidence/platform.toml",
    "../evidence/vendor-radio-libraries.toml",
    "../evidence/hil-open.toml",
]

[applicability]
ecosystems = ["esp-idf"]
chip = "esp32c5"
chip-revisions = ["rev100"]
artifact-lineages = ["esp32c5-radio"]

[outputs]
svd = "../published/radio.svd"
pac-raw = "../../../crates/hardware/esp32c5/pac/raw/src/lib.rs"
pac-api = "../../../crates/hardware/esp32c5/pac/src/generated.rs"
bindings = "../published/radio.bindings.toml"
crate-name = "oer_esp32c5_pac_raw"
target = "none"
edition = "2024"
""")
print("ok")

# --- stage 2: MODEM_ETM channels 0/1 and the ZB_MAC interrupt route ---------
UTIL_SHA = "e1a012d5f359e2445128977e82a304ba94c100c2994729e062e26586596df38a"
DEV_SHA = "9aaccfa2832cb89bfdfd98086a984269e621400a272b02926c4e088d16222830"
INTR_SHA = "b9c461a3a6a86e77ba2ee0f8db999dc75f098bae31aa4f8ade0fcf7a070def00"
PACS = "https://github.com/esp-rs/esp-pacs/blob/5cd68d2"
PACS_INTR_SHA = "265966e91e656261ca8dccef1115c84d26de056bd61589e27e1cb344c688293a"
PACS_MAP_SHA = "da2da18dc58eabd5f07f84974e8bd39b4b5ba44774e550344aa77d3f7e9a45bf"


def bit_fields(prefix):
    return "".join(
        f'\n[[peripherals.registers.register.fields]]\nname = "CH{c}"\ndescription = "{prefix} channel {c}."\nbitOffset = {c}\nbitWidth = 1\n'
        for c in (0, 1)
    )


def review(entity, sources, completeness):
    return (f'\n[[review]]\nentity = "{entity}"\nsources = [{", ".join(q(s) for s in sources)}]\n'
            f'provenance = "imported"\naccuracy = "exact"\ncompleteness = "{completeness}"\n')


ETM_SOURCES = ["ESP_IDF_4D59230D_C5_IEEE802154_REG", "ESP_IDF_4D59230D_IEEE802154_DRIVER_ETM"]
etm = f'''schema = 2

[[peripherals]]
name = "MODEM_ETM"
description = """
ESP32-C5 modem event-task matrix at MODEM_BASE + 0x8800. The public
IEEE 802.15.4 driver names the channel-enable, set and clear words and the
per-channel event/task pair at an eight-byte stride from 0x18, and programs
channels zero and one. Other channels and identifiers remain absent and no
reset values are claimed."""
baseAddress = 0x600A8800

[[peripherals.registers]]

[peripherals.registers.register]
name = "CHANNEL_ENABLE"
description = "Channel-enable status word. The IEEE 802.15.4 driver reads it to decide whether a channel must be disabled."
addressOffset = 0x000
size = 32
access = "read-only"
{bit_fields("Enabled state of")}
[[peripherals.registers]]

[peripherals.registers.register]
name = "CHANNEL_ENABLE_SET"
description = "Channel-enable set word. The IEEE 802.15.4 driver reads it and writes it back with the selected channel bit added; the effect of writing other bits is not reviewed."
addressOffset = 0x004
size = 32
access = "read-write"
{bit_fields("Enable request of")}
[[peripherals.registers]]

[peripherals.registers.register]
name = "CHANNEL_ENABLE_CLEAR"
description = "Channel-enable clear word. The IEEE 802.15.4 driver reads it and writes it back with the selected channel bit added, only when that channel is enabled."
addressOffset = 0x008
size = 32
access = "read-write"
{bit_fields("Disable request of")}
[[peripherals.registers]]

[peripherals.registers.register]
dim = 2
dimIncrement = 0x8
dimIndex = "0,1"
name = "CH%s_EVENT"
description = "Complete event identifier monitored by the selected channel."
addressOffset = 0x018
size = 32
access = "read-write"

[[peripherals.registers.register.fields]]
name = "ID"
description = "Event identifier written as a complete word."
bitOffset = 0
bitWidth = 32

[[peripherals.registers.register.fields.enumeratedValues]]
usage = "read-write"

[[peripherals.registers.register.fields.enumeratedValues.values]]
name = "IEEE802154_TIMER1_OVERFLOW"
value = 58

[[peripherals.registers.register.fields.enumeratedValues.values]]
name = "IEEE802154_TIMER0_OVERFLOW"
value = 59

[[peripherals.registers]]

[peripherals.registers.register]
dim = 2
dimIncrement = 0x8
dimIndex = "0,1"
name = "CH%s_TASK"
description = "Complete task identifier triggered when the selected channel's event occurs."
addressOffset = 0x01c
size = 32
access = "read-write"

[[peripherals.registers.register.fields]]
name = "ID"
description = "Task identifier written as a complete word."
bitOffset = 0
bitWidth = 32

[[peripherals.registers.register.fields.enumeratedValues]]
usage = "read-write"

[[peripherals.registers.register.fields.enumeratedValues.values]]
name = "IEEE802154_ED_TRIG_TX"
value = 65

[[peripherals.registers.register.fields.enumeratedValues.values]]
name = "IEEE802154_RX_START"
value = 66

[[peripherals.registers.register.fields.enumeratedValues.values]]
name = "IEEE802154_TX_START"
value = 69
'''
etm += review("MODEM_ETM", ["C5_REG_BASE"] + ETM_SOURCES, "partial")
for reg in ["CHANNEL_ENABLE", "CHANNEL_ENABLE_SET", "CHANNEL_ENABLE_CLEAR"]:
    etm += review(f"MODEM_ETM.{reg}", ETM_SOURCES, "partial")
    for c in (0, 1):
        etm += review(f"MODEM_ETM.{reg}.CH{c}", ETM_SOURCES, "complete")
for reg in ["CH%s_EVENT", "CH%s_TASK"]:
    etm += review(f"MODEM_ETM.{reg}", ETM_SOURCES, "partial")
    etm += review(f"MODEM_ETM.{reg}.ID", ETM_SOURCES, "partial")
w("model/peripherals/modem-etm.toml", etm)

route_fields = [("MAP", 0, 6, "CPU-interrupt destination selected for the peripheral source."),
                ("UNCLASSIFIED_6_7", 6, 2, None),
                ("PASS_IN_SEC", 8, 1, "Secure-world pass-through selection for the peripheral source."),
                ("UNCLASSIFIED_9_31", 9, 23, None)]
route = '''schema = 2

[[peripherals]]
name = "IEEE802154_INTERRUPT_ROUTE"
description = """
The CPU interrupt-matrix word for ESP32-C5 modem source 12 (ZB_MAC). This
source-specific view is retained with the IEEE 802.15.4 owner and publishes
only the generated field readers needed to prove that a polled or validation
transaction starts with the CPU route detached."""
baseAddress = 0x60010030

[[peripherals.registers]]

[peripherals.registers.register]
name = "CORE0_ROUTE"
description = "Destination and security routing for ZB_MAC."
addressOffset = 0x000
size = 32
access = "read-write"
resetValue = 0
'''
for name, off, width, desc in route_fields:
    desc = desc or "Unassigned bits retained solely so reset-state observation cannot erase an unexpected nonzero value."
    route += f'\n[[peripherals.registers.register.fields]]\nname = "{name}"\ndescription = "{desc}"\nbitOffset = {off}\nbitWidth = {width}\n'
route += review("IEEE802154_INTERRUPT_ROUTE", ["ESP_PACS_5CD68D2_C5_INTERRUPT_MATRIX", "ESP_IDF_4D59230D_C5_INTERRUPT_SOURCES"], "partial")
route += review("IEEE802154_INTERRUPT_ROUTE.CORE0_ROUTE", ["ESP_PACS_5CD68D2_C5_INTERRUPT_MATRIX"], "partial")
for name in ("MAP", "PASS_IN_SEC"):
    route += review(f"IEEE802154_INTERRUPT_ROUTE.CORE0_ROUTE.{name}", ["ESP_PACS_5CD68D2_C5_INTERRUPT_MATRIX"], "complete")
w("model/peripherals/ieee802154-interrupt-route.toml", route)

device = open(os.path.join(ROOT, "model/device.toml")).read()
device = device.replace('    "peripherals/ieee802154-mac.toml",\n',
                        '    "peripherals/ieee802154-mac.toml",\n    "peripherals/ieee802154-interrupt-route.toml",\n    "peripherals/modem-etm.toml",\n')
device = device.replace("publishes the IEEE 802.15.4 MAC aperture only.",
                        "publishes the IEEE 802.15.4 MAC aperture, its interrupt route and\n    the modem ETM channels it programs.")
w("model/device.toml", device)

memory = open(os.path.join(ROOT, "model/memory.toml")).read()
memory += """
[[regions]]
name = "modem-etm-ieee802154"
address-space = "cpu"
kind = "mmio"
start = 0x600A8800
end-exclusive = 0x600A8828
permissions = "rw"

[[regions]]
name = "ieee802154-core0-route"
address-space = "cpu"
kind = "mmio"
start = 0x60010030
end-exclusive = 0x60010034
permissions = "rw"
"""
w("model/memory.toml", memory)
w("policy/ownership.toml", '# Reviewed MMIO publication scope shared by explicit project compositions.\nschema = 1\nowned-ranges = [\n    "modem-ieee802154",\n    "modem-etm-ieee802154",\n    "ieee802154-core0-route",\n]\n')

platform = open(os.path.join(ROOT, "evidence/platform.toml")).read()
platform += f'''
[[sources]]
id = "ESP_IDF_4D59230D_IEEE802154_DRIVER_ETM"
description = "Official ESP-IDF commit {IDF}: components/ieee802154/driver/esp_ieee802154_util.c sha256 {UTIL_SHA} and components/ieee802154/driver/esp_ieee802154_dev.c sha256 {DEV_SHA}. ieee802154_etm_channel_clear reads the modem ETM channel-enable word and, only when the channel bit is set, reads the clear word and writes it back with that bit added; ieee802154_etm_set_event_task clears the channel, writes the complete event and task words at the channel stride and reads the set word and writes it back with the channel bit added. The driver programs channel zero from TIMER0 overflow to TX_START or ED_TRIG_TX and channel one from TIMER1 overflow to RX_START. The ESP32-C5 ieee802154_reg.h ({RAW}/soc/esp32c5/register/soc/ieee802154_reg.h) places ETM_REG_BASE at 0x600A8800, the set and clear words at +0x4 and +0x8, CH0 event and task at +0x18 and +0x1c with an eight-byte stride, and names events TIMER1_OVERFLOW 58 and TIMER0_OVERFLOW 59 and tasks ED_TRIG_TX 65, RX_START 66 and TX_START 69."

[[sources]]
id = "ESP_IDF_4D59230D_C5_INTERRUPT_SOURCES"
description = "Official ESP-IDF commit {IDF}, components/soc/esp32c5/include/soc/interrupts.h, sha256 {INTR_SHA}. The hardware source table assigns ETS_ZB_MAC_INTR_SOURCE to 12."

[[sources]]
id = "ESP_PACS_5CD68D2_C5_INTERRUPT_MATRIX"
description = "Official esp-rs/esp-pacs commit 5cd68d2, {PACS}/esp32c5/src/lib.rs publishes INTERRUPT_CORE0 at 0x60010000; esp32c5/src/interrupt_core0.rs sha256 {PACS_INTR_SHA} places 84 source-indexed CORE_0_INTR_MAP words at four-byte stride, so source 12 resolves to offset 0x30; interrupt_core0/core_0_intr_map.rs sha256 {PACS_MAP_SHA} exposes MAP at bits 5:0 and PASS_IN_SEC at bit 8 with a zero reset value."
'''
w("evidence/platform.toml", platform)

# ETM and route transactions.
api_text = open(os.path.join(ROOT, "policy/api.toml")).read()
api_text = api_text.replace(
    '"description": "IEEE 802.15.4 MAC registers owned by the IEEE 802.15.4 hardware lifecycle."',
    '"description": "IEEE 802.15.4 MAC registers owned by the IEEE 802.15.4 hardware lifecycle."')
api_text = api_text.replace('peripherals = ["IEEE802154_MAC"]',
                            'peripherals = ["IEEE802154_MAC", "IEEE802154_INTERRUPT_ROUTE"]')
api_text = api_text.replace('description = "IEEE 802.15.4 MAC registers owned by the IEEE 802.15.4 hardware lifecycle."',
                            'description = "IEEE 802.15.4 MAC and source-specific interrupt-route registers owned by the IEEE 802.15.4 hardware lifecycle."')
api_text = api_text.replace("\n[[w1c-register-snapshots]]", '''
[[ownership-partitions]]
name = "ModemEtmPeripherals"
member = "modem_etm"
description = "Modem event-task matrix channels zero and one, programmed by the IEEE 802.15.4 driver."
peripherals = ["MODEM_ETM"]

[[w1c-register-snapshots]]''', 1)
etm_src = ["ESP_IDF_4D59230D_IEEE802154_DRIVER_ETM"]
extra = ""
for verb, reg in (("enable", "CHANNEL_ENABLE_SET"), ("disable", "CHANNEL_ENABLE_CLEAR")):
    for c in (0, 1):
        extra += table("field-or-modifies", {
            "name": f"{verb}_ieee802154_etm_channel{c}", "peripheral": "MODEM_ETM", "register": reg,
            "fields": [{"field": f"CH{c}", "source-bit-offset": c}], "value": 1 << c,
            "exposure": "facade", "sources": etm_src})
for name, ch, ev, task in (("route_ieee802154_etm_timer0_to_tx_start", 0, 59, 69),
                           ("route_ieee802154_etm_timer0_to_ed_trig_tx", 0, 59, 65),
                           ("route_ieee802154_etm_timer1_to_rx_start", 1, 58, 66)):
    extra += table("fixed-register-sequences", {
        "name": name, "peripheral": "MODEM_ETM",
        "steps": [{"register": "CH%s_EVENT", "index": ch, "value": ev},
                  {"register": "CH%s_TASK", "index": ch, "value": task}],
        "sources": etm_src})
extra += table("field-snapshot-reads", {
    "name": "observe_ieee802154_core0_route", "peripheral": "IEEE802154_INTERRUPT_ROUTE",
    "register": "CORE0_ROUTE", "fields": ["MAP", "UNCLASSIFIED_6_7", "PASS_IN_SEC", "UNCLASSIFIED_9_31"],
    "sources": ["ESP_PACS_5CD68D2_C5_INTERRUPT_MATRIX"]})
w("policy/api.toml", api_text + extra)
print("stage 2 ok")

w("evidence/hil-open.toml", """# Reviewed register evidence: hil open.
schema = 1

[[sources]]
id = "HIL_OPEN_C5_REGISTER_PROBE_2026_09_27"
description = "ESP32-C5 v1.0 board (stand board esp32c5, MAC 38:44:BE:AA:25:64) on 2026-09-27, verification/esp32c5/hardware/register-probe at repository commit 8afaaec7c, flashed and captured by `cargo hil flash` at cdc277aa5; console log sha256 c8a6e59e38c83e80d4faf56a0171e92861b43a39d77fcde88718dfaa4547e212. After opening the IEEE 802.15.4 MAC clocks, a read-modify-write of all ones read back CHANNEL 0x000000ff, TX_POWER 0x0000001f, EVENT_ENABLE 0x00001fff and COEX_PTI 0x000001ff. This establishes which bits of those words are implemented and read-write; it assigns no meaning to CHANNEL bit 7, which the public struct declares reserved."
""")
