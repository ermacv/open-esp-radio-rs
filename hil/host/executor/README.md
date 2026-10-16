# HIL scenario executor

`oer-hil-executor` executes the typed scenarios of
[`oer-hil-model`](../../model/README.md) ([#761](https://github.com/ermacv/open-esp-radio-rs/issues/761)).

| Module | Owns |
| --- | --- |
| `execute` | `Run<X>`: a scenario's execution by executor `X`, implemented for every executor it can use (for a conformance scenario, generically over its port's harness trait, so the host model and the stand's proxy run the same code); `execute`, which runs one instance and judges its trace with the scenario's pure verdict; `Infrastructure`, a failure that leaves no trace and so no verdict |
| `recorder` | `Recorder`: a run's counts, measurements and `InfrastructureRetry` records; every infrastructure retry is recorded and counted |
| `retry` | `ProtocolRetry<P>`: retries only where the protocol loss `P: AdmitsLoss` names its basis and attempt limit, counting `<name>.attempts` and `<name>.lost` so that the requirement can bound the loss rate |
| `plan` | A tier's plan before anything is built or leased: one image per composition, split into segments (one image on one lane, one lease each) whose lease limit is the estimate times a margin, capped by the stand's longest lease; refusal of a plan over the tier's budget, naming its largest scenarios, and of a scenario no lease can hold |

The crate's `clippy.toml` forbids duration literals, `Duration::new` and
`thread::sleep`: every wait is an `oer_hil_model::Wait`.

The [`harness_template` test](tests/harness_template.rs) is the conformance
harness template that the per-port conformance crates follow (#761 Q3): one
scenario runs with a model harness and with a stand proxy that returns the
traces the target reports, both get the same verdict, and a stored trace is
judged again without a run.
