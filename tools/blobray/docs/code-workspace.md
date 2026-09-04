# Reviewed code boundaries

The code workspace overlays human decisions on the current symbol inventory.
`advanced code init-pack` creates the project identity and source SHA-256 guards;
it does not copy the generated candidate inventory into reviewed TOML. Missing
boundary decisions are unreviewed. Discovery can add or remove unreviewed
candidates without editing the pack.

Use `advanced code review --project PATH` to render the current candidates,
source/member identities, ranges and evidence. To accept a candidate, add a
boundary decision using its exact source guard and identity:

```toml
[[boundaries]]
source = "rom"
artifact-sha256 = "<current artifact SHA-256 from the review>"
# Set member for an archive member; omit it for a standalone image.
section = ".text"
entry-offset = 0x10
end-exclusive-offset = 0x20
status = "accepted"
name = "recovered_function"
```

The reviewed end must stay inside the generated candidate limit. Accepted names
must be unique identifiers. For rejection, use `status = "rejected"`, omit
`name`, and provide a non-empty `reason`. Leave candidates without a decision
out of the pack. `advanced code validate --project PATH` checks all explicit
decisions and source guards against the current inventory. The generated review
and TUI still show every unreviewed candidate.

Schema 1 packs containing `status = "unreviewed"` remain readable. Those legacy
backlog rows have no authority over candidate ranges or continued existence;
loading reconstructs their state from current facts, and rebasing omits them.

`advanced code rebase --check --project PATH` checks whether the overlay is
current. `--apply` can refresh source guards when no human decisions are affected.
New generated candidates alone do not require a rebase. The rebase summary counts
reviewed decisions, so its retained `added` field is zero for a sparse overlay.

An accepted or rejected decision cannot be carried to a different artifact
SHA-256 solely because its offsets still fit. A changed digest, invalid range,
or removed reviewed candidate requires fresh review and blocks `--apply`.
`--output PATH` writes a candidate with applicable decisions active and former
inapplicable decisions preserved as comments. The original pack remains intact.
Inspect the new artifact evidence before re-entering a decision with its new
guard; commented decisions are intentionally inactive.
