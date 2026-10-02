# Tidy

`oer-tidy` is the repository's fast integrity tier. It reads the checkout as
text, with no Cargo resolution and no compilation, so every check over the
whole tree takes seconds. Each check fails closed and lists the offending
paths.

```console
cargo xtask check tidy
cargo run -p oer-tidy -- workspaces
```

`check changed` always runs it, and CI runs it as an early job of its own.

| Check | Contract |
| --- | --- |
| Orphan sources | Every tracked Rust file is reachable from a crate root of some package: library, binaries, tests, benches, examples and build script, followed through `mod`, `#[path]`, `#[cfg_attr(…, path = …)]` and `include!`, whatever their `cfg`. Every module declaration and explicit target names an existing file |
| Anchors and citations | `// CAPABILITY:` anchors and vendor `SOURCE` citations appear only in reachable files, since qualification and the provenance check trust them as statements about compiled code |
| Record paths | Every repository path a qualification program, catalog or HIL review names exists, every `packages` entry names a package, and every evidence shard below `verification/<chip>/evidence/` or a program's evidence directory records only existing sources. Output directories a program names (`evidence-index`, `[hil] evidence`, `runs`) need not exist |
| Workspaces | Every package belongs to a workspace as Cargo finds it, every listed member exists, and every workspace root has its `Cargo.lock` and every lock file a root. `workspaces` prints the discovered list, which CI formats |
| Unused dependencies | Every declared dependency is named by its package: as an identifier, in a string or doc comment of a reachable file, in an `include_str!` document, or as the target of a `dependency/feature` forward. Plain comments do not count |

[`allowlist.toml`](allowlist.toml) holds the reviewed exceptions, each with a
reason: Rust files that tests compile directly with `rustc`, and dependencies
declared only to select features of a crate the graph already uses. An entry
that no longer excuses anything fails its check.

The checks are text rules, not compilation: a module behind a macro or a
dependency used only through another crate's macro expansion needs an
allowlist entry. Compiler, Clippy and architecture checks remain the
authority on what builds.
