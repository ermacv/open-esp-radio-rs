//! Random-access reader for persistent linked-IR bundles.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fs,
    io::{BufRead, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use super::linked_ir_read::schema::{
    StoredDataObject, StoredFunction, StoredFunctionReviewProjection, StoredMmioRegister,
};
use crate::Result;

pub(crate) const BUNDLE_FILES: [&str; 8] = [
    "manifest.json",
    "functions.jsonl",
    "function-overview.jsonl",
    "function-index.json",
    "graph.json",
    "register-index.json",
    "data-objects.jsonl",
    "data-object-index.json",
];
const MANIFEST: &str = BUNDLE_FILES[0];
const FUNCTIONS: &str = BUNDLE_FILES[1];
const FUNCTION_OVERVIEW: &str = BUNDLE_FILES[2];
const FUNCTION_INDEX: &str = BUNDLE_FILES[3];
const GRAPH: &str = BUNDLE_FILES[4];
const REGISTER_INDEX: &str = BUNDLE_FILES[5];
const DATA_OBJECTS: &str = BUNDLE_FILES[6];
const DATA_OBJECT_INDEX: &str = BUNDLE_FILES[7];

pub(crate) fn bundle_files(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    BUNDLE_FILES.into_iter().map(|name| path.join(name))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionIndexDocument {
    schema_version: u32,
    command: String,
    records: Vec<FunctionIndexRecord>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct FunctionIndexRecord {
    identity: String,
    source: String,
    member: Option<String>,
    symbol: String,
    address: Option<u32>,
    offset: u64,
    length: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StoredGraphEdge {
    pub(crate) caller: String,
    pub(crate) callee: String,
    pub(crate) site: Option<u32>,
    pub(crate) kind: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphDocument {
    schema_version: u32,
    command: String,
    edges: Vec<StoredGraphEdge>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RegisterIndexDocument {
    schema_version: u32,
    command: String,
    registers: Vec<StoredMmioRegister>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DataObjectIndexRecord {
    source: String,
    member: Option<String>,
    symbol: String,
    address: Option<String>,
    offset: u64,
    length: u64,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DataObjectIndexDocument {
    schema_version: u32,
    command: String,
    records: Vec<DataObjectIndexRecord>,
}

pub(crate) struct LinkedIrReader {
    root: PathBuf,
    manifest: super::LinkedIrStoredDocument,
    index: Vec<FunctionIndexRecord>,
    data_object_index: Vec<DataObjectIndexRecord>,
    graph: Vec<StoredGraphEdge>,
    outgoing: BTreeMap<String, Vec<usize>>,
    incoming: BTreeMap<String, Vec<usize>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GraphSearchLimits {
    pub(crate) max_depth: usize,
    pub(crate) max_visited_nodes: usize,
    pub(crate) max_examined_edges: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GraphPathSearch {
    pub(crate) path: Option<Vec<StoredGraphEdge>>,
    pub(crate) visited_nodes: usize,
    pub(crate) examined_edges: usize,
    pub(crate) limit: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GraphReachability {
    pub(crate) identities: BTreeSet<String>,
    pub(crate) visited_nodes: usize,
    pub(crate) examined_edges: usize,
    pub(crate) limit: Option<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GraphSlice {
    pub(crate) edges: Vec<StoredGraphEdge>,
    pub(crate) visited_nodes: usize,
    pub(crate) examined_edges: usize,
    pub(crate) limit: Option<&'static str>,
}

pub(crate) struct LinkedIrReviewProjection {
    pub(crate) inputs: Vec<(String, String)>,
    pub(crate) functions: Vec<StoredFunctionReviewProjection>,
}

fn parse_persisted_address(value: &str) -> Option<u32> {
    value
        .strip_prefix("0x")
        .and_then(|value| u32::from_str_radix(value, 16).ok())
        .or_else(|| value.parse().ok())
}

impl LinkedIrReader {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if !path.is_dir() {
            return Err(crate::Error::invalid(format!(
                "linked-IR output {} is not a schema-v{} bundle directory",
                path.display(),
                super::LINKED_IR.version
            )));
        }
        for member in BUNDLE_FILES {
            if !path.join(member).is_file() {
                return Err(crate::Error::invalid(format!(
                    "linked-IR schema-v{} bundle {} is missing required member {member:?}",
                    super::LINKED_IR.version,
                    path.display()
                )));
            }
        }
        let manifest_input = fs::read_to_string(path.join(MANIFEST))?;
        let manifest = super::parse_linked_ir(&manifest_input)?;
        let index: FunctionIndexDocument =
            serde_json::from_str(&fs::read_to_string(path.join(FUNCTION_INDEX))?)?;
        if index.schema_version != super::LINKED_IR.version || index.command != "ir function index"
        {
            return Err(crate::Error::invalid(format!(
                "invalid linked-IR function index in {}",
                path.display()
            )));
        }
        let data_object_index: DataObjectIndexDocument =
            serde_json::from_str(&fs::read_to_string(path.join(DATA_OBJECT_INDEX))?)?;
        if data_object_index.schema_version != super::LINKED_IR.version
            || data_object_index.command != "ir data object index"
        {
            return Err(crate::Error::invalid(format!(
                "invalid linked-IR data object index in {}",
                path.display()
            )));
        }
        let graph: GraphDocument = serde_json::from_str(&fs::read_to_string(path.join(GRAPH))?)?;
        if graph.schema_version != super::LINKED_IR.version || graph.command != "ir graph index" {
            return Err(crate::Error::invalid(format!(
                "invalid linked-IR graph index in {}",
                path.display()
            )));
        }
        let graph = graph.edges;
        let mut outgoing = BTreeMap::<String, Vec<usize>>::new();
        let mut incoming = BTreeMap::<String, Vec<usize>>::new();
        for (index, edge) in graph.iter().enumerate() {
            outgoing.entry(edge.caller.clone()).or_default().push(index);
            incoming.entry(edge.callee.clone()).or_default().push(index);
        }
        Ok(Self {
            root: path.to_owned(),
            manifest,
            index: index.records,
            data_object_index: data_object_index.records,
            graph,
            outgoing,
            incoming,
        })
    }

    pub(crate) fn summary(&self) -> &super::linked_ir_read::schema::StoredReportSummary {
        &self.manifest.summary
    }

    pub(crate) fn get_function(
        &self,
        source: &str,
        symbol: &str,
        member: Option<&str>,
        address: u64,
    ) -> Result<Option<StoredFunction>> {
        let address = u32::try_from(address).ok();
        let matches = self
            .index
            .iter()
            .filter(|record| record.source == source && record.symbol == symbol)
            .filter(|record| member.is_none_or(|member| record.member.as_deref() == Some(member)))
            .filter(|record| address.is_none_or(|address| record.address == Some(address)))
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            return Err(crate::Error::invalid(format!(
                "linked-IR bundle has multiple functions for {source}:{symbol}; use an exact identity"
            )));
        }
        matches
            .first()
            .map(|record| self.read_function(record))
            .transpose()
    }

    pub(crate) fn get_function_by_identity(
        &self,
        identity: &str,
    ) -> Result<Option<StoredFunction>> {
        self.index
            .iter()
            .find(|record| record.identity == identity)
            .map(|record| self.read_function(record))
            .transpose()
    }

    pub(crate) fn function_identities(&self, source: &str, selector: &str) -> Vec<String> {
        self.index
            .iter()
            .filter(|record| record.source == source)
            .filter(|record| record.identity == selector || record.symbol == selector)
            .map(|record| record.identity.clone())
            .collect()
    }

    /// Return exact linked labels at one constant address for human
    /// provenance. Aliases are all retained and never silently selected.
    pub(crate) fn labels_at_address(&self, address: u32) -> Vec<String> {
        let mut labels = self
            .index
            .iter()
            .filter(|record| record.address == Some(address))
            .map(|record| record.identity.clone())
            .collect::<BTreeSet<_>>();
        labels.extend(
            self.data_object_index
                .iter()
                .filter(|record| {
                    record.address.as_deref().and_then(parse_persisted_address) == Some(address)
                })
                .map(|record| {
                    format!(
                        "{}::{}::{}",
                        record.source,
                        record.member.as_deref().unwrap_or("<linked>"),
                        record.symbol
                    )
                }),
        );
        labels.into_iter().collect()
    }

    pub(crate) fn matching_function_identities(&self, selector: &str) -> BTreeSet<String> {
        self.index
            .iter()
            .filter(|record| record.identity == selector || record.symbol == selector)
            .map(|record| record.identity.clone())
            .collect()
    }

    pub(crate) fn mmio_function_identities(
        &self,
        register: Option<&str>,
        address: Option<u32>,
    ) -> Result<BTreeSet<String>> {
        let registers = self.read_registers()?;
        Ok(registers
            .into_iter()
            .filter(|candidate| {
                register.is_none_or(|name| candidate.names.iter().any(|item| item == name))
                    && address.is_none_or(|value| candidate.address == value)
            })
            .flat_map(|candidate| candidate.functions)
            .collect())
    }

    pub(crate) fn shortest_path_to_any(
        &self,
        root: &str,
        targets: &BTreeSet<String>,
        limits: GraphSearchLimits,
    ) -> GraphPathSearch {
        if targets.contains(root) {
            return GraphPathSearch {
                path: Some(Vec::new()),
                visited_nodes: 1,
                examined_edges: 0,
                limit: None,
            };
        }

        let mut queue = VecDeque::from([(root.to_owned(), 0usize)]);
        let mut visited = BTreeSet::from([root.to_owned()]);
        let mut predecessor = BTreeMap::<String, usize>::new();
        let mut examined_edges = 0usize;
        let mut depth_exhausted = false;
        let mut limit = None;
        let mut reached = None;

        'search: while let Some((node, depth)) = queue.pop_front() {
            if depth >= limits.max_depth {
                depth_exhausted |= self.has_traversable_outgoing(&node);
                continue;
            }
            for &edge_index in self.outgoing.get(&node).into_iter().flatten() {
                if examined_edges >= limits.max_examined_edges {
                    limit = Some("max-examined-edges");
                    break 'search;
                }
                examined_edges += 1;
                let edge = &self.graph[edge_index];
                if !traversable_call_edge(edge) {
                    continue;
                }
                if visited.contains(&edge.callee) {
                    continue;
                }
                if visited.len() >= limits.max_visited_nodes {
                    limit = Some("max-visited-nodes");
                    break 'search;
                }
                visited.insert(edge.callee.clone());
                predecessor.insert(edge.callee.clone(), edge_index);
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
                let edge_index = predecessor[&node];
                let edge = self.graph[edge_index].clone();
                node = edge.caller.clone();
                reversed.push(edge);
            }
            reversed.reverse();
            reversed
        });
        if path.is_none() && limit.is_none() && depth_exhausted {
            limit = Some("max-depth");
        }
        GraphPathSearch {
            path,
            visited_nodes: visited.len(),
            examined_edges,
            limit,
        }
    }

    pub(crate) fn reachable_from(
        &self,
        root: &str,
        limits: GraphSearchLimits,
    ) -> GraphReachability {
        let mut queue = VecDeque::from([(root.to_owned(), 0usize)]);
        let mut visited = BTreeSet::from([root.to_owned()]);
        let mut examined_edges = 0usize;
        let mut depth_exhausted = false;
        let mut limit = None;

        'search: while let Some((node, depth)) = queue.pop_front() {
            if depth >= limits.max_depth {
                depth_exhausted |= self.has_traversable_outgoing(&node);
                continue;
            }
            for &edge_index in self.outgoing.get(&node).into_iter().flatten() {
                if examined_edges >= limits.max_examined_edges {
                    limit = Some("max-examined-edges");
                    break 'search;
                }
                examined_edges += 1;
                let edge = &self.graph[edge_index];
                if !traversable_call_edge(edge) {
                    continue;
                }
                if visited.contains(&edge.callee) {
                    continue;
                }
                if visited.len() >= limits.max_visited_nodes {
                    limit = Some("max-visited-nodes");
                    break 'search;
                }
                visited.insert(edge.callee.clone());
                queue.push_back((edge.callee.clone(), depth + 1));
            }
        }
        if limit.is_none() && depth_exhausted {
            limit = Some("max-depth");
        }
        GraphReachability {
            identities: visited.clone(),
            visited_nodes: visited.len(),
            examined_edges,
            limit,
        }
    }

    pub(crate) fn graph_slice(
        &self,
        root: &str,
        depth: usize,
        include_callers: bool,
        limits: GraphSearchLimits,
    ) -> GraphSlice {
        let mut frontier = BTreeSet::from([root.to_owned()]);
        let mut visited = frontier.clone();
        let mut selected = BTreeSet::new();
        let mut examined_edges = 0usize;
        let mut limit = None;
        if include_callers {
            for &index in self.incoming.get(root).into_iter().flatten() {
                if examined_edges >= limits.max_examined_edges {
                    limit = Some("max-examined-edges");
                    break;
                }
                examined_edges += 1;
                let edge = &self.graph[index];
                selected.insert(index);
                if !visited.contains(&edge.caller) && visited.len() >= limits.max_visited_nodes {
                    limit = Some("max-visited-nodes");
                    break;
                }
                if visited.insert(edge.caller.clone()) {
                    frontier.insert(edge.caller.clone());
                }
            }
        }
        'search: for _ in 0..depth {
            let mut next = BTreeSet::new();
            for node in &frontier {
                for &index in self.outgoing.get(node).into_iter().flatten() {
                    if examined_edges >= limits.max_examined_edges {
                        limit = Some("max-examined-edges");
                        break 'search;
                    }
                    examined_edges += 1;
                    let edge = &self.graph[index];
                    selected.insert(index);
                    if !traversable_call_edge(edge) || visited.contains(&edge.callee) {
                        continue;
                    }
                    if visited.len() >= limits.max_visited_nodes {
                        limit = Some("max-visited-nodes");
                        break 'search;
                    }
                    if visited.insert(edge.callee.clone()) {
                        next.insert(edge.callee.clone());
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
        if limit.is_none()
            && frontier
                .iter()
                .any(|node| self.has_traversable_outgoing(node))
        {
            limit = Some("max-depth");
        }
        GraphSlice {
            edges: selected
                .into_iter()
                .map(|index| self.graph[index].clone())
                .collect(),
            visited_nodes: visited.len(),
            examined_edges,
            limit,
        }
    }

    pub(crate) fn read_all_functions(&self) -> Result<Vec<StoredFunction>> {
        let file = fs::File::open(self.root.join(FUNCTIONS))?;
        let mut functions: Vec<StoredFunction> = Vec::with_capacity(self.index.len());
        for line in std::io::BufReader::new(file).lines() {
            let line = line?;
            if !line.is_empty() {
                functions.push(super::json::from_str(&line)?);
            }
        }
        if functions.len() != self.index.len() {
            return Err(crate::Error::invalid(format!(
                "linked-IR function stream has {} records but its index has {}",
                functions.len(),
                self.index.len()
            )));
        }
        let indexed = self
            .index
            .iter()
            .map(|record| {
                (
                    &record.identity,
                    &record.source,
                    &record.symbol,
                    record.address,
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        let streamed = functions
            .iter()
            .map(|function| {
                (
                    &function.identity,
                    &function.source,
                    &function.symbol,
                    function.address,
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        if streamed != indexed {
            return Err(crate::Error::invalid(
                "linked-IR function stream identities do not match its index",
            ));
        }
        Ok(functions)
    }

    pub(crate) fn read_review_projection(&self) -> Result<LinkedIrReviewProjection> {
        let file = fs::File::open(self.root.join(FUNCTION_OVERVIEW))?;
        let mut functions: Vec<StoredFunctionReviewProjection> =
            Vec::with_capacity(self.index.len());
        for line in std::io::BufReader::new(file).lines() {
            let line = line?;
            if !line.is_empty() {
                functions.push(super::json::from_str(&line)?);
            }
        }
        if functions.len() != self.index.len() {
            return Err(crate::Error::invalid(format!(
                "linked-IR review projection has {} records but its index has {}",
                functions.len(),
                self.index.len()
            )));
        }
        let indexed = self
            .index
            .iter()
            .map(|record| {
                (
                    &record.identity,
                    &record.source,
                    &record.symbol,
                    &record.member,
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        let overview = functions
            .iter()
            .map(|function| {
                (
                    &function.identity,
                    &function.source,
                    &function.symbol,
                    &function.member,
                )
            })
            .collect::<std::collections::BTreeSet<_>>();
        if overview != indexed {
            return Err(crate::Error::invalid(
                "linked-IR function overview identities do not match its index",
            ));
        }
        let inputs = self
            .manifest
            .artifacts
            .iter()
            .map(|artifact| (artifact.source.clone(), artifact.artifact.sha256.clone()))
            .collect();
        Ok(LinkedIrReviewProjection { inputs, functions })
    }

    fn into_document(mut self, include_registers: bool) -> Result<super::LinkedIrStoredDocument> {
        let functions = self.read_all_functions()?;
        let registers = if include_registers {
            self.read_registers()?
        } else {
            Vec::new()
        };
        self.manifest
            .replace_bundle_payload(functions, registers, Vec::new());
        Ok(self.manifest)
    }

    pub(crate) fn read_registers(&self) -> Result<Vec<StoredMmioRegister>> {
        let registers: RegisterIndexDocument =
            serde_json::from_str(&fs::read_to_string(self.root.join(REGISTER_INDEX))?)?;
        if registers.schema_version != super::LINKED_IR.version
            || registers.command != "ir register index"
        {
            return Err(crate::Error::invalid(format!(
                "invalid linked-IR register index in {}",
                self.root.display()
            )));
        }
        Ok(registers.registers)
    }

    pub(crate) fn get_data_object(
        &self,
        source: &str,
        symbol: &str,
    ) -> Result<Vec<StoredDataObject>> {
        self.data_object_index
            .iter()
            .filter(|record| record.source == source && record.symbol == symbol)
            .map(|record| self.read_data_object(record))
            .collect()
    }

    fn read_function(&self, record: &FunctionIndexRecord) -> Result<StoredFunction> {
        let mut file = fs::File::open(self.root.join(FUNCTIONS))?;
        file.seek(SeekFrom::Start(record.offset))?;
        let size = usize::try_from(record.length)
            .map_err(|_| crate::Error::invalid("linked-IR function record exceeds host size"))?;
        let mut bytes = vec![0; size];
        file.read_exact(&mut bytes)?;
        let function: StoredFunction = super::json::from_slice(&bytes)?;
        if function.identity != record.identity
            || function.source != record.source
            || function.member != record.member
            || function.symbol != record.symbol
            || function.address != record.address
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR function index does not match record {:?}",
                record.identity
            )));
        }
        Ok(function)
    }

    fn read_data_object(&self, record: &DataObjectIndexRecord) -> Result<StoredDataObject> {
        let mut file = fs::File::open(self.root.join(DATA_OBJECTS))?;
        file.seek(SeekFrom::Start(record.offset))?;
        let size = usize::try_from(record.length)
            .map_err(|_| crate::Error::invalid("linked-IR data object record exceeds host size"))?;
        let mut bytes = vec![0; size];
        file.read_exact(&mut bytes)?;
        let object: StoredDataObject = super::json::from_slice(&bytes)?;
        if object.source != record.source
            || object.member != record.member
            || object.symbol != record.symbol
            || object.address != record.address
        {
            return Err(crate::Error::invalid(format!(
                "linked-IR data object index does not match {}:{}",
                record.source, record.symbol
            )));
        }
        Ok(object)
    }

    fn has_traversable_outgoing(&self, identity: &str) -> bool {
        self.outgoing
            .get(identity)
            .into_iter()
            .flatten()
            .any(|index| traversable_call_edge(&self.graph[*index]))
    }
}

fn traversable_call_edge(edge: &StoredGraphEdge) -> bool {
    matches!(
        edge.kind.as_str(),
        "internal" | "project-linked" | "indexed-dispatch" | "structural-relocation"
    )
}

pub(crate) fn load_linked_ir_functions(path: &Path) -> Result<super::LinkedIrStoredDocument> {
    LinkedIrReader::open(path)?.into_document(false)
}

#[cfg(test)]
pub(crate) fn write_fixture_bundle(path: &Path, input: &str) -> Result<()> {
    let mut document = super::parse_linked_ir(input)?;
    let functions = std::mem::take(&mut document.functions);
    let registers = std::mem::take(&mut document.mmio_registers);
    let data_objects = std::mem::take(&mut document.data_objects);
    let mut records = Vec::with_capacity(functions.len());
    let mut function_lines = String::new();
    let mut function_overview_lines = String::new();
    for function in &functions {
        let encoded = serde_json::to_string(function)?;
        let offset = u64::try_from(function_lines.len())
            .map_err(|_| crate::Error::invalid("fixture linked-IR bundle exceeds u64"))?;
        function_lines.push_str(&encoded);
        function_lines.push('\n');
        function_overview_lines.push_str(&fixture_function_overview(&encoded)?);
        function_overview_lines.push('\n');
        records.push(FunctionIndexRecord {
            identity: function.identity.clone(),
            source: function.source.clone(),
            member: function.member.clone(),
            symbol: function.symbol.clone(),
            address: function.address,
            offset,
            length: u64::try_from(encoded.len())
                .map_err(|_| crate::Error::invalid("fixture function record exceeds u64"))?,
        });
    }
    records.sort_by(|left, right| {
        (&left.source, &left.identity).cmp(&(&right.source, &right.identity))
    });
    fs::create_dir_all(path)?;
    fs::write(
        path.join(MANIFEST),
        serde_json::to_string_pretty(&document)? + "\n",
    )?;
    fs::write(path.join(FUNCTIONS), function_lines)?;
    fs::write(path.join(FUNCTION_OVERVIEW), function_overview_lines)?;
    fs::write(
        path.join(FUNCTION_INDEX),
        serde_json::to_string_pretty(&FunctionIndexDocument {
            schema_version: super::LINKED_IR.version,
            command: "ir function index".to_owned(),
            records,
        })? + "\n",
    )?;
    fs::write(
        path.join(GRAPH),
        format!(
            "{{\n  \"schema_version\": {},\n  \"command\": \"ir graph index\",\n  \"edges\": []\n}}\n",
            super::LINKED_IR.version
        ),
    )?;
    fs::write(
        path.join(REGISTER_INDEX),
        serde_json::to_string_pretty(&RegisterIndexDocument {
            schema_version: super::LINKED_IR.version,
            command: "ir register index".to_owned(),
            registers,
        })? + "\n",
    )?;
    let mut object_lines = String::new();
    let mut object_records = Vec::new();
    for object in data_objects {
        let encoded = serde_json::to_string(&object)?;
        let offset = object_lines.len() as u64;
        let length = encoded.len() as u64;
        object_lines.push_str(&encoded);
        object_lines.push('\n');
        object_records.push(DataObjectIndexRecord {
            source: object.source,
            member: object.member,
            symbol: object.symbol,
            address: object.address,
            offset,
            length,
        });
    }
    fs::write(path.join(DATA_OBJECTS), object_lines)?;
    fs::write(
        path.join(DATA_OBJECT_INDEX),
        serde_json::to_string_pretty(&DataObjectIndexDocument {
            schema_version: super::LINKED_IR.version,
            command: "ir data object index".to_owned(),
            records: object_records,
        })? + "\n",
    )?;
    Ok(())
}

#[cfg(test)]
fn fixture_function_overview(encoded: &str) -> Result<String> {
    let full: serde_json::Value = serde_json::from_str(encoded)?;
    let summary = &full["effect_summary"];
    let event_dispatches = summary["event_dispatches"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|dispatch| {
            serde_json::json!({
                "mechanism": dispatch["mechanism"],
                "execution_context": dispatch["execution_context"],
                "receiver": dispatch["receiver"],
                "interface_complete": dispatch["interface_complete"],
                "bindings": dispatch["bindings"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|binding| serde_json::json!({
                        "role": binding["role"],
                        "value": binding["argument"]["value"],
                    }))
                    .collect::<Vec<_>>(),
            })
        })
        .collect::<Vec<_>>();
    let mmio_addresses = full["mmio_accesses"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|access| access["address"].as_u64())
        .collect::<std::collections::BTreeSet<_>>();
    let mmio = full["mmio_accesses"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|access| {
            serde_json::json!({
                "address": access["address"],
                "width": access["width"],
            })
        })
        .collect::<Vec<_>>();
    let diagnostics = [
        "call_graph_diagnostics",
        "direct_diagnostics",
        "reference_diagnostics",
    ]
    .into_iter()
    .zip(["call-graph", "direct", "reference"])
    .flat_map(|(field, channel)| {
        full[field]
            .as_array()
            .into_iter()
            .flatten()
            .map(move |diagnostic| {
                serde_json::json!({
                    "channel": channel,
                    "root_id": diagnostic["root_id"],
                    "kind": diagnostic["kind"],
                    "site": diagnostic["site"],
                    "rendered": diagnostic["rendered"],
                })
            })
    })
    .collect::<Vec<_>>();
    let overview = serde_json::json!({
        "source": full["source"],
        "identity": full["identity"],
        "selection": full["selection"],
        "member": full["member"],
        "symbol": full["symbol"],
        "binding": full["binding"],
        "complete": full["complete"],
        "dependencies": full["dependencies"],
        "direct_calls": full["calls"].as_array().map_or(0, Vec::len),
        "calls": full["calls"].as_array().into_iter().flatten().map(|call| serde_json::json!({
            "kind": call["kind"], "target": call["target"], "site": call["site"],
            "project_symbol": call["project_symbol"],
        })).collect::<Vec<_>>(),
        "mmio": mmio,
        "mmio_addresses": mmio_addresses,
        "direct_context_fields": full["context_fields"].as_array().map_or(0, Vec::len),
        "direct_memory_fields": full["memory_fields"].as_array().map_or(0, Vec::len),
        "diagnostics": diagnostics,
        "effect_summary": {
            "transitive_effects_materialized": summary["transitive_effects_materialized"],
            "call_graph_closed": summary["call_graph_closed"],
            "context_projection_materialized": summary["context_projection_materialized"],
            "context_projection_complete": summary["context_projection_complete"],
            "context_projection_blockers": summary["context_projection_blockers"],
            "context_fields": summary["context_fields"].as_array().into_iter().flatten().map(|field| serde_json::json!({
                "argument": field["argument"], "offset": field["offset"], "width": field["width"],
                "reads": field["reads"], "writes": field["writes"], "write_mask": field["write_mask"],
            })).collect::<Vec<_>>(),
            "memory_fields": summary["memory_fields"].as_array().into_iter().flatten().map(|field| serde_json::json!({
                "object": field["object"], "offset": field["offset"], "width": field["width"],
                "reads": field["reads"], "writes": field["writes"], "write_mask": field["write_mask"],
                "origins": field["origins"],
            })).collect::<Vec<_>>(),
            "semantic_operations": summary["semantic_operations"].as_array().into_iter().flatten().map(|operation| operation["operation"].clone()).collect::<Vec<_>>(),
            "trampoline_calls": summary["trampoline_calls"].as_array().map_or(0, Vec::len),
            "event_dispatches": event_dispatches,
        },
        "decode_blockers": full["decode_blockers"],
    });
    let strict: StoredFunctionReviewProjection = serde_json::from_value(overview)?;
    Ok(serde_json::to_string(&strict)?)
}
