# Documentation build

Guides live in [`docs/`](../../docs/README.md) and render as an
[mdBook](https://rust-lang.github.io/mdBook/) book configured by
[`docs/book.toml`](../../docs/book.toml) and ordered by
[`docs/SUMMARY.md`](../../docs/SUMMARY.md). API documentation comes from
`cargo doc` through `cargo xtask doc`. Documents elsewhere in the repository
stay beside their owners and are read on GitHub.

## Build locally

Install the mdBook versions that CI pins (see
[the workflow](../../.github/workflows/docs.yml)):

```console
cargo install --locked mdbook --version 0.5.4
cargo install --locked mdbook-mermaid --version 0.17.1
```

From the repository root:

```console
mdbook-mermaid install docs
mdbook build docs
mdbook test docs
mdbook serve docs
cargo xtask doc
```

`mdbook-mermaid install docs` writes the Mermaid scripts that `docs/book.toml`
loads; they are ignored by Git. `mdbook build docs` writes `target/book/`.
`cargo xtask doc` writes the standard Cargo documentation directories:
`target/doc/` for host packages and `target/riscv32imafc-unknown-none-elf/doc/`
for chip packages, plus the Wi-Fi composition's own workspace target.

## Links that leave the book

The guides link to documents and code beside their owners with relative paths,
which work on GitHub and in every branch. mdBook publishes only the book, so the
`repo-links` preprocessor, [`repo_links.py`](repo_links.py), rewrites a relative
link that resolves outside `docs/` to the same file on GitHub at the commit
being built. Links inside the book and code blocks are unchanged. A link to a
missing file or outside the repository fails the build.

## Publication

The [Documentation workflow](../../.github/workflows/docs.yml) builds the API
documentation, runs `cargo xtask check docs`, builds and tests the book and
packages one site: the book at the root and API documentation under `api/host/`,
`api/esp32s31/` and `api/esp32s31-wifi/`. Pushes and pull requests only build
and check. A manual run from `main` deploys the site to the
[Pages address](https://ermacv.github.io/open-esp-radio-rs/) with the official
GitHub Pages actions.
