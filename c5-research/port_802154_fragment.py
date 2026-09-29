"""Derive the ESP32-C5 IEEE802154_MAC fragment from the ESP32-S31 one.

Every edit below corresponds to a difference between the ESP32-C5 and
ESP32-S31 ieee802154_struct.h / ieee802154_ll.h at ESP-IDF 4d59230d, or to
the ESP32-C5 libbtbb ieee802154_txon_delay_set. The result is checked
against the ESP32-C5 struct by structcheck.py.
"""
import re
import sys

src, dst = sys.argv[1], sys.argv[2]
text = open(src).read()

# Split into table chunks: each starts at a header line.
chunks, cur = [], []
for line in text.splitlines(keepends=True):
    if re.match(r"^\[", line) and cur:
        chunks.append(cur)
        cur = []
    cur.append(line)
chunks.append(cur)


def header(c):
    for line in c:
        if line.startswith("["):
            return line.strip()
    return ""


def value(c, key):
    for line in c:
        m = re.match(rf'^{key} = "(.*)"', line)
        if m:
            return m.group(1)
    return None


REMOVED_FIELDS = {
    ("EVENT_ENABLE", "UNCLASSIFIED_13"),
    ("WORD_064_BASE", "UNCLASSIFIED_13"),
    ("RX_STATUS", "CURRENT_CHANNEL_INDEX"),
    ("SFD_TIMEOUT_COUNTER", "RX_FILTER_NOT_WORK_COUNT"),
    ("CRC_ERROR_COUNTER", "RX_PREAMBLE_DETECT_ERROR_COUNT"),
    ("DIAGNOSTIC_COUNTER_CLEAR", "RX_FILTER_NOT_WORK"),
    ("DIAGNOSTIC_COUNTER_CLEAR", "RX_PREAMBLE_DETECT_ERROR"),
}
removed_entities = {f"IEEE802154_MAC.{r}.{f}" for r, f in REMOVED_FIELDS}

out, register, dropping, removed = [], None, False, set()
for c in chunks:
    h = header(c)
    if h == "[peripherals.registers.register]":
        register = value(c, "name")
    if h == "[[peripherals.registers.register.fields]]":
        key = (register, value(c, "name"))
        dropping = key in REMOVED_FIELDS
        if dropping:
            removed.add(key)
            # Keep any comment/blank lines that precede nothing: drop the chunk.
            continue
    elif h.startswith("[peripherals.registers.register.fields."):
        if dropping:
            continue
    else:
        dropping = False
    if h == "[[review]]" and value(c, "entity") in removed_entities:
        continue
    out.append("".join(c))
assert removed == REMOVED_FIELDS, REMOVED_FIELDS - removed
text = "".join(out)


def sub(pattern, repl, count=1, flags=0):
    global text
    new, n = re.subn(pattern, repl, text, count=count, flags=flags)
    assert n == (count if count else n) and n > 0, pattern
    text = new


def field(register_name, field_name):
    """Regex span of one field chunk (and its subtables) inside a register."""
    return (
        rf'(name = "{register_name}"\n(?:(?!\[peripherals\.registers\.register\]).)*?'
        rf'name = "{field_name}"\n(?:(?!\[\[).)*?)'
    )


# Header: aperture, base address, source identities.
sub(r"Dedicated ESP32-S31 IEEE 802\.15\.4 MAC aperture at 0x20103000\.",
    "Dedicated ESP32-C5 IEEE 802.15.4 MAC aperture at 0x600A3000.")
sub(r"baseAddress = 0x20103000", "baseAddress = 0x600A3000")
for old, new in [
    ("ESP_IDF_7B9CC1AC_S31_IEEE802154_STRUCT", "ESP_IDF_4D59230D_C5_IEEE802154_STRUCT"),
    ("ESP_IDF_7B9CC1AC_S31_IEEE802154_REG", "ESP_IDF_4D59230D_C5_IEEE802154_REG"),
    ("esp-idf-7b9cc1ac-ieee802154-common-ll", "esp-idf-4d59230d-ieee802154-common-ll"),
    ("BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET", "C5_BLOB_LIBBTBB_IEEE802154_TXON_DELAY_SET"),
    ('"S31_REG_BASE"', '"C5_REG_BASE"'),
]:
    sub(re.escape(old), new, count=0)
# The ESP32-C5 LL adds no accessor, and the esp-pacs RXON cross-check is S31's.
for dropped in ["ESP_IDF_7B9CC1AC_S31_IEEE802154_LL", "ESP_PACS_AAA5C2EA_S31_IEEE802154_RXON_DELAY"]:
    text = re.sub(rf'\n\s*"{dropped}",', "", text)
    text = re.sub(rf', "{dropped}"', "", text)
sub(r"ESP32-S31", "ESP32-C5", count=0)

# CHANNEL.freq is seven bits.
sub(r"Eight-bit code read and written by ieee802154_ll_get_freq/set_freq",
    "Seven-bit code read and written by ieee802154_ll_get_freq/set_freq")
sub(field("CHANNEL", "FREQUENCY_CODE") + r"bitWidth = 8(\n(?:(?!\[\[).)*?)maximum = 0xff\n",
    r"\1bitWidth = 7\2maximum = 0x7f\n", flags=re.S)
# TX_POWER.power is five bits.
sub(r"Eight-bit transmit-power code", "Five-bit transmit-power code")
sub(r"Raw eight-bit hardware code", "Raw five-bit hardware code")
sub(field("TX_POWER", "POWER_CODE") + r"bitWidth = 8", r"\1bitWidth = 5", flags=re.S)
# Thirteen events: no bit 13.
sub(r"bits seven and thirteen have no assigned vendor event semantics",
    "bit seven has no assigned vendor event semantics")
# COEX_PTI: pti 3:0, hw_ack_pti 7:4.
sub(field("COEX_PTI", "TXRX_PTI") + r"bitWidth = 5(\n(?:(?!\[\[).)*?)maximum = 0x1f\n",
    r"\1bitWidth = 4\2maximum = 0xf\n", flags=re.S)
sub(field("COEX_PTI", "ACK_PTI") + r"bitOffset = 5\nbitWidth = 5(\n(?:(?!\[\[).)*?)maximum = 0x1f\n",
    r"\1bitOffset = 4\nbitWidth = 4\2maximum = 0xf\n", flags=re.S)
# 0x144/0x148 carry one counter each.
sub(r"Two read-only diagnostic counters independently read by the common and ESP32-C5 LL headers\.",
    "Read-only diagnostic counter read by the common LL; the upper half is reserved.", count=2)
# ieee802154_txon_delay_set of the ESP32-C5 libbtbb: TXON 40, TXOFF cleared, RXON 50, TXRX 122.
sub(r"The literal value 45 is reviewed", "The literal value 40 is reviewed")
sub(r"written with literal 45 by", "written with literal 40 by")
sub(r"The literal value 5 is reviewed", "The literal value 0 is reviewed")
sub(r"written with literal 5 by", "cleared to 0 by")
sub(r"The literal value 117 is reviewed", "The literal value 122 is reviewed")
sub(r"written with literal 117 by", "written with literal 122 by")

# Board observation: CHANNEL bit 7 is implemented read-write although the
# struct declares a seven-bit freq field (register probe, 2026-09-27).
sub(r'(name = "FREQUENCY_CODE"\n(?:(?!\[\[).)*?maximum = 0x7f\n)',
    r'\1\n[[peripherals.registers.register.fields]]\nname = "UNCLASSIFIED_7"\n'
    r'description = "Implemented read-write bit above the seven-bit frequency code; the public struct declares it reserved and no vendor accessor writes it."\n'
    r'bitOffset = 7\nbitWidth = 1\n', flags=re.S)
sub(r'(\[\[review\]\]\nentity = "IEEE802154_MAC.CHANNEL"\n(?:(?!\[\[).)*?completeness = "partial"\n)',
    r'\1\n[[review]]\nentity = "IEEE802154_MAC.CHANNEL.UNCLASSIFIED_7"\n'
    r'sources = ["HIL_OPEN_C5_REGISTER_PROBE_2026_09_27"]\nprovenance = "observed"\naccuracy = "exact"\ncompleteness = "complete"\n', flags=re.S)

assert "S31" not in text and "0x2010" not in text and "7B9CC1AC" not in text.upper(), [
    l for l in text.splitlines() if "S31" in l or "0x2010" in l or "7B9CC1AC" in l.upper()
]
open(dst, "w").write(text)
