//! Editable function/context pack and its resolved workspace view.

use std::{fs, path::Path};

use toml_edit::{Document, DocumentMut, Item};

use super::FunctionFacts;
use crate::Result;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FunctionReviewStatus {
    Unreviewed,
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
    pub(crate) contexts: Vec<ReviewedContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FunctionPack {
    pub(crate) id: String,
    pub(crate) inputs: Vec<ReviewedFunctionInput>,
    pub(crate) functions: Vec<ReviewedFunction>,
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
    #[tracing::instrument(name = "load_function_pack", fields(path = %path.display()))]
    fn load(path: &Path) -> Result<LoadedFunctionPack> {
        let input = fs::read_to_string(path)?;
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
        if document.get("schema").and_then(Item::as_integer) != Some(1) {
            return Err(crate::error::WorkbenchError::manifest_source(
                "function pack",
                path,
                &input,
                "requires schema = 1",
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
