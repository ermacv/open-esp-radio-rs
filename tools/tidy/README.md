# Tidy

`oer-tidy` is the repository's fast integrity tier. It reads the checkout as
text, with no Cargo resolution and no compilation, so every check over the
whole tree takes seconds. Each check fails closed and lists the offending
paths.

```console
cargo xtask check tidy
cargo run -q -p oer-tidy -- workspaces [--json]
cargo run -q -p oer-tidy -- chips [--json]
```

The push gate always runs it, and CI runs it as an early job of its own.
`cargo tidy` is the alias of `cargo run --quiet -p oer-tidy --`.

It is also the repository's one model of its own structure. `workspaces`
lists every Cargo workspace and `chips` every chip profile
(`platform/<chip>/chip.toml`, read through `oer-chip-profile`), one per line
or as a JSON array; `oer-xtask` (lock, metadata, the push gate) uses the same
library, and CI builds its job matrices from the JSON. `fetch` downloads
what every workspace's lock names and the Cargo cache lacks, online only for
a workspace that misses something; it needs only this small package built,
so it works after a pull that changed what xtask itself depends on. Package
classification is read by [`classification`](src/classification.rs) alone:
`oer-xtask` classifies the packages it audits through it.

| Check | Contract |
| --- | --- |
| Orphan sources | Every tracked Rust file is reachable from a crate root of some package: library, binaries, tests, benches, examples and build script, followed through `mod`, `#[path]`, `#[cfg_attr(…, path = …)]` and `include!`, whatever their `cfg`. Every module declaration and explicit target names an existing file |
| Anchors and citations | `// CAPABILITY:` anchors and vendor `SOURCE` citations appear only in reachable files, since qualification and the provenance check trust them as statements about compiled code |
| Record paths | Every repository path a qualification program or catalog names exists, every `packages` entry names a package, and every evidence shard below `verification/<chip>/evidence/` or a program's evidence directory records only existing sources. Output directories a program names (`evidence-index`, `[hil] evidence`, `runs`) need not exist |
| Workspaces | Every package belongs to a workspace as Cargo finds it, every listed member exists, and every workspace root has its `Cargo.lock` and every lock file a root. `workspaces` prints the discovered list, which CI formats |
| Classification | Every package declares `[package.metadata.open-radio]` with known keys only, a known `layer` (which implies the scope: production, experimental or development) and a consistent `platform`; a `chip` names a chip with a profile and a `family` the family of some chip; every package outside the Blobray workspace is `oer-<tokens>` and only the facade is `open-esp-radio`; verification packages declare an `evidence` role and HIL packages a `hil` role, no verdict package depends on a report package and no observation package on a stand operation package; every `inputs` pattern (files outside the package its tests read, which the push gate follows) matches a file |
| Unused dependencies | Every declared dependency is named by its package: as an identifier, in a string or doc comment of a reachable file, in an `include_str!` document, or as the target of a `dependency/feature` forward. Plain comments do not count |

[`allowlist.toml`](allowlist.toml) holds the reviewed exceptions, each with a
reason: Rust files that tests compile directly with `rustc`, and dependencies
declared only to select features of a crate the graph already uses. An entry
that no longer excuses anything fails its check.

The checks are text rules, not compilation: a module behind a macro or a
dependency used only through another crate's macro expansion needs an
allowlist entry. Compiler, Clippy and architecture checks remain the
authority on what builds.
