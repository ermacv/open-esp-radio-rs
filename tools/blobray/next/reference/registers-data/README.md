# Registers, tables and constants

Inspect MMIO candidates and captured data before proposing a reviewed interpretation.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Captured data, tables and coefficients

`data` reads exact ranges and selected saved analyses from one captured object.
It does not run an analyzer or infer a table boundary. `export-data` selects a
reviewed integer table or constant at an explicit knowledge revision. Both use
the same application-owned query, supervision and delivery budget as other reads.

```console
blobray data --project research --request data-request.json --limit-mode watchdog
blobray data --project research --request data-request.json --output observations --limit-mode watchdog
blobray knowledge --project research --limit-mode watchdog propose-data --request table-proposal.json
blobray knowledge --project research --limit-mode watchdog propose-constant --request constant-proposal.json
blobray knowledge --project research --limit-mode watchdog show
blobray knowledge --project research --limit-mode watchdog accept --base PROPOSAL_REVISION --assertion ASSERTION_ID --actor researcher --reason "Checked exact source evidence"
blobray export-data --project research --revision ACCEPTED_REVISION --assertion ASSERTION_ID --output accepted-data --limit-mode watchdog
```

The `DataRequest` JSON has `occurrence`, `ranges`, `analyses` and optional
`pointer_table` (null/absent for ordinary byte observations). Copy the exact
revision, source and object identity from inventory or an analysis recipe;
`occurrence.symbol` is optional. Object identity retains the archive member
ordinal even when names and bytes repeat. `analyses` is an array of analysis IDs
from that same revision/source/object. All their records are retained, including
call inputs, unknown targets, MMIO, expression definitions and semantic gaps.
Record ordinals address that retained stream, not instruction offsets.

Each range uses one of these selectors:

| Selector | JSON fields in addition to `kind` | Meaning |
| --- | --- | --- |
| `section` | `section`, `offset`, `length` | Explicit section-relative bytes |
| `symbol` | `symbol`, `length` (integer or null) | Exact static or dynamic SymbolId; null uses its declared size |
| `image` | `address`, `length` | Virtual address in a file-backed load mapping of an executable ELF |

The prepared-object profile requires little-endian RV32 ET_REL/ET_EXEC with at most one each
of SHT_SYMTAB and SHT_DYNSYM; table-free section/range selection is supported; names
such as `.symtab` are not table identity. Dynamic symbol selection does not enable
dynamic loading, TLS or relocation application. Section relocations must reference
the static table through `sh_link`; a different table is an explicit unsupported
profile, never an index interpreted in the static table. There are at most 32 ranges and 32 analysis IDs per request; at
least one is required. A zero-sized symbol needs an explicit length. A sized symbol cannot
be expanded past its declared size. Overflow, out-of-range, ambiguous addresses,
compressed sections and NOBITS are explicit errors. Requests never guess length
from the next symbol. Several ranges share one prepared ELF owner and section
metadata; borrowed views cannot escape its callback.

A `DataProposalRequest` contains `occurrence`, `analyses`, `subject`, `selector`,
`layout`, `purpose`, `applicability`, `expected_base`, `actor` and `reason`.
For example, a signed little-endian array of 100 contiguous 16-bit elements has:

```json
{"kind":"integer","encoding":{"width":2,"signed":true,"byte_order":"little"},"count":100,"stride":2}
```

Widths are 1, 2, 4 or 8 bytes; byte order is `little` or `big`. Stride is in bytes
and cannot be less than element width. The layout must cover exactly the selected
range, including internal padding. The application canonicalizes symbol/image selectors to exact section ranges,
adds matching payload/range evidence and validates it again during review.
Generic knowledge changes, specialized proposals, review and export share physical
occurrence validation. An optional image data symbol must exist in its declared
object/table even when the selector is a canonical section range. Source evidence
and the selected range are validated against that same prepared object.
This also detects conflicts between physical aliases of the same table. `purpose` and `applicability`
record the proposed meaning; their presence is not automatic semantic acceptance.

A `ConstantProposalRequest` contains `analysis`, `record`, `operand`, `value`,
`subject`, `purpose`, `applicability`, `expected_base`, `actor` and `reason`.
`value` is an RV32 unsigned bit pattern. The operand is a tagged object such as
`{"kind":"value"}`, `{"kind":"write-value"}`, `{"kind":"address"}`,
`{"kind":"call-argument","index":0}`, `{"kind":"return-low"}` or
`{"kind":"return-high"}`. It must match a known constant in that exact saved
record. Unknown values and expressions are not accepted as numeric constants.
Instruction-derived coefficients retain their analysis evidence; no fictitious
contiguous data table is created for them.

Export creates a new directory containing `manifest.json`, `object.elf`,
`data.bin` and `records.jsonl`. The object is the exact captured ELF member/image,
not its enclosing archive. The manifest records its digest, occurrence, source
file and section ranges, optional image addresses, per-range digests and offsets
into concatenated `data.bin`. The records preserve analysis IDs and ordinals;
`ranges` links known relocation references into the selected ranges. Empty links
do not assert absence of other uses. The source object's byte order is distinct
from a reviewed table's explicit interpretation.

For tables, integer records decode captured bytes. Writable sections are marked
as initialization data, never current runtime state. All section relocations are
retained. Data manifest schema 3 reports `overlapping_relocations` for the selected
byte range and `unknown_relocation_extents` for the section. Integer decoding is
withheld with an explicit `unresolved` record if either count is nonzero, even
when the layout is accepted. Known fixed-width writes ending at the range start
or starting at its end do not overlap. Unknown transformations cannot establish
nonoverlap from their offset alone. The pinned structural parser currently
supplies RV32 NONE/32/64 classifications; other types retain unknown extents.
Known writes beyond the section are rejected. ET_EXEC relocation sites are
normalized from virtual to section-relative coordinates. No relocation is
applied by data export. For constants, `data.bin` is empty; the object and
analysis instruction/value records are the evidence. Analysis coverage remains
in each retained function manifest and is not promoted by successful export.

Pointer observations use `pointer_table: {"count":11,"stride":4}` in a `DataRequest`
with exactly one selected range. The captured RV32 little-endian profile reads
four-byte slots; count/stride must cover that range exactly. For proposal through
the same `knowledge propose-data` command, use
`layout: {"kind":"pointers","count":11,"stride":4}`. Acceptance records this
layout and exact source evidence; it does not accept inferred callback signatures
or manufacture resolved external definitions.

The injected RISC-V profile `rv32-absolute-rela/1` interprets `R_RISCV_32` RELA
as a physical symbol plus addend. Defined and external symbols remain distinct.
Absolute/null symbol arithmetic is modulo 2³². A retained ET_EXEC relocation must
agree with the captured linked word; an ET_REL observation leaves the original
bytes unchanged. NONE relocations do not write. Other transformations, unknown
write extents, partial/multiple writes, unsupported symbols or implicit addends
produce a typed unresolved pointer. Invalid structural bounds fail the operation.

Each `pointer` record contains index, byte offset, captured bits and a value:
`null`, `address`, `defined-symbol`, `external-symbol` or `unresolved`. A numeric
address's `image_address` flag identifies an executable ELF address domain; it
proves neither a load mapping nor an executable target. A defined symbol may be
an internal label, not a function boundary. `pointers` summary counts each class;
there is no blanket resolved/complete verdict. `pointer_producer` identifies the
interpretation. Slot lookup uses the sorted section relocation index and bounded
write widths; it does not restart a whole-section scan for every slot.

Unreviewed observation exports have no accepted assertion. Reviewed exports
include the assertion and selected knowledge revision; pending, rejected and
superseded assertions are refused at that revision. Historical acceptance can
still be read at its original revision. The output remains available after
source removal, project move and backup/restore. Review/export does not generate
Rust, publish register definitions or claim qualification. Export never overwrites
an existing directory; a failed delivery may leave a prefix and cannot be retried
implicitly.

A data export contains the selected object and explicitly selected analyses.
Interprocedural provenance may reference analyses outside that bundle; IDs do not
include their payloads implicitly. Use project backup/restore to preserve the full
research closure in the supported format.

## Saved register research

`blobray registers --project PROJECT --request query.json [--output result.json]`
reads saved research through the same supervised API as other navigation commands:

```json
{
  "scope": {
    "revision": "<revision-id>",
    "publications": [],
    "analyses": ["<analysis-id>"],
    "knowledge": null
  },
  "ranges": [{"start": 131072, "length": 256}]
}
```

The scope is explicit and frozen; `knowledge: null` selects no declarations.
Empty `ranges` selects all numeric memory candidates. Unknown/nonnumeric addresses
remain visible even with a numeric filter. Ranges filter observations and do not
classify hardware. The command does not schedule analysis or read live binaries.
JSON export uses the shared new-file publication path and works after source removal.

Schema-1 records include selected-function coverage/unavailable members, exact
original facts and record ordinals, address alternatives, read-selection and
load-preserve-OR write masks, declarations with review/evidence/applicability,
conflicts and matching bindings. Address rows aggregate candidates across the
selected analyses; they count local accesses and composed effects separately.
Instruction access widths remain a set of observations, not physical register widths.
Masks describe saved expressions, not proven physical fields or safe hardware RMW.
Finite alternatives are may-addresses; unresolved values are never discarded.

Bindings use the declaration's exact source revision, source and object. A byte
access can be contained within a wider explicitly reviewed register. Crossing
accesses are distinct. Composed effects keep their child provenance and are not
classified using the caller's declarations. Query the child analysis to inspect
its own applicable declarations. Proposed/rejected declarations remain visible;
only accepted matches contribute to the accepted-binding counter. The summary
does not promise complete hardware coverage, absence of register accesses or PASS.

Use generic knowledge proposal/review or `knowledge propose-register` for an
explicit physical interpretation; `--width` is required and is in bytes. Fields
remain explicit nonoverlapping declarations. The independent
[register tool](../../../../registers/README.md) owns source-model initialization,
SVD import, reviewed source applicability and four-output publication. A Next
review is scoped research knowledge, not an automatic hardware-source promotion.
