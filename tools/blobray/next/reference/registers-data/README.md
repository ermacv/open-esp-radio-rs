# Registers and captured data

Inspect MMIO candidates and captured data.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Captured data, tables and coefficients

`data` reads exact ranges and selected saved analyses from one captured object.
It does not run an analyzer or infer a table boundary. It uses the same
application-owned query, supervision and delivery budget as other reads.

```console
blobray data --project research --request data-request.json --limit-mode watchdog
blobray data --project research --request data-request.json --output observations --limit-mode watchdog
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

`--output` creates a new directory containing `manifest.json`, `object.elf`,
`data.bin` and `records.jsonl`. The object is the exact captured ELF member/image,
not its enclosing archive. The manifest records its digest, occurrence, source
file and section ranges, optional image addresses, per-range digests and offsets
into concatenated `data.bin`. The records preserve analysis IDs and ordinals;
`ranges` links known relocation references into the selected ranges. Empty links
do not assert absence of other uses.

Writable sections are marked as initialization data, never current runtime
state. All section relocations are retained. Data manifest schema 4 reports
`overlapping_relocations` for the selected byte range and
`unknown_relocation_extents` for the section. Known fixed-width writes ending at the range start
or starting at its end do not overlap. Unknown transformations cannot establish
nonoverlap from their offset alone. The pinned structural parser currently
supplies RV32 NONE/32/64 classifications; other types retain unknown extents.
Known writes beyond the section are rejected. ET_EXEC relocation sites are
normalized from virtual to section-relative coordinates. No relocation is
applied by the export. Analysis coverage remains in each retained function
manifest and is not promoted by a successful export.

Pointer observations use `pointer_table: {"count":11,"stride":4}` in a `DataRequest`
with exactly one selected range. The captured RV32 little-endian profile reads
four-byte slots; count/stride must cover that range exactly. The observation
does not infer callback signatures or manufacture resolved external definitions.

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

Observation exports have no accepted assertion. The output remains available
after source removal, project move and backup/restore. An export does not
generate Rust, publish register definitions or claim qualification. It never
overwrites an existing directory; a failed delivery may leave a prefix and
cannot be retried implicitly.

A data export contains the selected object and explicitly selected analyses.
Interprocedural provenance may reference analyses outside that bundle; IDs do not
include their payloads implicitly. Use project backup/restore to preserve the full
research closure in the supported format.

## Library register accesses

`register-accesses` analyzes every function of the given libraries in its own
process and reports the memory addresses they access. It keeps no project:
inputs are read once and identified by content, and nothing is retained.

```console
blobray --format json register-accesses --input libphy=/path/to/libphy.a --range 0x20000000:0x9c00
```

Each `--input ROLE=PATH` is an archive or ELF; a function names its input by
position. Every STT_FUNC symbol of a nonempty executable section of every
object is analyzed. Each `--range START:LENGTH` selects resolved addresses that
overlap it; without a range every numeric address is reported, and unresolved
addresses are reported either way. `--working-memory-mib`, `--timeout-secs`
and `--max-work-units` bound the analysis cooperatively.

The JSON document streams `{"schema":1,"inputs":[...],"records":[...],"summary":{...}}`.
`inputs` lists each input's role and SHA-256. A record is an `observation` (the
function, the record ordinal and exact original fact, the address or null, its
alternative index and a read-selection or write-replacement mask), a `blocked`
function whose analysis stopped (for example an unknown extent), or a `gap`
naming code no function was selected from: an unsupported object, a thin
archive member, a malformed container or an ELF diagnostic. The summary counts
analyzed, partial and blocked functions, gaps and observations. `cargo xtask
register-inventory` compares these accesses with the register model.

## Saved register research

`blobray registers --project PROJECT --request query.json [--output result.json]`
reads saved research through the same supervised API as other navigation commands:

```json
{
  "scope": {
    "revision": "<revision-id>",
    "publications": [],
    "analyses": ["<analysis-id>"]
  },
  "ranges": [{"start": 131072, "length": 256}]
}
```

The scope is explicit and frozen. Empty `ranges` selects all numeric memory candidates. Unknown/nonnumeric addresses
remain visible even with a numeric filter. Ranges filter observations and do not
classify hardware. The command does not schedule analysis or read live binaries.
JSON export uses the shared new-file publication path and works after source removal.

Schema-2 records include selected-function coverage/unavailable members, exact
original facts and record ordinals, address alternatives, and read-selection and
load-preserve-OR write masks. Address rows aggregate candidates across the
selected analyses and count their local accesses.
Instruction access widths remain a set of observations, not physical register widths.
Masks describe saved expressions, not proven physical fields or safe hardware RMW.
Finite alternatives are may-addresses; unresolved values are never discarded.

Observations name no hardware registers: the reviewed register model owns
register identity and fields. The summary does not promise complete hardware
coverage, absence of register accesses or PASS.

The independent [register tool](../../../../registers/README.md) owns
source-model initialization, SVD import, reviewed source applicability and
four-output publication. A Blobray observation is research evidence, not an
automatic hardware-source promotion.
