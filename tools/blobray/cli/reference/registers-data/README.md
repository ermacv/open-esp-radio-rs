# Registers and captured data

Inspect MMIO candidates and captured data.

[Choose a task](../../../README.md#choose-a-task) · [All command references](../../README.md#reference-navigation).

## Captured data, tables and coefficients

`blobray_application::data::export(request, executables, memory, control)`
reads exact ranges from one captured object in process. It does not run an
analyzer or infer a table boundary.

A `DataRequest` has the `object` (`ObjectId`: the containing executable's
content and the standalone or archive-member location), an optional anchor
`symbol` and 1–32 `ranges`. Copy the object identity from an inventory; it
retains the archive member ordinal even when names and bytes repeat. The
executable the object names must be among `executables`. Each range uses one
of these selectors:

| Selector | JSON fields in addition to `kind` | Meaning |
| --- | --- | --- |
| `section` | `section`, `offset`, `length` | Explicit section-relative bytes |
| `symbol` | `symbol`, `length` (integer or null) | Exact static or dynamic SymbolId of this object; null uses its declared size |
| `image` | `address`, `length` | Virtual address in a file-backed load mapping of an executable ELF |

The prepared-object profile requires little-endian RV32 ET_REL/ET_EXEC with at most one each
of SHT_SYMTAB and SHT_DYNSYM; table-free section/range selection is supported; names
such as `.symtab` are not table identity. Dynamic symbol selection does not enable
dynamic loading, TLS or relocation application. Section relocations must reference
the static table through `sh_link`; a different table is an explicit unsupported
profile, never an index interpreted in the static table. A zero-sized symbol needs
an explicit length. A sized symbol cannot be expanded past its declared size.
Overflow, out-of-range, ambiguous or unmapped addresses, compressed sections and
NOBITS are explicit errors. Requests never guess length from the next symbol.
Several ranges share one prepared ELF owner and section metadata; borrowed views
cannot escape its callback.

The `DataExport` returns the object's content identity, the selected bytes
concatenated in range order, and one span per range: selector, section and
section range, file range, image address when known, writability, the SHA-256
of its bytes and its offset into the concatenated bytes.

Writable sections are marked as initialization data, never current runtime
state. Each span reports the section's relocation count,
`overlapping_relocations` for the selected byte range and
`unknown_relocation_extents` for the section. Known fixed-width writes ending at the range start
or starting at its end do not overlap. Unknown transformations cannot establish
nonoverlap from their offset alone. The pinned structural parser currently
supplies RV32 NONE/32/64 classifications; other types retain unknown extents.
Known writes beyond the section are rejected. ET_EXEC relocation sites are
normalized from virtual to section-relative coordinates. No relocation is
applied by the export.

An export has no accepted assertion. It does not generate Rust, publish
register definitions or claim qualification.

## Library register accesses

`register-accesses` analyzes every function of the given libraries in its own
process and reports the memory addresses they access. Inputs are read once and
identified by content, and nothing is retained.

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
analyzed, partial and blocked functions, gaps and observations. `cargo registers
inventory` compares these accesses with the register model.

Observations name no hardware registers: the reviewed register model owns
register identity and fields. Instruction access widths remain observations,
not physical register widths. Masks describe analyzed expressions, not proven
physical fields or safe hardware RMW. Finite alternatives are may-addresses;
unresolved values are never discarded. The summary does not promise complete
hardware coverage or absence of register accesses.

The independent [register tool](../../../../registers/README.md) owns
source-model initialization, SVD import, reviewed source applicability and
four-output publication. A Blobray observation is research evidence, not an
automatic hardware-source promotion.
