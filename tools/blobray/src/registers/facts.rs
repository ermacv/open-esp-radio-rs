//! Loading immutable typed JSON emitted by `mmio discover`.

use std::{collections::BTreeSet, fs, path::Path};

use crate::{Result, error::BlobrayError};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FactRange {
    pub(crate) name: String,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

impl FactRange {
    pub(crate) fn contains(&self, address: u32) -> bool {
        address >= self.start && address < self.end
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterWritePatternFact {
    pub(crate) occurrences: usize,
    pub(crate) modified_mask: u32,
    pub(crate) preserved_mask: u32,
    pub(crate) inverted_mask: u32,
    pub(crate) forced_zero_mask: u32,
    pub(crate) forced_one_mask: u32,
    pub(crate) read_derived_mask: u32,
    pub(crate) dynamic_mask: u32,
    pub(crate) functions: BTreeSet<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RegisterAccessSiteFact {
    pub(crate) function: String,
    pub(crate) pc: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterFact {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) catalog_name: String,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) read_functions: BTreeSet<String>,
    pub(crate) write_functions: BTreeSet<String>,
    pub(crate) read_sites: BTreeSet<RegisterAccessSiteFact>,
    pub(crate) write_sites: BTreeSet<RegisterAccessSiteFact>,
    pub(crate) write_patterns: Vec<RegisterWritePatternFact>,
    pub(crate) candidate_masks: Vec<u32>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RegisterFactArtifact {
    pub(crate) source: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RegisterFacts {
    pub(crate) artifacts: Vec<RegisterFactArtifact>,
    pub(crate) ranges: Vec<FactRange>,
    pub(crate) registers: Vec<RegisterFact>,
}

impl RegisterFacts {
    #[tracing::instrument(name = "load_register_facts", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        Self::parse(&input).map_err(|error| {
            BlobrayError::manifest_document("MMIO discovery report", path, &input, error)
        })
    }

    fn parse(input: &str) -> Result<Self> {
        let document = crate::artifacts::parse_mmio_facts(input)?;
        let artifacts = document
            .artifacts
            .into_iter()
            .map(|artifact| RegisterFactArtifact {
                source: artifact.source,
                sha256: artifact.artifact.sha256,
            })
            .collect();
        let ranges = document
            .ranges
            .into_iter()
            .map(|range| FactRange {
                name: range.name,
                start: range.start,
                end: range.end_exclusive,
            })
            .collect();
        let registers = document
            .registers
            .into_iter()
            .map(|register| {
                let write_patterns = register
                    .write_patterns
                    .into_iter()
                    .map(|pattern| RegisterWritePatternFact {
                        occurrences: pattern.occurrences,
                        modified_mask: pattern.modified_mask,
                        preserved_mask: pattern.preserved_mask,
                        inverted_mask: pattern.inverted_mask,
                        forced_zero_mask: pattern.forced_zero_mask,
                        forced_one_mask: pattern.forced_one_mask,
                        read_derived_mask: pattern.read_derived_mask,
                        dynamic_mask: pattern.dynamic_mask,
                        functions: pattern.functions.into_iter().collect(),
                    })
                    .collect::<Vec<_>>();
                let mut candidate_masks = write_patterns
                    .iter()
                    .map(|pattern| pattern.modified_mask)
                    .collect::<Vec<_>>();
                candidate_masks.sort_unstable();
                candidate_masks.dedup();
                RegisterFact {
                    address: register.address,
                    width: register.width,
                    catalog_name: register.name,
                    reads: register.reads,
                    writes: register.writes,
                    read_functions: register.read_functions.into_iter().collect(),
                    write_functions: register.write_functions.into_iter().collect(),
                    read_sites: register
                        .read_sites
                        .into_iter()
                        .map(|site| RegisterAccessSiteFact {
                            function: site.function,
                            pc: site.pc,
                        })
                        .collect(),
                    write_sites: register
                        .write_sites
                        .into_iter()
                        .map(|site| RegisterAccessSiteFact {
                            function: site.function,
                            pc: site.pc,
                        })
                        .collect(),
                    write_patterns,
                    candidate_masks,
                }
            })
            .collect();
        let facts = Self {
            artifacts,
            ranges,
            registers,
        };
        facts.validate()?;
        Ok(facts)
    }

    fn validate(&self) -> Result<()> {
        if self.ranges.is_empty() {
            return Err(crate::Error::invalid(
                "MMIO facts contain no discovery ranges",
            ));
        }
        let mut range_names = BTreeSet::new();
        for range in &self.ranges {
            if range.name.is_empty() || range.start >= range.end || !range_names.insert(&range.name)
            {
                return Err(crate::Error::invalid(format!(
                    "invalid or duplicate MMIO fact range {:?}",
                    range.name
                )));
            }
        }
        for (index, left) in self.ranges.iter().enumerate() {
            for right in self.ranges.iter().skip(index + 1) {
                if left.end > right.start && right.end > left.start {
                    return Err(crate::Error::invalid(format!(
                        "MMIO fact ranges {:?} and {:?} overlap",
                        left.name, right.name
                    )));
                }
            }
        }
        let mut keys = BTreeSet::new();
        for register in &self.registers {
            if register
                .read_sites
                .iter()
                .any(|site| !register.read_functions.contains(&site.function))
                || register
                    .write_sites
                    .iter()
                    .any(|site| !register.write_functions.contains(&site.function))
            {
                return Err(crate::Error::invalid(format!(
                    "MMIO register {:#010x}/{} has an access site whose function is absent from the matching function inventory",
                    register.address, register.width
                )));
            }
            if !matches!(register.width, 8 | 16 | 32) {
                return Err(crate::Error::invalid(format!(
                    "MMIO fact at {:#010x} has unsupported width {}",
                    register.address, register.width
                )));
            }
            if !keys.insert((register.address, register.width)) {
                return Err(crate::Error::invalid(format!(
                    "duplicate MMIO fact at {:#010x}/{}",
                    register.address, register.width
                )));
            }
            let width_mask = if register.width == 32 {
                u32::MAX
            } else {
                (1_u32 << register.width) - 1
            };
            if register
                .candidate_masks
                .iter()
                .any(|mask| mask & !width_mask != 0)
            {
                return Err(crate::Error::invalid(format!(
                    "MMIO fact candidate mask at {:#010x}/{} exceeds its width",
                    register.address, register.width
                )));
            }
            let owners = self
                .ranges
                .iter()
                .filter(|range| range.contains(register.address))
                .count();
            if owners != 1 {
                return Err(crate::Error::invalid(format!(
                    "MMIO fact at {:#010x}/{} belongs to {owners} ranges",
                    register.address, register.width
                )));
            }
        }
        Ok(())
    }

    pub(crate) fn select_ranges(&self, names: &[String]) -> Result<Self> {
        let selected = names.iter().map(String::as_str).collect::<BTreeSet<_>>();
        if let Some(missing) = selected
            .iter()
            .find(|name| !self.ranges.iter().any(|range| range.name == **name))
        {
            return Err(crate::Error::invalid(format!(
                "register owned range {missing:?} is absent from MMIO discovery facts"
            )));
        }
        Ok(Self {
            artifacts: self.artifacts.clone(),
            ranges: self
                .ranges
                .iter()
                .filter(|range| selected.contains(range.name.as_str()))
                .cloned()
                .collect(),
            registers: self
                .registers
                .iter()
                .filter(|register| {
                    self.ranges.iter().any(|range| {
                        selected.contains(range.name.as_str()) && range.contains(register.address)
                    })
                })
                .cloned()
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_discovery_identity_and_candidate_masks() {
        let path = std::env::temp_dir().join(format!(
            "blobray-register-facts-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{
  "schema_version": 5,
  "command": "mmio discover",
  "analysis_mode": "best-effort",
  "access_count_mode": "maximum-per-path",
  "completeness_claim": false,
  "code_selection": {"symbols":"all","symbol_prefix":""},
  "ranges": [{"name":"radio","start":"0x1000","end_exclusive":"0x2000"}],
  "artifacts": [],
  "registers": [{"address":"0x1010","width":32,"name":"UNMAPPED","reads":1,"writes":2,"read_functions":["rom:read"],"write_functions":["lib:member.o:write"],"read_sites":[{"function":"rom:read","pc":"0x00001020"}],"write_sites":[{"function":"lib:member.o:write","pc":"0x00001024"}],"write_patterns":[{"occurrences":2,"modified_mask":"0x3","candidate_bit_ranges":"0-1","preserved_mask":"0xfffffffc","inverted_mask":"0x0","forced_zero_mask":"0x0","forced_one_mask":"0x1","read_derived_mask":"0x0","dynamic_mask":"0x2","functions":["lib:member.o:write"]}]}],
  "diagnostics": []
}"#,
        )
        .unwrap();
        let facts = RegisterFacts::load(&path).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(facts.registers[0].candidate_masks, [3]);
        assert_eq!(
            facts.registers[0].read_functions,
            ["rom:read".to_owned()].into()
        );
        assert_eq!(facts.registers[0].write_patterns[0].occurrences, 2);
        assert_eq!(facts.registers[0].write_patterns[0].dynamic_mask, 2);
        assert_eq!(
            facts.registers[0].read_sites.iter().next().unwrap().pc,
            0x1020
        );
        assert_eq!(facts.ranges[0].name, "radio");
        assert!(facts.ranges[0].contains(0x1010));
    }

    #[test]
    fn selects_owned_ranges_without_discarding_width_or_evidence() {
        let facts = RegisterFacts {
            artifacts: Vec::new(),
            ranges: vec![
                FactRange {
                    name: "radio".to_owned(),
                    start: 0x1000,
                    end: 0x2000,
                },
                FactRange {
                    name: "system".to_owned(),
                    start: 0x3000,
                    end: 0x4000,
                },
            ],
            registers: vec![
                RegisterFact {
                    address: 0x1010,
                    width: 32,
                    catalog_name: "RADIO".to_owned(),
                    reads: 1,
                    writes: 0,
                    read_functions: ["read_radio".to_owned()].into(),
                    write_functions: BTreeSet::new(),
                    read_sites: BTreeSet::new(),
                    write_sites: BTreeSet::new(),
                    write_patterns: Vec::new(),
                    candidate_masks: Vec::new(),
                },
                RegisterFact {
                    address: 0x3010,
                    width: 32,
                    catalog_name: "SYSTEM".to_owned(),
                    reads: 1,
                    writes: 0,
                    read_functions: BTreeSet::new(),
                    write_functions: BTreeSet::new(),
                    read_sites: BTreeSet::new(),
                    write_sites: BTreeSet::new(),
                    write_patterns: Vec::new(),
                    candidate_masks: Vec::new(),
                },
            ],
        };
        let selected = facts.select_ranges(&["radio".to_owned()]).unwrap();
        assert_eq!(selected.ranges.len(), 1);
        assert_eq!(selected.registers.len(), 1);
        assert_eq!(
            selected.registers[0].read_functions,
            ["read_radio".to_owned()].into()
        );
    }
}
