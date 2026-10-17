# Host toolchain

`oer-toolchain` is the one place repository tools locate the host programs
builds run and record which ones ran.

- `program(Tool)` / `command(Tool)`: a non-empty override variable names the
  program (`CARGO`, `RUSTC`, `ESPFLASH`, `LLVM_NM`, `LLVM_OBJCOPY`,
  `LLVM_OBJDUMP`). Otherwise Cargo, `rustc` and `espflash` come from `PATH`,
  and the LLVM tools from the `llvm-tools` component of the active Rust
  toolchain (`<sysroot>/lib/rustlib/<host>/bin`, the toolchain
  `rust-toolchain.toml` pins), never from an unrelated LLVM on `PATH`.
- `require(Tool)` fails unless the tool runs; `versions()` lists every tool
  with the program and the version it reports, which image builds record as
  evidence (`llvm-objdump` included).
- `image::configure` applies the image compiler setup to an image's Cargo
  command: the unstable stack and move flags under `RUSTC_BOOTSTRAP`, the
  stack-size section for C and C++, and the image linker, which
  `image::install_linker` builds from the tree being built (a checkout or a
  source snapshot) through `oer-process` and installs under
  `<build root>/image-linkers/<identity>/` as a hard link to the built binary
  (a copy only across file systems), its modification time renewed at
  every installation as its last use, and refusing a build when a source its
  dep-info names changed after it. The identity is a digest of the linker's inputs (its dep-info,
  their package manifests, the workspace manifest, lock, Cargo configuration
  and toolchain pin) and of `rustc -vV`, so trees with the same linker
  sources name one linker path and a changed linker gets a new one.
- `image::compile_cache(build_root, root)` is the firmware compile cache of
  the tree at `root`, a `CompileCache`: `<build root>/firmware/<digest of the
  canonical root path>`, whose `tree` and `tree-id` files record the tree's
  path and its root directory's device and inode. Cargo names a path package
  by its path relative to the workspace root and judges freshness by
  modification times, so a cache two trees shared would hand one tree units
  compiled from the other's sources (#793). Each tree compiles only in its own
  cache, one build at a time (`CompileCache::lock`, which records the tree
  again under the lock). A different tree moved to the same path finds the
  old tree's cache by its identity; it is removed and the tree starts afresh.
  Registry and Git crates are shared between trees only by a
  content-addressed compiler cache such as `sccache` (`build.rustc-wrapper`);
  the pipeline neither requires nor configures one, and without it each new
  tree compiles them again.
  `compile_caches` lists the caches with the trees they record;
  `remove_compile_cache` (for `cargo xtask worktree remove`) removes one no
  build holds, and `remove_orphaned_compile_cache` (for `cargo hil sweep`)
  one whose tree is still gone (`CachedTree::is_gone`), both decided under
  the cache's lock. A removal first moves the cache aside (`.removing-*`),
  so a later build creates a fresh cache instead of entering it, and
  `CompileCache::lock` takes the lock again when the cache it waited on was
  moved aside.
- `host_target()` names the host's target triple, for a workspace whose
  Cargo configuration defaults to a chip target.
- `blobray::{cargo, binary, host}`: Cargo commands in the separate Blobray
  workspace with its own target directory, and the built `blobray` host, as
  every caller that builds or runs Blobray from the root workspace (the
  vendor scenarios, the register inventory, the conformance check) reaches them.

It depends only on `oer-process`, `oer-durable` and `tempfile`.

```console
cargo test -p oer-toolchain
```
