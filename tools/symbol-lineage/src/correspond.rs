//! Function correspondence between two archive revisions.
//!
//! Evidence is applied from strongest to weakest, and each step only pairs
//! functions that are still unpaired on both sides:
//!
//! 1. the same symbol name, unique in both revisions;
//! 2. the same normalized body, unique among the unpaired functions;
//! 3. the call graph: paired callers with the same call count vote for the
//!    callees at equal call positions, until no vote changes. A voted pair
//!    must still share a minimum part of its body;
//! 4. body similarity with a required margin over the second candidate,
//!    accepted only as a mutual best, followed by another call-graph pass.
//!
//! Ambiguity is never resolved by choice: an unpaired function stays unpaired.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Serialize;

use crate::archive::Revision;
use crate::body::similarity_ppm;

/// Candidates kept after the shingle prefilter.
const PREFILTER_CANDIDATES: usize = 6;

/// How a pair was established.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Evidence {
    SameName,
    ExactBody,
    CallGraph,
    Similar { ppm: u32 },
}

/// One function pair between the left and right revisions.
#[derive(Clone, Copy, Debug)]
pub struct Pair {
    pub left: usize,
    pub right: usize,
    pub evidence: Evidence,
    /// Whether the normalized bodies are identical.
    pub identical: bool,
    /// Body similarity in parts per million; one million when identical.
    pub similarity_ppm: u32,
}

/// Similarity acceptance policy.
#[derive(Clone, Copy, Debug)]
pub struct Policy {
    /// Minimum similarity in parts per million.
    pub minimum_ppm: u32,
    /// Minimum lead over the next candidate in parts per million.
    pub margin_ppm: u32,
    /// Minimum body similarity of a call-graph pair in parts per million.
    pub call_graph_minimum_ppm: u32,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            minimum_ppm: 850_000,
            margin_ppm: 50_000,
            call_graph_minimum_ppm: 500_000,
        }
    }
}

/// Pair the functions of `left` and `right`.
pub fn correlate(left: &Revision, right: &Revision, policy: Policy) -> Vec<Pair> {
    let mut state = State::new(left, right);
    state.pair_same_names();
    state.pair_exact_bodies();
    state.pair_call_graph(policy);
    state.pair_similar(policy);
    state.pair_call_graph(policy);
    let mut pairs: Vec<Pair> = state.pairs.into_values().collect();
    pairs.sort_by_key(|pair| pair.right);
    pairs
}

struct State<'a> {
    left: &'a Revision,
    right: &'a Revision,
    left_names: HashMap<&'a str, usize>,
    right_names: HashMap<&'a str, usize>,
    /// Pairs keyed by the left function.
    pairs: BTreeMap<usize, Pair>,
    right_paired: HashSet<usize>,
}

impl<'a> State<'a> {
    fn new(left: &'a Revision, right: &'a Revision) -> Self {
        Self {
            left,
            right,
            left_names: unique_names(left),
            right_names: unique_names(right),
            pairs: BTreeMap::new(),
            right_paired: HashSet::new(),
        }
    }

    fn insert(&mut self, left: usize, right: usize, evidence: Evidence) {
        let (left_body, right_body) = (
            &self.left.functions[left].body,
            &self.right.functions[right].body,
        );
        let identical = left_body.fingerprint == right_body.fingerprint;
        let similarity_ppm = match evidence {
            Evidence::Similar { ppm } => ppm,
            _ if identical => 1_000_000,
            _ => similarity_ppm(&left_body.parcels, &right_body.parcels),
        };
        self.pairs.insert(
            left,
            Pair {
                left,
                right,
                evidence,
                identical,
                similarity_ppm,
            },
        );
        self.right_paired.insert(right);
    }

    fn unpaired_left(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.left.functions.len()).filter(|index| !self.pairs.contains_key(index))
    }

    fn unpaired_right(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.right.functions.len()).filter(|index| !self.right_paired.contains(index))
    }

    fn pair_same_names(&mut self) {
        let matches: Vec<(usize, usize)> = self
            .left_names
            .iter()
            .filter_map(|(name, left)| self.right_names.get(name).map(|right| (*left, *right)))
            .collect();
        for (left, right) in matches {
            self.insert(left, right, Evidence::SameName);
        }
    }

    fn pair_exact_bodies(&mut self) {
        let mut left_bodies: HashMap<[u8; 32], Vec<usize>> = HashMap::new();
        for index in self.unpaired_left() {
            left_bodies
                .entry(self.left.functions[index].body.fingerprint)
                .or_default()
                .push(index);
        }
        let mut right_bodies: HashMap<[u8; 32], Vec<usize>> = HashMap::new();
        for index in self.unpaired_right() {
            right_bodies
                .entry(self.right.functions[index].body.fingerprint)
                .or_default()
                .push(index);
        }
        for (fingerprint, lefts) in left_bodies {
            if let (&[left], Some(&[right])) = (
                lefts.as_slice(),
                right_bodies.get(&fingerprint).map(Vec::as_slice),
            ) {
                self.insert(left, right, Evidence::ExactBody);
            }
        }
    }

    fn pair_call_graph(&mut self, policy: Policy) {
        // Rejected pairs are not voted for again.
        let mut rejected: BTreeSet<(usize, usize)> = BTreeSet::new();
        loop {
            let mut votes: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
            let mut voters: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
            for pair in self.pairs.values() {
                let left_calls = &self.left.functions[pair.left].body.calls;
                let right_calls = &self.right.functions[pair.right].body.calls;
                if left_calls.len() != right_calls.len() {
                    continue;
                }
                for (left_callee, right_callee) in left_calls.iter().zip(right_calls) {
                    let (Some(&left), Some(&right)) = (
                        self.left_names.get(left_callee.as_str()),
                        self.right_names.get(right_callee.as_str()),
                    ) else {
                        continue;
                    };
                    if self.pairs.contains_key(&left)
                        || self.right_paired.contains(&right)
                        || rejected.contains(&(left, right))
                    {
                        continue;
                    }
                    votes.entry(left).or_default().insert(right);
                    voters.entry(right).or_default().insert(left);
                }
            }
            let accepted: Vec<(usize, usize)> = votes
                .iter()
                .filter_map(|(left, rights)| {
                    let right = single(rights)?;
                    (single(&voters[&right]) == Some(*left)).then_some((*left, right))
                })
                .collect();
            let mut progressed = false;
            for (left, right) in accepted {
                let score = similarity_ppm(
                    &self.left.functions[left].body.parcels,
                    &self.right.functions[right].body.parcels,
                );
                if score < policy.call_graph_minimum_ppm {
                    rejected.insert((left, right));
                } else {
                    self.insert(left, right, Evidence::CallGraph);
                }
                progressed = true;
            }
            if !progressed {
                return;
            }
        }
    }

    fn pair_similar(&mut self, policy: Policy) {
        let lefts: Vec<usize> = self.unpaired_left().collect();
        let rights: Vec<usize> = self.unpaired_right().collect();
        let right_shingles: Vec<HashSet<u64>> = rights
            .iter()
            .map(|index| self.right.functions[*index].body.shingles())
            .collect();
        // Best and second-best score per function, in both directions.
        let mut best_right: HashMap<usize, (usize, u32, u32)> = HashMap::new();
        let mut best_left: HashMap<usize, (usize, u32, u32)> = HashMap::new();
        for left in lefts {
            let body = &self.left.functions[left].body;
            let shingles = body.shingles();
            let mut candidates: Vec<(u64, usize)> = rights
                .iter()
                .zip(&right_shingles)
                .filter(|(right, _)| comparable(body.size, self.right.functions[**right].body.size))
                .map(|(right, other)| (jaccard_ppm(&shingles, other), *right))
                .collect();
            candidates.sort_by(|a, b| b.cmp(a));
            candidates.truncate(PREFILTER_CANDIDATES);
            for (_, right) in candidates {
                let score =
                    similarity_ppm(&body.parcels, &self.right.functions[right].body.parcels);
                record(&mut best_right, left, right, score);
                record(&mut best_left, right, left, score);
            }
        }
        let accepted: Vec<(usize, usize, u32)> = best_right
            .iter()
            .filter_map(|(left, (right, score, second))| {
                let (back, _, back_second) = best_left.get(right)?;
                let lead = |second: u32| score.saturating_sub(second) >= policy.margin_ppm;
                (*back == *left
                    && *score >= policy.minimum_ppm
                    && lead(*second)
                    && lead(*back_second))
                .then_some((*left, *right, *score))
            })
            .collect();
        for (left, right, ppm) in accepted {
            self.insert(left, right, Evidence::Similar { ppm });
        }
    }
}

fn unique_names(revision: &Revision) -> HashMap<&str, usize> {
    let mut seen: HashMap<&str, Option<usize>> = HashMap::new();
    for (index, function) in revision.functions.iter().enumerate() {
        seen.entry(function.name.as_str())
            .and_modify(|slot| *slot = None)
            .or_insert(Some(index));
    }
    seen.into_iter()
        .filter_map(|(name, index)| index.map(|index| (name, index)))
        .collect()
}

fn single(set: &BTreeSet<usize>) -> Option<usize> {
    let mut iter = set.iter();
    match (iter.next(), iter.next()) {
        (Some(value), None) => Some(*value),
        _ => None,
    }
}

fn comparable(left: usize, right: usize) -> bool {
    let (small, large) = if left < right {
        (left, right)
    } else {
        (right, left)
    };
    large <= small * 2 + 16
}

fn jaccard_ppm(left: &HashSet<u64>, right: &HashSet<u64>) -> u64 {
    let union = left.union(right).count() as u64;
    if union == 0 {
        return 0;
    }
    left.intersection(right).count() as u64 * 1_000_000 / union
}

/// Keep the best candidate and the best score among the others.
fn record(table: &mut HashMap<usize, (usize, u32, u32)>, key: usize, candidate: usize, score: u32) {
    let entry = table.entry(key).or_insert((candidate, score, 0));
    if entry.0 == candidate {
        entry.1 = entry.1.max(score);
    } else if score > entry.1 {
        *entry = (candidate, score, entry.1);
    } else {
        entry.2 = entry.2.max(score);
    }
}
