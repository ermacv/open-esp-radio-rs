use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::{ProjectSpec, Result, artifacts};

use super::{MAX_EXAMINED_EDGES, MAX_VISITED_NODES};

pub(super) const PROJECT_ASSOCIATED: &str = "project-associated";

pub(super) struct ProjectGraph<'project> {
    profiles: Vec<ProfileReader<'project>>,
    locations: BTreeMap<String, Vec<usize>>,
    definitions: BTreeMap<String, Vec<String>>,
}

struct ProfileReader<'project> {
    profile: &'project crate::project_ir::ProjectIrProfile,
    reader: artifacts::LinkedIrReader,
}

pub(super) struct ProjectGraphSearch {
    pub(super) path: Option<Vec<artifacts::StoredGraphEdge>>,
    pub(super) visited_nodes: usize,
    pub(super) examined_edges: usize,
    pub(super) limit: Option<&'static str>,
}

impl<'project> ProjectGraph<'project> {
    pub(super) fn open(project: &'project ProjectSpec) -> Result<Self> {
        let mut profiles = Vec::new();
        let mut locations = BTreeMap::<String, Vec<usize>>::new();
        let mut definitions = BTreeMap::<String, Vec<String>>::new();
        for profile in project
            .ir_profiles
            .iter()
            .filter(|profile| profile.output.is_dir())
        {
            let reader = artifacts::LinkedIrReader::open(&profile.output)?;
            let projection = reader.read_review_projection()?;
            let profile_index = profiles.len();
            for function in &projection.functions {
                locations
                    .entry(function.identity.clone())
                    .or_default()
                    .push(profile_index);
                if function.is_exported() {
                    definitions
                        .entry(function.symbol.clone())
                        .or_default()
                        .push(function.identity.clone());
                }
            }
            profiles.push(ProfileReader { profile, reader });
        }
        for candidates in definitions.values_mut() {
            candidates.sort();
            candidates.dedup();
        }
        Ok(Self {
            profiles,
            locations,
            definitions,
        })
    }

    pub(super) fn root_identities(&self, source: &str, symbol: &str) -> BTreeSet<String> {
        self.profiles
            .iter()
            .flat_map(|profile| profile.reader.function_identities(source, symbol))
            .collect()
    }

    pub(super) fn target_identities(
        &self,
        target: &super::FlowTargetRequest<'_>,
    ) -> Result<BTreeSet<String>> {
        let mut targets = BTreeSet::new();
        for profile in &self.profiles {
            match target {
                super::FlowTargetRequest::Function(selector) => {
                    targets.extend(profile.reader.matching_function_identities(selector));
                }
                super::FlowTargetRequest::Register(register) => {
                    targets.extend(
                        profile
                            .reader
                            .mmio_function_identities(Some(register), None)?,
                    );
                }
                super::FlowTargetRequest::Address(address) => {
                    targets.extend(
                        profile
                            .reader
                            .mmio_function_identities(None, Some(*address))?,
                    );
                }
            }
        }
        Ok(targets)
    }

    pub(super) fn function(
        &self,
        identity: &str,
    ) -> Result<
        Option<(
            artifacts::StoredFunction,
            &crate::project_ir::ProjectIrProfile,
        )>,
    > {
        let Some(profile) = self.unique_profile(identity) else {
            return Ok(None);
        };
        Ok(profile
            .reader
            .get_function_by_identity(identity)?
            .map(|function| (function, profile.profile)))
    }

    pub(super) fn profile_labels(&self) -> (String, String) {
        let profile = self
            .profiles
            .iter()
            .map(|item| item.profile.id.as_str())
            .collect::<Vec<_>>()
            .join(" + ");
        let outputs = self
            .profiles
            .iter()
            .map(|item| item.profile.output.display().to_string())
            .collect::<Vec<_>>()
            .join("; ");
        (profile, outputs)
    }

    pub(super) fn shortest_path(
        &self,
        root: &str,
        targets: &BTreeSet<String>,
        max_depth: usize,
    ) -> Result<ProjectGraphSearch> {
        if targets.contains(root) {
            return Ok(ProjectGraphSearch {
                path: Some(Vec::new()),
                visited_nodes: 1,
                examined_edges: 0,
                limit: None,
            });
        }
        let mut queue = VecDeque::from([(root.to_owned(), 0usize)]);
        let mut visited = BTreeSet::from([root.to_owned()]);
        let mut predecessor = BTreeMap::<String, artifacts::StoredGraphEdge>::new();
        let mut examined_edges = 0usize;
        let mut depth_exhausted = false;
        let mut limit = None;
        let mut reached = None;

        'search: while let Some((node, depth)) = queue.pop_front() {
            if depth >= max_depth {
                depth_exhausted |= !self.outgoing(&node, targets)?.is_empty();
                continue;
            }
            for edge in self.outgoing(&node, targets)? {
                if examined_edges >= MAX_EXAMINED_EDGES {
                    limit = Some("max-examined-edges");
                    break 'search;
                }
                examined_edges += 1;
                if !traversable(&edge) || visited.contains(&edge.callee) {
                    continue;
                }
                if visited.len() >= MAX_VISITED_NODES {
                    limit = Some("max-visited-nodes");
                    break 'search;
                }
                visited.insert(edge.callee.clone());
                predecessor.insert(edge.callee.clone(), edge.clone());
                if targets.contains(&edge.callee) {
                    reached = Some(edge.callee.clone());
                    break 'search;
                }
                queue.push_back((edge.callee.clone(), depth + 1));
            }
        }
        let path = reached.map(|mut node| {
            let mut reversed = Vec::new();
            while node != root {
                let edge = predecessor[&node].clone();
                node = edge.caller.clone();
                reversed.push(edge);
            }
            reversed.reverse();
            reversed
        });
        if path.is_none() && limit.is_none() && depth_exhausted {
            limit = Some("max-depth");
        }
        Ok(ProjectGraphSearch {
            path,
            visited_nodes: visited.len(),
            examined_edges,
            limit,
        })
    }

    fn outgoing(
        &self,
        identity: &str,
        preferred_targets: &BTreeSet<String>,
    ) -> Result<Vec<artifacts::StoredGraphEdge>> {
        let Some(profile) = self.unique_profile(identity) else {
            return Ok(Vec::new());
        };
        let mut edges = profile.reader.outgoing_edges(identity)?;
        let unresolved = edges
            .iter()
            .filter(|edge| edge.kind == "unresolved")
            .map(|edge| (edge.site, edge.callee.clone()))
            .collect::<BTreeSet<_>>();
        if !unresolved.is_empty()
            && let Some(function) = profile.reader.get_function_by_identity(identity)?
        {
            for call in &function.calls {
                if call.kind != "unresolved"
                    || !unresolved.contains(&(call.site, call.target.clone()))
                {
                    continue;
                }
                let Some(symbol) = call.project_symbol() else {
                    continue;
                };
                let Some(candidates) = self.definitions.get(symbol) else {
                    continue;
                };
                let Some(candidate) = select_definition(candidates, preferred_targets) else {
                    continue;
                };
                if self.unique_profile(candidate).is_none() {
                    continue;
                }
                edges.push(artifacts::StoredGraphEdge {
                    caller: identity.to_owned(),
                    callee: candidate.clone(),
                    site: call.site,
                    kind: PROJECT_ASSOCIATED.to_owned(),
                });
            }
        }
        edges.sort_by(|left, right| {
            (&left.caller, &left.callee, left.site, &left.kind).cmp(&(
                &right.caller,
                &right.callee,
                right.site,
                &right.kind,
            ))
        });
        edges.dedup();
        Ok(edges)
    }

    fn unique_profile(&self, identity: &str) -> Option<&ProfileReader<'project>> {
        let locations = self.locations.get(identity)?;
        (locations.len() == 1).then(|| &self.profiles[locations[0]])
    }
}

fn select_definition<'candidate>(
    candidates: &'candidate [String],
    preferred_targets: &BTreeSet<String>,
) -> Option<&'candidate String> {
    if candidates.len() == 1 {
        return candidates.first();
    }
    let mut preferred = candidates
        .iter()
        .filter(|candidate| preferred_targets.contains(*candidate));
    let candidate = preferred.next()?;
    preferred.next().is_none().then_some(candidate)
}

fn traversable(edge: &artifacts::StoredGraphEdge) -> bool {
    matches!(
        edge.kind.as_str(),
        "internal"
            | "project-linked"
            | "indexed-dispatch"
            | "structural-relocation"
            | PROJECT_ASSOCIATED
    )
}

#[cfg(test)]
mod tests {
    use super::select_definition;
    use std::collections::BTreeSet;

    #[test]
    fn one_exported_definition_is_project_associated() {
        let candidates = vec!["libpp::target".to_owned()];

        assert_eq!(
            select_definition(&candidates, &BTreeSet::new()).map(String::as_str),
            Some("libpp::target")
        );
    }

    #[test]
    fn ambiguous_definitions_require_one_explicit_target() {
        let candidates = vec![
            "libpp::target".to_owned(),
            "wifi-key-role::target".to_owned(),
        ];

        assert!(select_definition(&candidates, &BTreeSet::new()).is_none());
        assert_eq!(
            select_definition(&candidates, &BTreeSet::from(["libpp::target".to_owned()]))
                .map(String::as_str),
            Some("libpp::target")
        );
        assert!(select_definition(&candidates, &candidates.iter().cloned().collect()).is_none());
    }
}
