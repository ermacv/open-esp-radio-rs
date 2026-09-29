"""One-off P1-6 migration: tag chip-neutral SOURCE blocks that cite
ESP32-S31 registry functions with (esp32s31); list the rest for review."""
import re, sys, tomllib, pathlib
root = pathlib.Path(".")
chips = ["esp32c5", "esp32s31"]
registry = {f["symbol"] for f in tomllib.load(open("verification/esp32s31/facts/provenance.toml","rb")).get("function", [])}
def is_comment(l): 
    t=l.lstrip(); return t.startswith("//") or t.startswith("/*") or t.startswith("*")
def words(text):
    out=set()
    for w in re.findall(r"[A-Za-z0-9_]+", text):
        if len(w)>=4 and not w[0].isdigit() and ("_" in w or any(c.isdigit() for c in w) or any(c.isupper() for c in w[1:])):
            out.add(w)
    return out
tagged, unmatched = 0, []
for path in sorted(pathlib.Path("crates").rglob("*.rs")):
    parts = set(path.parts)
    if "target" in parts or any(c in parts for c in chips):
        continue
    lines = path.read_text().splitlines(keepends=True)
    blocks=[]; cur=None
    for i,l in enumerate(lines):
        if not is_comment(l): cur=None; continue
        if "SOURCE" in l:
            cur=[i,[l]]; blocks.append(cur)
        elif cur: cur[1].append(l)
    changed=False
    for start, text in blocks:
        marker = lines[start]
        at = marker.index("SOURCE")
        if marker[at+6:at+7] == "(":
            continue
        if words("".join(text)) & registry:
            lines[start] = marker[:at+6] + "(esp32s31)" + marker[at+6:]
            tagged += 1; changed=True
        else:
            unmatched.append(f"{path}:{start+1}: {marker.strip()[:120]}")
    if changed and "--write" in sys.argv:
        path.write_text("".join(lines))
print(f"tagged {tagged}, unmatched {len(unmatched)}")
open("target/provenance/unmatched-sources.txt","w").write("\n".join(unmatched)+"\n")
