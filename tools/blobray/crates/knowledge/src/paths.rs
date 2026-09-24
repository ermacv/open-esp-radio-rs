//! Pure finite structural path shape; application authenticates saved hops.
use super::*;
pub(super) fn validate(proposal: &KnowledgeProposal, path: &ReviewedPath) -> Result<()> {
    if path.hops.is_empty()
        || path.hops.len() > 64
        || path.purpose.trim().is_empty()
        || path.applicability.trim().is_empty()
    {
        return Err(invalid(
            "reviewed path needs 1..64 hops, purpose and applicability",
        ));
    }
    if !proposal
        .evidence
        .iter()
        .any(|e| matches!(e,EvidenceRef::Analysis {analysis,..} if *analysis==path.hops[0].caller))
    {
        return Err(invalid(
            "reviewed path needs analysis evidence for its exact root",
        ));
    }
    for (i, hop) in path.hops.iter().enumerate() {
        if i > 0 && path.hops[i - 1].callee != hop.caller
            || path.hops[..=i]
                .iter()
                .any(|earlier| earlier.caller == hop.callee)
        {
            return Err(invalid("reviewed path must be contiguous and acyclic"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finite_path_requires_continuity_root_evidence_and_no_cycle() {
        let id = |s: &[u8]| ArtifactId::of_bytes(s);
        let a = id(b"a").as_str().parse().unwrap();
        let b = id(b"b").as_str().parse().unwrap();
        let d = id(b"d").as_str().parse().unwrap();
        let path = ReviewedPath {
            hops: vec![
                FlowHop {
                    caller: a,
                    record: 10,
                    callee: b,
                },
                FlowHop {
                    caller: id(b"b").as_str().parse().unwrap(),
                    record: 20,
                    callee: d,
                },
            ],
            purpose: "selected call path".into(),
            applicability: "fixture only".into(),
        };
        let p = KnowledgeProposal {
            subject: "test.path".to_owned().try_into().unwrap(),
            occurrence: KnowledgeOccurrence {
                revision: id(b"revision").as_str().parse().unwrap(),
                source: FunctionSource::Input { input: 0 },
                object: ObjectId {
                    artifact: id(b"object"),
                    location: ObjectLocation::Standalone,
                },
                symbol: None,
            },
            claim: KnowledgeClaim::Path {
                path: Box::new(path.clone()),
            },
            evidence: vec![EvidenceRef::Analysis {
                analysis: path.hops[0].caller.clone(),
                record: None,
            }],
            note: None,
        };
        validate(&p, &path).unwrap();
        for fault in 0..5 {
            let mut bad = path.clone();
            match fault {
                0 => bad.hops.clear(),
                1 => bad.hops[1].caller = bad.hops[0].caller.clone(),
                2 => bad.hops[1].callee = bad.hops[0].caller.clone(),
                3 => bad.purpose.clear(),
                _ => bad.hops[0].caller = id(b"wrong root").as_str().parse().unwrap(),
            }
            assert!(validate(&p, &bad).is_err());
        }
    }
}
