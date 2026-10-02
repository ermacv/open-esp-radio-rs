---
name: new-package
description: Use when creating, splitting, moving, renaming or reclassifying a Cargo package in this repository, or editing the package.metadata.open-radio table (layer, platform, chip, family, inputs) or package dependencies across layers.
---

# Add or move a package

Read first (about 3k tokens): [package names](../../../docs/architecture.md#package-names),
[package classification](../../../docs/architecture.md#package-classification),
[layer dependencies](../../../docs/architecture.md#layer-dependencies) and the
[tidy checks](../../../tools/tidy/README.md).

## Checklist

1. **Need a crate?** A logical module does not require a new package; a
   directory is an owner, a workspace a build and lock boundary.
2. **Name.** `oer-` then lowercase tokens: optional chip or family, domain
   (`ieee80211`, `bluetooth`, `ieee802154`, `coex`, `radio`, …), optional
   component, and the binding for adapters; compositions end in `-system`.
   The directory repeats the tokens under its layer directory.
3. **Classify** in `[package.metadata.open-radio]`: a `layer` (it implies the
   scope: production, experimental or development) and a `platform` —
   `portable`, `host`, `chip` (with `chip = "<id>"`), `family` (with
   `family = "<id>"`) or `selected`. Declare `inputs` for repository files
   outside the package its tests read, so the gate follows them.
4. **Dependencies** follow the layer table; build dependencies are host or
   portable; internal packages never depend on the `oer` facade.
5. **Lints.** Inherit `[lints] workspace = true`; a crate root declares its
   unsafe policy ([UNSAFE.md](../../../crates/UNSAFE.md)).
6. **Docs.** Add `[package.metadata.docs.rs]` (chip packages set
   `default-target = "riscv32imafc-unknown-none-elf"`), a README, and a row
   in the owning source map such as [crates/README.md](../../../crates/README.md#source-map).
7. **Workspace and lock.** Add the member, then `cargo xtask lock`.
8. **Moves.** Update every path, `include_str!`, catalog `packages` entry,
   record path and link in the same change; no re-export shim at the old path.

## Commands (all `run_in_background: true`)

```console
cargo xtask check tidy
cargo xtask lock --check
cargo xtask check architecture
cargo xtask check changed
```

`cargo tidy workspaces --json` lists the workspaces tidy discovered.
