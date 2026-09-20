//! Explicit target review for linked code not covered by compiler metadata.
//! A review explains a missing measurement; it never invents a zero-byte frame.
use super::*;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoveragePolicy {
    pub schema: u32,
    pub reviewed: Vec<CoverageReview>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoverageReview {
    pub symbols: Vec<String>,
    pub category: CoverageCategory,
    pub source: String,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CoverageCategory {
    Assembly,
    VectorData,
    CompilerRuntime,
}

/// Applied review for one unmeasured linked symbol, never a size estimate.
#[derive(Clone, Debug, Serialize)]
pub struct CoverageMatch {
    pub address: u64,
    pub symbol: String,
    pub category: CoverageCategory,
    pub source: String,
    pub reason: String,
}

impl CoveragePolicy {
    pub fn load(path: &Path) -> Result<Self> {
        let source = fs::read_to_string(path).map_err(|source| Error::Read {
            path: path.into(),
            source,
        })?;
        let policy: Self = toml_edit::de::from_str(&source).map_err(|source| Error::Policy {
            path: path.into(),
            source,
        })?;
        if policy.schema != 1
            || policy.reviewed.iter().any(|r| {
                r.symbols.is_empty()
                    || r.symbols.iter().any(|s| s.trim().is_empty())
                    || r.source.trim().is_empty()
                    || r.reason.trim().is_empty()
            })
        {
            return Err(Error::InvalidPolicy(
                "coverage review requires schema 1, exact symbols, source and reason".into(),
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for review in &policy.reviewed {
            for symbol in &review.symbols {
                if !names.insert(without_crate_disambiguators(symbol)) {
                    return Err(Error::InvalidPolicy(format!(
                        "overlapping stack coverage review: {symbol}"
                    )));
                }
            }
        }
        Ok(policy)
    }

    pub(super) fn audit(
        &self,
        coverage: &StackCoverage,
        audit: &mut AuditReport,
    ) -> Vec<CoverageMatch> {
        let mut applied = Vec::new();
        if coverage.linked_text_status == StackCoverageStatus::Unavailable {
            audit
                .errors
                .push("stack coverage unavailable: no linked text symbols".into());
        }
        for function in &coverage.unmeasured_functions {
            if !matches!(
                function.origin,
                StackCoverageOrigin::LinkedRustSymbol | StackCoverageOrigin::LinkedOtherText
            ) {
                continue;
            }
            for name in &function.functions {
                let stable = without_crate_disambiguators(name);
                let matches = self
                    .reviewed
                    .iter()
                    .filter(|r| {
                        r.symbols
                            .iter()
                            .any(|s| without_crate_disambiguators(s) == stable)
                    })
                    .count();
                if matches != 1 {
                    audit.errors.push(format!("unmeasured linked function {name} at {:#x}: {matches} coverage reviews (expected one)", function.address));
                } else {
                    let review = self
                        .reviewed
                        .iter()
                        .find(|r| {
                            r.symbols
                                .iter()
                                .any(|s| without_crate_disambiguators(s) == stable)
                        })
                        .unwrap();
                    applied.push(CoverageMatch {
                        address: function.address,
                        symbol: name.clone(),
                        category: review.category,
                        source: review.source.clone(),
                        reason: review.reason.clone(),
                    });
                }
            }
        }
        applied
    }
}

// v0 demangling exposes a 16-hex crate disambiguator, which changes with the
// consumer's build. Ignore only that decoration; retain the full definition,
// type arguments, closure identity and const parameters of the reviewed symbol.
fn without_crate_disambiguators(name: &str) -> String {
    let mut result = String::new();
    let mut rest = name;
    while let Some(open) = rest.find('[') {
        result.push_str(&rest[..open]);
        rest = &rest[open..];
        if rest.as_bytes().get(17) == Some(&b']')
            && rest.as_bytes()[1..17].iter().all(u8::is_ascii_hexdigit)
        {
            rest = &rest[18..];
        } else {
            result.push('[');
            rest = &rest[1..];
        }
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stable_names_preserve_monomorphization_and_non_hash_brackets() {
        assert_eq!(
            without_crate_disambiguators(
                "hal[1234567890abcdef]::entry::<app[abcdef1234567890]::main::{closure#0}>"
            ),
            "hal::entry::<app::main::{closure#0}>"
        );
        for name in [
            "hal::entry_impl::<A>",
            "hal::entry::<[u8; 16]>",
            "hal[not-a-crate-hash]::entry::<B>",
        ] {
            assert_eq!(without_crate_disambiguators(name), name);
        }
    }
    #[test]
    fn duplicate_review_and_unknown_categories_fail_closed() {
        let path =
            std::env::temp_dir().join(format!("oer-stack-review-{}.toml", std::process::id()));
        let record = "\n[[reviewed]]\nsymbols = ['entry']\ncategory = 'assembly'\nsource = 'entry.rs'\nreason = 'naked entry'\n";
        fs::write(&path, format!("schema = 1\n{record}{record}")).unwrap();
        assert!(matches!(
            CoveragePolicy::load(&path),
            Err(Error::InvalidPolicy(_))
        ));
        fs::write(
            &path,
            format!("schema = 1\n{record}").replace("'assembly'", "'any-rust'"),
        )
        .unwrap();
        assert!(CoveragePolicy::load(&path).is_err());
        fs::remove_file(path).unwrap();
    }
    #[test]
    fn reviewing_assembly_does_not_exempt_unknown_code_or_aliases() {
        let policy = CoveragePolicy {
            schema: 1,
            reviewed: vec![CoverageReview {
                symbols: vec!["irq_entry".into()],
                category: CoverageCategory::Assembly,
                source: "entry.rs".into(),
                reason: "assembly entry; separate IRQ watermark".into(),
            }],
        };
        let mut coverage = StackCoverage {
            linked_text_status: StackCoverageStatus::Incomplete,
            linked_text_addresses: 1,
            measured_linked_text_addresses: 0,
            unmeasured_functions: vec![StackCoverageFunction {
                address: 100,
                functions: vec!["irq_entry".into()],
                origin: StackCoverageOrigin::LinkedOtherText,
            }],
            metadata_without_text_symbol: vec![],
        };
        let mut audit = AuditReport::default();
        policy.audit(&coverage, &mut audit);
        assert!(audit.errors.is_empty());
        coverage.unmeasured_functions[0]
            .functions
            .push("unreviewed_alias".into());
        policy.audit(&coverage, &mut audit);
        assert_eq!(audit.errors.len(), 1);
        assert!(audit.errors[0].contains("unreviewed_alias"));
        assert_eq!(coverage.measured_linked_text_addresses, 0);
    }
}
