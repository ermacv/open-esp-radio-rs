//! Reusable public interface layouts and fail-closed project overlay composition.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use serde::Deserialize;

use super::{InterfaceSlot, PackOrigin, ReviewStatus, SemanticCatalogs, validate_abi_type};
use crate::Result;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct InterfaceTemplateDocument {
    schema: u32,
    id: String,
    #[serde(default)]
    templates: Vec<InterfaceTemplate>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct InterfaceTemplate {
    pub(super) id: String,
    provenance: InterfaceTemplateProvenance,
    pub(super) layout_version: String,
    pub(super) pointer_width: u8,
    pub(super) layout_size: u32,
    pub(super) slot_stride: u8,
    #[serde(default)]
    pub(super) slots: Vec<InterfaceTemplateSlot>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct InterfaceTemplateProvenance {
    pub(super) repository: String,
    pub(super) revision: String,
    pub(super) path: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub(super) struct InterfaceTemplateSlot {
    pub(super) offset: i32,
    pub(super) width: u8,
    pub(super) name: String,
    pub(super) arguments: Vec<String>,
    #[serde(rename = "return")]
    pub(super) return_type: String,
    #[serde(default)]
    pub(super) variadic: bool,
    #[serde(default)]
    pub(super) semantic: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(super) struct InterfaceTemplateCatalog {
    pack_ids: Vec<String>,
    templates: BTreeMap<String, InterfaceTemplate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InterfaceTemplateSummary {
    pub(crate) id: String,
    pub(crate) repository: String,
    pub(crate) revision: String,
    pub(crate) path: String,
}

impl InterfaceTemplateCatalog {
    pub(super) fn load(paths: &[impl AsRef<Path>], catalogs: &SemanticCatalogs) -> Result<Self> {
        let mut templates = BTreeMap::<String, InterfaceTemplate>::new();
        let mut pack_ids = BTreeSet::new();
        for path in paths {
            let path = path.as_ref();
            let input = fs::read_to_string(path)
                .map_err(|error| crate::Error::read("interface template pack", path, error))?;
            let document: InterfaceTemplateDocument =
                toml_edit::de::from_str(&input).map_err(|error| {
                    crate::error::BlobrayError::manifest_source(
                        "interface template pack",
                        path,
                        &input,
                        &error,
                        error.span(),
                    )
                })?;
            if document.schema != 1 {
                return Err(crate::error::BlobrayError::manifest(
                    "interface template pack",
                    path,
                    "requires schema = 1",
                ));
            }
            super::validate_dotted_id(&document.id, "interface template pack id")?;
            if !pack_ids.insert(document.id.clone()) {
                return Err(crate::error::BlobrayError::manifest(
                    "interface template pack",
                    path,
                    format!("duplicate interface template pack id {:?}", document.id),
                ));
            }
            let mut local = BTreeSet::new();
            for template in document.templates {
                validate_template(&template, catalogs)?;
                if !local.insert(template.id.clone()) {
                    return Err(crate::error::BlobrayError::manifest(
                        "interface template pack",
                        path,
                        format!("duplicate interface template id {:?}", template.id),
                    ));
                }
                match templates.get(&template.id) {
                    Some(existing) if existing != &template => {
                        return Err(crate::error::BlobrayError::manifest(
                            "interface template pack",
                            path,
                            format!(
                                "interface template {:?} conflicts with an earlier pack",
                                template.id
                            ),
                        ));
                    }
                    Some(_) => {}
                    None => {
                        templates.insert(template.id.clone(), template);
                    }
                }
            }
        }
        Ok(Self {
            pack_ids: pack_ids.into_iter().collect(),
            templates,
        })
    }

    pub(super) fn get(&self, id: &str) -> Option<&InterfaceTemplate> {
        self.templates.get(id)
    }

    pub(super) fn len(&self) -> usize {
        self.templates.len()
    }

    pub(super) fn pack_ids(&self) -> &[String] {
        &self.pack_ids
    }

    pub(super) fn summaries(&self) -> Vec<InterfaceTemplateSummary> {
        self.templates
            .values()
            .map(|template| InterfaceTemplateSummary {
                id: template.id.clone(),
                repository: template.provenance.repository.clone(),
                revision: template.provenance.revision.clone(),
                path: template.provenance.path.clone(),
            })
            .collect()
    }
}

impl InterfaceTemplateSlot {
    pub(super) fn materialize(&self) -> InterfaceSlot {
        InterfaceSlot {
            offset: self.offset,
            width: self.width,
            status: ReviewStatus::Reviewed,
            origin: PackOrigin::Reviewed,
            name: Some(self.name.clone()),
            arguments: Some(self.arguments.clone()),
            return_type: Some(self.return_type.clone()),
            variadic: self.variadic,
            semantic: self.semantic.clone(),
            execution_model: None,
        }
    }
}

fn validate_template(template: &InterfaceTemplate, catalogs: &SemanticCatalogs) -> Result<()> {
    super::validate_dotted_id(&template.id, "interface template id")?;
    if !template.provenance.repository.starts_with("https://")
        || template
            .provenance
            .repository
            .chars()
            .any(char::is_whitespace)
        || template.provenance.revision.len() != 40
        || !template
            .provenance
            .revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
        || template.provenance.path.is_empty()
        || Path::new(&template.provenance.path).is_absolute()
        || template.provenance.path.contains(['\r', '\n'])
    {
        return Err(crate::Error::invalid(format!(
            "interface template {:?} requires an HTTPS repository, 40-hex revision, and relative provenance path",
            template.id
        )));
    }
    if template.layout_version.trim().is_empty() {
        return Err(crate::Error::invalid(format!(
            "interface template {:?} has an empty layout-version",
            template.id
        )));
    }
    if !matches!(template.pointer_width, 16 | 32 | 64) {
        return Err(crate::Error::invalid(format!(
            "interface template {:?} has unsupported pointer-width {}",
            template.id, template.pointer_width
        )));
    }
    if template.layout_size == 0 || template.slot_stride == 0 {
        return Err(crate::Error::invalid(format!(
            "interface template {:?} requires non-zero layout-size and slot-stride",
            template.id
        )));
    }
    if template.slots.is_empty() {
        return Err(crate::Error::invalid(format!(
            "interface template {:?} must declare at least one public slot",
            template.id
        )));
    }
    let mut offsets = BTreeSet::new();
    let mut names = BTreeSet::new();
    for slot in &template.slots {
        if !offsets.insert(slot.offset) {
            return Err(crate::Error::invalid(format!(
                "interface template {:?} has duplicate slot offset {:+#x}",
                template.id, slot.offset
            )));
        }
        if !names.insert(slot.name.as_str()) {
            return Err(crate::Error::invalid(format!(
                "interface template {:?} has duplicate slot name {:?}",
                template.id, slot.name
            )));
        }
        if slot.width != template.pointer_width {
            return Err(crate::Error::invalid(format!(
                "interface template {:?} slot {:+#x} width {} differs from pointer-width {}",
                template.id, slot.offset, slot.width, template.pointer_width
            )));
        }
        let offset = u32::try_from(slot.offset).map_err(|_| {
            crate::Error::invalid(format!(
                "interface template {:?} has negative slot offset {:+#x}",
                template.id, slot.offset
            ))
        })?;
        if offset % u32::from(template.slot_stride) != 0
            || offset
                .checked_add(u32::from(slot.width) / 8)
                .is_none_or(|end| end > template.layout_size)
        {
            return Err(crate::Error::invalid(format!(
                "interface template {:?} slot {offset:#x} is misaligned or outside layout-size",
                template.id
            )));
        }
        validate_identifier(&slot.name, "interface template slot name")?;
        for argument in &slot.arguments {
            validate_abi_type(argument, false, &format!("slot {:?} argument", slot.name))
                .map_err(crate::Error::invalid)?;
        }
        validate_abi_type(
            &slot.return_type,
            true,
            &format!("slot {:?} return", slot.name),
        )
        .map_err(crate::Error::invalid)?;
        if let Some(semantic) = &slot.semantic {
            super::validate_dotted_id(semantic, "interface template semantic operation")?;
            let operation = catalogs.get(semantic).ok_or_else(|| {
                crate::Error::invalid(format!(
                    "interface template {:?} slot {:?} refers to unknown semantic operation {semantic:?}",
                    template.id, slot.name
                ))
            })?;
            if operation.argument_roles.len() != slot.arguments.len()
                || operation.variadic != slot.variadic
                || ((operation.return_role == "none") != (slot.return_type == "void"))
            {
                return Err(crate::Error::invalid(format!(
                    "interface template {:?} slot {:?} ABI does not match semantic operation {semantic:?}",
                    template.id, slot.name
                )));
            }
        }
    }
    Ok(())
}

fn validate_identifier(value: &str, context: &str) -> Result<()> {
    let mut bytes = value.bytes();
    if !bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        || !bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(crate::Error::invalid(format!(
            "invalid {context} {value:?}"
        )));
    }
    Ok(())
}

pub(crate) fn validate_interface_template_packs(
    template_paths: &[impl AsRef<Path>],
    semantic_paths: &[impl AsRef<Path>],
) -> Result<usize> {
    let catalogs = SemanticCatalogs::load(semantic_paths)?;
    InterfaceTemplateCatalog::load(template_paths, &catalogs).map(|templates| templates.len())
}
