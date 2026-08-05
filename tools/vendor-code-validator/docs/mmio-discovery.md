# MMIO discovery

`mmio discover` is a best-effort, artifact-wide inventory for reverse
engineering register blocks. It accepts multiple ELF/ar inputs and explicit
half-open address ranges independently of whether every address already has an
SVD register name:

```console
cargo vendor-code-validator mmio discover \
  --target-spec validation/esp32s31/target.spec \
  --artifact rom="$ESP32S31_ROM_ELF" \
  --artifact libphy="$ESP32S31_LIBPHY_ARCHIVE" \
  --range phy=0x20100000..0x20110000 \
  --json-report /tmp/esp32s31-phy-mmio.json
```

The report groups statically addressed 8/16/32-bit reads and writes by
address, names known SVD registers, assigns stable `RANGE.REG_ADDRESS`
candidate names to unknown addresses, and lists every artifact/member/function
that used each register. For writes it reports output-bit provenance as
preserved, inverted, forced zero, forced one, derived from a register read, or
dynamic. `modified_mask`, `candidate_bit_ranges` and `field_candidates` are
mechanical data-flow facts; they do not claim field names, reset values, W1C
semantics or any other peripheral behavior. Field candidates combine partial
write masks, poll masks and MMIO-backed branch predicates, and link the
resulting bit ranges to access functions and guarded semantic actions for
manual analysis.

Discovery deliberately retains events recovered before unsupported control
flow and emits per-function diagnostics without failing the run. Its JSON says
`"analysis_mode": "best-effort"` and `"completeness_claim": false`. Use the
existing reference/verification workflows when a fail-closed completeness
claim is required. The initial discovery slice covers statically resolved
addresses; indexed and pointer-derived range recovery remains part of the
reference analyzer rather than this inventory.

Input-dependent conditional branches are explored in both directions with
explicit bounds of 127 symbolic states and 12 decisions per path. Artifact
summaries report explored states, terminal paths and distinct branch sites;
exhausting either bound produces an `exploration` diagnostic. Access counts use
the maximum multiplicity of an observable shape on any explored path, rather
than summing paths and double-counting their common prefix. The JSON records
this as `"access_count_mode": "maximum-per-path"`.
