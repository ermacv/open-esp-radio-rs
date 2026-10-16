# HIL scenario model

`oer-hil-model` defines what a HIL scenario is, independently of how a run
executes it ([#761](https://github.com/ermacv/open-esp-radio-rs/issues/761)).
It is a `formats` package, so the qualification evaluator may link it to
recompute verdicts; execution lives in
[`oer-hil-executor`](../host/executor/README.md).

| Module | Owns |
| --- | --- |
| `spec` | `Spec`: the scenario's stable `ScenarioId`, its one `Purpose`, the `Subject` contract it exercises, typed `Params`, `Criteria` and `Trace`, its `Resources`, `TargetCapabilities` and `Estimate`, and the pure `verdict(trace, criteria)`; `Instance`, a named parameterisation that replaces copied scenarios; reading a requirement's criteria and an instance's parameters with the scenario's own types |
| `purpose` | `Conformance`, `Interop`, `Measurement`, or `Investigation { issue }`; there is no protocol purpose, because protocol behaviour is proven on the host |
| `verdict` | `Verdict` and `Finding`; `Agreement`, a recomputed verdict beside the recorded one, which is never overwritten |
| `identity` | A scenario instance's identity: SHA-256 of its ID, instance name, parameters and criteria |
| `wait` | `Wait`: a contract bound the image reports (`ReportedBounds`) times a `Factor`, a protocol interval times a count, or a span measured on the stand; `Wait::resolve` is the one place host HIL code turns a wait into a `Duration` |
| `profile` | `StandProfile`: measured spans, each the maximum of its samples (which covers the 99.9th percentile at 95 % once there are 2 995), its margin and why it was chosen, the runs it comes from (`RunRef`: ID and SHA-256), and a provisional label while samples are too few |
| `repetitions` | Samples for a quantile bound, Clopper–Pearson trials and upper bound for a loss rate the protocol admits, and repetitions per arm for a performance comparison from the measured variation |
| `resources` | Boards by role (`Dut`, `Peer`, `Observer`) and capability (`LpClock`, U.FL), fixtures, the `AirFootprint` the arbiter grants, and `Resources::unmet`, the plan-time refusal of what the stand lacks |
| `capabilities` | `TargetCapabilities`: the product composition (the same crate as the product's) and the agent capabilities an image must serve; their union over a tier |
| `estimate`, `tier` | An instance's derived time from its waits and repetitions; the tiers (smoke, stage gate, full, nightly) and their budgets |
| `time` | `TimeFit`: the affine map from a source's clock to the host's, fitted from request/response exchanges, with an error bound; one fit per boot |

Scenario types live in each block's scenario crate. A conformance scenario
follows its port's harness template: the port's conformance crate holds
`exercise(case) -> Trace`, the trace types with a `wire` feature, and a
public pure `verify(case, trace, bounds)` (#761 Q3). The executor's
[`harness_template` test](../host/executor/tests/harness_template.rs) shows
the shape on a toy port.
