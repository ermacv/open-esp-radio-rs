"""Captured RFPLL search and frequency maintenance against compiled production."""
import copy

# Independently read candidate domains and signed outputs of the pinned archive.
# These finite expectations are never an execution model or replacement body.
SEARCH = [
    ("all-accepted",100,[0]*20,list(range(100,90,-1))+list(range(101,111)),100),
    ("lowest-initial-cap",0,[0]*20,list(range(0,-10,-1))+list(range(1,11)),0),
    ("highest-initial-cap",511,[0]*20,list(range(511,501,-1))+list(range(512,522)),511),
    ("no-accepted",100,[3]*20,list(range(100,90,-1))+list(range(101,111)),100),
    ("positive",100,[1]*2+[0]*10,[100,99]+list(range(101,111)),105),
    ("negative",300,[0]*10+[2]*2,list(range(300,290,-1))+[301,302],295),
    ("nonconsecutive",100,[1,0,1,2,3,2],[100,99,98,101,102,103],99),
    ("signed-wrap",0,[1,0,1,2,2],[0,-1,-2,1,2],-1),
    ("opposite-boundaries",100,[2]*10+[1]*10,list(range(100,90,-1))+list(range(101,111)),100),
]


def exercise(call, doc, symbol, roots, vendor, replacement, parameter):
    from phy_i2c import invocation, region, case

    shim = symbol(2,"open_phy_trace_i2c_entry")["value"]
    callbacks = [roots[n] for n in ("phy_i2c_enter_critical","phy_i2c_exit_critical",
                                   "phy_get_i2c_read_mask_new","phy_get_i2c_hostid_new")]

    def words(values):
        return [b for value in values for b in value.to_bytes(4,"little")]

    def enter(target, arguments, models, memory=()):
        return invocation(shim,[target,0x3fff4000],
            [region(0x3fff4000,32,words(list(arguments)+[0]*(8-len(arguments)))),
             region(0x2f07fc3c,4,words([0x3fff3000])),
             region(0x3fff3000,16,words(callbacks))]+list(memory),models)

    def bank(cap,statuses,busy):
        cells = [dict(selector=(r<<8)|0x62,initial=v,reads=None) for r,v in
                 [(1,cap&255),(2,0x95),(5,cap&255),(7,0xc2|((cap>>8)<<2)),(11,0x15)]]
        cells.append(dict(selector=0x0c62,initial=0,reads=[0xa3|(s<<2) for s in statuses]))
        return [dict(id="rfpll",applicability="explicit capacitor bytes and finite lock-status samples; no search algorithm",
            lifetime="phase",behavior=dict(kind="command-bank",selector_mask=0xffff,data_mask=0xff0000,
                read_command=0x04000000,write_command=0x05000000,busy_mask=0x02000000,reset_command=0x04000000,
                ports=[dict(address=0x2010f800+4*i,initial=0,initial_busy_reads=0,busy_reads=busy) for i in range(2)],cells=cells)),
            dict(id="transport-controls",applicability="explicit retained host-map/read-mask controls",
                lifetime="phase",behavior=dict(kind="register-bank",cells=[
                    dict(address=0x2010f81c,width=4,value=0),dict(address=0x2010f820,width=4,value=0)]))]

    def delays(side,count):
        return [dict(id="requested-delay",applicability="declared delay ABI; requested microseconds only",
            lifetime="phase",binding=dict(address=symbol(2 if side else 1,"open_phy_trace_delay_event" if side else "ets_delay_us")["value"],
                boundary="captured-code",allow_tail=True),argument_words=1,
            responses=[dict(return_words=[0,0],outputs=[],allocation=None,
                delay_micros=dict(kind="argument",word=0)) for _ in range(count)])]

    def expected_commands(candidates,selected):
        result = [0x04000562,0x04000762,0x04000b62,0x05550b62]
        high = 0x95
        for i,candidate in enumerate(candidates+[selected]):
            programmed = max(0,candidate)
            high = ((high & ~0x40) | ((programmed>>8)<<6)) & 255
            result.extend([0x05000162|((programmed&255)<<16),0x04000262,0x05000262|(high<<16)])
            if i < len(candidates):
                result.append(0x04000c62)
        return [(0x2010f804,value) for value in result]

    def execute(label,rows,targets,verdict,maximum=32768):
        left,right = targets
        request = dict(schema=17,vendor=left,replacement=right,binding="shared-core" if right else None,
                       cases=rows,max_events=maximum)
        identity = call(label,["compare" if right else "execute","--request",doc(label,request)])["run"]["execution"]
        evidence = call(label+"-evidence",["execution","--id",identity])
        assert evidence["summary"]["manifest"]["verdict"] == verdict, (label,evidence["summary"]["manifest"]["verdict"])
        return identity,evidence

    artifacts = []
    for name,cap,statuses,candidates,selected in SEARCH:
        baseline = None
        for fill in (0x5a,0xa5):
            for busy in (0,1):
                label = f"rfpll-search-{name}-{fill}-{busy}"
                targets = (copy.deepcopy(vendor),copy.deepcopy(replacement))
                for target in targets:
                    target["stack"]["fill"] = fill
                inputs = []
                for side in (False,True):
                    v = enter(symbol(2,"open_phy_rfpll_trace_search")["value"] if side else roots["phy_rfpll_cap_init_cal_new"],
                              [],bank(cap,statuses,busy))
                    v["calls"] = delays(side,len(statuses))
                    inputs.append(v)
                row = case(label,*inputs)
                row["relation"]["returns"]["low"] = True
                row["relation"]["events"]["mmio_read"] = False
                identity,evidence = execute(label,[row],targets,"MATCH")
                records = [r["value"] for r in evidence["records"]]
                for side in (False,True):
                    chosen = [r for r in records if r.get("replacement") == side]
                    stop = next(r["stop"] for r in chosen if r["kind"] == "outcome")
                    assert stop["kind"] == "returned" and stop["low"] == ((selected-cap)&0xffffffff), (label,side,stop)
                    events = [r["event"] for r in chosen if r["kind"] == "event"]
                    commands = [(e["address"],e["value"]) for e in events if e["kind"] == "write" and e["address"] in (0x2010f800,0x2010f804)]
                    assert commands == expected_commands(candidates,selected), (label,side,commands)
                    waits = [e["value"] for e in events if e["kind"] == "delay-micros"]
                    assert waits == [5]*len(statuses), (label,side,waits)
                    facts = (stop["low"],commands,waits)
                    assert baseline is None or baseline == facts, (label,side)
                    baseline = facts
                    for record in chosen:
                        if record["kind"] in ("model","call-model"):
                            assert record["observation"]["status"] == "complete", (label,side,record)
                artifacts.append((label,identity,evidence))
    def sequence(address, values, name):
        return dict(id=name, applicability="finite caller-supplied peripheral observations",
            lifetime="phase", behavior=dict(kind="sequence-read", address=address, width=4,
                runs=[dict(value=v,count=1) for v in values]))

    def maintenance_models(statuses, initial, contents, busy=0):
        result = bank(100,statuses,busy)
        cells = [(0x20100028,initial)]
        if contents is not None:
            cells += [(0x2010001c,0x41280055),(0x20100020,0xa5a45678),
                      (0x2010002c,0),(0x20100030,0x12345678)]
            result.append(sequence(0x20100040,contents,"frequency-memory"))
        else:
            result.append(sequence(0x20100030,[0x12345678],"i2c-number"))
        result.append(dict(id="frequency-control",applicability="explicit retained frequency control words",
            lifetime="phase",behavior=dict(kind="register-bank",cells=[
                dict(address=a,width=4,value=v) for a,v in sorted(cells)])))
        result.append(sequence(0x2010d800,[0x98765432],"sdm"))
        return result

    def setup(side):
        data = [0]*516
        data[:2] = [100,0]
        target = 0x3fff6000 if side else parameter
        memory = [region(0x3fff5000,516,data)]
        if side:
            memory.append(region(target,516,lifetime="session"))
        return enter(symbol(1,"memcpy")["value"],[target,0x3fff5000,516],[],memory)

    def maintain(side,statuses,initial,contents,channel,busy=0,diagnostics=0):
        memory = [region(0x2f07fc40,4,words([0x3fff0000])),
                  region(0x3fff0000,286,[0]*284+list(channel.to_bytes(2,"little")))]
        v = enter(symbol(2,"open_phy_rfpll_trace_maintain")["value"] if side else roots["phy_rfpll_cap_track_new"],
                  [channel] if side else [diagnostics],maintenance_models(statuses,initial,contents,busy),memory)
        v["calls"] = delays(side,len(statuses)+1)
        return v

    def envelope(events):
        return [e for e in events if e["kind"] in ("read","write","fence","delay-micros")
            and not (e["kind"] == "read" and 0x2010f800 <= e["address"] < 0x2010f824)
            and not (e["kind"] == "write" and e["address"] in (0x2010f81c,0x2010f820))]

    def frequency_expectation(initial, contents, delta, channel):
        # Independent instruction-derived retained-word transaction oracle. It
        # does not feed execution: the device receives only finite input words.
        regs = {0x1c:0x41280055,0x20:0xa5a45678,0x28:initial,0x30:0x12345678}
        result = []
        def read(offset):
            result.append(("read",0x20100000+offset,regs[offset]))
        def write(offset,value):
            regs[offset] = value & 0xffffffff
            result.append(("write",0x20100000+offset,regs[offset]))
        read(0x28)
        write(0x28,(initial&~3)|2)
        result += [("read",0x2010d800,0x98765432)]
        read(0x30)
        if contents is not None:
            for i,word in enumerate(contents):
                address = 0x20+7*i
                read(0x1c)
                write(0x1c,(regs[0x1c]&~0x7ff00)|(address<<8))
                read(0x30)
                write(0x30,(regs[0x30]&~3)|2)
                read(0x20)
                write(0x20,regs[0x20]|0x10000)
                read(0x20)
                write(0x20,regs[0x20]&~0x10000)
                result.append(("read",0x20100040,word))
                corrected = (((word&255)|((word>>6)&0x100))+delta)&0xffff
                signed = corrected if corrected < 32768 else corrected-65536
                encoded = (corrected&255)|(word&0xbf00)|((signed>>8)<<14)|0x03000000
                read(0x1c)
                write(0x1c,(regs[0x1c]&~0x7ff00)|(address<<8))
                write(0x2c,encoded)
                read(0x1c)
                write(0x1c,regs[0x1c]|0x100000)
                read(0x1c)
                write(0x1c,regs[0x1c]&~0x100000)
            frequency = {1:2412,13:2472,14:2484,2412:2412,2484:2484}[channel]
            read(0x1c)
            write(0x1c,(regs[0x1c]&~255)|((frequency-0x60)&255))
        read(0x28)
        write(0x28,regs[0x28]&~3)
        return result

    maintenance = [(f"zero-{status}-{initial:x}",[status]*20,0,initial,None,13)
        for status in (0,3) for initial in (0x25824e58,0xa5a55a5b)]
    for name,statuses,delta,boundary in [
        ("positive",[1]*2+[0]*10,5,None),
        ("negative",[0]*10+[2]*2,-5,None),
        ("underflow",[0]*10+[2]*2,-5,0x00aabf00),
        ("overflow",[1]*2+[0]*10,5,0x00aaffff),
    ]:
        contents = [boundary if boundary is not None else 0x00550000|(i<<8)|(100+i) for i in range(85)]
        maintenance += [(name,statuses,delta,0x25824e58,contents,channel) for channel in (1,13,14,2412,2484)]
    for name,statuses,delta,initial,contents,channel in maintenance:
        for fill in (0x5a,0xa5):
            label = f"rfpll-maintain-{name}-{channel}-{fill}"
            targets = (copy.deepcopy(vendor),copy.deepcopy(replacement))
            for target in targets:
                target["stack"]["fill"] = fill
            rows = [case("initialize-parameters",setup(False),setup(True)),
                    case(label,maintain(False,statuses,initial,contents,channel),
                         maintain(True,statuses,initial,contents,channel),reset="warm")]
            rows[1]["relation"]["events"]["mmio_read"] = False
            identity,evidence = execute(label,rows,targets,"MATCH")
            records = [r["value"] for r in evidence["records"] if r["value"].get("case") == 1]
            effects = []
            for side in (False,True):
                chosen = [r for r in records if r.get("replacement") == side]
                stop = next(r["stop"] for r in chosen if r["kind"] == "outcome")
                assert stop["kind"] == "returned", (label,side,stop)
                if side:
                    assert stop["low"] == (delta&0xffffffff), (label,stop)
                events = [r["event"] for r in chosen if r["kind"] == "event"]
                assert [e["value"] for e in events if e["kind"] == "delay-micros"] == [2]+[5]*len(statuses), label
                projected = envelope(events)
                if not side and contents is not None:
                    indices = [i for i,e in enumerate(projected) if e["kind"] == "read" and e["address"] == 0x20100028]
                    assert len(indices) == 3, (label,indices)
                    removed = projected.pop(indices[1])
                    assert removed["value"] == 0x25824e5a, (label,removed)
                effects.append(projected)
                frequency = [(e["kind"],e["address"],e["value"]) for e in projected
                    if e["kind"] in ("read","write") and
                    (0x20100000 <= e["address"] < 0x20100044 or e["address"] == 0x2010d800)]
                assert frequency == frequency_expectation(initial,contents,delta,channel), (label,side,frequency[:30])
                for r in chosen:
                    if r["kind"] in ("model","call-model"):
                        assert r["observation"]["status"] == "complete", (label,side,r)
            assert effects[0] == effects[1], (label,"frequency envelope differs")
            artifacts.append((label,identity,evidence))

    label = "rfpll-maintenance-timeout"
    target = copy.deepcopy(replacement)
    target["stack"]["fill"] = 0x5a
    failed = maintain(True,[],0x25824e58,None,13,busy=65535)
    # No status sample is expected while the first command remains pending.
    failed["models"][0]["behavior"]["cells"] = [c for c in failed["models"][0]["behavior"]["cells"] if c["reads"] is None]
    row = case(label,failed,None)
    row["relation"] = None
    identity,evidence = execute(label,[row],(target,None),None)
    records = [r["value"] for r in evidence["records"]]
    stop = next(r["stop"] for r in records if r["kind"] == "outcome")
    assert stop["kind"] == "returned" and stop["low"] == 0x80000000, stop
    events = [r["event"] for r in records if r["kind"] == "event"]
    assert [(e["address"],e["value"]) for e in events if e["kind"] == "write" and 0x20100000 <= e["address"] < 0x20100044] == [(0x20100028,0x25824e5a)]
    pending = next(r["observation"] for r in records if r["kind"] == "model" and r["observation"]["id"] == "rfpll")
    assert pending["commands"]["pending"] == 1 and pending["status"] == "incomplete", pending
    artifacts.append((label,identity,evidence))
    fault_targets = (copy.deepcopy(vendor),copy.deepcopy(replacement))
    for target in fault_targets:
        target["stack"]["fill"] = 0x5a
    search_inputs = []
    for side in (False,True):
        v = enter(symbol(2,"open_phy_rfpll_trace_search")["value"] if side else roots["phy_rfpll_cap_init_cal_new"],
                  [],bank(100,[0]*20,0))
        v["calls"] = delays(side,20)
        search_inputs.append(v)
    original = [case("rfpll-search",*search_inputs)]
    original[0]["relation"]["returns"]["low"] = True
    original[0]["relation"]["events"]["mmio_read"] = False
    changed = copy.deepcopy(original)
    cells = changed[0]["replacement"]["models"][0]["behavior"]["cells"]
    next(c for c in cells if c["selector"] == 0x0562)["initial"] = 101
    identity,evidence = execute("rfpll-changed-capacitor",changed,fault_targets,"DIFF")
    artifacts.append(("rfpll-changed-capacitor",identity,evidence))
    unknown = copy.deepcopy(original)
    unknown[0]["vendor"]["memory"][1]["seed"]["bytes"] = []
    identity,evidence = execute("rfpll-unknown-callbacks",unknown,fault_targets,"INCOMPLETE")
    stop = next(r["value"]["stop"] for r in evidence["records"] if r["value"]["kind"] == "outcome" and not r["value"]["replacement"])
    assert stop["reason"] == dict(kind="memory",address=0x2f07fc3c,access="read"), stop
    artifacts.append(("rfpll-unknown-callbacks",identity,evidence))
    diagnostics = [case("initialize-parameters",setup(False),setup(True)),
        case("diagnostics-unmapped",maintain(False,[0]*20,0x25824e58,None,13,diagnostics=1),
             maintain(True,[0]*20,0x25824e58,None,13),reset="warm")]
    diagnostics[1]["relation"]["events"]["mmio_read"] = False
    identity,evidence = execute("rfpll-diagnostics-unmapped",diagnostics,fault_targets,"INCOMPLETE")
    stop = next(r["value"]["stop"] for r in evidence["records"] if r["value"]["kind"] == "outcome" and r["value"]["case"] == 1 and not r["value"]["replacement"])
    assert stop["reason"] == dict(kind="memory",address=symbol(4,"phy_printf")["value"],access="fetch"), stop
    artifacts.append(("rfpll-diagnostics-unmapped",identity,evidence))
    raw = copy.deepcopy(original)
    raw[0]["relation"]["events"]["mmio_read"] = True
    identity,evidence = execute("rfpll-raw-polling",raw,fault_targets,"DIFF")
    artifacts.append(("rfpll-raw-polling",identity,evidence))
    limited = dict(schema=17,vendor=fault_targets[0],replacement=fault_targets[1],binding="shared-core",cases=original,max_events=1)
    failed = call("rfpll-capacity",["compare","--request",doc("rfpll-capacity",limited)],expected=1)
    assert failed["run"].get("execution") is None and failed["run"].get("publication") is None
    assert failed["run"]["error"]["code"] == "resource-limited"
    return artifacts
