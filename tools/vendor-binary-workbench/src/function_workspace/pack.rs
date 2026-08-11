//! Editable function/context pack and its resolved workspace view.

use std::{fs, path::Path};

use toml_edit::{Document, DocumentMut, Item};

use super::FunctionFacts;
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FunctionReviewStatus {
    Reviewed,
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedFunctionInput {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedContextField {
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) display_type: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedContext {
    pub(crate) argument: u8,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) type_name: Option<String>,
    pub(crate) fields: Vec<ReviewedContextField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedFunction {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) role: Option<String>,
    pub(crate) summary: Option<String>,
    pub(crate) accept_incomplete: bool,
    pub(crate) preconditions: Vec<ReviewedPrecondition>,
    pub(crate) paths: Vec<ReviewedPath>,
    pub(crate) contexts: Vec<ReviewedContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedPrecondition {
    pub(crate) id: String,
    pub(crate) expression: String,
    pub(crate) rationale: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedPath {
    pub(crate) id: String,
    pub(crate) class: String,
    pub(crate) summary: String,
    pub(crate) evidence: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedEventRoute {
    pub(crate) id: String,
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) dispatcher: String,
    pub(crate) mechanism: String,
    pub(crate) selector_role: String,
    pub(crate) selector_value: u32,
    pub(crate) receiver: Option<String>,
    pub(crate) execution_context: String,
    pub(crate) handler_profile: String,
    pub(crate) handler_source: String,
    pub(crate) handler: String,
    pub(crate) rationale: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum ReviewedMemoryObject {
    Argument {
        function: String,
        index: u8,
    },
    Global {
        member: Option<String>,
        symbol: String,
    },
    Dereferenced {
        pointer: Box<ReviewedMemoryObject>,
        pointer_offset: i64,
    },
    Absolute {
        address_space: String,
        address: u32,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedTypeBinding {
    pub(crate) profile: String,
    pub(crate) source: String,
    pub(crate) name: String,
    pub(crate) object: ReviewedMemoryObject,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedTypeField {
    pub(crate) offset: i64,
    pub(crate) width: u8,
    pub(crate) status: FunctionReviewStatus,
    pub(crate) name: Option<String>,
    pub(crate) display_type: Option<String>,
    pub(crate) description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ReviewedLogicalType {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) description: Option<String>,
    pub(crate) bindings: Vec<ReviewedTypeBinding>,
    pub(crate) fields: Vec<ReviewedTypeField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionPack {
    pub(crate) id: String,
    pub(crate) inputs: Vec<ReviewedFunctionInput>,
    pub(crate) functions: Vec<ReviewedFunction>,
    pub(crate) types: Vec<ReviewedLogicalType>,
    pub(crate) event_routes: Vec<ReviewedEventRoute>,
}

struct LoadedFunctionPack {
    value: FunctionPack,
    input: String,
    document: Document<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct FunctionWorkspaceSummary {
    pub(crate) inputs: usize,
    pub(crate) observed_functions: usize,
    pub(crate) reviewed_functions: usize,
    pub(crate) ignored_functions: usize,
    pub(crate) unreviewed_functions: usize,
    pub(crate) reviewed_contexts: usize,
    pub(crate) ignored_contexts: usize,
    pub(crate) unreviewed_contexts: usize,
    pub(crate) reviewed_fields: usize,
    pub(crate) ignored_fields: usize,
    pub(crate) unreviewed_fields: usize,
    pub(crate) accepted_incomplete: usize,
    pub(crate) logical_types: usize,
    pub(crate) type_bindings: usize,
    pub(crate) type_fields: usize,
    pub(crate) reviewed_type_fields: usize,
    pub(crate) ignored_type_fields: usize,
    pub(crate) unreviewed_type_fields: usize,
    pub(crate) event_routes: usize,
}

#[derive(Debug)]
pub(crate) struct FunctionWorkspace {
    pub(crate) facts: FunctionFacts,
    pub(crate) pack: FunctionPack,
    summary: FunctionWorkspaceSummary,
}

impl FunctionWorkspace {
    pub(crate) fn load(reports: &[(String, std::path::PathBuf)], pack_path: &Path) -> Result<Self> {
        let facts = FunctionFacts::load(reports)?;
        let pack = FunctionPack::load(pack_path)?;
        let summary = super::pack_validate::validate(&pack.value, &facts).map_err(|error| {
            crate::error::WorkbenchError::manifest_source(
                "function pack",
                pack_path,
                &pack.input,
                &error,
                error.span(&pack.document),
            )
        })?;
        Ok(Self {
            facts,
            pack: pack.value,
            summary,
        })
    }

    pub(crate) const fn summary(&self) -> FunctionWorkspaceSummary {
        self.summary
    }
}

impl FunctionPack {
    pub(crate) fn load_reviewed(path: &Path) -> Result<Self> {
        Ok(Self::load(path)?.value)
    }

    #[tracing::instrument(name = "load_function_pack", fields(path = %path.display()))]
    fn load(path: &Path) -> Result<LoadedFunctionPack> {
        let input = fs::read_to_string(path)
            .map_err(|error| crate::Error::read("function pack", path, error))?;
        let source_document = Document::parse(input.clone()).map_err(|error| {
            crate::error::WorkbenchError::manifest_source(
                "function pack",
                path,
                &input,
                &error,
                error.span(),
            )
        })?;
        let document: DocumentMut = source_document.clone().into_mut();
        if document.get("schema").and_then(Item::as_integer) != Some(5) {
            return Err(crate::error::WorkbenchError::manifest_source(
                "function pack",
                path,
                &input,
                "requires schema = 5",
                source_document.get("schema").and_then(Item::span),
            ));
        }
        let value = super::pack_parse::parse(&document).map_err(|error| {
            crate::error::WorkbenchError::manifest("function pack", path, error)
        })?;
        Ok(LoadedFunctionPack {
            value,
            input,
            document: source_document,
        })
    }
}
