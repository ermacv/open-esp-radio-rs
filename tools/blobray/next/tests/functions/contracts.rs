use super::interfaces::{cli, propose, review};
use super::*;
fn proposal(f: &Fixture, bytes: &[u8], range: bool) -> KnowledgeProposal {
    use object::{Object as _, ObjectSection as _};
    let selector = if range {
        let elf = object::File::parse(bytes).unwrap();
        let section = elf.section_by_name(".text.entry").unwrap();
        FunctionSelector::Range {
            object: f.request.selector.object().clone(),
            section: section.index().0 as u32,
            extent: CodeRange {
                start: 0,
                length: section.size(),
            },
        }
    } else {
        f.request.selector.clone()
    };
    serde_json::from_value(serde_json::json!({
        "subject":"fixture.context-function",
        "occurrence":{"revision":f.revision,"source":f.request.source,
            "object":selector.object(),"symbol":selector.symbol()},
        "claim":{"kind":"function","contract":{
            "selector":selector,"abi":"riscv-integer","name":"move_field","role":"fixture.copy",
            "return_role":null,"summary":"Declared field copy","applicability":"fixture input only",
            "signature":{"arguments":[{"role":"fixture.context","value_type":{"kind":"pointer","nullable":false}}],
                "result":{"kind":"void"},"variadic":false},
            "contexts":[{"argument":0,"name":"state","start":0,"length":12,"fields":[
                {"offset":4,"width":4,"name":"input","value_type":{"kind":"integer","bits":32,"signed":false},"access":"read","role":"fixture.input"},
                {"offset":8,"width":4,"name":"output","value_type":null,"access":"write","role":null}]}],
            "preconditions":[{"kind":"argument-bits","argument":0,"mask":3,"value":0,"reason":"aligned context"},
                {"kind":"assumption","id":"fixture.valid-context","statement":"Caller provides live storage for the declared extent","reason":"caller obligation, not an observed lifetime"}]
        }},
        "evidence":[{"kind":"source","payload":ArtifactId::of_bytes(bytes),"range":{"start":0,"length":bytes.len()}}],"note":null
    })).unwrap()
}
#[test]
fn function_contracts_validate_review_and_export_exact_symbols_ranges_and_contexts() {
    for thin in [false, true] {
        for range in [false, true] {
            let words = [0x00452583u32, 0x00b52423, 0x00008067];
            let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
            let bytes = object(&code, code.len() as u64, false);
            let f = fixture(bytes.clone(), thin);
            let original = analyze(&f);
            assert_eq!(original.state, RunState::Completed);
            let p = proposal(&f, &bytes, range);
            let mut bad = p.clone();
            let KnowledgeClaim::Function { contract } = &mut bad.claim else {
                panic!()
            };
            match &mut contract.selector {
                FunctionSelector::Symbol { symbol } => symbol.index = u64::MAX,
                FunctionSelector::Range { extent, .. } => extent.length = 0x100000,
            }
            bad.occurrence.symbol = None;
            let failed = propose(&f, bad, None);
            assert_eq!(failed.state, RunState::Failed, "{failed:?}");
            assert!(failed.knowledge.is_none());
            assert!(
                cli(&f, &["knowledge", "show"])["records"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            let change = KnowledgeChange {
                expected_base: None,
                actor: "fixture".into(),
                reason: "conditional contract".into(),
                action: KnowledgeAction::Propose {
                    proposal: p.clone(),
                },
            };
            let path = f.dir.path().join("function-change.json");
            fs::write(&path, serde_json::to_vec(&change).unwrap()).unwrap();
            cli(
                &f,
                &["knowledge", "validate", "--change", path.to_str().unwrap()],
            );
            assert!(
                cli(&f, &["knowledge", "show"])["records"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            let proposed = cli(
                &f,
                &["knowledge", "apply", "--change", path.to_str().unwrap()],
            );
            let base = proposed["run"]["knowledge"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let entries = cli(&f, &["knowledge", "show"]);
            let id = entries["records"][0]["value"]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            fs::remove_file(f.dir.path().join("entry.a")).unwrap();
            fs::remove_file(f.dir.path().join("entry.o")).unwrap();
            let accepted = review(&f, base, id, ReviewDecision::Accept);
            assert_eq!(accepted.state, RunState::Completed, "{accepted:?}");
            let revision = accepted.knowledge.unwrap();
            let saved = cli(&f, &["knowledge", "show", "--revision", revision.as_str()]);
            assert_eq!(
                saved["records"][0]["value"]["proposal"],
                serde_json::to_value(&p).unwrap()
            );
            assert_eq!(saved["records"][0]["value"]["state"], "accepted");
            let output = f.dir.path().join("function-contract.json");
            cli(
                &f,
                &[
                    "knowledge",
                    "export",
                    "--revision",
                    revision.as_str(),
                    "--output",
                    output.to_str().unwrap(),
                ],
            );
            let exported: serde_json::Value =
                serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
            assert_eq!(exported["records"].as_array().unwrap().len(), 2);
            assert_eq!(
                analyze(&f).analysis,
                original.analysis,
                "review cannot rewrite analysis facts"
            );
            let mut changed = p.clone();
            let KnowledgeClaim::Function { contract } = &mut changed.claim else {
                panic!()
            };
            contract.contexts[0].fields[0].access = ContextAccess::ReadWrite;
            let proposed = propose(&f, changed, Some(revision.clone()));
            assert_eq!(proposed.state, RunState::Completed);
            let entries = cli(&f, &["knowledge", "show"]);
            let id = entries["records"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["value"]["state"] == "proposed")
                .unwrap()["value"]["id"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap();
            let failed = review(
                &f,
                proposed.knowledge.clone().unwrap(),
                id,
                ReviewDecision::Accept,
            );
            assert_eq!(failed.state, RunState::Failed);
            assert_eq!(failed.error.as_ref().unwrap().code, ErrorCode::Conflict);
            assert!(failed.knowledge.is_none());
            let mut limits = budget();
            limits.max_work_units = Some(1);
            let mut change = change;
            change.expected_base = proposed.knowledge.clone();
            let limited = f
                .app
                .start_knowledge(&f.project, &change, limits)
                .unwrap()
                .wait();
            assert_eq!(limited.state, RunState::ResourceLimited);
            assert!(limited.knowledge.is_none());
            assert_eq!(
                cli(&f, &["knowledge", "show"])["summary"]["status"]["revision"],
                serde_json::to_value(proposed.knowledge).unwrap()
            );
            assert_eq!(
                cli(&f, &["knowledge", "show", "--revision", revision.as_str()]),
                saved
            );
        }
    }
}
