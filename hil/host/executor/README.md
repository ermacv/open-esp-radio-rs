# HIL scenario executor

`oer-hil-executor` executes the typed scenarios of
[`oer-hil-model`](../../model/README.md) ([#761](https://github.com/ermacv/open-esp-radio-rs/issues/761)).

| Module | Owns |
| --- | --- |
| `executor` | The executors `Model` (the host model) and `Stand` around a driver, their kinds, and `RunsOn`: which purposes each kind admits; the model runs conformance only |
| `execute` | `Run<X>`: a scenario's execution by executor `X`, typically generic over its port's harness trait so that the model and the stand's proxy run the same code; `execute`, which admits the pair only when the executor's kind runs the scenario's purpose, runs one instance, judges its trace with the scenario's pure verdict and records which executor ran it; `Failed` with its `Cause`: `Infrastructure`, which leaves no trace, or a scenario whose instance has no identity |
| `recorder` | `Recorder`: a run's counts, measurements and infrastructure retries; a name is a count or a measurement, never both, and a count that would overflow is refused |
| `retry` | `ProtocolRetry<P>`: retries only where the protocol loss `P: AdmitsLoss` names its basis and attempt limit, counting attempts and losses so that the requirement can bound the loss rate |
| `plan` | A tier's plan before anything is built or leased: `Planned::of` an instance; refusal of what the stand lacks (boards, capabilities such as a 32 kHz crystal, fixtures); one image per composition, split into segments (one image on one lane, one lease each) whose lease limit is the estimate times a margin and at most the stand's longest lease; bounds from the stand profile for a known recipe and the contract's declared constants, labelled provisional, for a new one; refusal of a plan over the tier's budget, naming its largest scenarios, and of a scenario no lease can hold |

The crate's `clippy.toml` forbids duration literals, `Duration::new` and
`thread::sleep` (every wait is an `oer_hil_model::Wait`); it is a copy of
the `hil-scenario` group that `cargo tidy check` keeps identical.

The [`harness_template` test](tests/harness_template.rs) is the conformance
harness template that the per-port conformance crates follow (#761 Q3).
