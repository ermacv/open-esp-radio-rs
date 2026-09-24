"""Compiled harness call-boundary acceptance; no hardware timing claim."""
import copy
from harness import case, compare, invocation


def exercise(call, doc, symbol, probes, replacement):
    shim = probes.entry("open_phy_trace_seeded_entry")["value"]
    owned = probes.entry("open_phy_trace_owned_software_frequency_control")["value"]
    ordinary = probes.entry("open_phy_trace_dis_hw_set_freq")["value"]
    delay_entry = symbol(1, "__call_ets_delay_us")["value"]
    delay = symbol(1, "ets_delay_us")["value"]
    # This retained cell is a concrete input for the captured frequency-control
    # RMW. Acceptance below concerns call boundaries, arguments and completion.
    registers = dict(id="frequency-control", applicability="explicit boundary-test RMW input",
                     lifetime="phase", behavior=dict(kind="register-bank", cells=[
                         dict(address=0x2010001c, width=4, value=0)]))
    phase = invocation(shim, probes.arguments("open_phy_trace_seeded_entry",
                       dict(entry=owned, argument=0)), models=[registers])
    phase["observe_calls"] = dict(include_tail=True, argument_words=1, overrides=[])
    phase["calls"] = [dict(id="delay", applicability="explicit ROM delay interception",
                           lifetime="phase", binding=dict(address=delay, boundary="captured-code",
                           allow_tail=True), argument_words=1, responses=[dict(
                           return_words=[None, None], outputs=[], allocation=None,
                           delay_micros=dict(kind="argument", word=0))])]
    row = case("ordinary-delay-tail", phase, copy.deepcopy(phase))
    row["relation"]["calls"] = True
    identity, evidence = compare(call, doc, "harness-edges", replacement, replacement,
                                 [row], "MATCH", 1024)
    for side in (False, True):
        records = [r["value"] for r in evidence["records"]
                   if r["value"].get("replacement") == side]
        outcome = next(r["stop"] for r in records if r["kind"] == "outcome")
        assert outcome["kind"] == "returned", outcome
        events = [r["event"] for r in records if r["kind"] == "event"]
        transfers = [e for e in events if e["kind"] == "call-transfer"]
        assert any(e["target"] == ordinary and not e["tail"] for e in transfers)
        assert any(e["target"] == delay_entry and not e["tail"] for e in transfers)
        assert any(e["target"] == delay and e["tail"] for e in transfers)
        assert [e["value"] for e in events if e["kind"] == "delay-micros"] == [2]
        assert all(r["observation"]["status"] == "complete" for r in records
                   if r["kind"] in ("model", "call-model"))
    return [("harness-edges", identity, evidence)]
