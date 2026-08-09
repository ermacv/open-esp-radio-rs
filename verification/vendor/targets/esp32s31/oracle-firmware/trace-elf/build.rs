use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixtureDataSymbol {
    name: String,
    size: usize,
    alignment: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
struct LinkedOracleSpec {
    schema: u32,
    source: String,
    #[serde(default)]
    linker_scripts: Vec<PathBuf>,
    #[serde(default)]
    archives: Vec<PathBuf>,
    #[serde(default)]
    objects: Vec<PathBuf>,
    #[serde(default)]
    stub_symbols: Vec<String>,
    #[serde(default)]
    fixture_data_symbols: Vec<FixtureDataSymbol>,
    #[serde(default)]
    whole_archive: Option<bool>,
    #[serde(default)]
    emit_relocations: Option<bool>,
    #[serde(default)]
    gc_sections: Option<bool>,
    #[serde(default)]
    unresolved_symbols: Option<String>,
}

impl LinkedOracleSpec {
    fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut spec: Self = toml_edit::de::from_str(&input)?;
        if spec.schema != 1 {
            return Err("linked-oracle TOML requires schema = 1".into());
        }
        if spec.source.is_empty()
            || !spec.source.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_')
            })
        {
            return Err(format!("invalid source id {:?}", spec.source).into());
        }
        if spec.linker_scripts.is_empty() {
            return Err("linked-oracle TOML has no linker script".into());
        }
        if spec.archives.is_empty() && spec.objects.is_empty() {
            return Err("linked-oracle TOML has no archive or object input".into());
        }
        if spec
            .unresolved_symbols
            .as_deref()
            .is_some_and(|value| !matches!(value, "ignore-all" | "report-all"))
        {
            return Err(format!(
                "invalid unresolved-symbols policy {:?}",
                spec.unresolved_symbols.as_deref().unwrap()
            )
            .into());
        }
        spec.linker_scripts = spec
            .linker_scripts
            .into_iter()
            .map(|path| resolve(base, path))
            .collect();
        spec.archives = spec
            .archives
            .into_iter()
            .map(|path| resolve(base, path))
            .collect();
        spec.objects = spec
            .objects
            .into_iter()
            .map(|path| resolve(base, path))
            .collect();
        let mut stub_symbols = BTreeSet::new();
        for symbol in &spec.stub_symbols {
            validate_symbol(symbol, "stub")?;
            if !stub_symbols.insert(symbol) {
                return Err(format!("duplicate stub symbol {symbol:?}").into());
            }
        }
        let mut fixture_symbols = BTreeSet::new();
        for symbol in &spec.fixture_data_symbols {
            validate_symbol(&symbol.name, "fixture data")?;
            if !fixture_symbols.insert(&symbol.name) {
                return Err(format!("duplicate fixture data symbol {:?}", symbol.name).into());
            }
            if symbol.size == 0 {
                return Err("fixture-data-symbol size must be non-zero".into());
            }
            if symbol.alignment == 0
                || symbol.alignment > 4096
                || !symbol.alignment.is_power_of_two()
            {
                return Err(
                    "fixture-data-symbol alignment must be a power of two up to 4096".into(),
                );
            }
        }
        Ok(spec)
    }

    fn emit(&self, spec_path: &Path) -> Result<()> {
        println!("cargo:rerun-if-changed={}", spec_path.display());
        println!("cargo:rerun-if-env-changed=OPEN_RADIO_LINKED_ORACLE_SPEC");
        println!(
            "cargo:rustc-env=OPEN_RADIO_LINKED_ORACLE_SOURCE={}",
            self.source
        );
        for script in &self.linker_scripts {
            println!("cargo:rerun-if-changed={}", script.display());
            println!("cargo:rustc-link-arg=-T{}", script.display());
        }
        if self.gc_sections == Some(false) {
            println!("cargo:rustc-link-arg=--no-gc-sections");
        }
        if self.emit_relocations == Some(true) {
            println!("cargo:rustc-link-arg=--emit-relocs");
        }
        if self.whole_archive == Some(true) {
            println!("cargo:rustc-link-arg=--whole-archive");
        }
        for input in self.archives.iter().chain(&self.objects) {
            println!("cargo:rerun-if-changed={}", input.display());
            println!("cargo:rustc-link-arg={}", input.display());
        }
        if self.whole_archive == Some(true) {
            println!("cargo:rustc-link-arg=--no-whole-archive");
        }
        if self.unresolved_symbols.as_deref() == Some("ignore-all") {
            println!("cargo:rustc-link-arg=--unresolved-symbols=ignore-all");
        }
        let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo provides OUT_DIR"));
        let mut stubs = String::new();
        for (index, symbol) in self.stub_symbols.iter().enumerate() {
            stubs.push_str(&format!(
                "#[unsafe(export_name = {symbol:?})]\n#[inline(never)]\npub extern \"C\" fn linked_oracle_stub_{index}() -> u32 {{\n    // A stub exists only to make the aggregate ELF linkable. Reaching it is\n    // deliberately non-executable so the verifier cannot learn invented\n    // return behavior from this fixture.\n    unsafe {{ core::arch::asm!(\"ebreak\", options(noreturn, nomem, nostack)) }}\n}}\n"
            ));
        }
        for (index, symbol) in self.fixture_data_symbols.iter().enumerate() {
            let name = &symbol.name;
            let size = symbol.size;
            let alignment = symbol.alignment;
            stubs.push_str(&format!(
                "#[repr(C, align({alignment}))]\npub struct LinkedOracleFixtureData{index}([u8; {size}]);\n#[unsafe(export_name = {name:?})]\n#[used]\npub static mut LINKED_ORACLE_FIXTURE_DATA_{index}: LinkedOracleFixtureData{index} = LinkedOracleFixtureData{index}([0; {size}]);\n"
            ));
        }
        fs::write(out_dir.join("linked_oracle_stubs.rs"), stubs)?;
        Ok(())
    }
}

fn validate_symbol(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$'))
    {
        return Err(format!("invalid {kind} symbol {value:?}").into());
    }
    Ok(())
}

fn resolve(base: &Path, path: PathBuf) -> PathBuf {
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn main() -> Result<()> {
    let manifest = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo provides CARGO_MANIFEST_DIR"),
    );
    let spec_path = env::var_os("OPEN_RADIO_LINKED_ORACLE_SPEC")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest.join("linked-oracle.toml"));
    let spec_path = fs::canonicalize(&spec_path).map_err(|error| {
        format!(
            "failed to resolve linked-oracle spec {}: {error}",
            spec_path.display()
        )
    })?;
    LinkedOracleSpec::load(&spec_path)?.emit(&spec_path)?;
    Ok(())
}
