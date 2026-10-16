# HIL scenario model

`oer-hil-model` defines what a HIL scenario is, independently of how a run
executes it ([#761](https://github.com/ermacv/open-esp-radio-rs/issues/761)).
It is a `formats` package, so the qualification evaluator may link it to
recompute verdicts; execution lives in
[`oer-hil-executor`](../host/executor/README.md).

| Module | Owns |
| --- | --- |
| `spec` | `Spec`: the scenario's stable `ScenarioId`, its purpose type, the `Subject` contract it exercises (with the contract's declared bounds), typed `Params`, `Criteria` and `Trace`, its `Resources`, `TargetCapabilities` and `Estimate`, and the pure `verdict(trace, criteria)`; `Instance`, a named parameterisation that replaces copied scenarios, and its identity: SHA-256 over the canonical JSON of scenario, instance, parameters and criteria |
| `verdict` | `Verdict` (`Pass`, or `Fail` with its `Finding`s), made from a scenario's findings |
| `finite` | The check that parameters and criteria hold no NaN or infinite number, which JSON writes as `null`, before an identity is computed |
| `purpose` | The purpose as a marker type: `Conformance`, `Interop`, `Measurement` or `Investigation<Q: OpenQuestion>`, so the executor admits a scenario by its purpose at compile time; there is no protocol purpose, because protocol behaviour is proven on the host |
| `name` | Validated names: kebab-case scenario, instance, chip, fixture and composition names; dotted bound, span, count, operation and capability names; `name!` checks a name at compile time, `parse` at run time |
| `digest` | `Sha256` in its one canonical text form, and `canonical_json`, which sorts object keys itself |
| `wait` | `Wait`: a contract bound times a `Factor`, a typed protocol interval (`ProtocolQuantity`) times a count, or a measured stand span; `Bounds` with their source (recorded for the image's recipe, or the contract's declared constants); `Provisional` labels; `Wait::resolve`, the one place HIL code turns a wait into a `Duration` |
| `profile` | `StandProfile`, readable only through its rules: measured spans (each the maximum of its samples, its margin and why it was chosen, its `RunRef`s, and `Support::Measured` only with the 2 995 samples the 99.9th percentile at 95 % needs, else `Provisional { reason }`), and the bounds images of each recipe reported |
| `repetitions` | `Probability` and `Confidence` in (0, 1), and the samples a quantile bound needs |
| `arithmetic` | Checked time arithmetic: an overflow is the typed `Overflow`, never a saturated value |
| `resources` | Boards by role (`Dut`, `Peer`, `Observer`) and capability (`LpClock`, U.FL), fixtures, and `Resources::unmet`, which matches needed boards to offered ones and names what the stand lacks |
| `capabilities` | `TargetCapabilities`: `Composition::Any` or the product composition (the same crate as the product's), the agent capabilities, their union over a tier and an image's recipe digest |
| `estimate`, `tier` | An instance's time from its waits (its boot included) and repetitions, with its provisional labels; the tiers and their budgets |

Scenario types live in each block's scenario crate. A conformance scenario
follows its port's harness template (#761 Q3): the port's conformance
crate holds the trace types with a `wire` feature, each trace starting with
a header that carries the image's bounds and its clock domain,
`exercise(case) -> Result<Trace, Infrastructure>`, and a public pure
`verify(case, &trace)` that reads the bounds from that header. The
executor's [`harness_template` test](../host/executor/tests/harness_template.rs)
shows the shape on a toy port.

The crate's `clippy.toml` belongs to the `hil-scenario` copy group:
`cargo tidy check` keeps it identical to the other HIL scenario crates'
copies.
