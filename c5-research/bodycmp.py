#!/usr/bin/env python3
"""Compare same-named functions of two RISC-V archives modulo immediates.

Classes per function:
  identical      same instruction text (relocated fields are zero in .o)
  lui-only       differs only in `lui` immediates -> collected as base pairs
  imm-only       differs only in immediates (lui and/or offsets)
  structural     different opcodes/registers/length
Outputs a summary, the lui pair histogram and per-function classes.
"""
import collections, json, re, subprocess, sys

def functions(archive):
    out = subprocess.run(["llvm-objdump", "-d", "--no-show-raw-insn", "--no-leading-addr", archive],
                         capture_output=True, text=True).stdout
    funcs, cur, name = {}, None, None
    for line in out.splitlines():
        m = re.match(r"^<(.+)>:$", line.strip())
        if m:
            name = m.group(1)
            if name.startswith(".L") and cur is not None:
                continue  # a local label continues the current function
            cur = []; funcs.setdefault(name, cur); continue
        if cur is None or not line.strip() or line.startswith("Disassembly") or line.endswith(":"):
            continue
        ins = line.strip().split("\t")
        ins = " ".join(x.strip() for x in ins if x.strip())
        ins = re.sub(r"\s*<[^>]*>", "", ins)       # drop symbolic targets
        ins = re.sub(r"\s+", " ", ins)
        cur.append(ins)
    return {k: v for k, v in funcs.items() if v and not k.startswith(".L")}

IMM = re.compile(r"-?0x[0-9a-f]+|-?\b\d+\b")

def classify(a, b, pairs):
    if a == b:
        return "identical"
    if len(a) != len(b):
        return "structural"
    lui = []
    only_lui = True
    for x, y in zip(a, b):
        if x == y:
            continue
        if IMM.sub("#", x) != IMM.sub("#", y):
            return "structural"
        if x.split(" ")[0] == "lui" and y.split(" ")[0] == "lui":
            lui.append((x.split(",")[-1].strip(), y.split(",")[-1].strip()))
        else:
            only_lui = False
    for p in lui:
        pairs[p] += 1
    return "lui-only" if only_lui else "imm-only"

old, new, tag = sys.argv[1], sys.argv[2], sys.argv[3]
fa, fb = functions(old), functions(new)
pairs = collections.Counter()
classes = {n: classify(fa[n], fb[n], pairs) for n in sorted(set(fa) & set(fb))}
count = collections.Counter(classes.values())
print(f"{tag}: common={len(classes)} only-s31={len(set(fa)-set(fb))} only-c5={len(set(fb)-set(fa))} " +
      " ".join(f"{k}={count[k]}" for k in ["identical", "lui-only", "imm-only", "structural"]))
json.dump({"classes": classes, "lui_pairs": [[a, b, n] for (a, b), n in pairs.most_common()]},
          open(f"target/c5-research/diff/{tag}.body.json", "w"), indent=1)
