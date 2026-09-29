"""Compare two linked RISC-V ELFs function by function, modulo placement.

Every address an instruction forms (jal/branch targets, auipc+use and
lui+use pairs) is replaced by the containing symbol and offset, symbol
names drop their legacy mangling hash, and each function's normalized
instruction list is compared by name. Reports functions whose normalized
code differs and names present in only one image.
"""
import bisect, re, subprocess, sys

HASH = re.compile(r"::h[0-9a-f]{16}$")
ALIAS = [("oer_esp32s31_pac::generated::MacInterface", "MacInterface"),
         ("oer_ieee80211_pac::generated::MacInterface", "MacInterface"),
         # identical-code-folded instantiations keep either name
         ("oer_esp32s31_hal::owner::RadioRuntimeOwner", "<radio-owner>"),
         ("oer_esp32s31_ieee80211::cooperative_hardware::CooperativeRadioHardware", "<radio-owner>")]


def clean(name):
    name = HASH.sub("", name)
    for a, b in ALIAS:
        name = name.replace(a, b)
    return name


def symbols(elf):
    out = subprocess.run(["llvm-nm", "-S", "-C", "--defined-only", elf], capture_output=True, text=True).stdout
    table = []
    for line in out.splitlines():
        p = line.split(None, 3)
        if len(p) == 4 and not p[3].startswith(".L"):
            table.append((int(p[0], 16), int(p[1], 16), p[2], clean(p[3])))
    table.sort()
    canonical = {}
    for addr, size, kind, name in table:
        if addr not in canonical or name < canonical[addr]:
            canonical[addr] = name
    return [(a, s, k, canonical[a]) for a, s, k, _ in table]


SECTIONS = {}


def load_sections(elf):
    """(start, bytes) of every allocated PROGBITS section."""
    out = subprocess.run(["llvm-readelf", "-S", "-W", elf], capture_output=True, text=True).stdout
    result = []
    for line in out.splitlines():
        if "]" not in line:
            continue
        fields = line.split("]", 1)[1].split()
        if len(fields) >= 7 and fields[1] == "PROGBITS" and "A" in fields[6]:
            name, addr = fields[0], int(fields[2], 16)
            path = f"/tmp/fncmp-{abs(hash((elf, name)))}.bin"
            subprocess.run(["llvm-objcopy", "-O", "binary", f"--only-section={name}", elf, path], check=True)
            result.append((addr, open(path, "rb").read()))
    SECTIONS[elf] = result


TABLES = {}


def in_image(elf, word):
    return any(start <= word < start + len(data) for start, data in SECTIONS[elf])


def content(elf, addr):
    """Up to 16 bytes at `addr`; words that point into the image become the
    symbol they point to, so relocated pointers compare equal."""
    for start, data in SECTIONS[elf]:
        if start <= addr < start + len(data):
            chunk = data[addr - start: addr - start + 16]
            words = []
            for k in range(0, len(chunk) - 3, 4):
                w = int.from_bytes(chunk[k:k + 4], "little")
                if in_image(elf, w):
                    table, starts = TABLES[elf]
                    words.append("&" + locate(table, starts, w))
                else:
                    words.append(chunk[k:k + 4].hex())
            return "data:" + ",".join(words)
    return None


def locate(table, starts, addr, elf=None):
    i = bisect.bisect_right(starts, addr) - 1
    while i >= 0:
        a, size, kind, name = table[i]
        if a <= addr < a + max(size, 1):
            return f"{name}+{addr - a:#x}"
        if a < addr and size == 0:
            i -= 1
            continue
        break
    if elf and in_image(elf, addr):
        return "data"
    return f"?{addr:#x}"


INS = re.compile(r"^\s*([0-9a-f]+):\s+(\S+)\s*(.*)$")
FUNC = re.compile(r"^([0-9a-f]+) <(.+)>:$")
IMM = re.compile(r"-?0x[0-9a-f]+|-?\b\d+\b")


def functions(elf, table):
    starts = [t[0] for t in table]
    dis = subprocess.run(["llvm-objdump", "-d", "-C", "--no-show-raw-insn", "--mattr=+c,+m,+a,+zba,+zbb,+zbs,+zcb,+zcmp,+f", elf],
                         capture_output=True, text=True).stdout
    funcs, cur, name, pending = {}, None, None, {}
    for line in dis.splitlines():
        m = FUNC.match(line)
        if m:
            label = clean(m.group(2))
            if label.startswith(".L") and cur is not None:
                continue
            addr = int(m.group(1), 16)
            label = locate(table, starts, addr).removesuffix("+0x0") if not label.startswith(".L") else label
            name, cur, pending = label, [], {}
            funcs.setdefault(name, []).append(cur)
            continue
        m = INS.match(line)
        if not m or cur is None:
            continue
        pc, op, args = int(m.group(1), 16), m.group(2), m.group(3)
        word = re.search(r"\.word\s+(0x[0-9a-f]+)", line)
        if word:
            value = int(word.group(1), 16)
            cur.append(".word " + (locate(table, starts, value) if in_image(elf, value) else hex(value)))
            continue
        args = args.split(" <", 1)[0].strip()
        a = [x.strip() for x in args.split(",")] if args else []
        text = f"{op} {args}"
        if op in ("jal", "j", "beq", "bne", "blt", "bge", "bltu", "bgeu", "beqz", "bnez", "bltz", "bgez", "blez", "bgtz",
                  "c.j", "c.jal", "c.beqz", "c.bnez") and a and re.fullmatch(r"0x[0-9a-f]+", a[-1]):
            text = f"{op} {','.join(a[:-1])} {locate(table, starts, int(a[-1], 16), elf)}"
        elif op == "auipc" and len(a) == 2:
            pending[a[0]] = pc + (int(a[1], 0) << 12)
            text = f"auipc {a[0]} <pair>"
        elif op == "lui" and len(a) == 2:
            pending[a[0]] = (int(a[1], 0) << 12) & 0xFFFFFFFF
            text = f"lui {a[0]} <pair>"
        else:
            if op in ("jalr", "jr", "c.jalr", "c.jr") and len(a) == 1 and re.fullmatch(r"[a-z]\w*", a[0]):
                a = [f"0({a[0]})"]
            if op == "mv" and len(a) == 2 and a[1] in pending:
                op, a = "addi", [a[0], a[1], "0"]
            mem = re.fullmatch(r"(-?0x[0-9a-f]+|-?\d+)\((\w+)\)", a[-1]) if a else None
            base = mem.group(2) if mem else (a[1] if op in ("addi", "jalr") and len(a) >= 3 else None)
            if base in pending:
                off = int(mem.group(1), 0) if mem else int(a[2], 0)
                target = (pending[base] + off) & 0xFFFFFFFF
                text = f"{op} {','.join(a[:-1] if mem else a[:2])} {locate(table, starts, target, elf)}"
                if op in ("addi",) and a[0] == base or op not in ("addi",) and not mem:
                    pending.pop(base, None)
            if a and op not in ("sw", "sh", "sb", "sd", "c.sw", "c.sd") and a[0] in pending and not (base == a[0]):
                pending.pop(a[0], None)
        if op in ("jal", "jalr", "c.jal", "c.jalr", "call", "tail", "jr", "c.jr", "j", "ret"):
            for reg in ["ra"] + [f"a{i}" for i in range(8)] + [f"t{i}" for i in range(7)]:
                pending.pop(reg, None)
        cur.append(text)
    return funcs


def main(old, new):
    load_sections(old)
    load_sections(new)
    ta, tb = symbols(old), symbols(new)
    TABLES[old] = (ta, [t[0] for t in ta])
    TABLES[new] = (tb, [t[0] for t in tb])
    fa, fb = functions(old, ta), functions(new, tb)
    only_a = sorted(set(fa) - set(fb))
    only_b = sorted(set(fb) - set(fa))
    fa = {n: sorted(bodies) for n, bodies in fa.items()}
    fb = {n: sorted(bodies) for n, bodies in fb.items()}
    differ = [n for n in sorted(set(fa) & set(fb)) if fa[n] != fb[n]]
    print(f"functions: {len(fa)} old, {len(fb)} new, {len(set(fa) & set(fb))} common")
    print(f"only old: {only_a[:10]}")
    print(f"only new: {only_b[:10]}")
    print(f"normalized code differs: {len(differ)}")
    for n in differ[:40]:
        x, y = [i for b in fa[n] for i in b], [i for b in fb[n] for i in b]
        ks = [i for i, (p, q) in enumerate(zip(x, y)) if p != q]
        print(f"  {n[:110]}: len {len(x)}/{len(y)} {len(ks)} differing lines")
        for k in ks[:3]:
            print(f"      {k}: {x[max(0,k-2):k+1]} | {y[max(0,k-2):k+1]}")
    # Identical-code-folded functions keep one name per image; pair the
    # names present in one image only by body.
    left = sorted((fa[n], n) for n in only_a)
    right = sorted((fb[n], n) for n in only_b)
    paired = sum(1 for (x, _), (y, _) in zip(left, right) if x == y)
    print(f"one-image names paired by identical body: {paired}/{len(only_a)}")
    return 1 if differ or only_a or only_b else 0


sys.exit(main(sys.argv[1], sys.argv[2]))
