"""Native transport cases over captured ROM callbacks and compiled PHY transactions.

Scenario expectations are independent of either execution result. The peripheral
assumption supplies responses only; ROM/PAC/PHY code performs every transaction.
"""
import copy


def exercise(call, doc, symbol, roots, vendor, replacement):
    from phy_i2c import invocation, region, case

    entry = symbol(2, "open_phy_trace_i2c_entry")["value"]
    transfer = symbol(2, "open_phy_trace_i2c_transfer")["value"]
    configuration = 0x1237fa08  # Independent .iram1 +0x44..+0x60 instruction reading.
    callbacks = [roots[name] for name in ("phy_i2c_enter_critical", "phy_i2c_exit_critical",
                                         "phy_get_i2c_read_mask_new", "phy_get_i2c_hostid_new")]

    def words(values):
        return [b for v in values for b in v.to_bytes(4, "little")]

    def memory(arguments):
        # The ROM interface pointer has no PT_LOAD mapping. Its explicit RAM
        # value selects captured code, not callback response models.
        return [region(0x2f07fc3c, 4, words([0x3fff3000])),
                region(0x3fff3000, 16, words(callbacks)),
                region(0x3fff4000, 32, words(list(arguments) + [0]*(8-len(arguments))))]

    def controls():
        return dict(id="controls", applicability="captured complemented read mask and host-map RMW",
                    lifetime="phase", behavior=dict(kind="register-bank", cells=[
                        dict(address=0x2010f81c, width=4, value=0),
                        dict(address=0x2010f820, width=4, value=0x12345678)]))

    def bank(selector=None, sample=0, scripted=False, busy=0, initial=(0, 0)):
        return dict(id="analog", applicability="shared analog bytes; samples at issue, writes at ready observation",
            lifetime="phase", behavior=dict(kind="command-bank", selector_mask=0xffff,
                data_mask=0xff0000, read_command=0x04000000, write_command=0x05000000,
                busy_mask=0x02000000, reset_command=0x04000000,
                ports=[dict(address=0x2010f800+4*i, initial=0, initial_busy_reads=initial[i],
                            busy_reads=busy) for i in range(2)],
                cells=[dict(selector=s, initial=sample if s == selector else 0,
                            reads=[sample] if scripted and s == selector else None)
                       for s in (0x026b, 0x0466)]))

    def invoke(target, arguments, models):
        return invocation(entry, [target, 0x3fff4000], memory(arguments), models)

    def paired(name, left_entry, left_words, right_entry, right_words, models, returns=False):
        result = case(name, invoke(left_entry, left_words, models), invoke(right_entry, right_words, models))
        # Production reads busy before issuing a read; ROM's org leaf does not.
        # Compare every write and selected return, retaining all excluded reads.
        result["relation"]["events"]["mmio_read"] = False
        result["relation"]["returns"]["low"] = returns
        return result

    cases, expected = [], []
    for block, host in zip(range(0x61, 0x6e), [1, 1, 1, 0, 0, 0, 1, 0, 0, 1, 1, 0, 0]):
        cases.append(paired(f"host-{block:x}", roots["phy_get_i2c_hostid_new"], [block],
            symbol(2, "open_phy_trace_i2c_host")["value"], [block], [controls()], True))
        expected.append(([(0x2010f820, configuration)], host, 0))
    for profile in range(8):
        host = profile // 4
        block, register, sample, high, low, maximum, mask, cleared, set_value = (
            (0x66, 4, 0xa6, 3, 2, 3, 0xffffff7f, 0xa2, 0xae) if host == 0 else
            (0x6b, 2, 0xa5, 7, 4, 15, 0xfffffff7, 5, 0xf5))
        selector = (register << 8) | block
        mode = profile % 4
        name = ["phy_i2c_readReg", "phy_i2c_writeReg", "phy_i2c_readReg_Mask", "phy_i2c_writeReg_Mask"][mode]
        for busy in (0, 2):
            for value in ([0, 255] if mode == 1 else [0, maximum] if mode == 3 else [0]):
                args = [block, host, register]
                if mode == 1:
                    args.append(value)
                if mode >= 2:
                    args.extend([high, low])
                if mode == 3:
                    args.append(value)
                models = [controls(), bank(selector, sample, mode != 1, busy)]
                cases.append(paired(f"transfer-{profile}-{busy}-{value}", symbol(1, name)["value"], args,
                    transfer, [profile, value, 32], models, mode in (0, 2)))
                writes = [(0x2010f820, configuration)]
                port = 0x2010f800 + 4*host
                if mode != 1:
                    writes += [(0x2010f81c, mask), (port, 0x04000000 | selector)]
                if mode == 3:
                    writes += [(0x2010f820, configuration)]
                if mode in (1, 3):
                    data = value if mode == 1 else cleared if value == 0 else set_value
                    writes += [(port, 0x05000000 | data << 16 | selector)]
                returned = sample if mode == 0 else (1 if host == 0 else 10) if mode == 2 else None
                expected.append((writes, returned, 2 if mode == 3 else 1))
    reset = symbol(1, "phy_i2c_master_reset")["value"]
    reset_replacement = symbol(2, "open_phy_trace_i2c_reset")["value"]
    for initial, busy in [((0, 0), 0), ((1, 1), 0), ((1, 1), 2)]:
        cases.append(paired(f"reset-{initial[0]}-{busy}", reset, [], reset_replacement, [],
            [bank(busy=busy, initial=initial)]))
        expected.append(([(0x2010f800+4*i, 0x04000000) for i in range(2) if initial[i]], None, sum(bool(v) for v in initial)))

    def execute(label, selected, verdict, max_events=32768):
        request = dict(schema=17, vendor=vendor, replacement=replacement, binding="shared-core",
                       cases=selected, max_events=max_events)
        identity = call(label, ["compare", "--request", doc(label, request)])["run"]["execution"]
        evidence = call(label+"-evidence", ["execution", "--id", identity])
        assert evidence["summary"]["manifest"]["verdict"] == verdict
        return identity, evidence

    artifacts = []
    # Independent cold cases are batched below the native 64-KiB request bound;
    # every declared case and expectation is still executed and retained.
    for start in range(0, len(cases), 10):
        label = f"transport-{start//10}"
        identity, evidence = execute(label, cases[start:start+10], "MATCH")
        rows = [r["value"] for r in evidence["records"]]
        for i, (writes, returned, commands) in enumerate(expected[start:start+10]):
            for side in (False, True):
                selected = [r for r in rows if r.get("case") == i and r.get("replacement") == side]
                stop = next(r["stop"] for r in selected if r["kind"] == "outcome")
                assert stop["kind"] == "returned", (i, side, stop)
                if returned is not None:
                    assert stop["low"] == returned, (i, side, stop)
                events = [r["event"] for r in selected if r["kind"] == "event"]
                assert all(e["kind"] in ("read", "write") and e["width"] == 4 for e in events)
                assert [(e["address"], e["value"]) for e in events if e["kind"] == "write"] == writes, (i, side, events)
                for r in selected:
                    if r["kind"] == "model":
                        model = r["observation"]
                        assert model["status"] == "complete" and model["closed"] and model["issue"] is None
                        if model["commands"] is not None:
                            assert model["commands"]["pending"] == 0
                            assert model["commands"]["issued"] == commands
        artifacts.append((label, identity, evidence))
    # No sample fallback: identical exhausted environments are still incomplete.
    exhausted = copy.deepcopy(cases[13])
    exhausted["name"] = "exhausted-samples"
    for side in ("vendor", "replacement"):
        for cell in exhausted[side]["models"][1]["behavior"]["cells"]:
            if cell["reads"] is not None:
                cell["reads"] = []
    identity, evidence = execute("transport-exhausted", [exhausted], "INCOMPLETE")
    assert all(r["value"]["observation"]["issue"] == {"kind": "exhausted-reads"}
               for r in evidence["records"] if r["value"]["kind"] == "model" and r["value"]["observation"]["id"] == "analog")
    artifacts.append(("transport-exhausted", identity, evidence))
    # The captured masked-write ABI permits out-of-field bits to spill; the
    # production field update clips them. Preserve that known difference.
    wide = paired("out-of-field-value", symbol(1, "phy_i2c_writeReg_Mask")["value"], [0x66, 0, 4, 3, 2, 7],
                  transfer, [3, 7, 32], [controls(), bank(0x0466, 0xa6, True)])
    identity, evidence = execute("transport-field-domain", [wide], "DIFF")
    last = [r["value"]["event"]["value"] for r in evidence["records"] if r["value"]["kind"] == "event"
            and r["value"]["event"]["kind"] == "write" and r["value"]["event"]["address"] == 0x2010f800]
    assert last == [0x04000466, 0x05be0466, 0x04000466, 0x05ae0466], last
    artifacts.append(("transport-field-domain", identity, evidence))
    # Finite supplied completion edges expire before a delayed read becomes ready.
    timeout = paired("edge-schedule-exhausted", symbol(1, "phy_i2c_readReg")["value"], [0x66, 0, 4],
                     transfer, [0, 0, 2], [controls(), bank(0x0466, 0xa6, True, 4)])
    identity, evidence = execute("transport-edge-timeout", [timeout], "INCOMPLETE")
    stop = next(r["value"]["stop"] for r in evidence["records"] if r["value"]["kind"] == "outcome" and r["value"]["replacement"])
    assert stop["kind"] == "returned" and stop["low"] == 0x10001
    artifacts.append(("transport-edge-timeout", identity, evidence))
    # The actual shipping reset helper has a finite 10,000-observation bound;
    # ROM keeps polling. Exclude void returns but keep the pending model obligation.
    timeout = paired("production-reset-timeout", reset, [], reset_replacement, [],
                     [bank(busy=10001, initial=(1, 0))])
    identity, evidence = execute("transport-reset-timeout", [timeout], "INCOMPLETE")
    stop = next(r["value"]["stop"] for r in evidence["records"] if r["value"]["kind"] == "outcome" and r["value"]["replacement"])
    assert stop["kind"] == "returned" and stop["low"] == 0x10001
    artifacts.append(("transport-reset-timeout", identity, evidence))
    return artifacts
