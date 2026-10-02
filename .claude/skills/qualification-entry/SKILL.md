---
name: qualification-entry
description: Use when adding or changing a qualification catalog entry, source fact, capability status, program (qualification/targets/), HIL requirement, or a // CAPABILITY: code anchor in this repository, or when asked whether a capability is ready or what work remains for it.
---

# Add or change a qualification entry

Read first (about 3k tokens): [code anchors](../../../qualification/README.md#code-anchors),
[declared and derived axes](../../../qualification/README.md#declared-and-derived-axes)
and [imports and required sets](../../../qualification/README.md#imports-and-required-sets).
For HIL requirements also read
[HIL requirements and scenario roles](../../../qualification/README.md#hil-requirements-and-scenario-roles)
and [named checks](../../../qualification/README.md#named-checks).

## Checklist

1. **Find the owner catalog.** Grep `qualification/catalog/<chip>/` for the
   capability or fact ID and read only that entry; catalogs are 100–170 KB.
   Wi-Fi/PHY, Bluetooth, products, coexistence, IEEE 802.15.4 and whole-radio
   each own their declarations ([catalog owners](../../../qualification/README.md#catalog-owners)).
2. **Declare, do not claim.** `implementation`, `host` and `async` are
   reviewed declarations with gaps for what is missing; `vendor` and `hil`
   come from evidence only.
3. **Anchor the code.** A status of `implemented`, `partial` or `fail-closed`,
   or `implementation = "complete"`, needs a `// CAPABILITY: <id>` comment
   directly above the owning item in a production-layer package (after its
   docs and attributes). An entry listing `packages` is anchored only there.
4. **Programs select.** A program names catalogs and selects IDs; it declares
   no capability. Keep its `required-capabilities` exact, or use
   `required-capabilities-from = "catalog-closure"`.
5. **HIL requirements.** Name a scenario and its named checks, never a
   duplicated threshold. Referencing a scenario makes it `qualification`;
   a requirement on an `investigation` scenario is never satisfied.
6. **Source contracts** describe implementation paths and limits; they never
   promote a parent capability.
7. **Docs.** A chip `FEATURES.md` links to the catalog; never copy rows or
   write a dated `PASS` table.

## Commands (all `run_in_background: true`)

```console
cargo xtask check capabilities --changed <file>
cargo qualification catalog check --catalog qualification/catalog/esp32s31/wifi-phy.toml
cargo qualification validate --manifest qualification/targets/esp32s31/wifi-sta.toml
cargo qualification status --manifest qualification/targets/esp32s31/wifi-sta.toml --capability <id>
cargo xtask check docs
```

`cargo qualification next … --capability <id>` lists the remaining work.
