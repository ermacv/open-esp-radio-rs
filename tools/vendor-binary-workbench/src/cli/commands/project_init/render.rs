//! Deterministic text templates emitted by project initialization.

use super::options::Options;

pub(super) fn render_manifest(options: &Options) -> String {
    let mut output = format!(
        "# Shareable project configuration. Keep local artifact paths in local.toml.\n\
schema = 1\n\
id = \"{}\"\n\
target-spec = \"target.toml\"\n\
platform-pack = \"platform.toml\"\n\
memory-map = \"memory.toml\"\n\
svd = []\n",
        options.id
    );
    output.push_str(
        "\n[analysis.symbols]\n\
output = \"generated/findings/symbols.json\"\n\
\n\
[analysis.navigation]\n\
output = \"generated/findings/navigation.json\"\n\
\n\
[code]\n\
pack = \"code/boundaries.toml\"\n\
\n\
[code.review]\n\
output = \"generated/reports/code-boundaries.md\"\n",
    );
    for source in &options.sources {
        output.push_str(&format!(
            "\n[[analysis.ir]]\n\
id = \"{source}\"\n\
sources = [\"{source}\"]\n\
roots = \"all\"\n\
include-reachable = true\n\
entry-contract = \"none\"\n\
output = \"generated/findings/{source}.ir\"\n\
"
        ));
    }
    let linked_ir = quoted_list(
        options
            .sources
            .iter()
            .map(|source| format!("generated/findings/{source}.ir")),
    );
    let profiles = quoted_list(options.sources.iter().cloned());
    output.push_str(&format!(
        "\n[registers]\n\
facts = \"generated/findings/mmio.json\"\n\
model = \"registers/device.toml\"\n\
owned-ranges = [{owned_ranges}]\n\
\n[registers.review]\n\
output = \"generated/reports/register-review.md\"\n\
linked-ir = [{linked_ir}]\n\
\n[registers.svd]\n\
output = \"generated/svd/device.svd\"\n\
\n[registers.pac-raw]\n\
output = \"generated/pac-raw/src/lib.rs\"\n\
target = \"none\"\n\
edition = \"2024\"\n\
\n[registers.bindings]\n\
output = \"generated/svd/device.bindings.toml\"\n\
crate-name = \"{}\"\n\
\n[registers.api]\n\
pack = \"registers/api.toml\"\n\
output = \"generated/pac/src/generated.rs\"\n\
\n[interfaces]\n\
facts = \"generated/findings/interfaces.json\"\n\
pack = \"interfaces/reviewed.toml\"\n\
\n[functions]\n\
pack = \"functions/reviewed.toml\"\n\
profiles = [{profiles}]\n\
\n[functions.review]\n\
output = \"generated/reports/function-review.md\"\n",
        options.pac_raw_crate_name,
        owned_ranges = quoted_list(options.ranges.iter().map(|range| range.name.clone()))
    ));
    output
}

pub(super) fn render_register_api() -> String {
    "# Reviewed public domains and transactions bridged to the internal raw PAC.\n\
# Keep this pack empty until a vendor access has exact evidence and policy.\n\
schema = 2\n\
\n[options]\n\
peripheral-ownership = false\n\
device-access = false\n\
allow-clippy-empty-docs = false\n"
        .to_owned()
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
schema = 1\n\
id = \"{}\"\n\
architecture = \"riscv32\"\n\
calling-convention = \"riscv-ilp32\"\n\
endianness = \"little\"\n\
pointer-width = 32\n\
rust-target = \"{}\"\n\
memory-map = \"memory.toml\"\n",
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
        "# Reference template. Prefer `project inputs init --bind ROLE=PATH`; keep local.toml untracked.\nschema = 1\n"
            .to_owned();
    for source in &options.sources {
        output.push_str(&format!(
            "\n[[inputs]]\nrole = \"source-artifact:{source}\"\npath = \"/path/to/{source}.elf\"\n"
        ));
    }
    output.push_str(
        "# For an archive plus linked ELF, add source-inventory:ID and source-companion:ID.\n",
    );
    output
}

pub(super) fn render_readme(options: &Options) -> String {
    let bindings = options
        .sources
        .iter()
        .map(|source| format!("  --bind source-artifact:{source}=/path/to/{source}.elf"))
        .collect::<Vec<_>>()
        .join(" \\\n");
    format!(
        "# {} vendor analysis project\n\n\
This directory is a generic Vendor Binary Workbench project. Hardware addresses live in\n\
`memory.toml`; reviewed register names and fields live under `registers/`.\n\
Generated findings and reports are ignored.\n\n\
## Bootstrap\n\n\
```console\n\
cargo vendor-binary-workbench project inputs init --project vendor-project.toml \\\n{bindings}\n\
cargo vendor-binary-workbench project doctor --project vendor-project.toml\n\
cargo vendor-binary-workbench project configure --project vendor-project.toml --check\n\
cargo vendor-binary-workbench project status --project vendor-project.toml\n\
cargo vendor-binary-workbench advanced symbols inventory --project vendor-project.toml\n\
cargo vendor-binary-workbench advanced code init-pack --project vendor-project.toml\n\
cargo vendor-binary-workbench advanced interfaces discover --project vendor-project.toml\n\
cargo vendor-binary-workbench advanced interfaces init-pack --project vendor-project.toml\n\
cargo vendor-binary-workbench advanced ir build --project vendor-project.toml\n\
cargo vendor-binary-workbench advanced functions init-pack --project vendor-project.toml\n\
cargo vendor-binary-workbench project analyze --project vendor-project.toml\n\
cargo vendor-binary-workbench registers review --project vendor-project.toml\n\
```\n\n\
Review `code/boundaries.toml`, `registers/peripherals/*.toml`,\n\
`interfaces/reviewed.toml` and `functions/reviewed.toml`. Then use `project analyze` to refresh evidence,\n\
`project analyze --check` in analysis CI, and `project publish --check` for SVD/raw-PAC.\n",
        options.id,
        bindings = bindings
    )
}

fn quoted_list(values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .map(|value| format!("\"{value}\""))
        .collect::<Vec<_>>()
        .join(", ")
}
