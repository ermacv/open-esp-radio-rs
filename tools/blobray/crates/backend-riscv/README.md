# RISC-V vendor-binary backend

RV32 ELF/archive decoding, relocation handling, symbolic analysis, concrete
execution, final-image auditing and Rust reference generation for the explicit
`riscv32` + `riscv-ilp32` backend pair.

Platform ABI tables and reviewed semantic summaries are injected through a
typed harness specification. The backend depends on the analysis model and
`open-radio-vendor-contracts`, never on a chip or production driver.
Reviewed memory facts are declared in `analysis-model`; this backend retains
its public re-exports for existing consumers. Pointer-layout recognition and
all instruction/ABI semantics remain here.

`CapturedArtifact` owns immutable container bytes and their SHA-256 identity.
Its object catalog retains physical archive order, including unsupported
payloads. Code queries share lazily parsed object catalogs and select exact
symbols through `SymbolLocation`; repeated member names remain distinct.
Static and dynamic symbol-table occurrences both remain selectable.
`object_bytes` exposes captured payloads even when code decoding is unavailable.
Thin members without captured external bytes are reported explicitly.
Data symbols and static initializers use lazy catalogs on the same capture;
repeated data occurrences are retained. Initializer records carry `DataIdentity`
qualified by the artifact digest, object ordinal and symbol table/index.
Both tables, unnamed definitions and co-located anchors remain queryable.
An anchor's candidate extent ends at the next definition or section boundary;
`synthetic_from_anchor` distinguishes this inference from a declared size.
`ReferenceResolver::from_captured`
builds its code, data and execution context without reopening source files.
`ExecutableImage::from_captured` and `add_captured_companion` copy execution
memory from those bytes. The caller owns the captures; resolver and execution
images own their derived state and cannot mutate the captured evidence.
`CapturedArtifact::from_shared` shares ownership of immutable `Arc` bytes with
the application source set without copying the container. Path-based convenience
constructors create their own captures. Execution-image companion merging
still keeps the first name binding and permits overlapping memory ranges;
capturing bytes does not establish that the composed address layout is valid.

Every code definition carries a `CodeIdentity`: captured artifact digest plus
physical symbol location, or a distinct section-range identity for reviewed
code boundaries. Display names and placement cannot merge occurrences.
Resolver work queues, relocated call ownership and semantic proofs retain this
identity. Ambiguous human selectors fail explicitly. `inspect_captured_function`
accepts an exact location and recovers labels from that same object's symbol
table; decoded bodies retain the identity. Synthetic code and modeled external
boundaries have separate identity variants and never imply captured code.
Competing projected origins remain available as candidates; registering a new
candidate does not replace previous evidence.

Code relocations retain their target symbol's physical occurrence and binding.
PC-relative LO relocations inherit the paired HI target; completing an address
requires the same target reference, not only the same symbol name. Symbolic
addresses and memory roots retain this reference through arithmetic and pointer
loads. Missing display names do not prevent relocation analysis. Full body
inspection exposes the reference on each instruction's relocation.

Interface discovery joins finite value alternatives without removing known
values when another path is unknown. Loads and supported arithmetic distribute
across those alternatives. The default function budget is 4096 state updates
and 64 alternatives per register; `discover_interface_calls_with_limits`
accepts explicit limits. Reaching a budget stops propagation and records every
pending state's complete register snapshot. Known candidates remain available;
this is an incomplete result, not a converged analysis. Gaps also record
unresolved indirect calls/stores, unsupported instructions and unmodeled value
transforms. Alternative combinations are candidates, not correlated path proofs.
Store observations retain both the destination and loaded or offset target.
Relocated interface roots preserve the relocation's physical target reference.
Function-argument roots include the owning `CodeIdentity`, so argument zero in
separate functions cannot merge into one table. Arithmetic, pointer loads and
value alternatives retain that ownership; canonical expressions remain display
labels and are not physical identities.

`ReferenceResolver` owns reviewed semantic projection through
`SemanticProjectionCatalog`. An archive origin selected by name remains a
candidate. The provider authenticates the raw body, and the backend verifies
the transfer before allowing an exact annotation or opaque body boundary.
The implemented transfer proof covers identical, relocation-free bodies with
closed control flow and no unproved PC-dependent operations. Relocations,
relaxation and calls requiring a link transformation proof remain explicit
`SemanticProjectionGap` records; byte or instruction-shape similarity does not
authorize them. Proofs are bound to the inspected destination body and layout.
Conflicting verified annotations remain visible and disable automatic selection.

`ExecutableImage::load_entry` accepts a linked ELF or a regular static archive.
For an archive, it links the selected entry and its dependencies into a temporary
RV32 analysis image using the installed Rust toolchain's `rust-lld`.
`BLOBRAY_RISCV_LINKER` can select a GNU-compatible RV32 linker instead.
The analysis link disables relaxation and retains relocations. Missing callees
remain unresolved, and missing data definitions poison their relocation sites;
reaching either cannot establish equivalence. Unrelated unreachable definitions
do not prevent execution. Link errors (including conflicting definitions and
unsupported relocations) are reported explicitly.

`advanced execute run` and `advanced execute compare` use this entry loader.
Reports identify the original archive and its hash. Archive code addresses use
an analysis placement starting at `0x40000000`; they are not firmware addresses
or evidence of final linker placement. Authentic firmware ELF images remain
necessary when the behavior depends on final placement or runtime initialization.

Concrete `Scenario::arguments` contains RV32 integer ABI words. The first eight
use `a0` through `a7`; up to 64 additional words occupy the executor's private
stack starting at the 16-byte-aligned entry SP, following the
[RISC-V integer calling convention](https://riscv-non-isa.github.io/riscv-elf-psabi-doc/#_integer_calling_convention).
The caller supplies scalar widening and any multiword padding. This is not an
automatic aggregate, variadic or floating-point ABI classifier. Stack argument
storage is private to each invocation; conflicting explicit byte seeds are
rejected and uninitialized stack bytes retain the configured poison/fill policy.
Call observations continue to record the eight integer argument registers.
