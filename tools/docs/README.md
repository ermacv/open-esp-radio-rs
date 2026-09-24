# Documentation portal

The portal presents owner Markdown, isolated public/private rustdoc snapshots
and generated static qualification catalogs. Source documents stay beside their
owners. `book.toml` configures mdBook; `portal.json` owns navigation, tool versions
and the Pages base path. New owned documents enter component references
automatically. Agent instructions, the Blobray work plan and the catalog test
fixture remain GitHub source links.

## Prerequisites and local preview

Use Linux, the repository's Rust toolchain, its embedded target and
`llvm-tools-preview`, plus Node.js at the version in `portal.json` (Node 24–26
is supported locally). Native build utilities and Playwright's browser system
libraries are also required. Full compilation caches need substantially more
disk space than the published site.
Run from the repository root:

```console
npm ci --prefix tools/docs
node tools/docs/setup.mjs
npm --prefix tools/docs test
node tools/docs/build.mjs
node tools/docs/check.mjs
node tools/docs/serve.mjs
```

`setup.mjs` installs the pinned mdBook/Mermaid tools under `target/docs/tooling`,
installs Playwright Chromium, and fetches locked public dependencies for every
source workspace. It requires network access. On a minimal Linux machine run
`npx --prefix tools/docs playwright install-deps chromium` to install browser
system libraries before the browser checks.

The default build runs `cargo xtask check docs`, then renders guides and the
declarations-only status views. Its API landing page explicitly identifies a
preview without API exports. It never reuses an older full build's API files.
Preview at `http://127.0.0.1:4173/open-esp-radio-rs/`; the subpath matches Pages.
The check command validates local HTML links/resources and renders all Mermaid
pages in Chromium, including documentation navigation and search smoke tests.

## Complete portal

```console
node tools/docs/build.mjs --full
node tools/docs/check.mjs
```

The full build invokes `cargo xtask check docs --full --export-html` and requires
every public/private API mapping in its successful report. API snapshots retain
their target, features and Cargo target, with equivalent requirements pointing
to the existing planner-selected execution. Public and private exports remain
separate. Rustdoc resources, search and source views must work in the snapshot.
Do not merge different feature configurations into one crate directory.

Private HTML exports also include `#[doc(hidden)]` items. Public references to
them lead to the corresponding private API; every API page identifies its
visibility and configuration. Links between isolated crate snapshots require
matching targets and feature arguments. Ambiguous destinations fail the build.
Inherited dependency links are resolved in their original crate's scope using
the workspace's locked version; the underlying docstrings remain unchanged.
The pinned stable rustdoc gates hidden-item rendering behind `-Z unstable-options`;
the exporter sets `RUSTC_BOOTSTRAP=1` only for its private HTML command. Ordinary
checks and production builds do not receive that opt-in.

Byte-identical snapshot trees within the same visibility can share one physical
copy, while every required configuration remains in the API index. Static
resources are shared by content identity. When rustdoc emits an optional external
implementor request without an index, the portal explains that limitation on
the trait page; it does not claim to enumerate implementations in other crates.

After changing only the portal assembler, `node tools/docs/build.mjs --full
--reuse-checks` can reassemble a successful export. It verifies the existing
gate's source fingerprint and commit before reuse; changed Rust, TOML, lockfiles
or checked Markdown require fresh checks. The publishing workflow always runs
the checks itself.

To fit the complete matrix within GitHub Pages' size limit, API HTML is stored
as gzip payloads. A small loader restores the complete rustdoc document at its
original URL, preserving relative links, query strings and source-line fragments.
API pages require JavaScript and a modern browser with `DecompressionStream`;
each loading/error page also offers the compressed document for download.
Guides, the API index and static qualification pages remain ordinary HTML.
Checks decompress and verify every payload before validating its links, and
browser checks exercise search, source navigation and a missing-payload error.

All generated pages and portal reports live under `target/docs/portal/`. Docs
gate reports remain under their existing owner outputs. The portal shows source
commit/dirty state; local edits are not represented as a clean published commit.
Static qualification views carry `not-evaluated`, never an inferred hardware
verdict. Catalog and program TOML remain the source of truth.

The assembler fails on missing inputs, snapshots or local links. The publication
check also enforces the Pages size limit. It does not omit API configurations to
fit the limit. Ordinary files are copied to the artifact; no dependency on local
symlinks or hard links survives publication.

## CI and manual publication

The configured [Pages address](https://ermacv.github.io/open-esp-radio-rs/)
requires a successful manual build **and deploy**. A running build or a green
push/PR preview does not publish a site. If the address returns 404, inspect the
[Documentation workflow](https://github.com/ermacv/open-esp-radio-rs/actions/workflows/docs.yml)
and its deployment job; before the first successful deployment, use local preview.
Repository Pages settings require maintainer access and cannot be inferred from
the presence of `book.toml` or a passing local build.

Pull requests and pushes to `main` run static checks, the guide/status preview,
portal regression tests and the two host tutorial scenarios. Changes to Rust
or Cargo inputs also run affected package API checks; shared documentation
infrastructure changes require the full gate.

In repository **Settings → Pages**, select **GitHub Actions** as the build source.
Run the **Documentation** workflow manually on `main` to build and publish the
complete portal. Build and deploy are separate jobs. Only the deployment job
gets Pages/OIDC write permissions, and a failed build leaves the previous site
intact. The site contains one current version, not a growing archive of commits.

The workflow prepares dependencies online before locked/offline checks. It does
not require vendor binaries, lab secrets, hardware or evidence archives. GitHub
repository administration and the first remote workflow run are separate from
local validation.

## Maintaining navigation and contracts

Use `portal.json` to put an owner document on a main route. Other checked
Markdown is grouped by `referenceGroups` into component references. Prefixes
are matched in order, with specific owners before broad directories. Every
remaining current document must have a group; missing ownership fails the build.
Group landing pages are generated navigation, not copies of owner contracts.
Retained legacy Blobray pages
have a separate section and visible legacy notice. Do not copy contracts into
portal-only Markdown. Source/code links point to the build's Git revision;
edit links point to the original path on `main`.

Current Blobray references live below `tools/blobray/next/reference/`, with an
owner README per topic. Files directly under `tools/blobray/docs/` are retained
legacy references; its `design/` subdirectory describes Next. Preserve old
operator section anchors as links to their canonical topic when reorganizing
the reference. The portal regression suite checks those bookmark destinations
and short content routes from the root README.

AST transformation rewrites document, reference and image links without touching
code examples. Generated catalog links resolve from their original output path. Local
HTML validation checks the built destinations and anchors, not just source
Markdown. Browser tests exercise actual Mermaid rendering and search resources.
See the [documentation policy](../../docs/documentation.md) for scope and report
ownership, and [contributing](../../CONTRIBUTING.md) for selecting focused checks.
