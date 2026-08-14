# ESP32-S31 vendor-analysis project

This directory is the reviewed ESP32-S31 configuration for Vendor Binary
Workbench. `vendor-project.toml` is the entry point. The target host links the
generic Workbench with ESP32-S31 contracts and semantic providers; target
addresses and driver dependencies do not enter the generic package.

## Local inputs

`local.toml` is ignored and contains machine-local paths to authenticated
vendor artifacts and compiled Rust probes. Initialize it from
`local.example.toml` or with:

```console
cargo vendor-binary-workbench project inputs init \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --bind source-artifact:rom=/path/to/esp32s31_rev0_rom.elf \
  --bind source-inventory:archive=/path/to/libphy.a \
  --bind source-inventory:libpp=/path/to/libpp.a \
  --bind source-inventory:libnet80211=/path/to/libnet80211.a \
  --bind rust-artifact=/path/to/rust-trace-probes.elf
```

Never commit vendor binaries, extracted tables, disassembly dumps or private
paths. `project files` lists every required role.

## Normal workflow

```console
cargo vendor-binary-workbench project doctor \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

tools/vendor-binary-workbench/scripts/run-limited \
  project analyze \
  --project verification/vendor/targets/esp32s31/vendor-project.toml --jobs 1

cargo vendor-binary-workbench project verify \
  --project verification/vendor/targets/esp32s31/vendor-project.toml

cargo vendor-binary-workbench project check \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Use `project status` for the quick overview, `project browse` for navigation,
and `inspect function SOURCE:SYMBOL` for a focused body. The suite and policy
TOML files are the coverage source of truth; this README intentionally does
not duplicate the suite inventory.

## Register publication

```console
cargo vendor-binary-workbench registers review \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
cargo vendor-binary-workbench registers validate \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
cargo vendor-binary-workbench project publish \
  --project verification/vendor/targets/esp32s31/vendor-project.toml
```

Discovery facts feed the reviewed register model, which publishes the clean
SVD, private raw PAC and restricted production PAC API. Generated facts do not
invent field semantics.

## Verification boundary

`verification-policy.toml` selects independent review scopes, function
requirements and bounded properties. It does not define product readiness.
The repository qualification ledger is the sole authority for that decision.

Bindings distinguish exact production entries, shared production core and
verification projections. Concrete replay may support release evidence only
when it reaches the declared compiled production boundary. Semantic contracts
remain useful research evidence but do not prove manually written driver
sequencing.

`phy_chip_set_chan` remains an explicit production-verification gap. The old
provider-owned semantic contract was removed because it normalized both traces
and computed its own verdict instead of proving the compiled shipping entry.
The retained observations show the first unreviewed difference at analog-I²C
host selection: the vendor ROM path uses the recovered `0x1a00` configuration,
while production uses the newer recovered `0x3fa00` configuration. Closing the
gap requires a generic comparison of the compiled production boundary, with
that difference and the remaining ordered effects reviewed explicitly.

For concepts and schemas, start at
[`tools/vendor-binary-workbench/README.md`](../../../../tools/vendor-binary-workbench/README.md).
