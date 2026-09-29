"""Derive ESP32-C5 Wi-Fi MAC model fragments from reviewed ESP32-S31 fragments.

Addresses follow the cross-chip map in diff/wifi-mac-c5-map.json. Every S31
source a fragment's reviews cite becomes one C5 evidence entry that names the
C5 archive member, size and cross-chip verdict of each function the S31
source describes. Fails closed: an untranslatable address, a non-uniform
array stride or a register without a C5 address aborts the fragment.
"""
import collections, glob, hashlib, json, os, re, subprocess, sys, tomllib

ROOT = "/home/ermacv/dev/open-esp-radio-rs-esp32c5"
RES = f"{ROOT}/target/c5-research"
TMP = os.environ.get("CLAUDE_JOB_DIR", "/tmp") + "/tmp"
WIFI = f"{ROOT}/target/vendor/esp32-wifi-lib/af55a0ca258ce9d791d1661d7c2bbb65f08c0c21/esp32c5"
ROM = f"{ROOT}/target/vendor/esp-rom-elfs/20260528/esp32c5_rev100_rom.elf"
S31_BASE, C5_BASE = 0x20100000, 0x600A0000

rows = json.load(open(f"{RES}/diff/wifi-mac-c5-map.json"))
verdicts = json.load(open(f"{RES}/diff/function-verdicts.json"))["verdict"]
by_elem = {(r["peripheral"], r["name"], r["index"]): r for r in rows}
by_s31 = {}
for r in rows:
    if r["c5"] is not None:
        by_s31[int(r["s31"], 16)] = int(r["c5"], 16)
fixed = {int(k, 16): int(max(v, key=v.get), 16)
         for k, v in json.load(open(f"{RES}/diff/modem-offset-map.json")).items()}


def bank(o):
    for i in range(8):
        b = 0x54D8 - 0x7C * i
        if b <= o < b + 0x7C:
            r = o - b
            if r <= 0x68:
                return 0x54B0 - 0x78 * i + r
            if r >= 0x74:
                return 0x54B0 - 0x78 * i + r - 4
            return None
    return None


def xlate(o):
    if o in by_s31:
        return by_s31[o]
    c = bank(o)
    if c is not None:
        return c
    return fixed.get(o)


def sha(path):
    return hashlib.sha256(open(path, "rb").read()).hexdigest()


def members(archive, cache):
    """function -> (member, size) for one archive, extracted under TMP."""
    os.makedirs(cache, exist_ok=True)
    if not os.listdir(cache):
        subprocess.run(["llvm-ar", "x", archive], cwd=cache, check=True)
    out = {}
    for o in sorted(os.listdir(cache)):
        if not o.endswith(".o"):
            continue
        t = subprocess.run(["llvm-objdump", "-t", os.path.join(cache, o)],
                           capture_output=True, text=True).stdout
        for line in t.splitlines():
            p = line.split()
            if len(p) >= 6 and " F " in line:
                out.setdefault(p[-1], (o, int(p[-2], 16), sha(os.path.join(cache, o))))
    return out


LIBS = {
    "libpp": ("libpp.a", members(f"{WIFI}/libpp.a", f"{TMP}/pp_esp32c5")),
    "libnet80211": ("libnet80211.a", members(f"{WIFI}/libnet80211.a", f"{TMP}/net_esp32c5")),
}
rom_sizes = {}
for line in subprocess.run(["llvm-nm", "-S", "--defined-only", ROM],
                           capture_output=True, text=True).stdout.splitlines():
    p = line.split()
    if len(p) == 4 and p[2] in "Tt":
        rom_sizes[p[3]] = int(p[1], 16)

IMM = re.compile(r"-?0x[0-9a-f]+|-?\b\d+\b")
sys.path.insert(0, RES)
_cwd = os.getcwd(); os.chdir(RES)
exec(open("bodycmp.py").read().split("IMM = re.compile")[0])
S31_ROM = functions(f"{ROOT}/target/vendor/esp-rom-elfs/20260528/esp32s31_rev0_rom.elf")
S31_LIB = {"libpp": functions(f"{ROOT}/target/vendor/esp32-wifi-lib/af55a0ca258ce9d791d1661d7c2bbb65f08c0c21/esp32s31/libpp.a"),
           "libnet80211": functions(f"{ROOT}/target/vendor/esp32-wifi-lib/af55a0ca258ce9d791d1661d7c2bbb65f08c0c21/esp32s31/libnet80211.a")}
os.chdir(_cwd)


def rom_matches_library(fn):
    """The library whose S31 body equals the S31 ROM body and whose C5 body is the same code."""
    rom = S31_ROM.get(fn)
    for lib, table in S31_LIB.items():
        body = table.get(fn)
        if rom and body and [IMM.sub("#", x) for x in rom] == [IMM.sub("#", x) for x in body] \
                and verdicts.get(f"{lib}:{fn}") in SAME and fn in LIBS[lib][1]:
            return lib
    return None


s31_sources = {}
for f in glob.glob(f"{ROOT}/registers/esp32s31/evidence/*.toml"):
    for s in tomllib.load(open(f, "rb")).get("sources", []):
        s31_sources[s["id"]] = s["description"]
known = {k.split(":", 1)[1] for k in verdicts}
SAME = ("identical", "same-modulo-map", "struct-layout")
WORDS = {"identical": "identical", "same-modulo-map": "identical modulo the modem address map",
         "struct-layout": "identical modulo the modem address map and software structure offsets"}


MANUAL = {
    "BLOB_LIBPP_WDEV_PROCESS_FIQ": "C5 libpp.a[wdev.o] wDev_ProcessFiq differs structurally from the ESP32-S31 body "
        "only by additional calls: it also samples and clears a separate RX-EVM interrupt "
        "(hal_mac_interrupt_get_rx_evm after pwr_hal_get_intr_raw_signal and hal_mac_interrupt_clr_rx_evm after "
        "hal_mac_interrupt_clr_bsscolor) and posts one more event before lmacProcessRxSucData. Its status "
        "dispatch masks (0x80, 0x100, 0x4000, 0x8000, 0x80000, 0x600000, 0x2000000, 0x18000000, 0xF0 and 0xF) "
        "and the order of the handlers they select (lmacPostTxComplete, lmacProcessAllTxTimeout, "
        "lmacProcessCollisions, lmacProcessRxSucData, the TSF-timer, TBTT, beacon-miss and beacon-filter "
        "handlers) equal the ESP32-S31 body, so the status bits these handlers identify are the same bits.",
    "BLOB_LIBPP_HAL_HE_SET_MMSS_AND_AID": None,
}
MANUAL_TEXT_EXTRA = {
    "BLOB_LIBPP_HAL_HE_SET_MMSS_AND_AID": " The C5 body writes, for interface i, bits 29:27 of 0x600A4004 + 8 * i "
        "with its MMSS argument (mask 0x38000000) and bits 26:16 with its AID argument (mask 0x07FF0000), each "
        "by a fresh read-modify-write preserving the other bits, as the ESP32-S31 body does at 0x20104004 + 8 * i.",
}
EXTRA_SOURCES = {
    "WIFI_MAC_BSSID_POLICY.BSSID_HIGH%s.ASSOCIATION_ID": ["BLOB_LIBPP_HAL_HE_SET_MMSS_AND_AID"],
    "WIFI_MAC_BSSID_POLICY.BSSID_HIGH%s.MINIMUM_MPDU_START_SPACING": ["BLOB_LIBPP_HAL_HE_SET_MMSS_AND_AID"],
}


def c5_source(s31_id):
    """(C5 id, description) or None when the S31 source has no C5 counterpart."""
    if MANUAL.get(s31_id):
        head = (f"ESP32-C5 libpp.a of espressif/esp32-wifi-lib af55a0ca258ce9d791d1661d7c2bbb65f08c0c21, "
                f"sha256 {sha(WIFI + '/libpp.a')}. Cross-chip derivation of the reviewed ESP32-S31 source {s31_id}: ")
        return "C5_" + s31_id, head + MANUAL[s31_id]
    if s31_id.startswith(("HIL_", "PROMOTED_")):
        return None
    desc = s31_sources[s31_id]
    fns = sorted({w for w in re.findall(r"[A-Za-z_][A-Za-z0-9_]+", desc) if w in known and len(w) > 6})
    rom = s31_id.startswith("ROM_")
    parts, excluded = [], []
    for fn in fns:
        if rom:
            v = verdicts.get("rom:" + fn)
            if v in SAME and fn in rom_sizes:
                parts.append(f"ROM {fn} (size {rom_sizes[fn]:#x}) is {WORDS[v]}")
                continue
            lib = rom_matches_library(fn)
            if lib:
                archive, table = LIBS[lib]
                member, size, msha = table[fn]
                parts.append(f"{archive}[{member}] {fn} (member sha256 {msha}, size {size:#x}), the definition "
                             f"the C5 libraries link, is {WORDS[verdicts[lib + ':' + fn]]} to the ESP32-S31 "
                             f"{archive} definition, whose body equals the reviewed S31 ROM body modulo "
                             "branch targets")
            else:
                excluded.append(fn)
            continue
        found = None
        for lib, (archive, table) in LIBS.items():
            v = verdicts.get(f"{lib}:{fn}")
            if v is not None and fn in table:
                found = (lib, archive, table[fn], v)
                break
        if found and found[3] in SAME:
            lib, archive, (member, size, msha), v = found
            parts.append(f"{archive}[{member}] {fn} (member sha256 {msha}, size {size:#x}) is {WORDS[v]}")
        else:
            excluded.append(fn)
    if not parts:
        return None
    if rom:
        cid = "C5_ROM_REV100_" + s31_id.removeprefix("ROM_REV0_")
        head = (f"ESP32-C5 rev 1.0 ROM esp32c5_rev100_rom.elf of espressif/esp-rom-elfs 20260528, "
                f"sha256 {sha(ROM)}.")
    else:
        cid = "C5_" + s31_id
        head = (f"ESP32-C5 archives of espressif/esp32-wifi-lib af55a0ca258ce9d791d1661d7c2bbb65f08c0c21 "
                f"(libpp.a sha256 {sha(WIFI + '/libpp.a')}, libnet80211.a sha256 {sha(WIFI + '/libnet80211.a')}).")
    text = (f"{head} Cross-chip derivation of the reviewed ESP32-S31 source {s31_id}: comparing full "
            f"function bodies, {'; '.join(parts)} to its ESP32-S31 counterpart. Modem addresses follow "
            "the reviewed map from 0x2010_xxxx to 0x600A_xxxx: equal offsets except the segments "
            "0x2014, 0x4CA0..0x4D3C and 0x4D4C..0x4EB8 (-4), 0x44E0..0x452C, 0x4990..0x49A0 and "
            "0x4D40 (-8), 0xD8B4..0xD8D0 (-4), and the eight reverse-stride TX queue banks, which "
            "move from 0x54D8 - 0x7C * i to 0x54B0 - 0x78 * i with bank offsets 0x74 and above "
            "lowered by 4.")
    text += MANUAL_TEXT_EXTRA.get(s31_id, "")
    if excluded:
        text += (" Functions of the S31 source whose C5 bodies differ structurally or are absent ("
                 + ", ".join(excluded) + ") are not evidence for the C5 facts.")
    return cid, text


ADDR = re.compile(r"0x2010_?([0-9A-Fa-f]{4})\b")


def rewrite_text(line, errors, where):
    def sub(m):
        o = int(m.group(1), 16)
        c = xlate(o)
        if c is None:
            errors.append(f"{where}: untranslatable address {m.group(0)}")
            return m.group(0)
        sep = "_" if "_" in m.group(0) else ""
        upper = any(ch in "ABCDEF" for ch in m.group(1))
        return f"0x600A{sep}{c:04X}" if upper else f"0x600a{sep}{c:04x}"
    return ADDR.sub(sub, line)


def port(fragment, evidence):
    src = f"{ROOT}/registers/esp32s31/model/peripherals/wifi-mac-{fragment}.toml"
    data = tomllib.load(open(src, "rb"))
    errors = []
    plan = []  # per peripheral: (c5 base, [(offset, increment)] per register)
    for p in data["peripherals"]:
        regs = []
        for r in p["registers"]:
            reg = r["register"]
            elems = []
            for i in range(reg.get("dim", 1)):
                row = by_elem.get((p["name"], reg["name"], i))
                if row is None or row["c5"] is None:
                    errors.append(f"{p['name']}.{reg['name']}[{i}] has no C5 address")
                    elems.append(None)
                else:
                    elems.append(int(row["c5"], 16))
            regs.append(elems)
        firsts = [e[0] for e in regs if e and e[0] is not None]
        if not firsts:
            errors.append(f"{p['name']}: no register has a C5 address")
            plan.append(None)
            continue
        base = min(firsts)
        entries = []
        for reg, elems in zip(p["registers"], regs):
            if None in elems:
                entries.append((0, None))
                continue
            inc = None
            if len(elems) > 1:
                steps = {b - a for a, b in zip(elems, elems[1:])}
                if len(steps) != 1:
                    errors.append(f"{p['name']}.{reg['register']['name']}: non-uniform C5 stride {sorted(steps)}")
                inc = steps.pop()
            entries.append((elems[0] - base, inc))
        plan.append((C5_BASE + base, entries))

    reviews = []
    for review in data.get("review", []):
        ids = []
        for s in list(review["sources"]) + EXTRA_SOURCES.get(review["entity"], []):
            c = c5_source(s)
            if c:
                evidence[c[0]] = c[1]
                ids.append(c[0])
        if not ids:
            errors.append(f"review {review['entity']}: no source has a C5 counterpart")
            continue
        reviews.append((review["entity"], ids))

    out, pi, ri = [], -1, -1
    for line in open(src).read().split("\n[[review]]")[0].splitlines():
        s = line.strip()
        if s == "[[peripherals]]":
            pi += 1
            ri = -1
        elif s == "[peripherals.registers.register]":
            ri += 1
        elif s.startswith("baseAddress =") and plan[pi]:
            line = f"baseAddress = {plan[pi][0]:#010X}".replace("0X", "0x")
        elif s.startswith("addressOffset =") and ri >= 0 and plan[pi]:
            line = f"addressOffset = {plan[pi][1][ri][0]:#X}".replace("0X", "0x")
        out.append(rewrite_text(line, errors, f"{fragment}:{len(out) + 1}"))
    # dimIncrement precedes the register's name; patch it in a second pass.
    pi, ri = -1, -1
    for k, line in enumerate(out):
        s = line.strip()
        if s == "[[peripherals]]":
            pi += 1
            ri = -1
        elif s == "[peripherals.registers.register]":
            ri += 1
        elif s.startswith("dimIncrement =") and plan[pi]:
            inc = plan[pi][1][ri][1]
            out[k] = f"dimIncrement = {inc:#X}".replace("0X", "0x")
    text = "\n".join(out).rstrip() + "\n"
    for entity, ids in reviews:
        body = ",\n".join(f'    "{i}"' for i in ids)
        text += (f"\n[[review]]\nentity = \"{entity}\"\nsources = [\n{body},\n]\n"
                 "provenance = \"derived\"\naccuracy = \"exact\"\ncompleteness = \"partial\"\n")
    return text, errors


if __name__ == "__main__":
    evidence = {}
    failed = False
    for fragment in sys.argv[1:]:
        text, errors = port(fragment, evidence)
        if errors:
            failed = True
            print(f"{fragment}: {len(errors)} errors", *errors[:10], sep="\n  ")
            continue
        open(f"{ROOT}/registers/esp32c5/model/peripherals/wifi-mac-{fragment}.toml", "w").write(text)
        print(f"{fragment}: written")
    ev = "# Reviewed register evidence: ESP32-C5 Wi-Fi libraries, derived from ESP32-S31 reviews.\nschema = 1\n"
    for cid in sorted(evidence):
        ev += f'\n[[sources]]\nid = "{cid}"\ndescription = {json.dumps(evidence[cid])}\n'
    open(f"{ROOT}/registers/esp32c5/evidence/vendor-wifi-libraries.toml", "w").write(ev)
    print(len(evidence), "evidence sources")
    sys.exit(1 if failed else 0)
