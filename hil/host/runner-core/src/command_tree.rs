//! The machine-readable command tree of a clap command line, for checking
//! that documentation shows only commands and flags that exist.

use serde::{Deserialize, Serialize};

/// One command of a tree: its path from the root, its subcommands and the
/// long flags it accepts.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct CommandNode {
    pub path: Vec<String>,
    pub subcommands: Vec<String>,
    pub flags: Vec<String>,
}

/// Every command of `command` below `prefix`, depth first. Hidden commands
/// and arguments are left out.
pub fn command_tree(command: &clap::Command, prefix: &[String]) -> Vec<CommandNode> {
    let mut nodes = Vec::new();
    walk(command, prefix.to_vec(), &mut nodes);
    nodes
}

fn walk(command: &clap::Command, path: Vec<String>, nodes: &mut Vec<CommandNode>) {
    let visible = command
        .get_subcommands()
        .filter(|subcommand| !subcommand.is_hide_set())
        .collect::<Vec<_>>();
    nodes.push(CommandNode {
        path: path.clone(),
        subcommands: visible
            .iter()
            .map(|subcommand| subcommand.get_name().to_owned())
            .collect(),
        flags: command
            .get_arguments()
            .filter(|argument| !argument.is_hide_set())
            .flat_map(|argument| {
                argument
                    .get_long()
                    .into_iter()
                    .chain(argument.get_visible_aliases().unwrap_or_default())
                    .map(|long| format!("--{long}"))
            })
            .collect(),
    });
    for subcommand in visible {
        let mut child = path.clone();
        child.push(subcommand.get_name().to_owned());
        walk(subcommand, child, nodes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tree_names_every_visible_command_and_long_flag() {
        let command = clap::Command::new("root")
            .arg(
                clap::Arg::new("json")
                    .long("json")
                    .visible_alias("machine")
                    .action(clap::ArgAction::SetTrue),
            )
            .subcommand(
                clap::Command::new("list")
                    .arg(clap::Arg::new("since").long("since"))
                    .arg(clap::Arg::new("secret").long("secret").hide(true)),
            )
            .subcommand(clap::Command::new("__hidden").hide(true));
        let tree = command_tree(&command, &[String::from("hil")]);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].path, ["hil"]);
        assert_eq!(tree[0].subcommands, ["list"]);
        assert!(tree[0].flags.contains(&String::from("--json")));
        assert!(tree[0].flags.contains(&String::from("--machine")));
        assert_eq!(tree[1].path, ["hil", "list"]);
        assert_eq!(tree[1].flags, ["--since"]);
    }
}
