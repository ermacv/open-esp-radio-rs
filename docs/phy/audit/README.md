# PHY instruction-audit evidence

These pages preserve per-object instruction and relocation comparisons from
the 2026-07-30 audit baseline. They answer what a pinned vendor/ROM function
did and what Rust implementation was present at that checkpoint. They are
evidence snapshots, not current crate-location or integration instructions.

- `libphy-*.md` covers each audited member of the pinned `libphy.a` archive.
- `rom-*.md` groups related revision-zero ROM leaves.

Current coverage counts and open proof state are maintained in the
[function-audit ledger](../function-audit-ledger.md). Current PHY scope and
lower-layer ownership are maintained in the [PHY index](../README.md) and
[PAC/HAL summary](../pac-hal-layer.md).

Do not rewrite an old transaction result when integration moves. Correct an
instruction or relocation claim only with stronger evidence; record current
ownership in the maintained summaries instead.
