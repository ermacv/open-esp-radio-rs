"""Captured PBus/DCODE children and compiled-production failure containment."""
import copy


def exercise(call, doc, symbol, roots, vendor, replacement, parameter, production_delay):
    from phy_i2c import invocation, region, case, selection

    shim = symbol(2, "open_phy_trace_i2c_entry")["value"]
    pbus = symbol(1, "phy_pbus_clear_reg")["value"]
    production = symbol(2, "open_phy_calibration_trace_pbus_clear")["value"]
    status = 0x20100890

    def invoke(target, models, settle, side):
        result = invocation(shim, [target, 0x3fff4000], [region(0x3fff4000, 32, fill=0)], models)
        if settle:
            result["calls"] = [dict(id="settle-delay", applicability="declared delay ABI; requested microseconds only",
                lifetime="phase", binding=dict(address=production_delay if side else symbol(1, "ets_delay_us")["value"],
                boundary="captured-code", allow_tail=True), argument_words=1,
                responses=[dict(return_words=[0, 0], outputs=[], allocation=None,
                                delay_micros=dict(kind="argument", word=0)) for _ in range(2)])]
        return result

    def models(initial, settle, busy):
        return [dict(id="pbus-registers", applicability="explicit retained PBus and work-mode registers",
                     lifetime="phase", behavior=dict(kind="register-bank", cells=[
                         dict(address=a, width=4, value=v) for a, v in
                         [(0x20100884, initial), (0x2010088c, initial),
                          (0x20109c18, 2 if settle else 0), (0x2010702c, initial)]])),
                dict(id="pbus-status", applicability="twelve finite command-completion scripts",
                     lifetime="phase", behavior=dict(kind="sequence-read", address=status, width=4,
                     runs=[run for _ in range(12) for run in
                           ([dict(value=0x80000000, count=busy)] if busy else []) + [dict(value=0, count=1)]]))]

    def expected_writes(initial, settle):
        # Independent instruction reading of captured force-mode/force-test
        # and the twelve call arguments in phy_pbus_clear_reg.
        second = initial & 0xfbffffff
        first = initial | 1
        writes = [(0x2010088c, second), (0x20100884, first)]
        for channel, mode, data in [(4, 1, 0), (4, 2, 0), (5, 1, 0), (5, 2, 0),
                                  (0, 1, 0), (0, 2, 0), (1, 1, 0), (1, 2, 0),
                                  (2, 1, 256), (3, 1, 256), (2, 2, 256), (3, 2, 256)]:
            issued = (first & 0xfffe0001) | ((channel * 4 | mode << 15 | data << 6) & 0x1fffc) | 2
            first = issued & 0xfffffffd
            writes.extend([(0x20100884, issued), (0x20100884, first)])
        writes.extend([(0x20100884, first & 0xfffffffe), (0x2010088c, second | 0x04000000)])
        if settle:
            pulse = (initial & 0xffffff) | 0x32000000
            writes.extend([(0x2010702c, pulse), (0x2010702c, pulse | 0x800000),
                           (0x2010702c, pulse & 0xff7fffff)])
        return writes

    def execute(label, cases, targets, verdict):
        left, right = targets
        request = dict(schema=17, vendor=left, replacement=right, binding="shared-core" if right is not None else None, cases=cases, max_events=32768)
        command = "compare" if right is not None else "execute"
        identity = call(label, [command, "--request", doc(label, request)])["run"]["execution"]
        evidence = call(label+"-evidence", ["execution", "--id", identity])
        assert evidence["summary"]["manifest"]["verdict"] == verdict, (label, evidence["summary"]["manifest"]["verdict"])
        return identity, evidence

    artifacts = []
    for fill in (0x5a, 0xa5):
        cases, expected = [], []
        for initial in (0, 0xa5a55a58, 0x5a5aa5a4):
            for settle in (False, True):
                for busy in (0, 2):
                    m = models(initial, settle, busy)
                    cases.append(case(f"pbus-{initial:x}-{settle}-{busy}-{fill}",
                        invoke(pbus, m, settle, False), invoke(production, m, settle, True)))
                    expected.append((expected_writes(initial, settle), settle, busy))
        targets = (copy.deepcopy(vendor), copy.deepcopy(replacement))
        for target in targets:
            target["stack"]["fill"] = fill
        for start in range(0, len(cases), 4):
            label = f"pbus-{fill}-{start//4}"
            identity, evidence = execute(label, cases[start:start+4], targets, "MATCH")
            rows = [r["value"] for r in evidence["records"]]
            for i, (writes, settle, busy) in enumerate(expected[start:start+4]):
                for side in (False, True):
                    selected = [r for r in rows if r.get("case") == i and r.get("replacement") == side]
                    outcome = next(r["stop"] for r in selected if r["kind"] == "outcome")
                    assert outcome["kind"] == "returned", outcome
                    if side:
                        assert outcome["low"] == 0, outcome
                    events = [r["event"] for r in selected if r["kind"] == "event"]
                    assert [(e["address"], e["value"]) for e in events if e["kind"] == "write"] == writes, (i, side, events, writes)
                    assert [e["value"] for e in events if e["kind"] == "delay-micros"] == ([1, 2] if settle else [])
                    assert sum(e["kind"] == "read" and e["address"] == status for e in events) == 12*(busy+1)
                    assert all(r["observation"]["status"] == "complete" for r in selected if r["kind"] in ("model", "call-model"))
            artifacts.append((label, identity, evidence))

    # Real production timeout after zero, five or eleven completed commands.
    # Twenty thousand explicit busy responses exceed its own polling bound.
    for completed in (0, 5, 11):
        m = models(0, True, 0)
        m[1]["behavior"]["runs"] = ([dict(value=0, count=completed)] if completed else []) + [dict(value=0x80000000, count=20_000)]
        v = invoke(production, m, False, True)
        row = case(f"pbus-timeout-{completed}", v, None)
        row["relation"] = None
        label = row["name"]
        identity, evidence = execute(label, [row], (replacement, None), None)
        rows = [r["value"] for r in evidence["records"]]
        outcome = next(r["stop"] for r in rows if r["kind"] == "outcome")
        assert outcome["kind"] == "returned" and outcome["low"] == 2, outcome
        events = [r["event"] for r in rows if r["kind"] == "event"]
        stuck = next(i for i, e in enumerate(events) if e["kind"] == "read" and e["address"] == status and e["value"] == 0x80000000)
        assert all(e["kind"] == "read" and e["address"] == status for e in events[stuck:])
        model = next(r["observation"] for r in rows if r["kind"] == "model" and r["observation"]["id"] == "pbus-status")
        assert model["issue"] is None and model["remaining_reads"] > 0 and model["status"] == "incomplete"
        assert all(e["address"] not in (0x20109c18, 0x2010702c) for e in events)
        artifacts.append((label, identity, evidence))
    # Setup executes the captured ROM memcpy; image bytes are not overwritten
    # by a side channel. Warm phases retain that initialized parameter buffer.
    def words(values):
        return [b for value in values for b in value.to_bytes(4, "little")]

    callbacks = [roots[name] for name in ("phy_i2c_enter_critical", "phy_i2c_exit_critical",
                                         "phy_get_i2c_read_mask_new", "phy_get_i2c_hostid_new")]
    dcode = symbol(1, "phy_dcode_cal_init")["value"]
    sequence = symbol(2, "open_phy_trace_two_void_entries")["value"]
    compiled = symbol(2, "open_phy_calibration_trace_dcode")["value"]
    destination = 0x3fff6000

    def enter(target, arguments, memory=(), models=(), observe=()):
        args = region(0x3fff4000, 32, words(list(arguments) + [0]*(8-len(arguments))))
        return invocation(shim, [target, 0x3fff4000], [args] + list(memory), models, observe)

    def setup(crystal, side):
        data = [0]*516
        data[79] = crystal
        data[417:425] = [0xa5]*8
        memory = [region(0x3fff5000, 516, data)]
        if side:
            memory.append(region(destination, 516, lifetime="session"))
        return enter(symbol(1, "memcpy")["value"],
                     [destination if side else parameter, 0x3fff5000, 516], memory)

    def dcode_models(fill, busy):
        result = models(0, False, 0)
        result.append(dict(id="dcode-registers", applicability="explicit frequency, NRX and transport control values",
            lifetime="phase", behavior=dict(kind="register-bank", cells=[
                dict(address=a, width=4, value=v) for a, v in
                [(0x2010001c, 0x41280055), (0x20107848, 0x1655a55a),
                 (0x2010f81c, 0), (0x2010f820, 0)]])))
        result.append(dict(id="channel-ready", applicability="four bounded frequency readiness scripts",
            lifetime="phase", behavior=dict(kind="sequence-read", address=0x20100028, width=4,
            runs=[run for _ in range(4) for run in
                  ([dict(value=0x25824e58, count=busy)] if busy else []) + [dict(value=0x25824f58, count=1)]])))
        result.append(dict(id="ckgen", applicability="shared retained CKGEN bytes and eight declared six-bit samples",
            lifetime="phase", behavior=dict(kind="command-bank", selector_mask=0xffff,
                data_mask=0xff0000, read_command=0x04000000, write_command=0x05000000,
                busy_mask=0x02000000, reset_command=0x04000000,
                ports=[dict(address=0x2010f800+4*i, initial=0, initial_busy_reads=0, busy_reads=busy) for i in range(2)],
                cells=sorted([dict(selector=(r<<8)|0x62, initial=fill, reads=None) for r in (4,19,20)] +
                      [dict(selector=0x1162, initial=0, reads=[0xc0,0xdf,0xe0,0xff]),
                       dict(selector=0x1262, initial=0, reads=[0xff,0xe0,0xdf,0xc0])], key=lambda cell: cell["selector"]))) )
        return result

    def measured(crystal, fill, busy, side):
        memory = [region(0x2f07fc3c, 4, words([0x3fff3000])),
                  region(0x3fff3000, 16, words(callbacks))]
        if not side:
            memory.append(region(0x2f07fc40, 4, words([parameter])))
        output = (destination if side else parameter) + 417
        result = enter(compiled if side else sequence,
                       [crystal, output] if side else [pbus, dcode], memory,
                       dcode_models(fill, busy), [selection(output, 8)])
        result["calls"] = [dict(id="frequency-delay", applicability="declared delay ABI; requested microseconds only",
            lifetime="phase", binding=dict(address=production_delay if side else symbol(1, "ets_delay_us")["value"],
                boundary="captured-code", allow_tail=True), argument_words=1,
            responses=[dict(return_words=[0,0], outputs=[], allocation=None,
                delay_micros=dict(kind="argument", word=0)) for _ in range(8)])]
        return result

    for crystal in range(4):
        for fill in (0x5a, 0xa5):
            for busy in (0, 2):
                label = f"dcode-{crystal}-{fill}-{busy}"
                targets = (copy.deepcopy(vendor), copy.deepcopy(replacement))
                for target in targets:
                    target["stack"]["fill"] = fill
                rows = [case("initialize-parameters", setup(crystal, False), setup(crystal, True)),
                        case(label, measured(crystal, fill, busy, False), measured(crystal, fill, busy, True),
                             reset="warm", memory=True)]
                # Preserve the raw event difference: production performs an
                # additional busy precheck before each read command. The native
                # selected relation covers all writes, fences, delays and final
                # bytes. The independent check below also compares every other
                # read, without pretending the native MATCH includes those reads.
                if crystal == 0 and fill == 0x5a and busy == 0:
                    raw_id, raw = execute("dcode-raw-polling-difference", rows, targets, "DIFF")
                    artifacts.append(("dcode-raw-polling-difference", raw_id, raw))
                rows[1]["relation"]["events"]["mmio_read"] = False
                identity, evidence = execute(label, rows, targets, "MATCH")
                records = [r["value"] for r in evidence["records"] if r["value"].get("case") == 1]
                for side in (False, True):
                    selected = [r for r in records if r.get("replacement") == side]
                    outcome = next(r["stop"] for r in selected if r["kind"] == "outcome")
                    assert outcome["kind"] == "returned", (label, side, outcome)
                    if side:
                        assert outcome["low"] == 0, (label, outcome)
                    chunks = [r["chunk"] for r in selected if r["kind"] == "final-memory"]
                    assert len(chunks) == 1 and chunks[0]["known"] == 255 and chunks[0]["available"] == 255
                    assert [b for c in chunks for b in c["bytes"][:c["length"]]] == [0,63,31,32,32,31,63,0], (label, side, chunks)
                    events = [r["event"] for r in selected if r["kind"] == "event"]
                    assert [e["value"] for e in events if e["kind"] == "delay-micros"] == [1,10]*4
                    writes = [(e["address"], e["value"]) for e in events if e["kind"] == "write"]
                    assert writes[:28] == expected_writes(0, False), label
                    # ROM's channel table is [1,5,10,14]: 2412,2432,2457,2484 MHz.
                    frequency, freq_writes, nrx_writes = 0x41280055, [], []
                    for mhz in (2412,2432,2457,2484):
                        frequency = (frequency & 0xffffff00) | (mhz-2400)
                        freq_writes.extend([frequency, frequency | 0x80000, frequency & 0xfff7ffff])
                        frequency &= 0xfff7ffff
                        nrx_writes.append(0x16000000 | ((80 << 22)//mhz))
                    assert [v for a,v in writes if a == 0x2010001c] == freq_writes, label
                    assert [v for a,v in writes if a == 0x20107848] == nrx_writes, label
                    retained = {r: fill for r in (4,19,20)}
                    commands = []
                    for _ in range(4):
                        for register, mask, value in [(19,0x40,0),(20,0x40,0),(4,0x80,0),(4,0x80,0x80)]:
                            selector = (register << 8) | 0x62
                            retained[register] = (retained[register] & ~mask) | value
                            commands.extend([0x04000000 | selector, 0x05000000 | retained[register]<<16 | selector])
                        commands.extend([0x04001162,0x04001262])
                    assert [(a,v) for a,v in writes if a in (0x2010f800,0x2010f804)] == [(0x2010f804,v) for v in commands], label
                    ckgen = next(r["observation"] for r in selected if r["kind"] == "model" and r["observation"]["id"] == "ckgen")
                    assert ckgen["commands"] == dict(issued=40, completed=40, resets=0, aborted=0, pending=0, scripted_reads=8), label
                    assert all(r["observation"]["status"] == "complete" for r in selected if r["kind"] == "model")
                def required_events(side):
                    return [r["event"] for r in records if r["kind"] == "event" and r["replacement"] == side
                            and r["event"]["kind"] in ("read", "write", "fence", "delay-micros")
                            and not (r["event"]["kind"] == "read" and r["event"]["address"] in (0x2010f800,0x2010f804))]
                assert required_events(False) == required_events(True), label
                artifacts.append((label, identity, evidence))
    for ready, busy, expected_commands in [(False,0,0),(True,65535,1),(True,6000,2)]:
        label = f"dcode-failure-{ready}-{busy}"
        target = copy.deepcopy(replacement)
        target["stack"]["fill"] = 0xa5
        initial = setup(0, True)
        failed = measured(0, 0x5a, busy, True)
        failed["calls"][0]["responses"] = failed["calls"][0]["responses"][:2]
        for model in failed["models"]:
            if model["id"] == "channel-ready":
                model["behavior"] = dict(kind="constant-read",address=0x20100028,width=4,value=0x100 if ready else 0)
            if model["id"] == "ckgen":
                model["behavior"]["cells"] = [c for c in model["behavior"]["cells"] if c["reads"] is None]
        rows = [case("initialize-parameters", initial, None), case(label, failed, None, reset="warm")]
        for row in rows:
            row["relation"] = None
        identity, evidence = execute(label, rows, (target,None), None)
        records = [r["value"] for r in evidence["records"] if r["value"].get("case") == 1]
        outcome = next(r["stop"] for r in records if r["kind"] == "outcome")
        assert outcome["kind"] == "returned" and outcome["low"] == (5 if ready else 7), (label,outcome)
        chunks = [r["chunk"] for r in records if r["kind"] == "final-memory"]
        assert len(chunks) == 1 and chunks[0]["known"] == 255 and chunks[0]["available"] == 255
        assert chunks[0]["bytes"][:8] == [0xa5]*8, (label,chunks)
        events = [r["event"] for r in records if r["kind"] == "event"]
        assert sum(e["kind"] == "write" and e["address"] in (0x2010f800,0x2010f804) for e in events) == expected_commands
        model = next(r["observation"] for r in records if r["kind"] == "model" and r["observation"]["id"] == "ckgen")
        assert model["issue"] is None and model["commands"]["issued"] == expected_commands
        assert model["commands"]["pending"] == int(ready)
        if not ready:
            assert not any(e["kind"] == "write" and e["address"] == 0x20107848 for e in events)
        artifacts.append((label,identity,evidence))

    original = [case("initialize-parameters",setup(0,False),setup(0,True)),
                case("measured",measured(0,0x5a,0,False),measured(0,0x5a,0,True),reset="warm",memory=True)]
    original[1]["relation"]["events"]["mmio_read"] = False
    fault_targets = (copy.deepcopy(vendor), copy.deepcopy(replacement))
    for target in fault_targets:
        target["stack"]["fill"] = 0x5a
    changed = copy.deepcopy(original)
    changed[1]["replacement"]["models"][-1]["behavior"]["cells"][1]["reads"][0] = 0xc1
    identity, evidence = execute("dcode-changed-sample",changed,fault_targets,"DIFF")
    assert any(r["value"]["kind"] == "comparison" and r["value"]["case"] == 1
               and r["value"]["result"]["difference"]["kind"] == "memory" for r in evidence["records"])
    artifacts.append(("dcode-changed-sample",identity,evidence))
    unknown = copy.deepcopy(original)
    unknown[1]["vendor"]["memory"][-1]["seed"]["bytes"] = []
    identity, evidence = execute("dcode-unknown-parameters",unknown,fault_targets,"INCOMPLETE")
    stop = next(r["value"]["stop"] for r in evidence["records"] if r["value"]["kind"] == "outcome" and r["value"]["case"] == 1 and not r["value"]["replacement"])
    assert stop["reason"] == dict(kind="memory",address=0x2f07fc40,access="read"), stop
    artifacts.append(("dcode-unknown-parameters",identity,evidence))
    limited = dict(schema=17,vendor=fault_targets[0],replacement=fault_targets[1],binding="shared-core",cases=original,max_events=1)
    failed = call("dcode-capacity",["compare","--request",doc("dcode-capacity",limited)],expected=1)
    assert failed["run"].get("execution") is None and failed["run"].get("publication") is None
    assert failed["run"]["error"]["code"] == "resource-limited"
    return artifacts
