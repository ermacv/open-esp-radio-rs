# registers/

Reviewed hardware register models and the publication that generates the
production PACs: [README](README.md).

| Path | Owns |
| --- | --- |
| `<chip>/model/` | Editable register geometry and semantics (`device.toml`, `peripherals/`, `reviewed.toml`) |
| `<chip>/policy/` | PAC API transactions (`api.toml`), ownership ranges, lint policy |
| `<chip>/evidence/` | Source identities and provenance behind each declaration |
| `<chip>/publication/registers.toml` | The publication: inputs and the four outputs |
| `<chip>/published/` | Generated `radio.svd` and `radio.bindings.toml` |
| `ieee80211/` | Wi-Fi MAC layouts both chips place at their own base addresses |

Generated outputs also land in `crates/hardware/<chip>/pac/raw/src/lib.rs` and
`crates/hardware/<chip>/pac/src/generated.rs`.

## Rules

- Edit the model, policy or evidence; never a generated output. Regenerate
  and commit the outputs with the model change, and call them out in the PR.
- Never read the generated outputs whole: grep them with an explicit path.
- A field that handwritten code needs is published here, never replaced by a
  local mask or shift in a driver.
- Every declaration keeps its evidence; register and field descriptions that
  cite vendor functions are covered by `cargo verification check provenance`.
- `policy/api.toml` is large: grep for the register or transaction name.

## Commands (in the background)

- `cargo registers validate --manifest registers/esp32s31/publication/registers.toml`
- `cargo registers generate --manifest registers/esp32s31/publication/registers.toml` (`--check` only compares)
- `cargo xtask check architecture` after PAC API changes.

Workflow: the `driver-or-hardware-change` skill.
