# Vendor contracts

Architecture- and platform-neutral immutable contract types shared by
analysis backends and optional knowledge providers. The crate owns provenance,
semantic identities and physical artifact locators, without instruction or
chip-specific semantics. `ObjectLocation` identifies an archive payload by its
ordinal; `SymbolLocation` adds the symbol table and index. Names and addresses
remain metadata and cannot collapse separate occurrences. These locators must
be qualified by the containing artifact's content identity. `CodeIdentity`
provides this qualification for physical symbols and section ranges, and
separates them from modeled boundaries and explicitly synthetic code.
`DataIdentity` qualifies data symbols and zero-sized anchors in the same way;
an anchor's inferred byte extent is not part of its identity.
`SymbolReference` retains a relocation's physical target entry and
`SymbolBinding` distinguishes local, global, weak and undefined symbols.
Non-local definitions remain candidates until link selection is established.
Unresolved references carry an explicit `unknown` reason.
`DataAddressResolution` records all candidate data definitions containing a
numeric access, qualified by `DataIdentity`, without changing its expression.
Its basis distinguishes a concrete access from indexed-base and offset hints;
a read-location hint may have unknown width. Overlaps and aliases do not grant
one candidate precedence. Unknown outcomes retain a typed reason.

Executable provider descriptors currently expose execution-model registries
and admission through the execution-model dependency. Artifact identity hashing
uses SHA-256; serialized identities are validated when read.
