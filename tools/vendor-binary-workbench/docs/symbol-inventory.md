# Artifact and symbol inventory

`symbols inventory` is the first artifact-facing step in a workbench project.
It answers what ELF and archive symbol tables actually contain before function
recovery, MMIO analysis, or platform semantics are applied.

```console
cargo vendor-binary-workbench advanced symbols inventory \
  --project verification/vendor/targets/esp32s31/vendor-project.toml \
  --run-spec /path/to/local.toml \
  --output generated/facts/symbols.json
```

Use `--name-prefix PREFIX` for a focused investigation and
`--undefined-only` to list imports. Both filters affect the human symbols
table and the JSON `symbols` array; summary counts still describe the complete
input. The default human view separates artifact, symbol and
resolution-candidate tables so archive association evidence stays visible.

A project can own the complete, unfiltered inventory destination:

```toml
[analysis.symbols]
output = "generated/findings/symbols.json"

[analysis.navigation]
output = "generated/findings/navigation.json"
```

With that table, `--output` is optional. `symbols inventory --check`
recreates and compares the report without modifying it. `project analyze`
includes the same operation as its first independent evidence stage. Filters
remain CLI-only so a project artifact cannot silently omit symbols needed by
later manual analysis.

The optional navigation index associates these complete facts with linked-IR
functions and interface call/root sites without changing the inventory or
claiming linker/semantic resolution. See [project navigation](project-navigation.md).

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

## Executable-byte coverage

The inventory also compares every executable section with all named, sized
text symbols in that exact ELF object or archive member. `code_sections`
records executable bytes, bytes covered by the union of symbol ranges,
zero-sized code-symbol counts and section-relative uncovered ranges. The human
view presents the same totals and gaps.

This distinguishes two different claims:

- `code-symbol coverage`: every named, sized local/global function can be made
  an analysis root;
- `executable-byte coverage`: every executable byte belongs to such a symbol.

The first is available directly from ELF facts. In project mode, accepted
entries from the reviewed code-boundary pack augment that catalog for linked
IR, interface, and MMIO analysis. Remaining uncovered bytes and unreviewed
zero-sized code symbols stay an explicit recovery backlog. Padding, literal
pools and alignment bytes may also appear in the uncovered ranges, so the
workbench never assumes every gap is a function.

Within those gaps the inventory emits an unreviewed function-boundary
candidate only when there is concrete entry evidence: a defined zero-sized
text symbol or a linked RISC-V `JAL`/tail transfer from a sized function. A
candidate records its section-relative entry, a conservative end limit, symbol
names and every direct-control-flow site. It remains `reviewed=false` and is
not fed into IR/MMIO analysis. Decode failures are retained as
`recovery_blockers`. This keeps discovery useful without promoting padding,
literal pools or a guessed prologue to executable function truth. Only a
separate accepted pack entry crosses that trust boundary.

Projects may place human decisions over these candidates in a separate
reviewed pack. The lifecycle, guards, and strict range rules are documented in
[reviewed code boundaries](code-boundaries.md). The inventory itself remains
immutable generated evidence and is never edited to mark a candidate accepted.

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

Schema v4 adds origin navigation in the opposite direction. An externally
selectable text definition in `source-artifact:NAME` is compared only with
externally selectable text definitions in `source-inventory:NAME`. Exact
symbol name and kind produce `unique-name-and-kind`,
`ambiguous-name-and-kind`, or `missing` `origin_association` values plus the
candidate artifact/member locations. Local `.L*` labels, data symbols, and
unrelated sources are excluded. This is provenance for navigating from link
truth back to source inventory; it does not claim which archive member the
linker extracted, and `linker_resolution_claim` remains false.

## Project input roles

A local run spec can expose both forms:

```toml
schema = 1

[[inputs]]
role = "source-inventory:libphy"
path = "/private/vendor/libphy.a"

[[inputs]]
role = "source-artifact:vendor-linked"
path = "/private/vendor/vendor-linked.elf"

[[inputs]]
role = "source-artifact:rom"
path = "/private/vendor/rom.elf"

[[inputs]]
role = "source-artifact:rust"
path = "/build/rust-driver.elf"
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
2. Run `project analyze` to save the configured complete symbol inventory and
   other generated evidence, or use `symbols inventory` directly.
3. Add the fully linked ELF when exact symbol selection is needed.
4. If executable gaps contain recovery candidates, run `code init-pack`,
   review every candidate, then run `code validate --deny-unreviewed`. After
   an artifact revision, use `code rebase`; direct apply is permitted only
   when every reviewed boundary remains structurally valid.
5. Run `interfaces discover` to recover pointer roots, load chains, indirect
   call sites and ABI argument provenance without assigning slot semantics.
6. Run `mmio discover` to create address and bit-pattern candidates.
7. Review register names and semantics in the separate editable model; do not
   edit generated facts.
8. Run `ir export` with an explicit target for structural IR, then through a
   project with a selected platform pack for typed globals, trampoline calls,
   delays, NVS, logging, or RTOS effects.
9. Promote reviewed behavior into reference/effect contracts and validate the
   Rust implementation against those contracts.

This staged model preserves evidence: regeneration can replace facts without
overwriting human decisions, and semantic annotations can always point back to
the artifact/member/symbol/offset that justified them.
