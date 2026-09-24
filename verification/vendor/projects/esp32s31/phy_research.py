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
parser.add_argument("--linker", type=pathlib.Path, required=True)
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
assert hashlib.sha256(sources[1].read_bytes()).hexdigest() == "d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"
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
# Independent ROM instruction reading: four branches form base 0x2010e000
# plus -0x7a8/-0x7a0/-0x798/-0x790, then one shared load/store pair.
rom_rows = call("finite-address-function", ["functions", "--id", publication,
    "--name", "tsf_hal_set_tbtt_rf_ctrl_disable"])["records"]
assert len(rom_rows) == 1
finite_analysis = rom_rows[0]["value"]["outcome"]["analysis"]
finite_records = call("finite-address-facts", ["analysis", "--id", finite_analysis])["records"]
expected_addresses = {"kind": "alternatives", "values": [
    {"kind": "constant", "value": address}
    for address in (0x2010D858, 0x2010D860, 0x2010D868, 0x2010D870)]}
for offset, access in ((0x2F82BCB6, "load"), (0x2F82BCC0, "store")):
    assert any(r["value"].get("kind") == "memory-access"
        and r["value"]["offset"] == offset and r["value"]["access"] == access
        and r["value"]["width"] == 4 and r["value"]["address"] == expected_addresses
        for r in finite_records)
call("finite-export", ["export-analysis", "--id", finite_analysis, "--output", str(run / "finite-export")])
# This real callback loads the table from mutable memory at 0x2f07fc3c,
# then calls slot +8. Without a reviewed binding the destination stays unknown.
callback = call("real-callback", ["research", "--id", publication,
    "--address", "0x2f829fa8", "--abi-contract", "riscv-integer"])
callback_analysis = callback["run"]["analysis"]
callback_records = call("real-callback-facts", ["analysis", "--id", callback_analysis])["records"]
assert any(r["value"].get("kind") == "transfer" and r["value"]["offset"] == 0x2F829FB6
    and r["value"]["call"] and r["value"]["target"] == {"kind": "unknown"}
    for r in callback_records)
assert any(r["value"].get("kind") == "call-resolution" and r["value"]["offset"] == 0x2F829FB6
    and r["value"]["analysis"] is None for r in callback_records)
assert not any(r["value"].get("kind") == "callee-effect" for r in callback_records)

callback_query = {"input": {"kind": "analysis", "analysis": callback_analysis,
                            "abi": "riscv-integer"}, "knowledge": None}
callback_discovery = call("callback-discovery", ["interfaces", "--request", doc("callback-discovery", callback_query)])
assert len(callback_discovery["records"]) == 1
callback_slot = callback_discovery["records"][0]["value"]
assert callback_slot["offset"] == 0x2F829FB6
assert callback_slot["paths"] == [{"root": {"kind": "address", "address": 0x2F07FC3C},
    "path": [{"kind": "load-pointer", "offset": 0}], "slot": 8}]
assert callback_slot["target"] == {"kind": "unknown"}
assert callback_slot["issue"] is None and callback_slot["bindings"] == []

coverage = call("coverage", ["coverage", "--id", publication])
assert coverage["assessment"]["coverage"]["scope"] == "selected-function-extents"
assert coverage["summary"]["extents"]["objects"] > 0
# Unlike archive-local research, this image resolves ordinary calls to exact
# captured ROM definitions. Its final tail transfer retains the profile's limits.
plan = run / "linked-plan.json"
call("link-plan", ["link-plan", "--entry", "phy_i2c_master_cmd_mem_init",
    "--entry-input", "0", "--inputs", "0", "--code-start", "0x10000000",
    "--data-start", "0x20000000", "--linker", str(options.linker.resolve()),
    "--companion", "1:phy_encode_i2c_master", "--companion", "1:phy_i2c_master_fill", "--companion", "1:phy_get_data_sat",
    "--output", str(plan)])
image = call("prepare-image", ["prepare-image", "--plan", str(plan),
    "--linker", str(options.linker.resolve())])["run"]["image"]
linked_publication = call("linked-analysis", ["analyze-project", "--image", image])["run"]["publication"]
callees = {}
for name in ("phy_encode_i2c_master", "phy_i2c_master_fill", "phy_get_data_sat"):
    rows = call(name, ["functions", "--id", publication, "--name", name])["records"]
    matches = [r["value"] for r in rows if r["value"]["entry"]["request"]["source"] == {"kind": "input", "input": 1}]
    assert len(matches) == 1
    callees[name] = matches[0]["outcome"]["analysis"]
# Exact static trace on authenticated ROM bytes: fill(index, value) writes one u32
# to 0x2010fc00 + index*4. This expectation is independent of the trace exporter.
ir_request = {"scope": {"revision": revision, "publications": [],
    "analyses": [callees["phy_i2c_master_fill"]], "knowledge": None},
    "profiles": [{"name": "fill", "roots": {"kind": "all"}, "include_reachable": True}]}
saved_ir = call("build-semantic-ir", ["ir", "build", "--request", doc("ir-build", ir_request)])["run"]["semantic_ir"]
call("export-semantic-ir", ["ir", "show", saved_ir, "--output", str(run / "semantic-ir-export.json")])
trace_target = {"ir": saved_ir, "profile": "fill", "entry": callees["phy_i2c_master_fill"],
    "abi": "riscv-integer", "registers": [{"register": 10, "value": 0}, {"register": 11, "value": 0x70267}]}
trace_request = {"left": trace_target, "right": trace_target,
    "observation": {"ranges": [{"start": 0x2010FC00, "length": 256}], "fences": True}}
static_trace = call("static-fill-trace", ["trace", "--request", doc("static-fill-trace", trace_request)])
assert static_trace["summary"]["summary"]["verdict"] == "MATCH"
assert static_trace["summary"]["summary"]["left"] == {"exact": True, "events": 1, "invocations": 1}
static_events = [r["value"]["event"] for r in static_trace["records"]
    if r["value"]["kind"] == "event" and r["value"]["side"] == "left"]
assert static_events == [{"kind": "memory", "access": "store", "address": 0x2010FC00,
    "width": 4, "value": {"kind": "constant", "value": 0x70267}}]
changed = json.loads(json.dumps(trace_request))
changed["right"]["registers"][1]["value"] = 0x70268
assert call("static-fill-diff", ["trace", "--request", doc("static-fill-diff", changed)])["summary"]["summary"]["verdict"] == "DIFF"
unknown = json.loads(json.dumps(trace_request))
unknown["right"]["registers"] = [{"register": 11, "value": 0x70267}]
assert call("static-fill-incomplete", ["trace", "--request", doc("static-fill-incomplete", unknown)])["summary"]["summary"]["verdict"] == "INCOMPLETE"
call("save-static-trace", ["trace", "--request", doc("static-trace-export", trace_request),
    "--output", str(run / "static-trace-export.json")])

linked_facts = None
phase_samples = []
for sample in range(3):
    result = call("research-" + str(sample), ["research", "--id", linked_publication,
        "--name", "phy_i2c_master_cmd_mem_init", "--abi-contract", "riscv-integer",
        "--companion-publication", publication])
    linked_analysis = result["run"]["analysis"]
    facts = call("research-facts-" + str(sample), ["analysis", "--id", linked_analysis])["records"]
    records = [r["value"] for r in facts]
    resolved = [r for r in records if r["kind"] == "call-resolution" and r["analysis"]]
    assert {r["analysis"] for r in resolved} == set(callees.values())
    # The authenticated root has 45 encode calls, 44 ordinary fill calls,
    # three saturation calls, and one final fill tail transfer (not composed).
    for name, count in (("phy_encode_i2c_master", 45), ("phy_i2c_master_fill", 44), ("phy_get_data_sat", 3)):
        assert sum(r["analysis"] == callees[name] for r in resolved) == count
    effects = [r for r in records if r["kind"] == "callee-effect"
        and r["analysis"] == callees["phy_i2c_master_fill"]]
    assert len(effects) == 44
    # Independent instruction interpretation: encode(0x67, 2, 7), then fill(0,...).
    assert any(r["address"] == {"kind": "constant", "value": 0x2010FC00}
        and r["value"] == {"kind": "constant", "value": 0x70267}
        and r["width"] == 4 for r in effects)
    if linked_facts is not None:
        assert records == linked_facts
    linked_facts = records
    progress = result["run"]["diagnostics"]["progress"]
    phases = progress["phases"]
    for phase in ("load_research", "compose_research"):
        assert phases[phase]["work_units"] > 0
        assert 0 < phases[phase]["peak_reserved_bytes"] <= 256 * 1024 * 1024
    phase_samples.append({k: phases[k] for k in ("load_research", "compose_research")})
(run / "linked-phase-measurements.json").write_text(json.dumps(phase_samples, indent=2))

# Navigate exactly the saved linked root and its selected physical callees.
# No whole-project implicit scope, linking or research is allowed in this read.
nav_request = {"scope": {"revision": revision, "publications": [],
    "analyses": [linked_analysis, *callees.values()], "knowledge": None},
    "filter": {"kind": "functions", "function": None}}
nav_functions = call("navigation-functions", ["navigate", "--request", doc("navigation-functions", nav_request)])
nav_root = next(r["value"]["function"]["location"] for r in nav_functions["records"]
    if r["value"]["kind"] == "function" and r["value"]["function"]["analysis"] == linked_analysis)
assert nav_functions["summary"]["summary"]["selected_analyses"] == 4
assert nav_functions["summary"]["summary"]["analyses_read"] == 0
nav_request["filter"] = {"kind": "calls", "function": nav_root, "direction": "callees"}
nav_calls = call("navigation-calls", ["navigate", "--request", doc("navigation-calls", nav_request)])
nav_rows = [r["value"] for r in nav_calls["records"] if r["value"]["kind"] == "call"]
assert nav_calls["summary"]["summary"]["analyses_read"] == 4
for name, count in (("phy_encode_i2c_master",45), ("phy_i2c_master_fill",44), ("phy_get_data_sat",3)):
    matched = [r for r in nav_rows if r["saved_resolution"] == callees[name]]
    assert len(matched) == count
    assert all(r["focus_match"] and any(c["analysis"] == callees[name] for c in r["candidates"]) for r in matched)
    assert all(linked_facts[r["record"]]["offset"] == r["offset"] for r in matched)
call("save-navigation", ["navigate", "--request", doc("navigation-export", nav_request),
    "--output", str(run / "navigation-export.json")])
flow_request = {"scope": nav_request["scope"], "root": linked_analysis,
    "goal": {"kind": "function", "analysis": callees["phy_encode_i2c_master"]}, "max_depth": 8}
flow_path = call("flow-target", ["flow", "--request", doc("flow-target", flow_request)])
assert flow_path["summary"]["summary"]["target_reached"] is True
assert any(r["value"]["kind"] == "function" and r["value"]["parent"] is not None
    and r["value"]["parent"]["caller"] == linked_analysis
    and r["value"]["parent"]["callee"] == callees["phy_encode_i2c_master"] for r in flow_path["records"])
flow_request["goal"] = {"kind": "effects", "profile": "memory", "address": None}
flow_effects = call("flow-effects", ["flow", "--request", doc("flow-effects", flow_request)])
assert flow_effects["summary"]["summary"]["facts_passes"] == 8
flow_writes = [r["value"] for r in flow_effects["records"] if r["value"]["kind"] == "effect"
    and r["value"]["analysis"] == linked_analysis
    and r["value"]["fact"]["kind"] == "callee-effect"
    and r["value"]["fact"]["analysis"] == callees["phy_i2c_master_fill"]]
assert len(flow_writes) == 44
assert all(linked_facts[r["record"]] == r["fact"] for r in flow_writes)
assert any(r["fact"]["value"] == {"kind": "constant", "value": 0x70267} for r in flow_writes)
register_request = {"scope": {"revision": revision, "publications": [],
    "analyses": [finite_analysis, linked_analysis], "knowledge": None},
    "ranges": [{"start": 0x2010D858, "length": 28}, {"start": 0x2010FC00, "length": 176}]}
register_catalog = call("register-catalog", ["registers", "--request", doc("register-catalog", register_request)])
register_rows = [r["value"] for r in register_catalog["records"]]
finite_accesses = [r for r in register_rows if r["kind"] == "observation"
    and r["function"]["analysis"] == finite_analysis and r["fact"]["kind"] == "memory-access"]
assert len(finite_accesses) == 8
assert {r["address"] for r in finite_accesses} == {0x2010D858, 0x2010D860, 0x2010D868, 0x2010D870}
assert all(r["alternative"] is not None for r in finite_accesses)
composed_registers = [r for r in register_rows if r["kind"] == "observation"
    and r["fact"]["kind"] == "callee-effect" and r["fact"]["analysis"] == callees["phy_i2c_master_fill"]]
assert len(composed_registers) == 44
assert {r["address"] for r in composed_registers} == {0x2010FC00 + 4 * i for i in range(44)}
assert register_catalog["summary"]["summary"]["declarations"] == 0
call("save-register-catalog", ["registers", "--request", doc("register-catalog-export", register_request),
    "--output", str(run / "register-catalog-export.json")])
address_request = {**flow_request, "goal": {"kind": "effects", "profile": "memory", "address": 0x2010FC00}}
address_effects = call("flow-address", ["flow", "--request", doc("flow-address", address_request)])
address_writes = [r["value"] for r in address_effects["records"]
    if r["value"]["kind"] == "effect" and r["value"]["address_match"] is True]
assert len(address_writes) == 1
assert address_writes[0]["fact"]["value"] == {"kind": "constant", "value": 0x70267}
# Other unknown-address facts remain visible; a focused query cannot prove absence.
assert any(r["value"]["kind"] == "effect" and r["value"]["address_match"] is None for r in address_effects["records"])
call("save-flow", ["flow", "--request", doc("flow-export", flow_request),
    "--output", str(run / "flow-export.json")])
# Independent prologue decoding: sw ra,28(sp) after a 32-byte frame allocation,
# at linked root + 10. The first ordinary call is root + 24, the second + 36.
# The saved root has an unexpanded tail, so this local witness remains candidate.
slice_anchor = next(i for i, r in enumerate(linked_facts)
    if r["kind"] == "transfer" and r["offset"] == 0x10000018)
slice_request = {"analysis": linked_analysis, "anchor": slice_anchor,
    "abi": "riscv-integer", "locations": [{"kind": "stack", "offset": -4, "width": 4}]}
memory_slice = call("memory-slice", ["memory-slice", "--request", doc("memory-slice", slice_request)])
slice_rows = [r["value"] for r in memory_slice["records"]]
slice_defs = [r for r in slice_rows if r["kind"] == "definition"]
assert len(slice_defs) == 1
assert slice_defs[0]["offset"] == 0x1000000A
assert slice_defs[0]["fact"] == linked_facts[slice_defs[0]["record"]]
assert slice_defs[0]["fact"]["address"] == {"kind": "entry-stack", "offset": -4}
assert slice_defs[0]["witness"] == [0x1000000A, 0x1000000C, 0x1000000E, 0x10000010, 0x10000012, 0x10000014, 0x10000018]
assert slice_defs[0]["class"] == "candidate"
assert next(r for r in slice_rows if r["kind"] == "location")["issues"] == ["partial-control-flow"]
after_request = {**slice_request, "anchor": next(i for i, r in enumerate(linked_facts)
    if r["kind"] == "transfer" and r["offset"] == 0x10000024)}
after_slice = call("memory-slice-after-call", ["memory-slice", "--request", doc("memory-slice-after-call", after_request)])
assert any(r["value"]["kind"] == "barrier" and r["value"]["issue"] == "call-clobber"
    and r["value"]["offset"] == 0x10000018 for r in after_slice["records"])
call("save-memory-slice", ["memory-slice", "--request", doc("memory-slice-export", slice_request),
    "--output", str(run / "memory-slice-export.json")])
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
# Established independently from the authenticated archive, not from Blobray's manifest.
assert hashlib.sha256((run / "i2c-observations/object.elf").read_bytes()).hexdigest() == "7e6ebb1353d1bd2c53b4b5b1176bbf795c57d899c3f07a26ce56e74b5803e9d9"
assert hashlib.sha256((run / "i2c-observations/data.bin").read_bytes()).hexdigest() == "927b3305a35468bb52f3de4e3305f4b4d0674831014376a094ceb00022bab183"
p = {
    "occurrence": q["occurrence"],
    "analyses": q["analyses"],
    "subject": "phy.captured-i2c-table",
    "selector": selector,
    "layout": {
        "kind": "integer",
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
constant_revision = base
# Authenticated ELF32 .rela.rodata records independently establish eleven
# absolute references to local code labels. These are pointer observations,
# not discovered function boundaries or reviewed callback ABI contracts.
pointer_obj = obj("phy_i2c.o")
pointer_section = section(pointer_obj, ".rodata")
pointer_layout = {"count": 11, "stride": 4}
pointer_request = {
    "occurrence": occurrence(pointer_obj), "analyses": [],
    "ranges": [{"kind": "section", "section": pointer_section["index"], "offset": 0, "length": 44}],
    "pointer_table": pointer_layout,
}
pointer_data = call("pointer-observations", ["data", "--request", doc("pointers", pointer_request), "--output", str(run / "pointer-observations")])
expected_targets = [36, 36, 36, 34, 34, 34, 36, 34, 34, 36, 36]
pointer_values = [r for r in map(json.loads, (run / "pointer-observations/records.jsonl").read_text().splitlines()) if r["kind"] == "pointer"]
assert pointer_data["summary"]["manifest"]["pointers"]["defined_symbols"] == 11
assert len(pointer_values) == 11
for i, r in enumerate(pointer_values):
    assert r["index"] == i and r["offset"] == i * 4
    assert r["value"] == {"kind": "defined-symbol", "symbol": {
        "object": pointer_obj["id"], "table": "static", "table_section": 17, "index": expected_targets[i]}, "addend": 0}
assert hashlib.sha256((run / "pointer-observations/data.bin").read_bytes()).hexdigest() == "85759b3811ff7dc47b03792ac85317be51431a3f9e01dcafce317ed736a391b0"
p = {
    "occurrence": pointer_request["occurrence"], "analyses": [], "subject": "phy.captured-code-pointer-table",
    "selector": pointer_request["ranges"][0], "layout": {"kind": "pointers", **pointer_layout},
    "purpose": "Captured absolute relocation targets; no inferred function boundaries",
    "applicability": "Authenticated phy_i2c.o table only", "expected_base": base,
    "actor": "source-byte-review", "reason": "Independent ELF relocation identities and table digest",
}
proposal = call("propose-pointers", ["knowledge", "propose-data", "--request", doc("pointer-proposal", p)])["run"]["knowledge"]
entries = call("pointer-claims", ["knowledge", "show"])["records"]
pointer_id = next(r["value"]["id"] for r in entries if r["value"]["state"] == "proposed")
base = call("accept-pointers", ["knowledge", "accept", "--base", proposal, "--assertion", pointer_id, "--actor", "source-byte-review", "--reason", "Exact captured representation only"])["run"]["knowledge"]
call("export-pointers", ["export-data", "--revision", base, "--assertion", pointer_id, "--output", str(run / "accepted-pointers")])

pointer_revision = base
# Review only the structural path independently established by the ROM instructions:
# lui/lw captures global 0x2f07fc3c, lw +8 loads the callback, jalr invokes it.
# Neither the callback signature nor its semantic purpose follows from these bytes.
callback_summary = callback_discovery["summary"]["summary"]
callback_contract = {
    **callback_slot["paths"][0], "layout_version": "captured-rom-callback/1",
    "layout_bytes": 12, "pointer_bytes": 4, "abi": "riscv-integer", "index_domains": [],
    "guards": [{"kind": "captured-payload", "payload": callback_summary["payload"]}],
    "slots": [{"offset": 8, "name": "captured-slot-8", "semantic": None, "signature": None}],
    "purpose": "Exact captured callback load path; signature and runtime target unknown",
    "applicability": "Authenticated ROM occurrence only; no hardware behavior assertion",
}
del callback_contract["slot"]
change = {"expected_base": base, "actor": "source-instruction-review",
    "reason": "Independent ROM instruction offsets and operands", "action": {"kind": "propose", "proposal": {
        "subject": "phy.captured-rom-callback", "occurrence": callback_summary["occurrence"],
        "claim": {"kind": "interface", "contract": callback_contract},
        "evidence": [{"kind": "analysis", "analysis": callback_analysis, "record": callback_slot["record"]}],
        "note": "Structural identity only; no inferred signature, semantic binding or runtime guard satisfaction",
    }}}
proposal = call("propose-callback", ["knowledge", "apply", "--change", doc("callback-proposal", change)])["run"]["knowledge"]
entries = call("callback-claims", ["knowledge", "show"])["records"]
callback_id = next(r["value"]["id"] for r in entries if r["value"]["state"] == "proposed")
base = call("accept-callback", ["knowledge", "accept", "--base", proposal, "--assertion", callback_id,
    "--actor", "source-instruction-review", "--reason", "Confirmed physical load path only"])["run"]["knowledge"]
callback_query["knowledge"] = base
callback_matched = call("matched-callback", ["interfaces", "--request", doc("callback-matched", callback_query)])
assert callback_matched["summary"]["summary"]["matched_accepted"] == 1
assert callback_matched["summary"]["summary"]["unresolved_paths"] == 0
binding = callback_matched["records"][0]["value"]["bindings"][0]
assert binding["assertion"] == callback_id and binding["state"] == "accepted"
assert binding["signature"] is None and binding["semantic"] is None
assert callback_matched["records"][0]["value"]["target"] == {"kind": "unknown"}
call("export-callback", ["interfaces", "--request", doc("callback-export", callback_query),
    "--output", str(run / "callback-export.json")])

for directory in ["i2c-observations", "accepted-table", "initial", "accepted-constant", "accepted-pointers"]:
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
        constant_revision,
        "--assertion",
        constant_id,
        "--output",
        str(run / "restored-constant"),
    ],
)
call("restored-pointers", ["export-data", "--revision", pointer_revision, "--assertion", pointer_id, "--output", str(run / "restored-pointers")])
for name in ["object.elf", "data.bin", "records.jsonl", "manifest.json"]:
    assert (run / "restored-pointers" / name).read_bytes() == (run / "accepted-pointers" / name).read_bytes()
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

# Reading restored composed evidence must not need linking, original files or replay.
restored = call("restored-research", ["analysis", "--id", linked_analysis])
assert [r["value"] for r in restored["records"]] == linked_facts

assert call("restored-finite-facts", ["analysis", "--id", finite_analysis])["records"] == finite_records
assert call("restored-callback-facts", ["analysis", "--id", callback_analysis])["records"] == callback_records
call("restored-finite-export", ["export-analysis", "--id", finite_analysis, "--output", str(run / "restored-finite-export")])
for file in ("manifest.json", "records.jsonl"):
    assert (run / "finite-export" / file).read_bytes() == (run / "restored-finite-export" / file).read_bytes()

assert call("restored-interfaces", ["interfaces", "--request", doc("restored-interfaces", callback_query)]) == callback_matched
call("export-restored-callback", ["interfaces", "--request", doc("restored-interface-export", callback_query),
    "--output", str(run / "restored-interface-export.json")])
assert (run / "callback-export.json").read_bytes() == (run / "restored-interface-export.json").read_bytes()

assert call("restored-navigation", ["navigate", "--request", doc("restored-navigation", nav_request)]) == nav_calls
call("save-restored-navigation", ["navigate", "--request", doc("restored-navigation-export", nav_request),
    "--output", str(run / "restored-navigation-export.json")])
assert (run / "navigation-export.json").read_bytes() == (run / "restored-navigation-export.json").read_bytes()

assert call("restored-flow", ["flow", "--request", doc("restored-flow", flow_request)]) == flow_effects
assert call("restored-register-catalog", ["registers", "--request", doc("restored-register-catalog", register_request)]) == register_catalog
call("save-restored-register-catalog", ["registers", "--request", doc("restored-register-catalog-export", register_request),
    "--output", str(run / "restored-register-catalog-export.json")])
assert (run / "restored-register-catalog-export.json").read_bytes() == (run / "register-catalog-export.json").read_bytes()
assert call("restored-memory-slice", ["memory-slice", "--request", doc("restored-memory-slice", slice_request)]) == memory_slice
call("save-restored-memory-slice", ["memory-slice", "--request", doc("restored-memory-slice-export", slice_request),
    "--output", str(run / "restored-memory-slice-export.json")])
assert (run / "restored-memory-slice-export.json").read_bytes() == (run / "memory-slice-export.json").read_bytes()
call("save-restored-flow", ["flow", "--request", doc("restored-flow-export", flow_request),
    "--output", str(run / "restored-flow-export.json")])
assert (run / "flow-export.json").read_bytes() == (run / "restored-flow-export.json").read_bytes()

assert call("restored-static-trace", ["trace", "--request", doc("restored-static-trace", trace_request)]) == static_trace
call("save-restored-static-trace", ["trace", "--request", doc("restore-static-trace-export", trace_request),
    "--output", str(run / "restored-static-trace-export.json")])
assert (run / "static-trace-export.json").read_bytes() == (run / "restored-static-trace-export.json").read_bytes()
call("save-restored-semantic-ir", ["ir", "show", saved_ir, "--output", str(run / "restored-semantic-ir-export.json")])
assert (run / "semantic-ir-export.json").read_bytes() == (run / "restored-semantic-ir-export.json").read_bytes()
