//! ESP32-S31 production SVD materialization from the shared project model.

use std::{
    collections::BTreeSet,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use open_esp_radio_register_model::RegisterModel;
use roxmltree::Document;

const MODEL_FILE: &str = "verification/vendor/targets/esp32s31/registers/device.toml";
const PAC_ADDON_FILE: &str = "verification/vendor/targets/esp32s31/registers/pac-addon.xml";
const AGGREGATE_FILE: &str = "svd/esp32s31-radio.svd";

#[derive(Debug)]
pub(crate) struct MaterializedRadioSvd {
    pub(crate) model_path: PathBuf,
    pub(crate) aggregate_path: PathBuf,
    pub(crate) contents: String,
    pub(crate) addon_path: PathBuf,
    pub(crate) addon_contents: String,
    pub(crate) review_sources: BTreeSet<String>,
}

fn required_attribute<'a>(
    node: roxmltree::Node<'a, 'a>,
    name: &str,
) -> Result<&'a str, Box<dyn Error>> {
    node.attribute(name)
        .ok_or_else(|| format!("{} is missing attribute {name:?}", node.tag_name().name()).into())
}

pub(crate) fn attach_pac_addon(svd: &str, addon: &str) -> Result<String, Box<dyn Error>> {
    let svd_document = Document::parse(svd)?;
    let svd_root = svd_document.root_element();
    if !svd_root.has_tag_name("device") {
        return Err("radio SVD has the wrong root element".into());
    }
    if svd_document
        .descendants()
        .any(|node| node.has_tag_name("vendorExtensions"))
    {
        return Err("radio SVD must not embed target PAC extensions".into());
    }

    let addon_document = Document::parse(addon)?;
    let addon_root = addon_document.root_element();
    if !addon_root.has_tag_name("openEspRadioPacAddon") {
        return Err("radio PAC add-on has the wrong root element".into());
    }
    if required_attribute(addon_root, "schema")? != "1" {
        return Err("radio PAC add-on requires schema=\"1\"".into());
    }
    let children = addon_root
        .children()
        .filter(roxmltree::Node::is_element)
        .collect::<Vec<_>>();
    if children.is_empty() {
        return Err("radio PAC add-on contains no extensions".into());
    }

    let closing = svd_root.range().end - "</device>".len();
    if !svd[closing..svd_root.range().end].starts_with("</device>") {
        return Err("radio SVD device closing tag cannot be located".into());
    }
    let mut output = String::with_capacity(svd.len() + addon.len());
    output.push_str(&svd[..closing]);
    output.push_str("  <vendorExtensions>\n");
    for child in children {
        for line in addon[child.range()].lines() {
            output.push_str("  ");
            output.push_str(line);
            output.push('\n');
        }
    }
    output.push_str("  </vendorExtensions>\n");
    output.push_str(&svd[closing..]);
    Ok(output)
}

pub(crate) fn materialize(repository_root: &Path) -> Result<MaterializedRadioSvd, Box<dyn Error>> {
    let model_path = repository_root.join(MODEL_FILE);
    let model = RegisterModel::load(&model_path)?;
    let review_sources = model
        .review()
        .iter()
        .flat_map(|annotation| annotation.sources.iter().cloned())
        .collect();
    let (contents, summary) = model.render_svd()?;
    if summary.peripherals == 0 || summary.registers == 0 {
        return Err(format!(
            "register model {} materialized an empty radio catalog",
            model_path.display()
        )
        .into());
    }
    let addon_path = repository_root.join(PAC_ADDON_FILE);
    let addon_contents = fs::read_to_string(&addon_path)?;
    Ok(MaterializedRadioSvd {
        model_path,
        aggregate_path: repository_root.join(AGGREGATE_FILE),
        contents,
        addon_path,
        addon_contents,
        review_sources,
    })
}

pub(crate) fn synchronize_aggregate(
    materialized: &MaterializedRadioSvd,
    check: bool,
) -> Result<(), Box<dyn Error>> {
    let checked_in = fs::read_to_string(&materialized.aggregate_path)?;
    if checked_in == materialized.contents {
        return Ok(());
    }
    if check {
        return Err(format!(
            "{} differs from register model {}; run `cargo pac-gen`",
            materialized.aggregate_path.display(),
            materialized.model_path.display()
        )
        .into());
    }
    fs::write(&materialized.aggregate_path, &materialized.contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{attach_pac_addon, materialize};
    use std::path::Path;

    #[test]
    fn target_addon_is_attached_only_to_the_in_memory_document() {
        let svd = "<device><peripherals/></device>";
        let addon = "<openEspRadioPacAddon schema=\"1\"><openEspRadioFixedRegisterWrites/></openEspRadioPacAddon>";
        let composite = attach_pac_addon(svd, addon).unwrap();
        assert!(!svd.contains("vendorExtensions"));
        assert!(composite.contains("<vendorExtensions>"));
        assert!(composite.contains("<openEspRadioFixedRegisterWrites/>"));
        assert!(attach_pac_addon(&composite, addon).is_err());
    }

    #[test]
    fn checked_project_model_materializes_a_clean_radio_svd() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let materialized = materialize(root).unwrap();
        assert!(!materialized.contents.contains("openEspRadio"));
        assert!(!materialized.contents.contains("SOURCE["));
        assert!(!materialized.contents.contains("CONFIDENCE["));
    }
}
