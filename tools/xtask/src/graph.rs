//! Cargo identities and normal/build reachability, independent of dependency aliases.

use crate::Result;
use cargo_metadata::{DependencyKind, Metadata, Node, Package, PackageId};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

#[derive(Debug)]
pub struct Graph {
    pub metadata: Metadata,
    packages: BTreeMap<PackageId, usize>,
    nodes: BTreeMap<PackageId, usize>,
}

fn required<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| format!("Cargo metadata field missing: {name}").into())
}
fn string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "Cargo metadata expected nonempty string".into())
}
fn array(value: &Value) -> Result<&Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| "Cargo metadata expected array".into())
}
fn nullable_string(value: &Value) -> Result<()> {
    if !value.is_null() && !value.is_string() {
        return Err("Cargo metadata expected nullable string".into());
    }
    Ok(())
}
fn kind(value: &Value) -> Result<()> {
    if value.is_null() || matches!(value.as_str(), Some("build" | "dev")) {
        return Ok(());
    }
    Err("Cargo metadata dependency kind missing or unknown".into())
}

pub(crate) fn validate_packages(document: &Value) -> Result<()> {
    let mut ids = BTreeSet::new();
    for package in array(required(document, "packages")?)? {
        for name in ["id", "name", "version", "manifest_path"] {
            string(required(package, name)?)?;
        }
        if !Path::new(string(required(package, "manifest_path")?)?).is_absolute() {
            return Err("package manifest path must be absolute".into());
        }
        if !ids.insert(string(required(package, "id")?)?) {
            return Err("duplicate package identity".into());
        }
        nullable_string(required(package, "source")?)?;
        if !required(package, "features")?.is_object() {
            return Err("declared feature map missing".into());
        }
        for dependency in array(required(package, "dependencies")?)? {
            string(required(dependency, "name")?)?;
            kind(required(dependency, "kind")?)?;
            nullable_string(required(dependency, "source")?)?;
            if !required(dependency, "optional")?.is_boolean() {
                return Err("declared optional dependency status invalid".into());
            }
            if let Some(path) = dependency.get("path").filter(|path| !path.is_null())
                && !Path::new(string(path)?).is_absolute()
            {
                return Err("declared dependency path must be absolute".into());
            }
        }
    }
    Ok(())
}

impl Graph {
    pub fn from_value(document: Value) -> Result<Self> {
        validate_packages(&document)?;
        let packages = array(required(&document, "packages")?)?;
        let ids: BTreeSet<_> = packages.iter().map(|p| string(&p["id"]).unwrap()).collect();
        let mut nodes = BTreeSet::new();
        let resolve = required(&document, "resolve")?;
        for node in array(required(resolve, "nodes")?)? {
            let id = string(required(node, "id")?)?;
            if !ids.contains(id) || !nodes.insert(id) {
                return Err("unknown or duplicate resolve identity".into());
            }
            for feature in array(required(node, "features")?)? {
                string(feature)?;
            }
            let declared_edges = array(required(node, "dependencies")?)?
                .iter()
                .map(string)
                .collect::<Result<BTreeSet<_>>>()?;
            let detailed_edges = array(required(node, "deps")?)?
                .iter()
                .map(|edge| string(required(edge, "pkg")?))
                .collect::<Result<BTreeSet<_>>>()?;
            if declared_edges != detailed_edges {
                return Err("resolved dependency edge inventories disagree".into());
            }
            for dependency in array(required(node, "deps")?)? {
                string(required(dependency, "name")?)?;
                if !ids.contains(string(required(dependency, "pkg")?)?) {
                    return Err("unknown dependency package identity".into());
                }
                let kinds = array(required(dependency, "dep_kinds")?)?;
                if kinds.is_empty() {
                    return Err("dependency kinds missing".into());
                }
                for edge in kinds {
                    kind(required(edge, "kind")?)?;
                    nullable_string(required(edge, "target")?)?;
                }
            }
        }
        for node in array(required(resolve, "nodes")?)? {
            for dependency in array(required(node, "deps")?)? {
                if !nodes.contains(string(required(dependency, "pkg")?)?) {
                    return Err("dependency resolve node missing".into());
                }
            }
        }
        let metadata: Metadata = serde_json::from_value(document)?;
        let packages = metadata
            .packages
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.clone(), i))
            .collect();
        let nodes = metadata
            .resolve
            .as_ref()
            .ok_or("resolved nodes missing")?
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.clone(), i))
            .collect();
        Ok(Self {
            metadata,
            packages,
            nodes,
        })
    }

    pub fn root(&self, manifest: &Path) -> Result<PackageId> {
        let manifest = manifest.canonicalize()?;
        let found: Vec<_> = self
            .metadata
            .packages
            .iter()
            .filter(|p| p.manifest_path.as_std_path() == manifest)
            .collect();
        if found.len() != 1 {
            return Err(format!(
                "manifest must identify exactly one package: {}",
                manifest.display()
            )
            .into());
        }
        let id = found[0].id.clone();
        if !self.nodes.contains_key(&id) {
            return Err("root resolve node missing".into());
        }
        Ok(id)
    }
    pub fn package(&self, id: &PackageId) -> &Package {
        &self.metadata.packages[self.packages[id]]
    }
    pub fn node(&self, id: &PackageId) -> &Node {
        &self
            .metadata
            .resolve
            .as_ref()
            .expect("validated resolve")
            .nodes[self.nodes[id]]
    }
    pub fn reachable(&self, root: &PackageId) -> BTreeMap<PackageId, Vec<PackageId>> {
        let mut paths = BTreeMap::from([(root.clone(), vec![root.clone()])]);
        let mut pending = vec![root.clone()];
        while let Some(current) = pending.pop() {
            for edge in &self.node(&current).deps {
                if edge
                    .dep_kinds
                    .iter()
                    .all(|kind| kind.kind == DependencyKind::Development)
                {
                    continue;
                }
                if !paths.contains_key(&edge.pkg) {
                    let mut path = paths[&current].clone();
                    path.push(edge.pkg.clone());
                    paths.insert(edge.pkg.clone(), path);
                    pending.push(edge.pkg.clone());
                }
            }
        }
        paths
    }
    pub fn same_resolution(&self, other: &Self) -> bool {
        let nodes = |graph: &Self| {
            graph
                .metadata
                .resolve
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .map(|node| (node.id.clone(), node.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        nodes(self) == nodes(other)
    }
}
