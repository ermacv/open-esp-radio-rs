# Artifact and symbol inventory

`symbols inventory` is the first artifact-facing step in a validator project.
It answers what ELF and archive symbol tables actually contain before function
recovery, MMIO analysis, or platform semantics are applied.

```console
cargo vendor-code-validator symbols inventory \
  --project verification/vendor/targets/esp32s31/vendor-validator.toml \
  --run-spec /path/to/local.run \
  --json-report generated/facts/symbols.json
```

Use `--name-prefix PREFIX` for a focused investigation and
`--undefined-only` to list imports. Both filters affect emitted `SYMBOL` rows
and the JSON `symbols` array; summary counts still describe the complete input.

## What is an artifact fact

The backend retains, for every named static or dynamic symbol:

- artifact path, container kind, archive member and object kind;
- static or dynamic symbol-table origin;
- name, address and size;
- ELF binding (`local`, `global`, `weak`, or GNU unique);
- ELF visibility (`default`, `hidden`, `protected`, or `internal`);
- symbol kind and defining section;
- definition state (`undefined`, section, absolute, or common).

This is intentionally broader than the function-body catalog. Undefined
imports, data objects, local symbols and absolute values matter when recovering
global state, pointer tables, and external calls even though they cannot be
decoded as functions.

The generated JSON authenticates every input with SHA-256. The digest records
which bytes produced the report; it is not a vendor-version acceptance policy.

## Definition and association classes

The report separates binary facts from conservative project associations:

| `resolution` | Meaning |
| --- | --- |
| `defined-local` | A definition exists but its binding or visibility does not make it externally selectable |
| `defined-exported` | A global/weak/GNU-unique, default/protected definition exists |
| `absolute`, `common` | The ELF definition uses that special section state |
| `archive-candidate` | An undefined symbol has an exported definition in another member of the same archive |
| `same-artifact-candidate` | A non-archive object contains both the undefined fact and a candidate definition |
| `project-associated` | Exactly one candidate definition exists in another project input |
| `ambiguous-project` | More than one project candidate exists |
| `undefined-import` | No project candidate is present |

Candidate does not mean resolved. Static archive extraction order, archive
groups, weak/strong precedence, COMDAT selection, linker scripts, symbol
versioning, wrapping, and garbage collection can change the final link. The
JSON therefore declares `linkage_mode="association-only"` and
`linker_resolution_claim=false`.

When exact resolution matters, add the fully linked ELF as a distinct
`source-artifact:NAME` input and treat its symbol table, relocations and code
addresses as linker truth. Keep the `.a` as `source-inventory:NAME` for
coverage and member-level navigation. Do not silently merge them into one
identity: one describes available objects, the other describes a selected
link.

## Project input roles

A local run spec can expose both forms:

```text
schema 1
input source-inventory:libphy /private/vendor/libphy.a
input source-artifact:vendor-linked /private/vendor/vendor-linked.elf
input source-artifact:rom /private/vendor/rom.elf
input source-artifact:rust /build/rust-driver.elf
```

The part after `:` is a stable logical source identifier. A path may have
multiple roles; the inventory groups identical paths while preserving every
role and source name. Paths remain local and untracked, whereas reports and
later reviewed overlays can be stored in the project.

## Boundary between generic analysis and semantic packs

The generic layer may establish only structural facts:

- a data symbol or read-only range looks like an array of aligned pointers;
- a load from `table + constant-offset` feeds an indirect call;
- relocations name the table, pointer cell, slot target, or referenced global;
- argument and return registers have recoverable value flow;
- a call is followed by particular MMIO, RAM, or control-flow effects.

It must not call that table an RTOS API or name a slot `queue_send` merely from
its offset. Those claims belong to a semantic pack selected by the project.
Such a pack declares table anchors, pointer indirection, layout/version guards,
slot names, ABI types, effects, and replacement hints. An RTOS pack can then
map a structurally recovered callback to an event-send effect; an async-Rust
adapter can propose a channel or wakeup replacement. The low-level indirect
call remains present underneath that annotation.

The existing ESP32-S31 harness follows this rule for reviewed trampoline
tables and external ABI calls. A future reusable RTOS pack should depend on a
small architecture-neutral semantic-pack interface, not on ESP32-S31 names or
the RISC-V decoder. Chip-specific table layouts can instantiate the same RTOS
effect vocabulary.

## Recommended project flow

1. Run `project doctor` to validate configuration and parse every input.
2. Save `symbols inventory` as immutable artifact facts.
3. Add the fully linked ELF when exact symbol selection is needed.
4. Run `interfaces discover` to recover pointer roots, load chains, indirect
   call sites and ABI argument provenance without assigning slot semantics.
5. Run `mmio discover` to create address and bit-pattern candidates.
6. Review register names and semantics in a separate editable overlay; do not
   edit generated facts.
7. Run `ir export` with no harness for structural IR, then with selected
   semantic packs for typed globals, trampoline calls, delays, NVS, logging,
   or RTOS effects.
8. Promote reviewed behavior into reference/effect contracts and validate the
   Rust implementation against those contracts.

This staged model preserves evidence: regeneration can replace facts without
overwriting human decisions, and semantic annotations can always point back to
the artifact/member/symbol/offset that justified them.
