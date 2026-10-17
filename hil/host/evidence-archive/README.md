# HIL evidence archive

`oer-hil-evidence-archive` keeps the sealed HIL runs that qualification
evidence cites outside the repository, in immutable versions
([#761](https://github.com/ermacv/open-esp-radio-rs/issues/761) Q7). It is a
`formats` package: `cargo hil archive` writes and publishes versions, and the
qualification evaluator reads a verified copy.

| Module | Owns |
| --- | --- |
| `manifest` | `Manifest`: schema, the payload's SHA-256 and size, and every `RunRef` (a run ID and the SHA-256 of its `integrity.json`) in run ID order; its bytes are its compact JSON, read back only in that form. `Version`: `evidence-<SHA-256 of the manifest>`, the release tag (rule 3) |
| `pin` | `Pin`, the version the repository tracks in `evidence/archive.toml` in `qualification/` (rule 4): the repository, the version, the manifest's digest and its fields, so the digest is recomputed offline. `unpinned`: rule 2, every citation the pinned version does not hold with the same digest; `unpinned_in`, the same against any set of runs, such as a candidate `pack` or `publish` is about to create |
| `pack` | The payload `evidence.tar.gz` of the cited runs (rule 1): the runs the pinned version already holds with the cited digest from its verified copy (`base`; the CLI downloads it), so a new version keeps them after the run store pruned them (owner decision [6231504542](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6231504542)), the others from a run store; the runs and the observer builds and source objects they name, regular files in path order with fixed metadata, so the same runs give the same bytes with the same build (another gzip backend may compress them differently and so name another version); extracted and verified before the output appears |
| `copy` | `Copy`: a version extracted into a directory laid out as a run store (`runs/`, `observers/`, `sources/`, `manifest.json`), opened only when every byte matches: each run's seal and its digest, and exactly the named files the runs refer to, each hashing to its name. `unpack` hashes the payload as it reads it and extracts only regular files at canonical paths below a held run, `observers/` or `sources/`, once each, within file and size limits |
| `release` | `publish` and `fetch` through `Releases`, the transport the caller supplies (the CLI's runs `gh`; tests use one in memory): a new version becomes a draft, is read back and verified, then published; a draft missing an asset (an interrupted upload) is refused with its recovery, deleting the draft by hand, which does not break rule 5: a draft is not yet in the archive, and nothing reads it; an existing version or an interrupted draft is accepted only when it holds exactly `manifest.json` and `evidence.tar.gz` and they are the version's bytes; nothing is overwritten or deleted (rule 5). `fetch` refuses an existing output before downloading and reads only a published version |

`Refused` marks a broken archive rule (a digest, a seal, a member, a
version); any other error is the environment's. The `cargo hil archive`
commands map them to exit status 1 and 3.

`cargo qualification check references` refuses a pinned version that holds
a run no record cites (rule 1). The first `cargo hil archive publish`
therefore lands in the same pull request as the first record that cites a
run, with its citation field, the field's reader in `check references` and
its test.

The archive repository is private. `cargo qualification check references`
checks the pin and rule 2 offline, and readiness against evidence is evaluated where the archive is reachable
(`cargo qualification evaluate --archive DIR`), owner decision
[6225000924](https://github.com/ermacv/open-esp-radio-rs/issues/761#issuecomment-6225000924).
