//! The machine-readable command tree of this command line, printed for
//! `__command-tree` and tracked as `cli/command-tree.json`, so that
//! documentation checks can confirm that shown commands and flags exist
//! without building Blobray. A node lists its path from `blobray`, its
//! visible subcommands and long flags, and whether it forwards its trailing
//! arguments to another program.

use clap::CommandFactory as _;

/// The only argument with which Blobray prints its command tree.
pub const REQUEST: &str = oer_command_tree::REQUEST;

/// Every visible command of the command line, depth first, as a JSON array
/// (`oer_command_tree`'s walk of the clap parser).
pub fn json() -> String {
    let tree = oer_command_tree::command_tree(&super::Cli::command(), &[String::from("blobray")]);
    let mut text = oer_command_tree::json(&tree);
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    /// The tracked tree, beside this package's manifest.
    const TRACKED: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/command-tree.json");
    /// Set to rewrite the tracked tree from the live command line.
    const UPDATE: &str = "BLOBRAY_COMMAND_TREE_UPDATE";

    #[test]
    fn the_tracked_command_tree_matches_the_command_line() {
        let live = super::json();
        if std::env::var_os(UPDATE).is_some() {
            std::fs::write(TRACKED, &live).unwrap();
        }
        let tracked = std::fs::read_to_string(TRACKED).unwrap();
        assert!(
            tracked == live,
            "{TRACKED} differs from the command line; rerun this test with {UPDATE}=1"
        );
    }

    #[test]
    fn the_tree_names_commands_and_global_flags() {
        let nodes: Vec<serde_json::Value> = serde_json::from_str(&super::json()).unwrap();
        assert_eq!(nodes[0]["path"], serde_json::json!(["blobray"]));
        assert!(
            nodes[0]["flags"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("--format"))
        );
        let audit = nodes
            .iter()
            .find(|node| node["path"] == serde_json::json!(["blobray", "audit-targets"]))
            .unwrap();
        assert!(
            audit["flags"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("--forbid"))
        );
        assert!(nodes.iter().all(|node| node["forwards"] == false));
    }
}
