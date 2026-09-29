"""Check model fragment fields against an ESP-IDF register struct whose word
offsets follow declaration order (no offset comments)."""
import re
import sys
import tomllib


def parse(path):
    text = open(path).read()
    body = text[text.index("typedef volatile struct"):]
    regs, offset = {}, 0
    # Tokenize top-level members: unions or plain uint32_t words.
    pos = body.index("{") + 1
    while True:
        m = re.compile(r"\s*(union\s*\{|uint32_t\s+(\w+)(\[(\d+)\])?\s*;|\}\s*\w+;)").match(body, pos)
        if not m:
            pos += 1
            if pos >= len(body):
                break
            continue
        if m.group(0).strip().startswith("}"):
            break
        if m.group(1).startswith("union"):
            end = body.index("uint32_t val;", m.end())
            inner = body[m.end():end]
            name = re.match(r"\s*\}\s*(\w+)\s*;", body[body.index("}", end):]).group(1)
            bit, fields = 0, []
            for fm in re.finditer(r"uint32_t\s+(\w+)\s*:\s*(\d+)\s*;", inner):
                fields.append((fm.group(1), bit, int(fm.group(2))))
                bit += int(fm.group(2))
            regs[offset] = (name, fields)
            offset += 4
            pos = body.index(";", body.index("}", end)) + 1
        else:
            count = int(m.group(4)) if m.group(4) else 1
            for i in range(count):
                regs[offset] = (m.group(2), [(m.group(2), 0, 32)])
                offset += 4
            pos = m.end()
    return regs


def main(fragment, struct):
    model = tomllib.load(open(fragment, "rb"))
    regs = parse(struct)
    bad = 0
    for p in model["peripherals"]:
        for r in p["registers"]:
            reg = r["register"]
            off = reg["addressOffset"]
            if off not in regs:
                print(f"MISSING-REG {off:#x} {reg['name']}"); bad += 1; continue
            sname, sfields = regs[off]
            for f in reg.get("fields", []):
                b, w = f["bitOffset"], f["bitWidth"]
                cover = [x for x in sfields if x[1] <= b and b + w <= x[1] + x[2] and not x[0].startswith("reserved")]
                if not cover:
                    print(f"NO-FIELD {off:#x} {reg['name']}.{f['name']} [{b}+{w}] in {sname}"); bad += 1
                elif cover[0][1] != b or cover[0][2] != w:
                    print(f"PART {off:#x} {reg['name']}.{f['name']} [{b}+{w}] inside {cover[0]}")
    print(f"mismatches: {bad}")


main(sys.argv[1], sys.argv[2])
