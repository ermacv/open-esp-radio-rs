//! Full broker-route validation over synthetic generated observations.

use super::*;
use crate::function_workspace::{
    FunctionCallFact, FunctionFact, FunctionMemoryObjectFact, FunctionMemoryWriteFact,
    ReviewedBrokerSubscriptionRoute, ReviewedEventRoute,
};

fn call(target: &str, site: u32, arguments: &[&str]) -> FunctionCallFact {
    FunctionCallFact {
        kind: "internal".to_owned(),
        target: target.to_owned(),
        direct: true,
        result_modeled: false,
        result_provenance: None,
        semantic_operation: None,
        site: Some(site),
        arguments: arguments.iter().map(|value| (*value).to_owned()).collect(),
        argument_exact: vec![true; arguments.len()],
        argument_result_provenance: Vec::new(),
        guard_paths: None,
    }
}

fn function(identity: &str, calls: Vec<FunctionCallFact>) -> FunctionFact {
    FunctionFact {
        profile: "controller".to_owned(),
        source: "vendor".to_owned(),
        identity: identity.to_owned(),
        member: None,
        symbol: identity.to_owned(),
        address: None,
        selection: "symbol-prefix-root".to_owned(),
        body_complete: true,
        call_targets_complete: true,
        transitive_effects_complete: false,
        executable_complete: false,
        transitive_effects_materialized: false,
        call_graph_closed: false,
        context_projection_materialized: false,
        context_projection_complete: false,
        context_projection_blockers: Vec::new(),
        decode_blockers: Vec::new(),
        direct_calls: calls.len(),
        calls,
        memory_writes: Vec::new(),
        mmio_addresses: Vec::new(),
        context_fields: Vec::new(),
        memory_fields: Vec::new(),
        semantic_operations: Vec::new(),
        trampoline_calls: 0,
        event_dispatches: Vec::new(),
        scenario_suggestions: Vec::new(),
        pseudo: String::new(),
    }
}

fn owner<'a>(facts: &'a mut FunctionFacts, identity: &str) -> &'a mut FunctionFact {
    facts
        .functions
        .iter_mut()
        .find(|function| function.identity == identity)
        .unwrap()
}

fn fixture() -> (ReviewedBrokerSubscriptionRoute, FunctionFacts) {
    let document = crate::function_workspace::tests::CALLBACK_ROUTES
        .parse()
        .unwrap();
    let pack = crate::function_workspace::pack_parse::parse(&document).unwrap();
    let ReviewedEventRoute::BrokerSubscription(route) = &pack.event_routes[1] else {
        panic!("broker fixture route");
    };
    let mut facts = FunctionFacts {
        inputs: Vec::new(),
        functions: vec![
            function(
                "vendor::publish",
                vec![call(
                    "vendor::broker_publish",
                    0x2010,
                    &["const:0x00003000", "const:0x00000004", "expr:mask"],
                )],
            ),
            function(
                "vendor::init",
                vec![call(
                    "vendor::attach",
                    0x2020,
                    &["const:0x00003000", "const:0x00000000"],
                )],
            ),
            function(
                "vendor::enable",
                vec![call(
                    "vendor::subscribe",
                    0x2030,
                    &["const:0x00000000", "memory:absolute:0x00006000+0x0"],
                )],
            ),
            function(
                "vendor::listener",
                vec![call("vendor::handle", 0x2040, &[])],
            ),
            function("vendor::handle", Vec::new()),
        ],
    };
    owner(&mut facts, "vendor::enable")
        .memory_writes
        .push(FunctionMemoryWriteFact {
            site: 0x202c,
            object: FunctionMemoryObjectFact::Dereferenced {
                pointer: Box::new(FunctionMemoryObjectFact::Absolute {
                    address_space: "cpu".to_owned(),
                    address: 0x6000,
                }),
                pointer_offset: 0,
            },
            offset: 0,
            width: 32,
            value: Some("const:0x00005000".to_owned()),
        });
    let callback = owner(&mut facts, "vendor::listener");
    callback.address = Some(0x5000);
    callback.calls[0].guard_paths = Some(vec!["arg0 == 0x00000004".to_owned()]);
    (route.clone(), facts)
}

fn inferred(mut route: ReviewedBrokerSubscriptionRoute) -> ReviewedBrokerSubscriptionRoute {
    route.dispatch_site = None;
    route.domain.call_site = None;
    route.binding_site = None;
    route.binding_callback_store_site = None;
    route.case_handler_site = None;
    route
}

#[test]
fn broker_validator_recovers_moved_sites_without_updating_reviewed_decisions() {
    let (explicit, mut facts) = fixture();
    validate_broker_subscription(&explicit, &facts).unwrap();
    let inferred = inferred(explicit.clone());
    validate_broker_subscription(&inferred, &facts).unwrap();

    // The same observed calls and ownership move to different instruction PCs.
    // Sparse selectors survive; stale explicit selectors must still fail.
    for function in &mut facts.functions {
        for call in &mut function.calls {
            call.site = call.site.map(|site| site + 0x1000);
        }
        for write in &mut function.memory_writes {
            write.site += 0x1000;
        }
    }
    validate_broker_subscription(&inferred, &facts).unwrap();
    assert!(validate_broker_subscription(&explicit, &facts).is_err());
    let mut relocated = explicit;
    for site in [
        &mut relocated.dispatch_site,
        &mut relocated.domain.call_site,
        &mut relocated.binding_site,
        &mut relocated.binding_callback_store_site,
        &mut relocated.case_handler_site,
    ] {
        *site = site.map(|site| site + 0x1000);
    }
    validate_broker_subscription(&relocated, &facts).unwrap();
}

#[test]
fn broker_validator_rejects_ambiguous_inferred_calls_at_every_role() {
    let (explicit, facts) = fixture();
    let inferred = inferred(explicit.clone());
    for identity in [
        "vendor::publish",
        "vendor::init",
        "vendor::enable",
        "vendor::listener",
    ] {
        let mut changed = facts.clone();
        let calls = &mut owner(&mut changed, identity).calls;
        let mut additional = calls[0].clone();
        additional.site = additional.site.map(|site| site + 4);
        calls.push(additional);
        let error = validate_broker_subscription(&inferred, &changed).unwrap_err();
        assert!(
            error.to_string().contains("ambiguous"),
            "{identity}: {error}"
        );
        // An explicit site remains an intentional, unique selector.
        validate_broker_subscription(&explicit, &changed).unwrap();
        let calls = &mut owner(&mut changed, identity).calls;
        calls[1].site = calls[0].site;
        assert!(validate_broker_subscription(&explicit, &changed).is_err());
    }
}

#[test]
fn broker_validator_requires_a_unique_store_into_the_subscribed_object() {
    let (explicit, mut facts) = fixture();
    let inferred = inferred(explicit.clone());
    validate_broker_subscription(&inferred, &facts).unwrap();
    let stores = &mut owner(&mut facts, "vendor::enable").memory_writes;
    let mut additional = stores[0].clone();
    additional.site += 0x10;
    stores.push(additional);
    let error = validate_broker_subscription(&inferred, &facts).unwrap_err();
    assert!(error.to_string().contains("found 2"), "{error}");
    validate_broker_subscription(&explicit, &facts).unwrap();

    // A store to another object cannot become evidence for this subscription.
    let stores = &mut owner(&mut facts, "vendor::enable").memory_writes;
    stores[1].object = FunctionMemoryObjectFact::Dereferenced {
        pointer: Box::new(FunctionMemoryObjectFact::Absolute {
            address_space: "cpu".to_owned(),
            address: 0x7000,
        }),
        pointer_offset: 0,
    };
    validate_broker_subscription(&inferred, &facts).unwrap();
    owner(&mut facts, "vendor::enable").memory_writes.remove(0);
    assert!(validate_broker_subscription(&inferred, &facts).is_err());
}

#[test]
fn broker_payload_is_generated_but_exactness_and_legacy_constraints_still_apply() {
    let (legacy, mut facts) = fixture();
    let mut sparse = inferred(legacy.clone());
    sparse.payload_value = None;
    validate_broker_subscription(&sparse, &facts).unwrap();
    owner(&mut facts, "vendor::publish").calls[0].arguments[2] =
        "expr:current-generated-mask".to_owned();
    validate_broker_subscription(&sparse, &facts).unwrap();
    let error = validate_broker_subscription(&legacy, &facts).unwrap_err();
    assert!(
        error.to_string().contains("exact reviewed payload"),
        "{error}"
    );

    // Removing the handwritten expression never makes an inexact value usable.
    owner(&mut facts, "vendor::publish").calls[0].argument_exact[2] = false;
    let error = validate_broker_subscription(&sparse, &facts).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("broker payload argument is not exact"),
        "{error}"
    );
    owner(&mut facts, "vendor::publish").calls[0]
        .arguments
        .truncate(2);
    let error = validate_broker_subscription(&sparse, &facts).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("broker payload argument is absent"),
        "{error}"
    );
}

#[test]
fn inferred_broker_sites_do_not_bypass_directness_or_selector_guards() {
    let (explicit, facts) = fixture();
    let inferred = inferred(explicit);
    for identity in [
        "vendor::publish",
        "vendor::init",
        "vendor::enable",
        "vendor::listener",
    ] {
        let mut changed = facts.clone();
        owner(&mut changed, identity).calls[0].direct = false;
        assert!(
            validate_broker_subscription(&inferred, &changed).is_err(),
            "{identity}"
        );
    }
    let mut changed = facts;
    owner(&mut changed, "vendor::listener").calls[0].guard_paths = None;
    let error = validate_broker_subscription(&inferred, &changed).unwrap_err();
    assert!(error.to_string().contains("not guarded"), "{error}");
}
