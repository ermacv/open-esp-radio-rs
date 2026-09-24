"""Compare authenticated PHY I2C and optional calibration leaves with production.

Uses native Blobray requests only. Private inputs, requests and evidence are written
under the explicitly selected output directory. This is software comparison under
explicit peripheral assumptions, never hardware qualification.
"""

import argparse
import copy
import hashlib
import json
import pathlib
import shutil
import subprocess
import tempfile
from harness import Runner, ProbeCatalog, Buffer, single_argument_entry, symbol as captured_symbol
from harness import seed, region, invocation, selection, case

LIBRARY_SHA = "d4218e359b9716c616cbf116172f44d9195d4f2e020fad73279067e92d08e580"
ROM_SHA = "d01bde81d9b3806e37ef1d9ac3b58af4f5b3d91eeef4f44d20e79d6a9f227542"
OBJECT_SHA = "7e6ebb1353d1bd2c53b4b5b1176bbf795c57d899c3f07a26ce56e74b5803e9d9"
TABLE_SHA = "927b3305a35468bb52f3de4e3305f4b4d0674831014376a094ceb00022bab183"
SDK_SHA = "e5e2929ae216e324dac3efd13cf1e05146dfcc4ea64098a1fead74b8ac453195"
PHY_SDK_SHA = "ea4197a4e8d40fe43f5b1590132fab7365b2b1034dfa61f498743778002b07d9"

# Independent instruction reading of the authenticated phy_i2c.o root and ROM
# encode/fill leaves. Low words specify block/register in call order; these are
# research expectations, not a production implementation or generated PAC test.
COMMAND_LOW = [
    0x0267, 0x016b, 0x026b, 0x036b, 0x046b, 0x056b, 0x066b, 0x076b,
    0x086b, 0x096b, 0x0a6b, 0x0b6b, 0x0c6b, 0x0d6b, 0x0e6b, 0x0f6b,
    0x0062, 0x0462, 0x0b62, 0x0d62, 0x0f62, 0x1562, 0x0266, 0x0267,
    0x0467, 0x0567, 0x0667, 0x0767, 0x0c67, 0x0d67, 0x0e67, 0x0f67,
    0x1467, 0x1567, 0x1667, 0x1767, 0x1867, 0x1967, 0x1c67, 0x1d67,
    0x1e67, 0x1f67, 0x0663, 0x006a, 0x016a,
]
FIXED_PREFIX = [7, 1, 0x73, 0xba, 0x88, 1, 0x11, 0xfd, 0xbf, 2, 8, 4,
                0xa7, 0x77, 0xf4, 0x81, 0x68, 0xa8, 0x44, 10]
# Explicit expected parameter-derived values at indices 20 and 24..41. Cases
# include both saturation bounds, byte wrapping and the forced filter bit.
PROFILES = [
    ("zero", [0, 0, 0, 0, 0, 0], [0] + [0]*8 + [6, 6, 2, 0, 2, 2, 0, 0, 64, 0]),
    ("mixed", [165, 33, 67, 10, 254, 85],
     [165, 33, 33, 67, 67, 33, 33, 67, 67, 16, 16, 8, 10, 0, 0, 85, 85, 85, 85]),
    ("lower", [255, 255, 255, 2, 255, 255],
     [255]*9 + [8, 8, 2, 2, 1, 1, 255, 255, 255, 255]),
    ("upper", [1, 2, 3, 255, 253, 1],
     [1, 2, 2, 3, 3, 2, 2, 3, 3, 60, 60, 60, 255, 255, 255, 1, 1, 65, 1]),
]
def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("binary", "library", "rom", "production", "linker", "nm", "output"):
        parser.add_argument("--" + name, type=pathlib.Path, required=True)
    parser.add_argument("--limit-mode", choices=["kernel", "watchdog"], required=True)
    parser.add_argument("--calibration-leaves", action="store_true",
                        help="also compare the four current calibration leaves")
    parser.add_argument("--rfpll", action="store_true",
                        help="include RFPLL search, maintenance and frequency-memory scenarios")
    parser.add_argument("--calibration-prefix", action="store_true",
                        help="also check the PBus/DCODE prefix (requires calibration leaves)")
    parser.add_argument("--sdk", type=pathlib.Path,
                        help="pinned linked SDK symbol companion, required for calibration leaves")
    parser.add_argument("--phy-sdk", type=pathlib.Path,
                        help="pinned SDK firmware symbol companion, required for RFPLL diagnostics binding")
    options = parser.parse_args()
    if options.rfpll != (options.phy_sdk is not None):
        parser.error("--rfpll and --phy-sdk must be supplied together")
    if options.rfpll and not options.calibration_prefix:
        parser.error("--rfpll requires --calibration-prefix")
    if options.calibration_prefix and not options.calibration_leaves:
        parser.error("--calibration-prefix requires --calibration-leaves")
    if options.calibration_leaves != (options.sdk is not None):
        parser.error("--calibration-leaves and --sdk must be supplied together")
    options.output.mkdir(parents=True, exist_ok=True)
    run = pathlib.Path(tempfile.mkdtemp(prefix="run-", dir=options.output.resolve()))
    (options.output / "latest").write_text(str(run))
    project = run / "project"
    runner = Runner(options.binary, run, project, options.limit_mode)
    call, doc = runner.call, runner.doc

    sources = [options.library, options.rom, options.production]
    if options.sdk is not None:
        sources.append(options.sdk)
    if options.phy_sdk is not None:
        sources.append(options.phy_sdk)
    revision, identities = runner.capture(sources,
        ["phy", "rom", "production"] + (["sdk"] if options.sdk is not None else []) + (["phy-sdk"] if options.phy_sdk is not None else []),
        [LIBRARY_SHA, ROM_SHA, None] + ([SDK_SHA] if options.sdk is not None else []) + ([PHY_SDK_SHA] if options.phy_sdk is not None else []))
    doc("inputs", dict(sha256=identities, scope="I2C software comparison",
                       calibration_leaves=options.calibration_leaves))
    inventory = call("inventory", ["inventory"])["snapshot"]["revision"]["inputs"]

    probes = ProbeCatalog.capture(runner, revision, inventory, 2)

    def symbol(input_index, name):
        return probes.entry(name) if input_index == 2 else captured_symbol(inventory, input_index, name)

    def entry(name):
        return dict(input=0, symbol=symbol(0, name)["id"])

    obj = next(o for o in inventory[0]["inventory"]["objects"] if bytes(o["name"]) == b"phy_i2c.o")
    section = next(s for s in obj["elf"]["sections"] if bytes(s["name"]) == b".rodata.CSWTCH.51")
    data = dict(occurrence=dict(revision=revision, source=dict(kind="input", input=0),
                object=obj["id"], symbol=None), analyses=[],
                ranges=[dict(kind="section", section=section["index"], offset=0, length=200)])
    call("table", ["data", "--request", doc("table", data), "--output", run/"table"])
    assert hashlib.sha256((run/"table/object.elf").read_bytes()).hexdigest() == OBJECT_SHA
    assert hashlib.sha256((run/"table/data.bin").read_bytes()).hexdigest() == TABLE_SHA
    leaves = ["phy_i2c_master_mem_cfg", "phy_i2c_master_command_mem_cfg",
              "phy_get_i2c_data", "phy_i2c_enter_critical", "phy_i2c_exit_critical"]
    calibration_roots = ["phy_txgain_comp_pacfg_new", "phy_force_dig_gain",
                         "phy_temp_to_power_new", "phy_reg_update_new"] if options.calibration_leaves else []
    if options.rfpll:
        calibration_roots += ["phy_rfpll_cap_init_cal_new", "phy_rfpll_cap_track_new"]
    link = dict(revision=revision, inputs=[0], entry=entry("phy_i2c_master_cmd_mem_init"),
                roots=[entry(name) for name in leaves + ["phy_get_i2c_read_mask_new", "phy_get_i2c_hostid_new"] + calibration_roots],
                companions=[dict(input=1, symbol=symbol(1, name)["id"]) for name in
                    ("phy_encode_i2c_master", "phy_i2c_master_fill", "phy_get_data_sat",
                     "phy_i2c_writeReg", "memset", "phy_get_i2c_mst0_mask", "phy_i2c_paral_write_num") +
                    (("phy_wifi_agc_sat_gain",) if options.calibration_leaves else ())],
                layout=dict(code=dict(start=0x11000000, length=0x1000000),
                            data=dict(start=0x20000000, length=0x1000000)))
    if options.calibration_leaves:
        # AGC shares .iram1 with unrelated roots. Close their physical link
        # references with authenticated symbols, without rewriting that section
        # or synthesizing bodies. Every selected binding is in the saved request.
        extra_rom = ("ets_delay_us", "phy_wait_i2c_sdm_stable", "phy_force_txrx_off",
                     "phy_dis_hw_set_freq", "phy_i2c_master_reset", "phy_open_fe_bb_clk",
                     "phy_bbpll_cal", "phy_pbus_clear_reg", "phy_i2c_clk_sel",
                     "phy_fe_txrx_reset", "phy_adc_rate_set", "phy_i2cmst_reg_init",
                     "phy_freq_reg_init", "phy_fe_reg_init", "phy_pwdet_reg_init",
                     "phy_write_chan_freq", "phy_set_pbus_reg", "phy_reg_init",
                     "phy_bb_agc_reg_update", "phy_set_chan_reg", "phy_set_txcap_reset",
                     "phy_bb_cbw_chan_cfg", "phy_enable_agc", "phy_wait_freq_set_busy",
                     "phy_reset_ckgen", "phy_en_hw_set_freq", "phy_wifi_enable_set",
                     "phy_disable_agc", "phy_tsens_temp_read", "phy_i2c_writeReg_Mask",
                     "phy_i2c_readReg", "phy_freq_i2c_write_set")
        link["companions"] += [dict(input=1, symbol=symbol(1, n)["id"]) for n in extra_rom]
        link["companions"] += [dict(input=3, symbol=symbol(3, n)["id"]) for n in
                               ("rtc_clk_xtal_freq_get",)]
    if options.rfpll:
        link["companions"] += [dict(input=1, symbol=symbol(1, n)["id"]) for n in
                               ("phy_read_pll_cap", "phy_write_pll_cap", "phy_pll_cap_mem_update",
                                "phy_abs_temp")]
        link["companions"].append(dict(input=4, symbol=symbol(4, "phy_printf")["id"]))
    plan = run / "link-plan.json"
    call("plan", ["link-plan", "--request", doc("plan", link), "--linker", options.linker.resolve(), "--output", plan])
    image = call("prepare", ["prepare-image", "--plan", plan, "--linker", options.linker.resolve()])["run"]["image"]
    view = call("image", ["image", "--id", image])
    manifest = view["summary"]["manifest"]
    roots = {bytes(r["name"]).decode(): r["address"] for r in manifest["roots"]}
    roots["phy_i2c_master_cmd_mem_init"] = manifest["entry"]
    mappings = [r["value"] for r in view["records"] if r["kind"] == "mapping"]
    call("export-image", ["export-image", "--id", image, "--output", run/"image"])
    # Independent current-ELF symbol selection: an inexact source mapping is not
    # sufficient to assert a physical mutable-data address.
    nm = subprocess.run([str(options.nm.resolve()), "--defined-only", "--format=posix",
                         str(run/"image/image.elf")], capture_output=True, check=True)
    (run/"image-symbols.txt").write_bytes(nm.stdout)
    matches = [line.split() for line in nm.stdout.decode().splitlines()
               if line.split()[0] == "phy_param"]
    assert len(matches) == 1 and int(matches[0][3], 16) == 516
    parameter = int(matches[0][2], 16)
    assert any(m["address"] == parameter and m["size"] == 516 for m in mappings)
    stack = seed(0x3ffe0000, 0x8000)
    vendor = dict(revision=revision, source=dict(kind="image", image=image),
                  companions=[1, 2], abi="riscv-integer", stack=stack)
    replacement = dict(revision=revision, source=dict(kind="input", input=2),
                       companions=[1], abi="riscv-integer", stack=stack)
    prefix = []
    if options.rfpll:
        from phy_rfpll_native import exercise
        prefix += exercise(call, doc, symbol, roots, vendor, replacement, parameter)
    if options.calibration_prefix:
        from phy_calibration_prefix import exercise
        # The ROM delay boundary is a captured symbol, not a declared probe.
        production_delay = captured_symbol(inventory, 2, "ets_delay_us")["value"]
        prefix += exercise(call, doc, symbol, roots, vendor, replacement, parameter, production_delay)
    seeded_entry = symbol(2, "open_phy_trace_seeded_entry")["value"]
    bank = dict(id="command-ram", applicability="45 aligned command slots; passive register bank",
                lifetime="phase", behavior=dict(kind="register-bank", cells=[
                    dict(address=0x2010fc00+i*4, width=4, value=0) for i in range(45)]))
    cases = []
    expected_commands = {}
    for name, parameters, expected_dynamic in PROFILES:
        full = [0]*400
        for offset, value in zip([0x18e, 0xe9, 0xea, 0xed, 0xee, 0xf0], parameters):
            full[offset] = value
        setup_left = probes.invoke("open_phy_trace_initialize_parameters",
            dict(destination=parameter, source=Buffer(0x3fff0000, full, lifetime="session")),
            observe=[selection(parameter, 400)])
        setup_right = probes.invoke("open_phy_trace_initialize_parameters",
            dict(destination=Buffer(0x3fff2000, lifetime="session"),
                 source=Buffer(0x3fff0000, full, lifetime="session")),
            observe=[selection(0x3fff2000, 400)])
        cases.append(case(name+"-setup", setup_left, setup_right, memory=True))
        left = invocation(seeded_entry, [roots["phy_i2c_master_cmd_mem_init"], 0], models=[bank])
        right = single_argument_entry(probes.invoke("open_phy_trace_command_memory",
            dict(parameters=Buffer(0x3fff1000, parameters)), models=[bank]), seeded_entry)
        values = FIXED_PREFIX + [expected_dynamic[0], 8, 0x70, 0x27] + expected_dynamic[1:] + [0, 0xaf, 0x7f]
        assert len(values) == len(COMMAND_LOW) == 45
        expected_commands[len(cases)] = [low + (v << 16) for low, v in zip(COMMAND_LOW, values)]
        cases.append(case(name, left, right, reset="warm"))
    descriptor_cases = {}
    for name, length, expected_bytes in [
        (leaves[0], 6, [0, 0, 1, 1, 44, 1]),
        (leaves[1], 12, [0, 0, 0, 1, 1, 1, 44, 1, 2, 0, 0, 0]),
    ]:
        for fill in (0, 255):
            args = [0x3fff0000, 0x3fff0008]
            mem = [region(0x3fff0000, length, fill=fill)]
            observe = [selection(0x3fff0000, length)]
            descriptor_cases[len(cases)] = expected_bytes
            cases.append(case(name+str(fill), invocation(roots[name], args, mem, observe=observe),
                invocation(symbol(2, "open_phy_trace_"+name)["value"], args, mem, observe=observe), memory=True))
    for name in leaves[2:]:
        cases.append(case(name, invocation(roots[name]), invocation(symbol(2, "open_phy_trace_"+name)["value"])))
    request = dict(schema=17, vendor=vendor, replacement=replacement, binding="shared-core", cases=cases, max_events=512)
    execution = call("compare", ["compare", "--request", doc("compare", request)])["run"]["execution"]
    evidence = call("evidence", ["execution", "--id", execution])
    assert evidence["summary"]["manifest"]["verdict"] == "MATCH"
    records = [r["value"] for r in evidence["records"]]
    outcomes = [r for r in records if r["kind"] == "outcome"]
    assert len(outcomes) == 2*len(cases) and all(r["stop"]["kind"] == "returned" for r in outcomes)
    comparisons = [r for r in records if r["kind"] == "comparison"]
    assert len(comparisons) == len(cases) and all(r["result"]["verdict"] == "MATCH" for r in comparisons)
    assert not any(r["kind"] == "event" and r["case"] >= 8 for r in records)
    models = [r["observation"] for r in records if r["kind"] == "model"]
    assert len(models) == 8 and all(m["writes"] == 45 and m["reads"] == 0
        and m["closed"] and m["status"] == "complete" and m["issue"] is None for m in models)
    for index, words in expected_commands.items():
        for side in (False, True):
            events = [r["event"] for r in records if r["kind"] == "event" and r["case"] == index and r["replacement"] == side]
            assert [(e["kind"], e["address"], e["width"], e["value"]) for e in events] == [
                ("write", 0x2010fc00+i*4, 4, w) for i, w in enumerate(words)], (index, side, events)
    for index, expected_bytes in descriptor_cases.items():
        for side in (False, True):
            chunks = [r["chunk"] for r in records if r["kind"] == "final-memory" and r["case"] == index and r["replacement"] == side]
            assert all(c["known"] == c["available"] == (1 << c["length"])-1 for c in chunks)
            actual = [b for c in chunks for b in c["bytes"][:c["length"]]]
            assert actual == expected_bytes, (index, side, chunks)
    # A changed replacement input must not be hidden by code-goal completion.
    different = copy.deepcopy(request)
    different["cases"] = different["cases"][:2]
    different["cases"][1]["replacement"]["memory"][0]["seed"]["bytes"][1] = 1
    diff_id = call("different", ["compare", "--request", doc("different", different)])["run"]["execution"]
    assert call("different-evidence", ["execution", "--id", diff_id])["summary"]["manifest"]["verdict"] == "DIFF"
    unknown = copy.deepcopy(request)
    unknown["cases"] = [unknown["cases"][1]]
    unknown["cases"][0]["reset"] = "cold"
    unknown["cases"][0]["replacement"]["arguments"][1] = None
    unknown_id = call("unknown", ["compare", "--request", doc("unknown", unknown)])["run"]["execution"]
    assert call("unknown-evidence", ["execution", "--id", unknown_id])["summary"]["manifest"]["verdict"] == "INCOMPLETE"
    missing = copy.deepcopy(request)
    missing["vendor"]["companions"] = [2]
    missing["cases"] = [missing["cases"][1]]
    missing["cases"][0]["reset"] = "cold"
    missing_id = call("missing-rom", ["compare", "--request", doc("missing-rom", missing)])["run"]["execution"]
    missing_evidence = call("missing-rom-evidence", ["execution", "--id", missing_id])
    assert missing_evidence["summary"]["manifest"]["verdict"] == "INCOMPLETE"
    missing_stop = next(r["value"]["stop"] for r in missing_evidence["records"]
                       if r["value"]["kind"] == "outcome" and not r["value"]["replacement"])
    assert missing_stop["reason"] == dict(kind="memory", address=symbol(1, "phy_encode_i2c_master")["value"], access="fetch")
    limited = copy.deepcopy(request)
    limited["max_events"] = 1
    failed = call("capacity", ["compare", "--request", doc("capacity", limited)], expected=1)
    assert failed["run"].get("execution") is None and failed["run"].get("publication") is None
    assert failed["run"]["error"]["code"] == "resource-limited"
    from phy_i2c_transport import exercise
    transport = prefix + exercise(call, doc, symbol, roots, vendor, replacement)
    if options.calibration_leaves:
        from phy_calibration_leaves import exercise as calibration
        transport += calibration(call, doc, symbol, roots, vendor, replacement)
    from harness_edges import exercise as harness_edges
    transport += harness_edges(call, doc, symbol, probes, replacement)
    # All original source copies were deleted before linking/execution. Preserve
    # the full project closure, including probe/ROM bytes and negative evidence.
    call("backup", ["backup", "--output", run/"backup.blobray"])
    shutil.move(project, run/"moved")
    project = runner.project = run/"moved"
    moved = call("moved-evidence", ["execution", "--id", execution])
    assert moved["records"] == evidence["records"]
    assert call("moved-replay", ["replay", "--id", execution])["run"]["execution"] == execution
    project = runner.project = run/"restored"
    call("restore", ["restore", "--backup", run/"backup.blobray"])
    for name, identity in [("positive", execution), ("different", diff_id), ("unknown", unknown_id), ("missing-rom", missing_id)]:
        before = evidence if name == "positive" else json.loads((run/(name+"-evidence.json")).read_text())
        restored = call("restored-"+name, ["execution", "--id", identity])
        assert restored["records"] == before["records"]
        assert restored["summary"]["manifest"] == before["summary"]["manifest"]
        replay = call("replay-"+name, ["replay", "--id", identity], expected=0)
        assert replay["run"]["execution"] == identity
    for name, identity, before in transport:
        restored = call("restored-"+name, ["execution", "--id", identity])
        assert restored["records"] == before["records"]
        assert restored["summary"]["manifest"] == before["summary"]["manifest"]
        assert call("replay-"+name, ["replay", "--id", identity])["run"]["execution"] == identity
    print("authenticated PHY comparisons and source-free replay passed", run, flush=True)


if __name__ == "__main__":
    main()
