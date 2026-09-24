# Contributing

Contributors can work on portable Rust protocols, ownership tests, binary
analysis, documentation and host tools without a radio board. Hardware changes
also need the appropriate source/profile review and, when readiness is claimed,
dated applicable device evidence.

## Start with a task

Read [From binary evidence to a Wi-Fi station](docs/binary-to-station.md), then
complete [the host tutorial](docs/first-contribution.md). For device work follow
[the ESP32-S31 route](docs/station-hardware.md). The
[capability map](qualification/README.md#everyday-status-and-next-work) connects
limitations and work candidates to their owners; it is not an aggregate
readiness verdict.

Use the toolchain in `rust-toolchain.toml`. Fetch public dependencies before
offline checks. Examples, platform, HIL, Blobray (`tools/blobray`) and
verification probes have separate workspaces and lockfiles; a root workspace
build does not cover them all.
The [repository guidelines](AGENTS.md) and
[documentation policy](docs/documentation.md) define the contribution rules.

## Make one reviewable change

Find the defining owner and existing behavior tests. Internal libraries depend
on specific lower contracts, never on the public `oer` facade. Keep production
behavior in production crates and probes thin. Add a focused regression for a
behavioral change; verify resource return and failure paths as well as success.

For handwritten MMIO, use typed PAC capabilities. Missing fields require review
and publication in the hardware model/PAC. Recovered tables and coefficients
are allowed and may be necessary: preserve source identity, purpose,
representation and hardware/profile applicability, and verify against the real
artifact. See [source policy](docs/source-policy.md).

## Choose checks by the changed boundary

Run commands from the repository root unless the component guide says otherwise.

| Change | Checks |
| --- | --- |
| Markdown or catalog | `cargo xtask check docs`; for portal behavior also follow [portal checks](tools/docs/README.md) |
| Rust behavior | `cargo test -p PACKAGE FILTER --locked --offline`, confirming the selector executes tests; `cargo fmt --all -- --check` |
| Public API | `cargo xtask check docs --package PACKAGE` plus focused behavior/compile tests |
| Private API | Add `--private` to the package documentation check |
| Hardware, ownership or dependency boundary | Relevant target, architecture, safety and artifact checks from [repository tooling](tools/repo/README.md) |
| Register publication | `cargo registers generate --manifest registers/esp32s31/publication/registers.toml --check` |
| Hardware readiness claim | Applicable dated [HIL](hil/README.md) evidence and independent [qualification](qualification/README.md) |

`cargo xtask check docs --full` is the complete API checkpoint;
`cargo xtask check source-only` is the full source checkpoint. Run them when the
change or final checkpoint requires their coverage, not after every paragraph.
Passing a focused check is not full repository coverage. Documentation examples
must distinguish host execution, target compilation and attached-hardware runs.

## Prepare the pull request

Explain the problem, the resulting behavior and the affected ownership boundary.
List checks actually run and their scope. Link qualification/HIL evidence when
claiming hardware behavior, and call out generated SVD/PAC changes. Use a scoped
imperative subject such as `docs: explain the station contribution route` or
`fix(esp32s31): return scan owners after channel failure`.

Keep detailed contracts in one owner location and link to them. Do not commit
private binaries, dumps, credentials, run reports or new work plans. Generated
reports belong in ignored owner outputs; reviewed machine provenance remains
tracked. Preserve unrelated changes in an already-dirty checkout.
