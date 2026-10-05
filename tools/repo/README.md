# Repository model

`oer-repo` is the one model of the repository's own structure. Every tool
that reasons about the tree as a whole — `oer-tidy`, `oer-xtask`, the
qualification evaluator, the HIL source snapshot and the vendor evidence
harness — reads it here instead of listing files, parsing manifests or
asking `cargo metadata` itself. It reads text only: no Cargo resolution, no
compilation, so loading the whole tree takes a fraction of a second.

| Module | Answers |
| --- | --- |
| [`files`](src/files.rs) | The file inventory: a checkout's tracked and untracked-but-not-ignored files (`git ls-files`), without any `target`, `_oracles` or `.git` component; tracked files apart |
| [`manifest`](src/manifest.rs) | Every `Cargo.toml`: packages, crate roots, library and README, features, dependencies (kind, target table, rename, path, features, optional, inherited from `[workspace.dependencies]`), workspace declarations |
| [`workspaces`](src/workspaces.rs) | The workspaces Cargo finds, explicit and implicit, and the one each package belongs to |
| [`chips`](src/chips.rs) | Every `platform/<chip>/chip.toml`, through `oer-chip-profile` |
| [`classification`](src/classification.rs) | Every key of `[package.metadata.open-radio]`, typed: layer (and the scope it implies), platform, chip, family, evidence and HIL roles, the host layer (`host-layer`, required of every host development package), supported feature profiles, test feature sets, inputs |
| [`policy`](src/policy.rs) | Which package may depend on which: layers, platforms, executor and time-driver bindings, evidence and HIL role edges ([layer dependencies](../../docs/architecture.md#layer-dependencies)), and host layers ([host layers](../../docs/architecture.md#host-layers): entry, orchestration, execution, verification, stand, build, foundation; a host package depends only on its own layer and those below) |
| [`closure`](src/closure.rs) | The path packages a package reaches, by dependency kind, target triple (`cfg(…)` evaluated, failing on a cfg it cannot evaluate) and features (resolved from the roots as Cargo unifies them, or every optional dependency); the packages that reach a set |
| [`lock`](src/lock.rs) | The entries of a `Cargo.lock` |

`Model::load(&Repo::load(root)?)` reads everything once.
`Model::owner(path)` names the package a repository path belongs to across
every workspace, `Model::package(name)` finds a package by its unique name,
`Model::classification(package)` returns its typed classification, and
`Model::closure(roots, edges, target, features)` and
`Model::dependents(candidates, selected)` walk the path-dependency graph.

HIL roles: `observation` code shapes what a run observes and reaches no
stand code; `orchestration` code drives runs (the runner, its families,
images, the stand library) and may use stand operation; `operation` code
only operates the stand (boards, flashing, the stand file) and never shapes
a passed observation, so the evaluator's evidence closures leave it out.
