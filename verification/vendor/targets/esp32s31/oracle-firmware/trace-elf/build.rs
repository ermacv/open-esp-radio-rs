use std::{
    env,
    error::Error,
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

struct FixtureDataSymbol {
    name: String,
    size: usize,
    alignment: usize,
}

#[derive(Default)]
struct LinkedOracleSpec {
    schema: Option<u32>,
    source: Option<String>,
    linker_scripts: Vec<PathBuf>,
    archives: Vec<PathBuf>,
    objects: Vec<PathBuf>,
    stub_symbols: Vec<String>,
    fixture_data_symbols: Vec<FixtureDataSymbol>,
    whole_archive: Option<bool>,
    emit_relocations: Option<bool>,
    gc_sections: Option<bool>,
    unresolved_symbols: Option<String>,
}

impl LinkedOracleSpec {
    fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut spec = Self::default();
        for (index, raw_line) in input.lines().enumerate() {
            let line_number = index + 1;
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let (directive, value) = line
                .split_once(char::is_whitespace)
                .map(|(directive, value)| (directive, value.trim()))
                .filter(|(_, value)| !value.is_empty())
                .ok_or_else(|| {
                    format!("linked-oracle directive needs a value at line {line_number}")
                })?;
            match directive {
                "schema" => set_once(&mut spec.schema, value.parse()?, directive, line_number)?,
                "source" => {
                    if value.is_empty()
                        || !value.bytes().all(|byte| {
                            byte.is_ascii_lowercase()
                                || byte.is_ascii_digit()
                                || matches!(byte, b'-' | b'_')
                        })
                    {
                        return Err(
                            format!("invalid source id {value:?} at line {line_number}").into()
                        );
                    }
                    set_once(&mut spec.source, value.to_owned(), directive, line_number)?;
                }
                "linker-script" => spec.linker_scripts.push(resolve(base, value)),
                "archive" => spec.archives.push(resolve(base, value)),
                "object" => spec.objects.push(resolve(base, value)),
                "stub-symbol" => {
                    validate_symbol(value, "stub", line_number)?;
                    if spec.stub_symbols.iter().any(|symbol| symbol == value) {
                        return Err(format!(
                            "duplicate stub symbol {value:?} at line {line_number}"
                        )
                        .into());
                    }
                    spec.stub_symbols.push(value.to_owned());
                }
                "fixture-data-symbol" => {
                    let mut fields = value.split_whitespace();
                    let name = fields.next().ok_or_else(|| {
                        format!("fixture-data-symbol has no name at line {line_number}")
                    })?;
                    validate_symbol(name, "fixture data", line_number)?;
                    let size = fields
                        .next()
                        .ok_or_else(|| {
                            format!("fixture-data-symbol has no size at line {line_number}")
                        })?
                        .parse::<usize>()?;
                    let alignment = fields
                        .next()
                        .ok_or_else(|| {
                            format!("fixture-data-symbol has no alignment at line {line_number}")
                        })?
                        .parse::<usize>()?;
                    if fields.next().is_some() {
                        return Err(format!(
                            "fixture-data-symbol has extra fields at line {line_number}"
                        )
                        .into());
                    }
                    if size == 0 {
                        return Err(format!(
                            "fixture-data-symbol size must be non-zero at line {line_number}"
                        )
                        .into());
                    }
                    if alignment == 0 || alignment > 4096 || !alignment.is_power_of_two() {
                        return Err(format!(
                            "fixture-data-symbol alignment must be a power of two up to 4096 at line {line_number}"
                        )
                        .into());
                    }
                    if spec
                        .fixture_data_symbols
                        .iter()
                        .any(|symbol| symbol.name == name)
                    {
                        return Err(format!(
                            "duplicate fixture data symbol {name:?} at line {line_number}"
                        )
                        .into());
                    }
                    spec.fixture_data_symbols.push(FixtureDataSymbol {
                        name: name.to_owned(),
                        size,
                        alignment,
                    });
                }
                "whole-archive" => set_once(
                    &mut spec.whole_archive,
                    parse_bool(value, directive, line_number)?,
                    directive,
                    line_number,
                )?,
                "emit-relocations" => set_once(
                    &mut spec.emit_relocations,
                    parse_bool(value, directive, line_number)?,
                    directive,
                    line_number,
                )?,
                "gc-sections" => set_once(
                    &mut spec.gc_sections,
                    parse_bool(value, directive, line_number)?,
                    directive,
                    line_number,
                )?,
                "unresolved-symbols" => {
                    if !matches!(value, "ignore-all" | "report-all") {
                        return Err(format!(
                            "invalid unresolved-symbols policy {value:?} at line {line_number}"
                        )
                        .into());
                    }
                    set_once(
                        &mut spec.unresolved_symbols,
                        value.to_owned(),
                        directive,
                        line_number,
                    )?;
                }
                _ => {
                    return Err(format!(
                        "unknown linked-oracle directive {directive:?} at line {line_number}"
                    )
                    .into());
                }
            }
        }
        if spec.schema != Some(1) {
            return Err("linked-oracle spec requires schema 1".into());
        }
        if spec.source.is_none() {
            return Err("linked-oracle spec has no source id".into());
        }
        if spec.linker_scripts.is_empty() {
            return Err("linked-oracle spec has no linker script".into());
        }
        if spec.archives.is_empty() && spec.objects.is_empty() {
            return Err("linked-oracle spec has no archive or object input".into());
        }
        Ok(spec)
    }

    fn emit(&self, spec_path: &Path) -> Result<()> {
        println!("cargo:rerun-if-changed={}", spec_path.display());
        println!("cargo:rerun-if-env-changed=OPEN_RADIO_LINKED_ORACLE_SPEC");
        println!(
            "cargo:rustc-env=OPEN_RADIO_LINKED_ORACLE_SOURCE={}",
            self.source.as_deref().expect("validated source")
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
                "#[unsafe(export_name = {symbol:?})]\n#[inline(never)]\npub extern \"C\" fn linked_oracle_stub_{index}() -> u32 {{\n    // A stub exists only to make the aggregate ELF linkable. Reaching it is\n    // deliberately non-executable so the validator cannot learn invented\n    // return behavior from this fixture.\n    unsafe {{ core::arch::asm!(\"ebreak\", options(noreturn, nomem, nostack)) }}\n}}\n"
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

fn validate_symbol(value: &str, kind: &str, line: usize) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$'))
    {
        return Err(format!("invalid {kind} symbol {value:?} at line {line}").into());
    }
    Ok(())
}

fn set_once<T>(slot: &mut Option<T>, value: T, directive: &str, line: usize) -> Result<()> {
    if slot.replace(value).is_some() {
        return Err(format!("duplicate {directive} at line {line}").into());
    }
    Ok(())
}

fn parse_bool(value: &str, directive: &str, line: usize) -> Result<bool> {
    value
        .parse()
        .map_err(|_| format!("invalid boolean for {directive} at line {line}").into())
}

fn resolve(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_owned()
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
        .unwrap_or_else(|| manifest.join("linked-oracle.spec"));
    let spec_path = fs::canonicalize(&spec_path).map_err(|error| {
        format!(
            "failed to resolve linked-oracle spec {}: {error}",
            spec_path.display()
        )
    })?;
    LinkedOracleSpec::load(&spec_path)?.emit(&spec_path)?;
    Ok(())
}
