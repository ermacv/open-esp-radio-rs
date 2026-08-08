//! Deterministic text templates emitted by project initialization.

use super::options::Options;

pub(super) fn render_manifest(options: &Options) -> String {
    let mut output = format!(
        "# Shareable project configuration. Keep local artifact paths in local.run.\n\
schema = 1\n\
id = \"{}\"\n\
target-spec = \"target.spec\"\n\
platform-pack = \"platform.toml\"\n\
memory-map = \"memory.toml\"\n\
svd = []\n",
        options.id
    );
    for source in &options.sources {
        output.push_str(&format!(
            "\n[[analysis.ir]]\n\
id = \"{source}\"\n\
sources = [\"{source}\"]\n\
include-reachable = true\n\
entry-contract = \"none\"\n\
output = \"generated/findings/{source}.ir.json\"\n\
pseudo-rust = \"generated/reports/{source}.pseudo.rs\"\n"
        ));
    }
    let linked_ir = quoted_list(
        options
            .sources
            .iter()
            .map(|source| format!("generated/findings/{source}.ir.json")),
    );
    let profiles = quoted_list(options.sources.iter().cloned());
    output.push_str(&format!(
        "\n[registers]\n\
facts = \"generated/findings/mmio.json\"\n\
model = \"registers/device.toml\"\n\
\n[registers.review]\n\
output = \"generated/reports/register-review.md\"\n\
linked-ir = [{linked_ir}]\n\
\n[registers.svd]\n\
output = \"generated/svd/device.svd\"\n\
\n[registers.pac]\n\
output = \"generated/pac/src/lib.rs\"\n\
target = \"none\"\n\
edition = \"2024\"\n\
\n[registers.bindings]\n\
output = \"generated/svd/device.bindings\"\n\
crate-name = \"{}\"\n\
\n[interfaces]\n\
facts = \"generated/findings/interfaces.json\"\n\
pack = \"interfaces/reviewed.toml\"\n\
\n[functions]\n\
pack = \"functions/reviewed.toml\"\n\
profiles = [{profiles}]\n\
\n[functions.review]\n\
output = \"generated/reports/function-review.md\"\n",
        options.pac_crate_name
    ));
    output
}

pub(super) fn render_platform(options: &Options) -> String {
    format!(
        "# Reviewed platform composition. Add a harness or semantic catalogs explicitly.\n\
schema = 1\n\
id = \"{}-platform\"\n\
architecture = \"riscv32\"\n\
calling-convention = \"riscv-ilp32\"\n\
semantic-catalogs = []\n",
        options.id
    )
}

pub(super) fn render_target(options: &Options) -> String {
    format!(
        "# Generic RV32 target; select reviewed platform semantics in platform.toml.\n\
schema 1\n\
target {}\n\
architecture riscv32\n\
calling-convention riscv-ilp32\n\
endianness little\n\
pointer-width 32\n\
rust-target {}\n\
memory-map memory.toml\n",
        options.id, options.rust_target
    )
}

pub(super) fn render_memory(options: &Options) -> String {
    let mut output = "schema = 1\ndefault-address-space = \"cpu\"\n\n[[address-spaces]]\nid = \"cpu\"\naddress-width = 32\nendianness = \"little\"\n".to_owned();
    for range in &options.ranges {
        output.push_str(&format!(
            "\n[[regions]]\nname = \"{}\"\naddress-space = \"cpu\"\nkind = \"mmio\"\nstart = {:#010x}\nend-exclusive = {:#010x}\npermissions = \"rw\"\n",
            range.name, range.start, range.end
        ));
    }
    output
}

pub(super) fn render_run_spec(options: &Options) -> String {
    let mut output =
        "# Copy to local.run, replace every path, and keep that file untracked.\nschema 1\n"
            .to_owned();
    for source in &options.sources {
        output.push_str(&format!(
            "input source-artifact:{source} /path/to/{source}.elf\n"
        ));
    }
    output.push_str(
        "# For an archive plus linked ELF, add source-inventory:ID and source-companion:ID.\n",
    );
    output
}

pub(super) fn render_readme(options: &Options) -> String {
    format!(
        "# {} vendor analysis project\n\n\
This directory is a generic Vendor Binary Workbench project. Hardware addresses live in\n\
`memory.toml`; reviewed register names and fields live under `registers/`.\n\
Generated findings and reports are ignored.\n\n\
## Bootstrap\n\n\
```console\n\
cp run.spec.example local.run\n\
# Edit local.run, then:\n\
cargo vendor-binary-workbench project doctor --project vendor-project.toml --run-spec local.run\n\
cargo vendor-binary-workbench project configure --project vendor-project.toml --check\n\
cargo vendor-binary-workbench project status --project vendor-project.toml --run-spec local.run\n\
cargo vendor-binary-workbench mmio discover --project vendor-project.toml --run-spec local.run\n\
cargo vendor-binary-workbench interfaces discover --project vendor-project.toml --run-spec local.run\n\
cargo vendor-binary-workbench ir build --project vendor-project.toml --run-spec local.run\n\
cargo vendor-binary-workbench registers review --project vendor-project.toml\n\
cargo vendor-binary-workbench interfaces init-pack --project vendor-project.toml\n\
cargo vendor-binary-workbench functions init-pack --project vendor-project.toml\n\
```\n\n\
Review `registers/peripherals/*.toml`, `interfaces/reviewed.toml` and\n\
`functions/reviewed.toml`. Then use `project analyze` to refresh evidence,\n\
`project analyze --check` in analysis CI, and `project publish --check` for SVD/PAC.\n",
        options.id
    )
}

fn quoted_list(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ")
}
