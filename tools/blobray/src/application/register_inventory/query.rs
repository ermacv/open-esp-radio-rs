//! Common filters and explicit pagination for every presentation layer.
use super::*;

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InventoryQuery {
    pub function: Option<String>,
    pub source: Option<String>,
    pub text: Option<String>,
    pub subject: Option<String>,
    pub start: Option<u64>,
    pub end_exclusive: Option<u64>,
    pub mask: Option<u32>,
    pub unknown: bool,
    pub conflicted: bool,
}

impl RegisterInventory {
    pub fn select(&self, query: &InventoryQuery) -> Vec<&InventoryRegister> {
        self.registers
            .values()
            .filter(|register| {
                query.subject.as_ref().is_none_or(|id| id == &register.id)
                    && query
                        .start
                        .is_none_or(|start| register.subject.address >= start)
                    && query
                        .end_exclusive
                        .is_none_or(|end| register.subject.address < end)
                    && query.function.as_ref().is_none_or(|function| {
                        register
                            .functions
                            .iter()
                            .any(|candidate| candidate.contains(function))
                    })
                    && query.mask.is_none_or(|mask| {
                        register
                            .fields
                            .values()
                            .any(|field| field.mask.is_some_and(|bits| bits & mask != 0))
                    })
                    && (!query.unknown
                        || register.names.is_unknown()
                        || register.width().is_none()
                        || register.semantics.is_unknown()
                        || register
                            .fields
                            .values()
                            .any(|field| field.names.is_unknown() || field.semantics.is_unknown()))
                    && (!query.conflicted
                        || matches!(register.names, KnowledgeProperty::Conflicted { .. })
                        || matches!(
                            register.physical_width,
                            KnowledgeProperty::Conflicted { .. }
                        )
                        || matches!(register.semantics, KnowledgeProperty::Conflicted { .. })
                        || register.fields.values().any(|field| {
                            matches!(field.names, KnowledgeProperty::Conflicted { .. })
                                || matches!(field.semantics, KnowledgeProperty::Conflicted { .. })
                        }))
                    && query.source.as_ref().is_none_or(|source| {
                        register
                            .evidence
                            .iter()
                            .filter_map(|id| self.evidence.get(id))
                            .any(|evidence| {
                                evidence.sources.iter().any(|candidate| {
                                    candidate.contains(source)
                                        || self.sources.iter().any(|input| {
                                            input.digest.as_ref() == Some(candidate)
                                                && input.path.contains(source)
                                        })
                                }) || evidence.identity.to_string().contains(source)
                            })
                    })
                    && query.text.as_ref().is_none_or(|text| {
                        register
                            .names
                            .values()
                            .into_iter()
                            .any(|name| name.contains(text))
                            || register.fields.values().any(|field| {
                                field
                                    .names
                                    .values()
                                    .into_iter()
                                    .any(|name| name.contains(text))
                            })
                            || register
                                .evidence
                                .iter()
                                .filter_map(|id| self.evidence.get(id))
                                .any(|evidence| evidence.payload.to_string().contains(text))
                    })
            })
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterQuestion {
    pub id: String,
    pub subject: String,
    pub field: Option<String>,
    pub dimension: String,
    pub evidence: BTreeSet<String>,
}

impl RegisterInventory {
    /// Investigation dimensions remain independent of identity review and PAC
    /// eligibility. A named register can still have unresolved field semantics.
    pub fn questions(&self) -> Vec<RegisterQuestion> {
        let mut output = Vec::new();
        for register in self.registers.values() {
            let mut add = |dimension: &str, field: Option<&InventoryField>| {
                let field_id = field.map(|field| field.id.clone());
                let identity = serde_json::to_vec(&(&register.id, &field_id, dimension))
                    .expect("identity serializes");
                output.push(RegisterQuestion {
                    id: format!("register-property-{:x}", Sha256::digest(identity)),
                    subject: register.id.clone(),
                    field: field_id,
                    dimension: dimension.to_owned(),
                    evidence: field
                        .map_or_else(|| register.evidence.clone(), |field| field.evidence.clone()),
                });
            };
            if register.names.is_unknown() {
                add("name", None);
            }
            if register.width().is_none() {
                add("geometry", None);
            }
            if register.semantics.is_unknown() {
                add("semantics", None);
            }
            if register.coverage.unobserved.is_none_or(|bits| bits != 0) {
                add("coverage", None);
            }
            if matches!(register.names, KnowledgeProperty::Conflicted { .. })
                || matches!(
                    register.physical_width,
                    KnowledgeProperty::Conflicted { .. }
                )
                || matches!(register.semantics, KnowledgeProperty::Conflicted { .. })
            {
                add("conflict", None);
            }
            for field in register
                .fields
                .values()
                .filter(|field| field.kind == "declared" || field.kind == "unknown")
            {
                if field.names.is_unknown() {
                    add("field-name", Some(field));
                }
                if field.semantics.is_unknown() {
                    add("field-semantics", Some(field));
                }
            }
        }
        output.sort_by(|a, b| a.id.cmp(&b.id));
        output
    }
}
