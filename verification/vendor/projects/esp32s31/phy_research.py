"""Exercise captured PHY data/research, review and current-format preservation.

Private inputs and generated evidence stay outside tracked source. Requires the
identified PHY artifact; assertions concern captured facts, never qualification.
"""

import argparse
import pathlib
import tempfile
import json
import subprocess
import shutil
import hashlib
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--binary", type=pathlib.Path, required=True)
parser.add_argument("--library", type=pathlib.Path, required=True)
parser.add_argument("--rom", type=pathlib.Path, required=True)
parser.add_argument("--output", type=pathlib.Path, required=True)
parser.add_argument("--limit-mode", choices=["kernel", "watchdog"], required=True)
options = parser.parse_args()
root = options.output.resolve()
root.mkdir(parents=True, exist_ok=True)
run = pathlib.Path(tempfile.mkdtemp(prefix="run-", dir=root))
(root / "latest").write_text(str(run))
project = run / "project"
binary = options.binary.resolve()


def call(name, args, expected=0):
    cmd = [str(binary), "--format", "json", args[0], "--project", str(project)]
    if args[0] != "init":
        cmd += [
            "--limit-mode",
            options.limit_mode,
            "--timeout-secs",
            "600",
            "--working-memory-mib",
            "256",
            "--max-work-units",
            "2000000000",
        ]
    cmd += args[1:]
    start = time.monotonic()
    p = subprocess.run(cmd, capture_output=True)
    (run / (name + ".json")).write_bytes(p.stdout)
    (run / (name + ".stderr")).write_bytes(p.stderr)
    print(name, p.returncode, round(time.monotonic() - start, 2), flush=True)
    assert p.returncode == expected, p.stderr.decode()[-2500:]
    return json.loads(p.stdout) if p.returncode == 0 and p.stdout else None


def doc(name, value):
    p = run / (name + ".request.json")
    p.write_text(json.dumps(value))
    return str(p)


sources = [options.library.resolve(), options.rom.resolve()]
assert (
    hashlib.sha256(sources[0].read_bytes()).hexdigest()
    == "d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"
)
local = []
for i, p in enumerate(sources):
    q = run / ("input-" + str(i))
    shutil.copyfile(p, q)
    local.append(q)
call("init", ["init"])
revision = call(
    "import",
    ["import", "--input", "phy=" + str(local[0]), "--input", "rom=" + str(local[1])],
)["run"]["revision"]
for p in local:
    p.unlink()
inventory = call("inventory", ["inventory"])["snapshot"]["revision"]
objects = inventory["inputs"][0]["inventory"]["objects"]
publication = call("whole", ["analyze-project"])["run"]["publication"]
coverage = call("coverage", ["coverage", "--id", publication])
assert coverage["assessment"]["coverage"]["scope"] == "selected-function-extents"
assert coverage["summary"]["extents"]["objects"] > 0
for sample in range(3):
    call(
        "research-" + str(sample),
        [
            "research",
            "--id",
            publication,
            "--name",
            "phy_txgain_comp_pacfg_new",
            "--abi-contract",
            "riscv-integer",
        ],
    )
usage = call("storage-usage", ["storage-usage"])
assert usage["summary"]["usage"]["cas"]["logical_bytes"] > 0
assert not usage["summary"]["usage"]["reachability_assessed"]


def occurrence(obj):
    return {
        "revision": revision,
        "source": {"kind": "input", "input": 0},
        "object": obj["id"],
        "symbol": None,
    }


def obj(name):
    return next(o for o in objects if bytes(o["name"]).decode() == name)


def section(o, name):
    return next(
        s for s in o["elf"]["sections"] if bytes(s["name"] or []).decode() == name
    )


def selected_analysis(name):
    result = call(
        name + "-functions", ["functions", "--id", publication, "--name", name]
    )
    m = result["records"][0]["value"]
    return m["outcome"]["analysis"]


analyses = {
    name: selected_analysis(name)
    for name in [
        "phy_force_dig_gain",
        "phy_txgain_comp_pacfg_new",
        "phy_i2c_master_cmd_mem_init",
    ]
}
queries = {}
o = obj("phy_reg.o")
q = {
    "occurrence": occurrence(o),
    "ranges": [],
    "analyses": [analyses["phy_force_dig_gain"], analyses["phy_txgain_comp_pacfg_new"]],
}
queries["registers"] = q
facts = call("register-data", ["data", "--request", doc("registers", q)])
records = [r["value"] for r in facts["records"]]
constants = [
    (r["ordinal"], r["record"]["value"]["value"])
    for r in records
    if r["kind"] == "analysis"
    and r["analysis"] == analyses["phy_txgain_comp_pacfg_new"]
    and r["record"]["kind"] == "value"
    and r["record"]["value"]["kind"] == "constant"
]
print("constant candidates", constants, flush=True)
assert any(
    r["kind"] == "analysis" and r["record"]["kind"] == "memory-access" for r in records
)
o = obj("phy_i2c.o")
s = section(o, ".rodata.CSWTCH.51")
assert s["size"] == 200
selector = {"kind": "section", "section": s["index"], "offset": 0, "length": s["size"]}
q = {
    "occurrence": occurrence(o),
    "ranges": [selector],
    "analyses": [analyses["phy_i2c_master_cmd_mem_init"]],
}
queries["i2c-table"] = q
call(
    "i2c-data",
    [
        "data",
        "--request",
        doc("i2c-table", q),
        "--output",
        str(run / "i2c-observations"),
    ],
)
assert len((run / "i2c-observations/data.bin").read_bytes()) == 200
p = {
    "occurrence": q["occurrence"],
    "analyses": q["analyses"],
    "subject": "phy.captured-i2c-table",
    "selector": selector,
    "layout": {
        "encoding": {"width": 1, "signed": False, "byte_order": "little"},
        "count": 200,
        "stride": 1,
    },
    "purpose": "Exact byte representation of the captured read-only table; no inferred hardware interpretation",
    "applicability": "Only the selected source occurrence and section range",
    "expected_base": None,
    "actor": "source-byte-review",
    "reason": "Verified captured range and digest, not a qualification claim",
}
proposal = call(
    "propose-table", ["knowledge", "propose-data", "--request", doc("table", p)]
)["run"]["knowledge"]
entries = call("table-claims", ["knowledge", "show"])["records"]
assertion = next(r["value"]["id"] for r in entries if r["value"]["state"] == "proposed")
call(
    "pending-refused",
    [
        "export-data",
        "--revision",
        proposal,
        "--assertion",
        assertion,
        "--output",
        str(run / "pending"),
    ],
    expected=1,
)
base = call(
    "accept-table",
    [
        "knowledge",
        "accept",
        "--base",
        proposal,
        "--assertion",
        assertion,
        "--actor",
        "source-byte-review",
        "--reason",
        "Confirmed physical byte representation only",
    ],
)["run"]["knowledge"]
call(
    "export-table",
    [
        "export-data",
        "--revision",
        base,
        "--assertion",
        assertion,
        "--output",
        str(run / "accepted-table"),
    ],
)
assert (run / "accepted-table/data.bin").read_bytes() == (
    run / "i2c-observations/data.bin"
).read_bytes()
o = obj("phy_init.o")
symbol = next(
    s for s in o["elf"]["symbols"] if bytes(s["name"] or []).decode() == "phy_param"
)
assert symbol["size"] == 516
q = {
    "occurrence": occurrence(o),
    "ranges": [{"kind": "symbol", "symbol": symbol["id"], "length": None}],
    "analyses": [],
}
queries["initial"] = q
result = call(
    "initial",
    ["data", "--request", doc("initial", q), "--output", str(run / "initial")],
)
assert result["summary"]["manifest"]["spans"][0]["writable"]
assert len((run / "initial/data.bin").read_bytes()) == 516
# Review only a known raw bit pattern backed by an exact saved record.
assert any(v == 0x30000 for _, v in constants)
ordinal, value = next((i, v) for i, v in constants if v == 0xFB00)
p = {
    "analysis": analyses["phy_txgain_comp_pacfg_new"],
    "record": ordinal,
    "operand": {"kind": "value"},
    "value": value,
    "subject": "phy.gain-compensation-instruction-constant",
    "purpose": "Raw 0xfb00 insertion mask for the gain compensation byte at bits 8..15; other byte updates remain in the retained analysis",
    "applicability": "Only this function in the selected current PHY artifact",
    "expected_base": base,
    "actor": "source-byte-review",
    "reason": "Known operand checked against retained instruction/value record",
}
proposal = call(
    "propose-constant",
    ["knowledge", "propose-constant", "--request", doc("constant", p)],
)["run"]["knowledge"]
entries = call("constant-claims", ["knowledge", "show"])["records"]
constant_id = next(
    r["value"]["id"] for r in entries if r["value"]["state"] == "proposed"
)
base = call(
    "accept-constant",
    [
        "knowledge",
        "accept",
        "--base",
        proposal,
        "--assertion",
        constant_id,
        "--actor",
        "source-byte-review",
        "--reason",
        "Confirmed exact known operand, not a hardware qualification claim",
    ],
)["run"]["knowledge"]
call(
    "export-constant",
    [
        "export-data",
        "--revision",
        base,
        "--assertion",
        constant_id,
        "--output",
        str(run / "accepted-constant"),
    ],
)
assert (run / "accepted-constant/data.bin").read_bytes() == b""
for directory in ["i2c-observations", "accepted-table", "initial", "accepted-constant"]:
    d = run / directory
    m = json.loads((d / "manifest.json").read_text())
    assert hashlib.sha256((d / "object.elf").read_bytes()).hexdigest() == m["payload"]
    for span in m["spans"]:
        r = span["file_range"]
        data = (d / "object.elf").read_bytes()[r["start"] : r["start"] + r["length"]]
        assert hashlib.sha256(data).hexdigest() == span["digest"]
        assert (
            data
            == (d / "data.bin").read_bytes()[
                span["export_offset"] : span["export_offset"] + r["length"]
            ]
        )
call("doctor", ["doctor"])
project.rename(run / "moved")
project = run / "moved"
call(
    "moved",
    [
        "export-data",
        "--revision",
        base,
        "--assertion",
        assertion,
        "--output",
        str(run / "moved-table"),
    ],
)
backup = run / "project.blobray"
call("backup", ["backup", "--output", str(backup)])
project = run / "restored"
call("restore", ["restore", "--backup", str(backup)])
call(
    "restored",
    [
        "export-data",
        "--revision",
        base,
        "--assertion",
        assertion,
        "--output",
        str(run / "restored-table"),
    ],
)
call(
    "restored-constant",
    [
        "export-data",
        "--revision",
        base,
        "--assertion",
        constant_id,
        "--output",
        str(run / "restored-constant"),
    ],
)
call("restored-doctor", ["doctor"])
for name in ["object.elf", "data.bin", "records.jsonl"]:
    assert (run / "accepted-table" / name).read_bytes() == (
        run / "restored-table" / name
    ).read_bytes()
a = json.loads((run / "accepted-table/manifest.json").read_text())
b = json.loads((run / "restored-table/manifest.json").read_text())
a["knowledge"] = b["knowledge"]
assert a == b
for name in ["manifest.json", "object.elf", "data.bin", "records.jsonl"]:
    assert (run / "accepted-constant" / name).read_bytes() == (
        run / "restored-constant" / name
    ).read_bytes()
(run / "acceptance.json").write_text(
    json.dumps(
        {
            "revision": revision,
            "publication": publication,
            "knowledge": base,
            "table": assertion,
            "constant": constant_id,
            "constant_value": value,
            "sources_removed": True,
            "moved_restored": True,
            "requests": queries,
        },
        indent=2,
    )
)
print(run, flush=True)
