# Project navigation index

The current index schema is version 2. Authenticated input paths are stored
relative to the index itself, so checking the same generated project from the
workspace root or from the project directory has identical results. Absolute
host paths are rejected by the strict reader.

The optional project navigation index joins independently generated symbol,
linked-IR, and interface facts without changing their schemas or making one
analyzer semantically depend on another:

```toml
[analysis.symbols]
output = "generated/findings/symbols.json"

[analysis.navigation]
output = "generated/findings/navigation.json"
```

`project analyze` writes the index after every configured input report has
completed. `project analyze --check` reconstructs it in memory and compares
the exact bytes. The index is generated evidence: users edit reviewed
register, interface, function, and semantic packs rather than this file.

## Stable identity

Each symbol location receives an opaque `symbol-v1:` ID derived from:

- the SHA-256 of the artifact bytes;
- the archive member, when present;
- the symbol name;
- the symbol's object address.

This distinguishes same-named local functions, archive members, and different
binary revisions. It also remains stable when a private artifact or project is
moved to another filesystem path. The structured components remain beside the
opaque ID so manual tools do not have to decode it.

The join deliberately uses `object_offset` from linked IR rather than a
runtime address. A linked ELF normally has equal object and runtime addresses;
a relocatable archive member does not. Artifact indices and paths are display
metadata, never identity components.

## Associations

Every symbol entry can contain four independent observation sets:

- `inventory`: symbol-table origin, definition kind, object kind, and the
  conservative linkage classification;
- `linked_ir`: profile ID, generated IR identity, and selection reason;
- `interface_calls`: indirect-call instruction sites found in that function;
- `interface_roots`: relocated or absolute pointer roots associated with that
  symbol fact.

An entry may contain only one of these. Absence means that the corresponding
configured analysis did not observe the location, not that the code or
behavior does not exist. `unmatched_interface_roots` is retained in the
summary so ambiguous or missing roots stay visible during manual review.

The index declares `semantic_claim=false` and
`linker_resolution_claim=false`. RTOS, NVS, logging, delay, slot ABI, register,
and W1C semantics remain in their reviewed packs. Exact linker selection still
comes from a fully linked ELF, not from navigation associations.

## Use from tools

Run the ordinary project workflow:

```console
cargo vendor-binary-workbench project analyze \
  --project path/to/vendor-project.toml \
  --run-spec /private/local.toml

cargo vendor-binary-workbench project status \
  --project path/to/vendor-project.toml
```

`project status` reports artifact/symbol counts, linked-IR functions,
interface callers and roots, and unmatched roots. The index also records the
path and SHA-256 of each input report, making stale manual joins auditable.
