"""Finite current-archive calibration leaves against shipping HAL/PHY code.

Expected transactions below come from independent instruction reading of the
authenticated archive and its ROM AGC child, not from the production result.
"""
import copy


def exercise(call, doc, symbol, roots, vendor, replacement):
    from phy_i2c import case, invocation, region

    shim = symbol(2, "open_phy_trace_i2c_entry")["value"]
    production = symbol(2, "open_phy_calibration_leaf")["value"]
    cases, expected = [], []

    def invoke(target, arguments, cells):
        words = list(arguments) + [0] * (8 - len(arguments))
        data = [b for word in words for b in word.to_bytes(4, "little")]
        models = [dict(id="leaf-registers", applicability="explicit retained calibration register inputs",
                       lifetime="phase", behavior=dict(kind="register-bank", cells=[
                           dict(address=a, width=4, value=v) for a, v in sorted(cells.items())]))] if cells else []
        return invocation(shim, [target, 0x3fff4000], [region(0x3fff4000, 32, data)], models)

    def add(name, root, profile, arguments, cells, writes, returned=None):
        row = case(name, invoke(roots[root], arguments, cells),
                   invoke(production, [profile] + list(arguments), cells))
        row["relation"]["returns"]["low"] = returned is not None
        cases.append(row)
        expected.append((writes, returned))

    for name, value in [("restore-a", 0xa596783c), ("restore-b", 0x5a6987c3)]:
        first = value & 0xffffff00
        second = (first & 0xffff00ff) | 0xfb00
        third = (second & 0xff00ffff) | 0x30000
        writes = [(0x20100410, v) for v in (first, second, third, third & 0xffffff)]
        add(name, "phy_txgain_comp_pacfg_new", 0, [1], {0x20100410: value}, writes)
    for name, enabled, a, b, initial in [
        ("enable-calibration-gain", 1, 0xffffff88, 0xffffff88, 0xa5fe1234),
        ("disable-calibration-gain", 0, 0xffffff88, 0xffffff88, 0x5aff4321),
        ("independent-signed-gains", 1, 0xffffff80, 0x7f, 0x96a55a69),
    ]:
        first = (initial & 0xfffeffff) | (enabled << 16)
        second = (first & 0xffffff00) | (a & 255)
        third = (second & 0xffff00ff) | ((b & 255) << 8)
        add(name, "phy_force_dig_gain", 1, [enabled, a, b], {0x20100408: initial},
            [(0x20100408, v) for v in (first, second, third)])
    # Current archive: positive Wi-Fi /8, negative /3; BT positive /5, negative /4.
    # This intentionally does not substitute the ROM's positive Wi-Fi policy.
    for name, arguments, result in [
        ("wifi-positive", [80, 0, 0], 10), ("wifi-negative", [0, 24, 0], -8),
        ("bluetooth-positive", [80, 0, 1], 16), ("bluetooth-negative", [0, 24, 1], -6),
    ]:
        add(name, "phy_temp_to_power_new", 2, arguments, {}, [], result & 0xffffffff)
    for name, value in [("agc-a", 0xa596783c), ("agc-b", 0x5a6987c3)]:
        first = (value & 0xffffff80) | 0x17
        writes = [(0x2010705c, value | 0x04000000),
                  (0x20107064, 0x0818212d), (0x20107114, 0x0818212d),
                  (0x20107104, (value & 0xfffffe00) | 0x1c0),
                  (0x201078c8, first), (0x201078c8, (first & 0xffffc07f) | 0xb80)]
        cells = {0x2010705c: value, 0x20107104: value, 0x201078c8: value,
                 0x20107064: 0, 0x20107114: 0}
        add(name, "phy_reg_update_new", 3, [], cells, writes)

    def execute(label, selected, verdict, maximum=4096):
        request = dict(schema=16, vendor=vendor, replacement=replacement, binding="shared-core",
                       cases=selected, max_events=maximum)
        identity = call(label, ["compare", "--request", doc(label, request)])["run"]["execution"]
        evidence = call(label+"-evidence", ["execution", "--id", identity])
        assert evidence["summary"]["manifest"]["verdict"] == verdict
        return identity, evidence

    artifacts = []
    for start in range(0, len(cases), 8):
        label = f"calibration-leaves-{start//8}"
        identity, evidence = execute(label, cases[start:start+8], "MATCH")
        rows = [r["value"] for r in evidence["records"]]
        for i, (writes, returned) in enumerate(expected[start:start+8]):
            for side in (False, True):
                selected = [r for r in rows if r.get("case") == i and r.get("replacement") == side]
                outcome = next(r["stop"] for r in selected if r["kind"] == "outcome")
                assert outcome["kind"] == "returned", (i, side, outcome)
                if returned is not None:
                    assert outcome["low"] == returned, (i, side, outcome)
                events = [r["event"] for r in selected if r["kind"] == "event"]
                assert all(e["kind"] in ("read", "write") and e["width"] == 4 for e in events)
                assert [(e["address"], e["value"]) for e in events if e["kind"] == "write"] == writes, (i, side, events)
                assert all(r["observation"]["status"] == "complete" for r in selected if r["kind"] == "model")
        artifacts.append((label, identity, evidence))

    different = copy.deepcopy(cases[5])  # Positive Wi-Fi temperature conversion.
    different["name"] = "changed-temperature"
    different["replacement"]["memory"][0]["seed"]["bytes"][4] = 88
    identity, evidence = execute("calibration-different", [different], "DIFF")
    returns = [r["value"]["stop"]["low"] for r in evidence["records"] if r["value"]["kind"] == "outcome"]
    assert returns == [10, 11], returns
    artifacts.append(("calibration-different", identity, evidence))

    unknown = copy.deepcopy(cases[5])
    unknown["name"] = "unknown-argument-buffer"
    unknown["replacement"]["arguments"][1] = None
    identity, evidence = execute("calibration-unknown", [unknown], "INCOMPLETE")
    artifacts.append(("calibration-unknown", identity, evidence))

    missing = copy.deepcopy(cases[0])
    missing["name"] = "missing-register-input"
    missing["vendor"]["models"] = []
    identity, evidence = execute("calibration-missing", [missing], "INCOMPLETE")
    stop = next(r["value"]["stop"] for r in evidence["records"] if r["value"]["kind"] == "outcome" and not r["value"]["replacement"])
    assert stop["reason"] == dict(kind="memory", address=0x20100410, access="read"), stop
    artifacts.append(("calibration-missing", identity, evidence))

    request = dict(schema=16, vendor=vendor, replacement=replacement, binding="shared-core",
                   cases=[cases[0]], max_events=1)
    failed = call("calibration-capacity", ["compare", "--request", doc("calibration-capacity", request)], expected=1)
    assert failed["run"].get("execution") is None and failed["run"].get("publication") is None
    assert failed["run"]["error"]["code"] == "resource-limited"
    return artifacts
