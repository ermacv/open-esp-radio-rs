"""Check an IEEE802154 model fragment against an ESP-IDF ieee802154_struct.h.

Prints, per fragment register/field, whether the struct declares the same
offset, bit position and width. Unions: every named bitfield of the union's
struct at the given `//0x..` offset; plain `uint32_t name; //0x..` words are
32-bit fields at bit 0.
"""
import re
import sys
import tomllib


def parse_struct(path):
    text = open(path).read()
    regs = {}
    # union { struct { ... }; uint32_t val; } name; //0xOFF
    for m in re.finditer(r"union\s*\{\s*struct\s*\{(.*?)\};\s*uint32_t val;\s*\}\s*(\w+)(?:\[(\d+)\])?;\s*//\s*(0x[0-9a-fA-F]+)", text, re.S):
        body, name, dim, off = m.group(1), m.group(2), m.group(3), int(m.group(4), 16)
        bit = 0
        fields = []
        for fm in re.finditer(r"uint32_t\s+(\w+)\s*:\s*(\d+)\s*;", body):
            w = int(fm.group(2))
            fields.append((fm.group(1), bit, w))
            bit += w
        regs[off] = (name, fields)
    for m in re.finditer(r"^\s*uint32_t\s+(\w+)\s*;\s*//\s*(0x[0-9a-fA-F]+)", text, re.M):
        regs.setdefault(int(m.group(2), 16), (m.group(1), [(m.group(1), 0, 32)]))
    return regs


def main(fragment, struct):
    model = tomllib.load(open(fragment, "rb"))
    regs = parse_struct(struct)
    bad = 0
    for p in model["peripherals"]:
        for r in p["registers"]:
            reg = r["register"]
            off = reg["addressOffset"]
            dim = reg.get("dim", 1)
            inc = reg.get("dimIncrement", 4)
            for i in range(dim):
                o = off + i * inc
                if o not in regs:
                    print(f"MISSING-REG {o:#05x} {reg['name']}")
                    bad += 1
                    continue
                sname, sfields = regs[o]
                for f in reg.get("fields", []):
                    b, w = f["bitOffset"], f["bitWidth"]
                    hit = [x for x in sfields if x[1] == b and not x[0].startswith("reserved")]
                    if not hit:
                        print(f"NO-FIELD    {o:#05x} {reg['name']}.{f['name']} [{b}+{w}] struct={sname} {sfields}")
                        bad += 1
                    elif hit[0][2] != w:
                        print(f"WIDTH       {o:#05x} {reg['name']}.{f['name']} [{b}+{w}] struct {hit[0][0]} width {hit[0][2]}")
                        bad += 1
    print(f"mismatches: {bad}")


main(sys.argv[1], sys.argv[2])
