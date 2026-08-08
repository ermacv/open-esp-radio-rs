# Read-only project browser

`project browse` opens a full-screen view of the same typed project state used
by the application API:

```console
cargo vendor-binary-workbench project browse \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

The browser requires interactive stdin and stdout. It accepts only the human
output mode and deliberately rejects `--target-spec`, `--run-spec` and `--svd`:
the checked-in project is its single configuration root. Use the ordinary CLI
for standalone target/backend experiments and machine-readable output.

## Views and keys

| View | Contents |
| --- | --- |
| Overview | Lifecycle phases, readiness, component diagnostics |
| Functions | Recovered functions, review status, blockers, replay-required scenario candidates and pseudo-Rust |
| Registers | Resolved SVD/reviewed catalog and workspace counts |
| Interfaces | Resolved table slots, ABI, semantics, execution models and sites |
| Comparisons | Project profiles, concrete MATCH/DIFF/INCOMPLETE, first trace difference, artifact provenance, model evidence and blockers |
| Diagnostics | Missing, incomplete and invalid component details |
| Types | Reviewed logical types, their exact memory-object bindings and fields |

| Key | Action |
| --- | --- |
| `Tab`, `h`/`l`, left/right | Change view |
| `j`/`k`, up/down | Change selected row |
| `g`/`G`, Home/End | First/last row |
| `r` | Reload the project |
| `Enter`, `c` | Execute the selected comparison profile |
| `q`, Esc, Ctrl-C | Exit |

Reload runs on a worker thread. The old immutable snapshot remains visible
while the project is being resolved; success atomically installs the new
generation, and failure leaves the old generation intact with an on-screen
message. Terminal input and drawing stay on one thread.

Comparison execution uses the same worker and the same application API. The
selected profile must be declared under `[verification]`, while its local
vendor and Rust binaries are resolved from `local.run`. A result stays attached
to the current snapshot generation and is discarded on reload. The TUI renders
the typed first-difference context, coverage blockers, table lifecycle and
per-device completeness; it never derives a verdict from display text.

The mode is intentionally read-only. It does not rewrite register, function or
interface packs and has no hidden state that competes with checked-in project
files. Editing, analysis generation and publication remain explicit CLI
workflows. See [Application API and alternate frontends](application-api.md)
for the boundary shared by both frontends.

Scenario candidates are derived from bounded argument guards, MMIO predicates,
and poll shapes in linked IR. The browser labels them `replay required` because
they are editing/navigation aids, not verified coverage. Copy or adapt them
into an execution profile; only concrete execution can close a branch outcome.
