# verification/

Evidence that production drivers follow the pinned vendor code, by level
(L0 pins … L3 hardware): [README](README.md). No production behavior and no
private vendor artifacts live here.

| Path | Owns |
| --- | --- |
| `<chip>/artifacts.toml` | The single pin (repository, revision, SHA-256) of every vendor archive, ROM ELF and SDK firmware |
| `<chip>/facts/` | Cited-function fingerprints (`provenance.toml`) and recovered name maps |
| `<chip>/probes/`, `<chip>/scenarios/` | Compiled production entries (built with the chip's image compiler, in the shared firmware cache) and typed Blobray comparisons (the verdict library; `scenarios/cli` its binary) |
| `<chip>/hil-vendor/`, `<chip>/hardware/` | Vendor firmware and isolated firmware probes of board state |
| `harness/` | Probe generation and the chip-neutral scenario engine |
| `evidence/` | `oer-vendor-evidence`: evidence producers, orchestration, the verdict source policy and the `Producer` contract; re-exports the shared shard format |

The host CLI, harness, evidence producers and chip scenarios belong to this
workspace; firmware probes and host stands have their own workspaces below
the chip directories. Root-workspace shared formats live in
[`../qualification/evidence-shard/`](../qualification/evidence-shard/README.md) and
[`../hil/phy/capture/`](../hil/phy/capture/README.md);
chip calibration relations and comparisons live in `../hil/phy/<chip>/`.

## Rules

- `_oracles/` is private input: never commit it, vendor binaries or dumps.
- Hashes outside `artifacts.toml` record where a reviewed fact was observed;
  they are not pins.
- A comparison fails closed with `MATCH`, `DIFF` or `INCOMPLETE` and exercises
  compiled production code, never a shadow implementation.
- The vendor evidence index is derived data, never tracked:
  `cargo verification evidence --chip <chip>` computes it into
  `target/verification/<chip>/evidence` for the checkout.
- `facts/*.toml` are large and partly generated: grep them by explicit path.
- Production behavior never moves into a probe.

## Commands (in the background)

`<chip>` is `esp32s31` or `esp32c5`; both have pins, provenance facts,
probes and scenarios.

- `cargo verification fetch <chip>` downloads and verifies the pins into `target/vendor/`.
- `cargo verification check provenance --chip <chip>` after touching a `SOURCE:` citation or a pin.
- `cargo verification diff --chip <chip> --old A --new B` after a pin change.

Workflow: the `vendor-evidence` skill.
