# Blobray artifacts

`blobray-artifacts` owns structural ELF/AR inventory over borrowed, already
captured bytes. Its only internal dependency is
[domain](../domain/README.md). It never resolves a filesystem path, acquires a
writer, decodes instructions or selects a linker definition.

`MemberCursor` enumerates physical payload ordinals from the domain `ByteSource`
port. It reads metadata and names on demand, returning a scoped name and a payload
range or an external-member request. GNU/BSD/COFF metadata and AIX index entries
never consume payload ordinals; AIX traversal follows the finite index rather
than input-controlled links. Cursor framing errors terminate enumeration; the
application records unknown remaining membership. Resource errors abort the run.

`inspect_source` hashes a payload with bounded reads. Unknown formats need only a
prefix; ELF32/64 parsing requires one admitted contiguous object buffer and name
workspace. `inspect_payload` also supports caller-owned bytes. Both require
explicit `WorkingMemory` and `RunControl` ports. They emit sections, symbols,
relocations and diagnostics synchronously through `ElfSink`, returning only ELF
header metadata. There is no container-wide inventory or symbol-set accumulator.
The former materializing `Container` interface is replaced by these ports.

A `ReadRef` adapter bounds ELF delimiter scans; table visits, name copies and
range reads participate in cooperative work accounting. Sink failures preserve
their original error and cannot become malformed-object diagnostics. The caller
owns any retained record copies and their memory capacity. Borrowed views cannot
outlive the admitted input buffer. See the
[memory boundary](../../next/README.md#current-memory-boundary) for admission
accounting and the single-ELF size limit.

ELF inspection reads raw section/symbol/relocation tables, including null and
local symbols and malformed-name entries. Table kind, section and entry index
remain part of symbol identity. The
[implemented scope](../../next/README.md#identity-and-schema-1) defines supported
formats and coverage limits. No inference here establishes linking, target
execution support or verification success.

[Image validation](src/image.rs) checks captured link inputs and prepared RV32
ELFs using admitted object buffers. It rejects unsupported ABI/TLS, ambiguous
roots, invalid segment placement and unresolved allocated relocations. Application
supplies exact map-derived root addresses; artifacts checks the output ELF against
them and never chooses a different source definition. Successful validation proves
the synthetic image contract, not firmware placement or execution readiness.

[Function views](src/function.rs) lend one admitted captured ELF object's selected
code range, physical relocation targets and ELF data/code mapping ranges to a
callback. The borrowed view cannot outlive its reservations. A missing symbol
extent requires an explicit caller range; this module does not infer function
boundaries or interpret ISA relocations. Reserved null symbols remain explicit
structural records instead of fabricated external definitions.

Static ET_EXEC functions use image virtual addresses and a borrowed `ImageMemory`
view over validated load segments. Input buffers and the segment table retain
working-capacity reservations until the consumer returns. Read-only file-backed
loads can provide constants; mutable, unmapped and zero-fill ranges cannot.
ELF ILP32/ILP32F/ILP32D ABI is recorded independently of ISA semantic coverage.
Static relocation metadata is retained without reapplying edits to linked bytes.


`execution_segments` lends validated static RV32 ELF segment bytes to a caller's
admitted loader. It shares `ProgramView` validation with static image research,
including overlapping mappings, permissions, dynamic/TLS rejection and input
capacity. Borrowed bytes expire after the callback; application owns any copied
mutable memory. This port performs no relocation, model selection or execution.

`executable_sections` lends validated static RV32 executable sections independently
of function symbols. Missing section coverage fails the final-image audit instead
of being interpreted as a clean empty program.

Function analysis and final-image audit share mapping-symbol validation and
admitted interval storage. Only local zero-sized `$d`/`$x` markers authorize data
intervals. ISA-qualified `$xrv32…` markers also end data intervals; other XLENs,
conflicting, misaligned code or out-of-section markers fail closed.

`with_prepared_object` owns one captured buffer, ELF/program view and lazily prepared sections. Callback-borrowed function views share target names and section metadata; object scope releases their admitted capacity. `MemberIndex` traverses an archive once and preserves ordinal payload identity, including thin-member markers.

An ordinal index retains valid prefix entries when a later archive header is malformed and exposes the terminal integrity error. Selecting an unavailable later ordinal returns that error. Inventory/publication coverage owns the incomplete-membership claim; an index never asserts archive completeness. Resource, I/O and cancellation failures abort indexing.
