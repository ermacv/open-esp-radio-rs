"""Classify esp32s31 coupling in chip-neutral parts of the repository."""
import collections
import re
import subprocess
import tomllib

PAT = re.compile(r"esp32s31|ESP32-S31|ESP32S31|\bS31\b|riscv32imafc")
files = subprocess.run(["git", "ls-files"], capture_output=True, text=True).stdout.split()

# Portable / host packages by metadata.
neutral_dirs = []
for f in files:
    if f.endswith("Cargo.toml"):
        try:
            meta = tomllib.load(open(f, "rb")).get("package", {}).get("metadata", {}).get("open-radio", {})
        except Exception:
            continue
        if meta.get("platform") in ("portable", "host"):
            neutral_dirs.append(f.rsplit("/", 1)[0] + "/")

groups = {
    "portable/host crates": lambda f: any(f.startswith(d) for d in neutral_dirs),
    "tools/xtask": lambda f: f.startswith("tools/xtask/"),
    "tools/firmware": lambda f: f.startswith("tools/firmware/"),
    "tools/memory-report": lambda f: f.startswith("tools/memory-report/"),
    "hil (non-target)": lambda f: f.startswith("hil/") and not f.startswith(("hil/targets/", "hil/evidence/")),
    "qualification/evaluator": lambda f: f.startswith("qualification/evaluator/"),
    "CI": lambda f: f.startswith(".github/"),
    "root Cargo.toml": lambda f: f in ("Cargo.toml", "rust-toolchain.toml", ".cargo/config.toml"),
}
out = collections.defaultdict(list)
for f in files:
    if not f.endswith((".rs", ".toml", ".yml", ".yaml", ".sh", ".py")):
        continue
    g = next((k for k, pred in groups.items() if pred(f)), None)
    if not g:
        continue
    try:
        lines = open(f, errors="ignore").read().splitlines()
    except Exception:
        continue
    for i, line in enumerate(lines, 1):
        if not PAT.search(line):
            continue
        s = line.strip()
        kind = "comment" if s.startswith(("//", "#", "*", "///", "//!")) else "code"
        out[g].append((kind, f, i, s[:150]))

for g, rows in out.items():
    c = collections.Counter(k for k, *_ in rows)
    print(f"\n## {g}: code={c['code']} comment={c['comment']} files={len({r[1] for r in rows})}")
    per = collections.Counter(r[1] for r in rows if r[0] == "code")
    for f, n in per.most_common(12):
        print(f"   {n:4d} {f}")
open("target/c5-research/coupling-code-lines.txt", "w").write(
    "\n".join(f"{g}\t{k}\t{f}:{i}\t{s}" for g, rows in out.items() for k, f, i, s in rows if k == "code"))
