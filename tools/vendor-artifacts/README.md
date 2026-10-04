# oer-vendor-artifacts

The pinned vendor artifacts of a chip. `verification/<chip>/artifacts.toml`
is the only pin: git sources at a revision, release assets and local build
outputs, each artifact with its SHA-256. This crate parses it, resolves every
artifact in the host-wide store (`$OER_VENDOR_CACHE`, default
`~/.cache/open-esp-radio/vendor`, linked from each checkout's
`target/vendor`), and fetches and verifies what is missing.

`cargo xtask vendor-fetch` and the vendor checks of
[xtask](../xtask/README.md) read the pins through it, and so does the HIL
stand's pinned ESP-IDF build in [`oer-hil-cli`](../../hil/host/cli/README.md),
which checks out the pinned IDF and its pinned submodules.
