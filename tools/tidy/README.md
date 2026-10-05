# Tidy

`oer-tidy` is the repository's fast integrity tier: text policy over the
repository model of [`oer-repo`](../repo/README.md), with no Cargo resolution
and no compilation, so every check over the whole tree takes seconds. Each
check fails closed and lists the offending paths.

```console
cargo tidy check
cargo tidy workspaces --json
cargo tidy chips --json
cargo tidy fetch
```

`oer-tidy` has its own workspace and lock file containing only registry
packages and host foundations. A changed firmware Git pin cannot prevent
its offline startup. `cargo tidy` runs `cargo run --quiet --manifest-path
tools/tidy/Cargo.toml -p oer-tidy --`; `--root DIR` checks another checkout.
Its host build outputs live in `tools/tidy/target/`.
The push gate invokes it as a process, and CI runs it as the first check of
the registry's `host` job (`cargo xtask check tier full --job host`). Like every
repository tool, it prints its commands for `__command-tree`, which `cargo
xtask check docs` holds the documentation to.

`workspaces` prints every Cargo workspace and `chips` every chip profile
(`platform/<chip>/chip.toml`) of that model, one per line or as a JSON
array. `fetch` downloads what
every workspace's lock names and the Cargo cache lacks, online only for a
workspace that misses something; it needs only this small package built,
so it works after a pull that changed what xtask itself depends on.

| Check | Contract |
| --- | --- |
| Orphan sources | Every tracked Rust file is reachable from a crate root of some package: library, binaries, tests, benches, examples and build script, followed through `mod`, `#[path]`, `#[cfg_attr(…, path = …)]` and `include!`, whatever their `cfg`. Every module declaration and explicit target names an existing file |
| Anchors and citations | `// CAPABILITY:` anchors and vendor `SOURCE` citations appear only in reachable files, since qualification and the provenance check trust them as statements about compiled code. Each marker has one recogniser: anchors `oer_tidy::anchors` (the qualification evaluator uses it), citations `oer_vendor_provenance::citation` (the provenance check's) |
| Record paths | Every repository path a qualification program or catalog names exists, every `packages` entry names a package, and every evidence shard below `verification/<chip>/evidence/` or a program's evidence directory records only existing sources. Output directories a program names (`evidence-index`, `[hil] evidence`, `runs`) need not exist |
| Workspaces | Every package belongs to a workspace as Cargo finds it, every listed member exists and belongs to the workspace claiming it; a package crossing a nearer workspace boundary explicitly declares `package.workspace`. Every workspace root has its `Cargo.lock` and every lock file a root. `workspaces` prints the discovered list, which CI formats |
| Classification | Every package's `[package.metadata.open-radio]` parses into `oer-repo`'s typed classification (known keys only, a known `layer`, which implies the scope, a consistent `platform`, verification packages an `evidence` role, HIL packages a `hil` role, every host development package a `host-layer`, feature sets naming declared features); a `chip` names a chip with a profile and a `family` the family of some chip; every package outside the Blobray workspace is `oer-<tokens>` and only the facade is `open-esp-radio`; every `inputs` pattern (files outside the package its tests read, which the push gate follows) matches a file |
| Layer dependencies | Every dependency keeps `oer-repo`'s policy ([layer dependencies](../../docs/architecture.md#layer-dependencies)): a production package's normal and build path dependencies are production packages of an allowed layer and platform, never the facade; only adapters, compositions and the facade bind an executor or the time driver; no verdict package depends on a report package, no HIL observation package on orchestration or stand operation, and no host package on a higher [host layer](../../docs/architecture.md#host-layers) |
| Host applications | Every host-layer package declares a known `host-app` and `host-boundary` (`application`, `library`, `format`). Command lines are application code; shared foundations, devices, images and analysis are libraries, and shared data formats are `formats/format`. Normal and build dependencies follow the [application boundaries](../../docs/architecture.md#host-applications); private code of another application is never linked |
| Command-line spawns | Only packages of the entry host layer run `cargo hil` or `cargo xtask`: outside comments, no other package's reachable Rust holds `.arg("hil")`, `.arg("xtask")`, an argument list starting with either, or the CLI packages' names `"oer-hil-cli"` and `"oer-xtask"` (`oer_tidy::spawns`); a library calls the owning library instead |
| Unused dependencies | Every declared dependency is named by its package: as an identifier, in a string or doc comment of a reachable file, in an `include_str!` document, or as the target of a `dependency/feature` forward. Plain comments do not count |
| Reviewed layouts | A `// REVIEWED-LAYOUT: <package> <version>` anchor marks code that relies on a foreign type's layout at one release (an `unsafe impl` over another crate's type); the lock file of the workspace that compiles the file must still resolve the package to that version or to a git revision starting with it, so a dependency update fails until the layout is reviewed again. A guarantee the other crate documents needs no anchor |

[`allowlist.toml`](allowlist.toml) holds the reviewed exceptions, each with a
reason: Rust files that tests compile directly with `rustc`, and dependencies
declared only to select features of a crate the graph already uses. An entry
that no longer excuses anything fails its check.

The checks are text rules, not compilation: a module behind a macro or a
dependency used only through another crate's macro expansion needs an
allowlist entry. Compiler, Clippy and architecture checks remain the
authority on what builds.

Rules that need a chip's platform layout as a library — no zeroed region in
a `link_section` literal, esp-hal's `static-interrupts` beside `esp32s31` —
are part of `cargo xtask check architecture`, so tidy depends on no chip.
