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
for standalone target/backend experiments and machine-readable output. The
minimum terminal size is 64×16; an 80×24 terminal uses compact headers, tabs
and key hints without hiding a readiness state or exit key.

## Views and keys

| View | Contents |
| --- | --- |
| Overview | Lifecycle phases, readiness, component diagnostics |
| Code | Generated executable-gap candidates, reviewed boundaries, artifact guards and control-flow evidence |
| Functions | Recovered functions, review status, typed per-PC decode blockers, replay-required scenario candidates and pseudo-Rust |
| Registers | Resolved catalog plus lazy review/access/field/predicate/poll/semantic evidence |
| Interfaces | Reviewed table slots plus unreviewed discovered slot evidence, ABI, semantics, execution models and sites |
| Comparisons | Project profiles, concrete MATCH/DIFF/INCOMPLETE, first trace difference, artifact provenance, model evidence and blockers |
| Diagnostics | Missing, incomplete and invalid component details |
| Types | Reviewed logical types, their exact memory-object bindings and fields |

| Key | Action |
| --- | --- |
| `Tab`, `h`/`l`, left/right | Change view |
| `j`/`k`, up/down | Change selected row |
| `g`/`G`, Home/End | First/last row |
| `/`, text, Enter | Edit and apply a case-insensitive section filter |
| Esc | Clear an active filter; otherwise exit |
| PageUp/PageDown, `u`/`d` | Scroll the detail pane without changing the selected row |
| Enter | Follow function/register/interface/semantic/type cross-references; run a comparison in Comparisons |
| `r` | Reload the project |
| `c` | Execute the selected comparison profile |
| `q`, Esc, Ctrl-C | Exit |

Reload runs on a worker thread. The old immutable snapshot remains visible
while the project is being resolved; success atomically installs the new
generation, and failure leaves the old generation intact with an on-screen
message. Terminal input and drawing stay on one thread.

Long lists use a selection-following viewport, while the right-hand detail
pane has independent vertical scrolling. Table viewports account separately
for borders and their header row, so the active register/comparison cannot be
scrolled into the lower border. List views such as Functions use the additional
header-free row. Filtering changes only the visible index and never discards
snapshot data. Function index rows and heavy details
(contexts, memory fields, scenario candidates and pseudo-Rust) are separate
typed DTOs keyed by stable function identity. The worker loads one detail on
selection and the current snapshot generation owns the resulting cache.
Function rows retain a bounded CFG-reachable blocker count; the lazy detail shows each
blocker's PC, best-effort mnemonic, width, raw encoding, class and whether
architectural linear continuation was safe. The mnemonic is review evidence,
not a claim that instruction semantics are implemented. A specific unsupported
instruction is therefore not collapsed into the generic `analysis incomplete`
state. Function search also matches blocker operations such as `flw` and
classes such as `floating-point` or
`zero-fill-or-illegal-trap` without loading the heavy detail.
Register rows and heavyweight detail use the same arrangement. Selecting a
register asks the worker for `register_detail(address)` once per snapshot
generation. The detail pane shows name provenance, width, review state,
read/write/RMW counts, function users, write masks, field candidates,
direct/transitive predicates, polls and linked semantic operations.
The Interfaces view includes every discovered slot that remains unreviewed,
not only semantic bindings already accepted by the interface pack. Such rows
are explicitly labelled `unreviewed`, show their offset, optional indexed
selector, functions and call sites, and keep ABI/semantics unknown. Reviewing
the corresponding TOML pack is the only operation that can promote them to a
named ABI or executable model.
Code boundaries are kept in the light snapshot and can be filtered by source,
section, address, reviewed name, symbol evidence, reason, or caller. The view
is read-only: edit `code/boundaries.toml`, validate it, then reload.
Implementation state follows the same boundary: `tui/state/detail.rs` owns
lazy caches, `state/filter.rs` owns matching and `state/navigation.rs` owns
reviewed cross-links. Code, function, register, interface and comparison views are
separate renderers; none performs project I/O or analysis.
The terminal redraws only after input, resize or a worker result; the periodic
worker poll does not repaint an unchanged frame.
Reviewed logical-type and field names are applied to the pseudo-Rust detail
without erasing the recovered access width. Scenario candidates also include
an editable verification-profile draft with explicit TODO arguments; the
draft remains `replay required` until the user reviews it and concrete
execution closes its coverage.

Comparison execution uses the same worker and the same application API. The
selected profile must be declared under `[verification]`, while its local
vendor and Rust binaries are resolved from `local.toml`. A result stays attached
to the current snapshot generation and is discarded on reload. The TUI renders
the typed first-difference context, coverage blockers, table lifecycle and
per-device completeness; it never derives a verdict from display text.
The first differing observable includes its producer PC and linked
symbol-relative offset on both sides.

The mode is intentionally read-only. It does not rewrite register, function or
interface packs and has no hidden state that competes with checked-in project
files. Editing, analysis generation and publication remain explicit CLI
workflows. See [Application API and alternate frontends](application-api.md)
for the boundary shared by both frontends.

Scenario candidates are derived from bounded argument guards, MMIO predicates,
and poll shapes in linked IR. The browser labels them `replay required` because
they are editing/navigation aids, not verified coverage. Copy or adapt them
into an execution profile; only concrete execution can close a branch outcome.
