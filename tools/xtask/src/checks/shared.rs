//! The shared crates ([changing a shared
//! contract](../../../../docs/architecture.md#changing-a-shared-contract)):
//! packages outside every block that the block packages of two or more
//! blocks reach, directly or transitively, through normal dependencies; a
//! test's dev-dependency is no edge. A block package carries its block's
//! domain token in its name ([`block_of`], the package naming rule's) and is
//! production code by its layer ([`oer_repo::classification::Scope`]); a
//! HIL, verification or tool package with a token is no block package, and
//! what it reaches is not shared for it. Neither is a closed register PAC of
//! the architecture policy (`CLOSED_PACS`), which the shared chip PAC
//! re-exports. What block packages reach counts
//! whatever its own scope. The set is derived, never listed by hand: [`run`]
//! checks the architecture's table against it.

use std::collections::{BTreeMap, BTreeSet};

use oer_process::Checkout;

use crate::Result;

/// A block: the packages of one radio protocol.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum Block {
    Ieee80211,
    Bluetooth,
    Ieee802154,
}

impl Block {
    const ALL: [Self; 3] = [Self::Ieee80211, Self::Bluetooth, Self::Ieee802154];

    /// The domain token of its packages' names.
    pub const fn token(self) -> &'static str {
        match self {
            Self::Ieee80211 => "ieee80211",
            Self::Bluetooth => "bluetooth",
            Self::Ieee802154 => "ieee802154",
        }
    }
}

/// The block of the package `name` by the domain token in its name, `None`
/// for a package outside every block; a name carrying two blocks' tokens is
/// refused. A block package without a token counts as outside the blocks.
pub fn block_of(name: &str) -> std::result::Result<Option<Block>, String> {
    let tokens: Vec<&str> = name
        .strip_prefix("oer-")
        .map(|rest| rest.split('-').collect())
        .unwrap_or_default();
    let blocks: Vec<Block> = Block::ALL
        .into_iter()
        .filter(|block| tokens.contains(&block.token()))
        .collect();
    match blocks.as_slice() {
        [] => Ok(None),
        [block] => Ok(Some(*block)),
        _ => Err(format!(
            "package {name} names more than one block: {}",
            blocks
                .iter()
                .map(|block| block.token())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// A package of the graph: whether it is production code, and the packages
/// it depends on.
#[derive(Clone, Debug, Default)]
pub struct Node {
    pub production: bool,
    pub dependencies: BTreeSet<String>,
}

/// The shared crates of a graph of packages.
pub fn shared(graph: &BTreeMap<String, Node>) -> Result<BTreeSet<String>> {
    // Every name follows the naming rule, block package or not.
    let mut blocks = BTreeMap::new();
    for (name, node) in graph {
        if let Some(block) = block_of(name)?
            && node.production
            && !crate::checks::architecture::unsafe_policy::CLOSED_PACS.contains(&name.as_str())
        {
            blocks.insert(name.as_str(), block);
        }
    }
    let mut reached: BTreeMap<&str, BTreeSet<Block>> = BTreeMap::new();
    for (&name, &block) in &blocks {
        let mut pending: Vec<&str> = vec![name];
        let mut seen = BTreeSet::new();
        while let Some(current) = pending.pop() {
            for dependency in graph
                .get(current)
                .into_iter()
                .flat_map(|node| &node.dependencies)
            {
                if seen.insert(dependency.as_str()) {
                    pending.push(dependency);
                }
            }
        }
        for dependency in seen {
            if !blocks.contains_key(dependency) {
                reached.entry(dependency).or_default().insert(block);
            }
        }
    }
    Ok(reached
        .into_iter()
        .filter(|(_, blocks)| blocks.len() >= 2)
        .map(|(name, _)| name.to_owned())
        .collect())
}

/// Every package of the repository with its scope and its normal path
/// dependencies.
fn package_graph(model: &oer_repo::Model) -> Result<BTreeMap<String, Node>> {
    let mut graph = BTreeMap::new();
    for package in model.packages() {
        let production = model.classification(package)?.layer.scope()
            == oer_repo::classification::Scope::Production;
        let dependencies = package
            .dependencies
            .iter()
            .filter(|dependency| dependency.kind == oer_repo::manifest::Kind::Normal)
            .filter_map(|dependency| model.target(dependency))
            .map(|target| target.name.clone())
            .collect();
        graph.insert(
            package.name.clone(),
            Node {
                production,
                dependencies,
            },
        );
    }
    Ok(graph)
}

/// The architecture document and the markers around its table of shared
/// crates.
const DOCUMENT: &str = "docs/architecture.md";
const BEGIN: &str = "<!-- shared-crates: begin -->";
const END: &str = "<!-- shared-crates: end -->";

/// The packages a table names: every name in backticks in the first cell
/// of each row.
fn listed(document: &str) -> Result<BTreeSet<String>> {
    let (_, rest) = document
        .split_once(BEGIN)
        .ok_or_else(|| format!("{DOCUMENT} has no `{BEGIN}`"))?;
    let (table, _) = rest
        .split_once(END)
        .ok_or_else(|| format!("{DOCUMENT} has no `{END}`"))?;
    let mut names = BTreeSet::new();
    for line in table.lines() {
        let Some(cell) = line.strip_prefix('|').and_then(|row| row.split('|').next()) else {
            continue;
        };
        names.extend(cell.split('`').skip(1).step_by(2).map(str::to_owned));
    }
    Ok(names)
}

/// Check that the architecture's table of shared crates names exactly the
/// derived set.
pub fn run(ctx: &Checkout) -> Result<()> {
    let model = crate::checks::common::model(ctx)?;
    let derived = shared(&package_graph(&model)?)?;
    let document = std::fs::read_to_string(ctx.root.join(DOCUMENT))?;
    let listed = listed(&document)?;
    if listed != derived {
        let missing: Vec<_> = derived.difference(&listed).collect();
        let extra: Vec<_> = listed.difference(&derived).collect();
        return Err(format!(
            "{DOCUMENT}'s table of shared crates is stale: add {missing:?}, remove {extra:?}; \
             the shared crates are {derived:?}"
        )
        .into());
    }
    println!(
        "shared crates: {} listed, matching the derivation",
        listed.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A graph of production packages, then the development ones `development`
    /// names.
    fn graph(edges: &[(&str, &[&str])], development: &[&str]) -> BTreeMap<String, Node> {
        edges
            .iter()
            .map(|(name, dependencies)| {
                (
                    (*name).to_owned(),
                    Node {
                        production: !development.contains(name),
                        dependencies: dependencies.iter().map(|d| (*d).to_owned()).collect(),
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_package_belongs_to_the_block_its_name_names() {
        assert_eq!(block_of("oer-ieee80211-sta"), Ok(Some(Block::Ieee80211)));
        assert_eq!(
            block_of("oer-esp32s31-bluetooth-radio"),
            Ok(Some(Block::Bluetooth))
        );
        assert_eq!(block_of("oer-radio-port"), Ok(None));
        // A token is a whole name segment.
        assert_eq!(block_of("oer-ieee80211x"), Ok(None));
        assert!(
            block_of("oer-bluetooth-ieee802154-bridge")
                .unwrap_err()
                .contains("bluetooth, ieee802154")
        );
    }

    #[test]
    fn a_crate_two_blocks_reach_is_shared_and_one_block_reaches_is_not() {
        let shared = shared(&graph(
            &[
                ("oer-ieee80211-sta", &["oer-time", "oer-radio-port"]),
                ("oer-bluetooth-ll", &["oer-radio-port"]),
                // Reached only transitively by both blocks.
                ("oer-radio-port", &["oer-esp32s31-pac"]),
                ("oer-ieee802154-mac", &["oer-permit-pool"]),
                ("oer-time", &[]),
                ("oer-esp32s31-pac", &[]),
                ("oer-permit-pool", &[]),
            ],
            &[],
        ))
        .unwrap();
        assert_eq!(
            shared,
            ["oer-esp32s31-pac", "oer-radio-port"]
                .map(String::from)
                .into()
        );
    }

    /// A HIL package with a block's token is no block package: what it
    /// reaches is not shared for it.
    #[test]
    fn a_tokened_hil_package_makes_nothing_shared() {
        let shared = shared(&graph(
            &[
                ("oer-hil-family-bluetooth", &["oer-hil-protocol"]),
                ("oer-hil-family-ieee80211", &["oer-hil-protocol"]),
                ("oer-bluetooth-ll", &["oer-radio-port"]),
                ("oer-hil-protocol", &[]),
                ("oer-radio-port", &[]),
            ],
            &[
                "oer-hil-family-bluetooth",
                "oer-hil-family-ieee80211",
                "oer-hil-protocol",
            ],
        ))
        .unwrap();
        assert!(shared.is_empty(), "{shared:?}");
    }

    /// A development crate the production packages of two blocks reach is
    /// shared: the filter is on the blocks' side only.
    #[test]
    fn a_development_crate_two_blocks_reach_is_shared() {
        let shared = shared(&graph(
            &[
                ("oer-ieee80211-sta", &["oer-esp32s31-platform"]),
                ("oer-bluetooth-ll", &["oer-esp32s31-platform"]),
                ("oer-esp32s31-platform", &[]),
            ],
            &["oer-esp32s31-platform"],
        ))
        .unwrap();
        assert_eq!(shared, ["oer-esp32s31-platform"].map(String::from).into());
    }

    /// A closed register PAC carries a block's token but is no block
    /// package: two blocks reaching it through the shared chip PAC list it.
    #[test]
    fn a_closed_pac_two_blocks_reach_through_the_shared_pac_is_shared() {
        let shared = shared(&graph(
            &[
                ("oer-bluetooth-ll", &["oer-esp32s31-pac"]),
                ("oer-ieee802154-mac", &["oer-esp32s31-pac"]),
                ("oer-esp32s31-pac", &["oer-ieee80211-pac"]),
                ("oer-ieee80211-pac", &[]),
            ],
            &[],
        ))
        .unwrap();
        assert_eq!(
            shared,
            ["oer-esp32s31-pac", "oer-ieee80211-pac"]
                .map(String::from)
                .into()
        );
    }

    #[test]
    fn a_two_block_name_refuses_the_derivation() {
        let error = shared(&graph(&[("oer-ieee80211-bluetooth-coex", &[])], &[]))
            .unwrap_err()
            .to_string();
        assert!(error.contains("more than one block"), "{error}");
    }

    #[test]
    fn the_table_names_its_first_cells() {
        let document = format!(
            "intro\n{BEGIN}\n| Contract | Path |\n| --- | --- |\n| `oer-a` | `a` |\n| `oer-b`, `oer-c` | b |\n{END}\n| `oer-z` | after |\n"
        );
        assert_eq!(
            listed(&document).unwrap(),
            ["oer-a", "oer-b", "oer-c"].map(String::from).into()
        );
    }
}
