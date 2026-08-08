//! Compilation and independent re-extraction of generated reference models.
//!
//! The first harness version is deliberately limited to exact MMIO-only leaf
//! functions. Unsupported memory, timing and platform boundaries remain
//! trapping implementations, so extending a vendor function cannot silently
//! turn an incomplete generated model into verification evidence.

use std::{
    fmt::Write as _,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

use crate::{
    ArtifactSymbolSelector, FunctionAnalysis, MmioMap, ObservableEvent, ResolvedReferenceProgram,
    Result, artifact_sha256, codegen, extract, extract_reference, harnesses, returns_equal,
    traces_equal,
};

const HARNESS_VERSION: &str = "exact-mmio-leaf-v1";
static TEMP_DIRECTORY_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub(crate) struct GeneratedReferenceProof {
    pub(crate) trace: FunctionAnalysis,
    target: String,
    generated_source_sha256: String,
    harness_source_sha256: String,
    compiler: String,
}

impl GeneratedReferenceProof {
    pub(crate) fn canonical(&self) -> String {
        let mut output = format!(
            "generated-reference {HARNESS_VERSION}\ntarget {}\ngenerated-source-sha256 {}\nharness-source-sha256 {}\ncompiler {}\n",
            self.target, self.generated_source_sha256, self.harness_source_sha256, self.compiler
        );
        for event in &self.trace.events {
            output.push_str("effect ");
            output.push_str(&event.canonical());
            output.push('\n');
        }
        output.push_str("return ");
        output.push_str(&self.trace.return_value.canonical());
        output.push('\n');
        output
    }
}

struct TemporaryBuildDirectory(PathBuf);

impl TemporaryBuildDirectory {
    fn create() -> Result<Self> {
        for _ in 0..100 {
            let sequence = TEMP_DIRECTORY_COUNTER.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "open-esp-radio-generated-reference-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(crate::Error::invalid(
            "could not allocate a unique generated-reference build directory",
        ))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TemporaryBuildDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn sha256(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn canonical_generated_source(source: &str) -> String {
    source
        .lines()
        .filter(|line| {
            !line.starts_with("// Source artifact:") && !line.starts_with("// Companion artifact:")
        })
        .fold(String::new(), |mut output, line| {
            output.push_str(line);
            output.push('\n');
            output
        })
}

fn sanitize_identifier(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn harness_source(reference_function: &str, probe_symbol: &str) -> String {
    let mut output = String::new();
    writeln!(output, "#![no_std]").unwrap();
    writeln!(output, "#[path = \"reference.rs\"]").unwrap();
    writeln!(output, "mod generated;").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[cold]").unwrap();
    writeln!(output, "#[inline(never)]").unwrap();
    writeln!(output, "fn unsupported(boundary: &str) -> ! {{").unwrap();
    writeln!(
        output,
        "    panic!(\"generated MMIO leaf used unsupported {{boundary}} boundary\")"
    )
    .unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "struct Io;").unwrap();
    writeln!(output, "impl generated::ReferenceIo for Io {{").unwrap();
    writeln!(output, "    #[inline(always)]").unwrap();
    writeln!(
        output,
        "    fn read(&mut self, width: u8, address: u32) -> u32 {{"
    )
    .unwrap();
    writeln!(output, "        match width {{").unwrap();
    writeln!(
        output,
        "            8 => unsafe {{ core::ptr::read_volatile(address as *const u8) as u32 }},"
    )
    .unwrap();
    writeln!(
        output,
        "            16 => unsafe {{ core::ptr::read_volatile(address as *const u16) as u32 }},"
    )
    .unwrap();
    writeln!(
        output,
        "            32 => unsafe {{ core::ptr::read_volatile(address as *const u32) }},"
    )
    .unwrap();
    writeln!(output, "            _ => unsupported(\"MMIO width\"),").unwrap();
    writeln!(output, "        }}").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(output, "    #[inline(always)]").unwrap();
    writeln!(
        output,
        "    fn write(&mut self, width: u8, address: u32, value: u32) {{"
    )
    .unwrap();
    writeln!(output, "        match width {{").unwrap();
    writeln!(
        output,
        "            8 => unsafe {{ core::ptr::write_volatile(address as *mut u8, value as u8) }},"
    )
    .unwrap();
    writeln!(output, "            16 => unsafe {{ core::ptr::write_volatile(address as *mut u16, value as u16) }},").unwrap();
    writeln!(
        output,
        "            32 => unsafe {{ core::ptr::write_volatile(address as *mut u32, value) }},"
    )
    .unwrap();
    writeln!(output, "            _ => unsupported(\"MMIO width\"),").unwrap();
    writeln!(output, "        }}").unwrap();
    writeln!(output, "    }}").unwrap();
    writeln!(
        output,
        "    fn delay_micros(&mut self, _micros: u32) {{ unsupported(\"delay\") }}"
    )
    .unwrap();
    writeln!(output, "    fn fence(&mut self, _fm: u8, _predecessor: u8, _successor: u8) {{ unsupported(\"fence\") }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "struct Memory;").unwrap();
    writeln!(output, "impl generated::ReferenceMemory for Memory {{").unwrap();
    writeln!(output, "    fn symbol_address(&mut self, _member: Option<&str>, _symbol: &str) -> u32 {{ unsupported(\"symbol-address\") }}").unwrap();
    writeln!(output, "    fn read(&mut self, _width: u8, _address: u32) -> u32 {{ unsupported(\"memory-read\") }}").unwrap();
    writeln!(output, "    fn write(&mut self, _width: u8, _address: u32, _value: u32) {{ unsupported(\"memory-write\") }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "struct Platform;").unwrap();
    writeln!(output, "impl generated::ReferencePlatform for Platform {{").unwrap();
    writeln!(output, "    fn external_table_version(&mut self, _table: &str) -> u32 {{ unsupported(\"external-table-version\") }}").unwrap();
    writeln!(output, "    fn external_table_magic(&mut self, _table: &str) -> u32 {{ unsupported(\"external-table-magic\") }}").unwrap();
    writeln!(output, "    fn external_table_size(&mut self, _table: &str) -> u32 {{ unsupported(\"external-table-size\") }}").unwrap();
    writeln!(output, "    fn external_call(&mut self, _table: &str, _function: &str, _arguments: &[u32]) -> u32 {{ unsupported(\"external-call\") }}").unwrap();
    writeln!(output, "    fn diagnostic_call(&mut self, _function: &str, _arguments: &[u32]) {{ unsupported(\"diagnostic-call\") }}").unwrap();
    writeln!(output, "}}").unwrap();
    writeln!(output).unwrap();
    writeln!(output, "#[unsafe(no_mangle)]").unwrap();
    writeln!(output, "pub extern \"C\" fn {probe_symbol}(").unwrap();
    writeln!(output, "    a0: u32, a1: u32, a2: u32, a3: u32,").unwrap();
    writeln!(output, "    a4: u32, a5: u32, a6: u32, a7: u32,").unwrap();
    writeln!(output, ") -> u32 {{").unwrap();
    writeln!(output, "    let mut io = Io;").unwrap();
    writeln!(output, "    let mut memory = Memory;").unwrap();
    writeln!(output, "    let mut platform = Platform;").unwrap();
    writeln!(output, "    generated::{reference_function}(").unwrap();
    writeln!(output, "        &mut io,").unwrap();
    writeln!(output, "        &mut memory,").unwrap();
    writeln!(output, "        &mut platform,").unwrap();
    writeln!(output, "        generated::Rv32ReferenceArguments {{").unwrap();
    writeln!(
        output,
        "            registers: [a0, a1, a2, a3, a4, a5, a6, a7],"
    )
    .unwrap();
    writeln!(output, "            stack: [0; 8],").unwrap();
    writeln!(output, "        }},").unwrap();
    writeln!(output, "    )").unwrap();
    writeln!(output, "    .exit_a0").unwrap();
    writeln!(output, "    .unwrap_or_default()").unwrap();
    writeln!(output, "}}").unwrap();
    output
}

fn compiler_identity() -> Result<(std::ffi::OsString, String)> {
    let executable = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let output = Command::new(&executable).arg("-vV").output()?;
    if !output.status.success() {
        return Err(crate::Error::invalid(format!(
            "failed to query generated-reference compiler: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let identity = String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| line.starts_with("release:") || line.starts_with("commit-hash:"))
        .collect::<Vec<_>>()
        .join(";");
    if identity.is_empty() {
        return Err(crate::Error::invalid(
            "generated-reference compiler did not report a release and commit hash",
        ));
    }
    Ok((executable, identity))
}

fn prove_exact_mmio_leaf(
    svd: &MmioMap,
    target: &str,
    vendor_trace: &FunctionAnalysis,
    generated_source: &str,
    vendor_symbol: &str,
) -> Result<GeneratedReferenceProof> {
    if vendor_trace
        .events
        .iter()
        .any(|event| !matches!(event, ObservableEvent::Memory { .. }))
    {
        return Err(crate::Error::invalid(format!(
            "generated harness {HARNESS_VERSION} only supports MMIO events for {vendor_symbol}"
        )));
    }
    if !vendor_trace.is_exact() {
        return Err(crate::Error::invalid(format!(
            "generated harness {HARNESS_VERSION} requires an exact vendor trace for {vendor_symbol}"
        )));
    }

    let build = TemporaryBuildDirectory::create()?;
    let reference_path = build.path().join("reference.rs");
    let harness_path = build.path().join("harness.rs");
    let artifact_path = build.path().join("libgenerated_reference.rlib");
    let probe_symbol = format!(
        "open_phy_generated_reference_{}",
        sanitize_identifier(vendor_symbol)
    );
    let harness = harness_source(
        &codegen::reference_function_name(vendor_symbol),
        &probe_symbol,
    );
    fs::write(&reference_path, generated_source)?;
    fs::write(&harness_path, &harness)?;

    let (compiler, compiler_identity) = compiler_identity()?;
    let output = Command::new(compiler)
        .current_dir(build.path())
        .args([
            "--edition=2024",
            "--crate-name=open_esp_radio_generated_reference",
            "--crate-type=rlib",
            "--target",
            target,
            "-Copt-level=3",
            "-Cpanic=abort",
            "-Ccodegen-units=1",
            "-Cembed-bitcode=no",
            "-Dwarnings",
            "-o",
        ])
        .arg(&artifact_path)
        .arg(&harness_path)
        .output()?;
    if !output.status.success() {
        return Err(crate::Error::invalid(format!(
            "generated reference for {vendor_symbol} did not compile for {target}:\n{}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let generated_trace = extract(
        &ArtifactSymbolSelector {
            artifact: artifact_path,
            member: None,
            symbol: probe_symbol,
        },
        svd,
    )?;
    if !generated_trace.is_exact() {
        return Err(crate::Error::invalid(format!(
            "compiled generated reference for {vendor_symbol} is incomplete: {}",
            generated_trace.blockers.join("; ")
        )));
    }
    if !traces_equal(vendor_trace, &generated_trace) {
        return Err(crate::Error::invalid(format!(
            "compiled generated reference for {vendor_symbol} does not reproduce the vendor MMIO trace"
        )));
    }
    if vendor_trace.return_value.is_resolved()
        && generated_trace.return_value.is_resolved()
        && !returns_equal(vendor_trace, &generated_trace)
    {
        return Err(crate::Error::invalid(format!(
            "compiled generated reference for {vendor_symbol} does not reproduce the vendor return value"
        )));
    }

    Ok(GeneratedReferenceProof {
        trace: generated_trace,
        target: target.to_owned(),
        generated_source_sha256: sha256(&canonical_generated_source(generated_source)),
        harness_source_sha256: sha256(&harness),
        compiler: compiler_identity,
    })
}

pub(crate) fn generate_compile_and_prove_exact_mmio_leaf(
    svd: &MmioMap,
    harness: &str,
    target: &str,
    vendor_input: &ArtifactSymbolSelector,
    companions: &[PathBuf],
    vendor_trace: &FunctionAnalysis,
) -> Result<GeneratedReferenceProof> {
    let riscv_harness = harnesses::riscv(harness)?;
    let entry_contract = harnesses::entry_contract(harness, "none")?;
    let reference_trace =
        extract_reference(vendor_input, companions, riscv_harness, entry_contract, svd)?;
    let resolved = ResolvedReferenceProgram::try_from(&reference_trace)
        .map_err(|error| -> crate::Error { crate::Error::invalid(error) })?;
    let artifact_digest = artifact_sha256(&vendor_input.artifact)?;
    let companion_provenance = companions
        .iter()
        .map(|companion| Ok((companion.display().to_string(), artifact_sha256(companion)?)))
        .collect::<Result<Vec<_>>>()?;
    let generated = codegen::generate(
        &resolved,
        &vendor_input.artifact.display().to_string(),
        &artifact_digest,
        vendor_input.member.as_deref(),
        &companion_provenance,
    )
    .map_err(|error| -> crate::Error { crate::Error::invalid(error) })?;
    prove_exact_mmio_leaf(
        svd,
        target,
        vendor_trace,
        &generated.source,
        &vendor_input.symbol,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_keeps_all_rv32_register_arguments_and_traps_other_boundaries() {
        let source = harness_source("open_phy_reference_leaf", "open_phy_generated_leaf");
        assert!(source.contains("registers: [a0, a1, a2, a3, a4, a5, a6, a7]"));
        assert!(source.contains("unsupported(\"memory-read\")"));
        assert!(source.contains("unsupported(\"delay\")"));
        assert!(source.contains("generated::open_phy_reference_leaf("));
    }

    #[test]
    fn generated_probe_symbols_are_valid_identifiers() {
        assert_eq!(sanitize_identifier("phy/a-b"), "phy_a_b");
    }

    #[test]
    fn generated_source_identity_ignores_only_local_artifact_paths() {
        let left = "// Source artifact: relative/oracle.elf\n// Source SHA-256: abcd\n// Companion artifact: relative/linked.elf\n// Companion SHA-256: ef01\nfn model() {}\n";
        let right = "// Source artifact: /private/oracle.elf\n// Source SHA-256: abcd\n// Companion artifact: /private/linked.elf\n// Companion SHA-256: ef01\nfn model() {}\n";
        assert_eq!(
            canonical_generated_source(left),
            canonical_generated_source(right)
        );
        assert_ne!(
            canonical_generated_source(left),
            canonical_generated_source(&right.replace("ef01", "ffff"))
        );
    }
}
