# Blobray knowledge validation

`blobray-knowledge` validates candidate claims and review transitions. It depends
only on domain values. It cannot open files, resolve evidence, start work or
publish a revision. Application verifies the occurrence and evidence, streams
accepted assertions through the conflict predicate, and submits a validated event
to store. Store owns durability and the expected-base transaction.

The supported claims are display names, semantic subject bindings, explicit
function extents, research hypotheses and reviewed MMIO regions/register fields. `SubjectId` is a caller-assigned key
scoped to a project. It is separate from the exact physical occurrence:
source revision, input/image source, object identity and optional symbol identity.
Names are never used to resolve physical identity.

A proposal has supporting evidence, an attributed actor and a reason. Review can
accept or reject a pending proposal. Acceptance may explicitly supersede one
conflicting accepted assertion. Other conflicts reject the transaction. The
expected knowledge base must be supplied: `null` means an empty history, not the
latest revision. There is no implicit rebase. Review attribution is recorded
text, not authentication of the reviewer.

An accepted hypothesis remains a hypothesis. Accepting a name, binding or extent
does not establish behavioral equivalence. Executable ABI models, register-publication policy and interface
validators are outside the implemented claim vocabulary.

See the [application commands and wire contracts](../../next/README.md#knowledge-and-preservation)
for evidence verification, revision selection and backup/restore ownership.
