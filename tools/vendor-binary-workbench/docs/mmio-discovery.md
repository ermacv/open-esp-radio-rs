# MMIO discovery

With a project memory map, SVD and explicit ranges are optional:

```console
cargo vendor-binary-workbench mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --json-report /tmp/radio-mmio.json
```

The human view is intentionally bounded: it shows artifact/range summaries
and the 32 most active registers. The JSON report remains the complete
register, function, bit-pattern and diagnostic inventory.

Every `mmio` region in the project's default address space becomes a discovery
range. Pass one or more explicit `--range` options to replace those defaults
for a narrower scan. SVD contributes register names only; unknown addresses
remain valid findings named `UNMAPPED`.

When the project has a `[registers]` table, its `facts` path is also the
default JSON destination. That report feeds the editable
[register workspace and SVD export](register-workspace.md); names and hardware
semantics stay in the reviewed register model rather than this generated file.

Pass `--check` to render the same discovery in memory and compare it with the
configured or explicit JSON report. A missing or different report fails
without writing it:

```console
cargo vendor-binary-workbench mmio discover \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --check
```

`--check` therefore requires `--json-report` or a project `[registers].facts`
default. Use [`project analyze --check`](project-pipeline.md) to verify MMIO
together with the other generated project evidence.

`mmio discover` is a best-effort, artifact-wide inventory for reverse
engineering register blocks. It accepts multiple ELF/ar inputs and explicit
half-open address ranges independently of whether every address already has an
SVD register name:

```console
cargo vendor-binary-workbench mmio discover \
  --target-spec verification/vendor/targets/esp32s31/target.toml \
  --artifact rom="$ESP32S31_ROM_ELF" \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --range phy=0x20100000..0x20110000 \
  --json-report /tmp/esp32s31-phy-mmio.json
```

By default every named, non-empty text symbol is an analysis root, including
local/private functions in archive members and ELF symbol tables. Use
`--code-symbols exported` only when intentionally restricting the scan to
global and weak definitions; `--symbol-prefix` can narrow either catalog. The
generated facts record both choices in `code_selection`, so a reviewed result
cannot silently change scope. This is symbol-complete, not byte-complete:
stripped functions, zero-sized symbols and executable bytes without a function
symbol still require code-boundary recovery before they can be analyzed.

The report groups statically addressed 8/16/32-bit reads and writes by
address, names known SVD registers, assigns stable `RANGE.REG_ADDRESS`
candidate names to unknown addresses, and lists every artifact/member/function
that used each register. Schema v5 also records every recovered instruction PC
in `read_sites` and `write_sites`; these are binary navigation evidence even
when later control flow prevents complete pseudo-code recovery. Reviewed or
synthesized summaries do not invent instruction sites. For writes it reports output-bit provenance as
preserved, inverted, forced zero, forced one, derived from a register read, or
dynamic. `modified_mask`, `candidate_bit_ranges` and `field_candidates` are
mechanical data-flow facts; they do not claim field names, reset values, W1C
semantics or any other peripheral behavior. The discovery JSON records
`modified_mask` and `candidate_bit_ranges` for every write pattern together
with its functions. `registers review` turns the boundaries induced by partial
write masks into copyable field drafts while keeping those placeholders
outside the reviewed model. The richer
`ir export` register index additionally combines partial writes, poll masks and
MMIO-backed branch predicates into `field_candidates` linked to access
functions and guarded semantic actions.

The project snapshot joins these artifact-wide sites back into the Functions
view. A function whose linked IR is incomplete therefore still shows its exact
static MMIO reads/writes and instruction PCs; linked-IR registers and static
sites are unioned, never treated as competing truth sources.

Discovery deliberately retains events recovered before unsupported control
flow and emits per-function diagnostics without failing the run. Its JSON says
`"analysis_mode": "best-effort"` and `"completeness_claim": false`. Use the
existing reference/verification workflows when a fail-closed completeness
claim is required. The initial discovery slice covers statically resolved
addresses; indexed and pointer-derived range recovery remains part of the
reference analyzer rather than this inventory.

Input-dependent conditional branches are explored in both directions with
explicit bounds of 127 symbolic states and 12 decisions per path. Each trace is
also bounded to 4,096 instruction steps, 1,024 observable events and 2,048
distinct merged events per function. Symbolic value trees are capped and
degrade to `unknown` when further expression expansion would exceed the host
resource boundary. All exhausted limits become scoped diagnostics rather than
silently claiming completeness. Artifact
summaries report explored states, terminal paths and distinct branch sites;
exhausting either bound produces an `exploration` diagnostic. Access counts use
the maximum multiplicity of an observable shape on any explored path, rather
than summing paths and double-counting their common prefix. The JSON records
this as `"access_count_mode": "maximum-per-path"`.

Independent functions can be processed concurrently with `--jobs N` (`1..=8`).
The safe default is one worker. Increase `--jobs` explicitly only after a
target-specific peak-memory measurement.
Workers use a bounded result queue, deterministic final sorting and explicit
stack size, so concurrency does not retain one whole artifact per worker.
Choose `--jobs 2` first on a new target and compare runtime and peak RSS before
raising it further.
