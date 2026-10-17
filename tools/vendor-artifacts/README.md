# oer-vendor-artifacts

The pinned vendor artifacts of a chip. `verification/<chip>/artifacts.toml`
is the only pin: git sources at a revision and release assets, each
artifact with its SHA-256, and firmware outputs, pinned by the tracked
catalog image whose recipe builds them and by no digest. This crate reads
it through `oer-vendor-pins` (`Manifest::parse`, `Manifest::load`, typed
`Source` and `Artifact`, `Manifest::location`): `ensure(root, chip, id)`
fetches and verifies one artifact, such as the ROM ELF the firmware stack
gate reads, `pinned(root, chip)` lists every verified one, and `run` fetches
a chip's pins (`cargo xtask vendor fetch`). It resolves every artifact in the
host-wide store (`$OER_VENDOR_CACHE`, default `open-esp-radio/vendor` in the
user's XDG cache directory, through `oer-durable`, linked from each
checkout's `target/vendor` by `link_store`, which `cargo xtask worktree`
runs).

It depends only on `toml`, `serde` and the foundation crates, so build
scripts and the register inventory read the pins through it too.
