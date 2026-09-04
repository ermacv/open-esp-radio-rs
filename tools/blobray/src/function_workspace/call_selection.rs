//! Resolve reviewed call selectors against generated observations.
//!
//! A missing address requests uniqueness, not a first-match fallback. Both
//! workspace validation and live flow inspection use this same selection rule.

use super::ReviewedEventCallMatcher;

pub(crate) const MAX_ROUTE_CALLS: usize = 64;

pub(crate) trait RouteCall {
    fn site(&self) -> Option<u32>;
    fn target(&self) -> &str;
    fn operation(&self) -> Option<&str>;
    fn direct(&self) -> bool;
}

impl RouteCall for super::FunctionCallFact {
    fn site(&self) -> Option<u32> {
        self.site
    }
    fn target(&self) -> &str {
        &self.target
    }
    fn operation(&self) -> Option<&str> {
        self.semantic_operation.as_deref()
    }
    fn direct(&self) -> bool {
        self.direct
    }
}

impl RouteCall for crate::artifacts::StoredCall {
    fn site(&self) -> Option<u32> {
        self.site
    }
    fn target(&self) -> &str {
        &self.target
    }
    fn operation(&self) -> Option<&str> {
        self.semantic_operation.as_deref()
    }
    fn direct(&self) -> bool {
        self.direct()
    }
}

fn matches(call: &impl RouteCall, matcher: &ReviewedEventCallMatcher) -> bool {
    match matcher {
        ReviewedEventCallMatcher::Function(target) => call.target() == target,
        ReviewedEventCallMatcher::Operation(operation) => call.operation() == Some(operation),
    }
}

fn require_located_direct(call: &impl RouteCall, role: &str) -> Result<(), String> {
    let site = call
        .site()
        .ok_or_else(|| format!("{role} call has no observed site"))?;
    if !call.direct() {
        return Err(format!("{role} call at {site:#010x} is indirect"));
    }
    Ok(())
}

pub(crate) fn select_route_call<'a, C: RouteCall>(
    calls: &'a [C],
    matcher: &ReviewedEventCallMatcher,
    site: Option<u32>,
    role: &str,
) -> Result<&'a C, String> {
    let mut candidates = calls.iter().filter(|call| {
        site.is_none_or(|site| call.site() == Some(site)) && matches(*call, matcher)
    });
    let first = candidates.next();
    if let Some(call) = first {
        if candidates.next().is_some() {
            return Err(format!(
                "ambiguous {role} call; use an explicit site selector"
            ));
        }
        require_located_direct(call, role)?;
        return Ok(call);
    }
    Err(match site {
        Some(site) => format!("no matching {role} call at {site:#010x}"),
        None => format!("no matching {role} call"),
    })
}

pub(crate) fn select_route_calls<'a, C: RouteCall>(
    calls: &'a [C],
    matcher: &ReviewedEventCallMatcher,
    sites: Option<&[u32]>,
    role: &str,
) -> Result<Vec<&'a C>, String> {
    let mut selected = Vec::new();
    let mut unique_sites = std::collections::BTreeSet::new();
    if let Some(sites) = sites {
        if sites.len() > MAX_ROUTE_CALLS {
            return Err(format!(
                "{role} exceeds the {MAX_ROUTE_CALLS}-call route limit"
            ));
        }
        for site in sites {
            if !unique_sites.insert(*site) {
                return Err(format!("duplicate {role} site {site:#010x}"));
            }
            selected.push(select_route_call(calls, matcher, Some(*site), role)?);
        }
    } else {
        for call in calls.iter().filter(|call| matches(*call, matcher)) {
            if selected.len() == MAX_ROUTE_CALLS {
                return Err(format!(
                    "{role} exceeds the {MAX_ROUTE_CALLS}-call route limit; refine its selector"
                ));
            }
            require_located_direct(call, role)?;
            if !unique_sites.insert(call.site().expect("located call checked above")) {
                return Err(format!("ambiguous {role} observations at one site"));
            }
            selected.push(call);
        }
        selected.sort_by_key(|call| call.site());
    }
    if selected.is_empty() {
        return Err(format!("{role} requires at least one matching call"));
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(site: u32) -> crate::function_workspace::FunctionCallFact {
        crate::function_workspace::FunctionCallFact {
            kind: "internal".to_owned(),
            target: "vendor::publish".to_owned(),
            direct: true,
            result_modeled: false,
            result_provenance: None,
            semantic_operation: Some("event.publish".to_owned()),
            site: Some(site),
            arguments: Vec::new(),
            argument_exact: Vec::new(),
            argument_result_provenance: Vec::new(),
            guard_paths: None,
        }
    }

    fn matcher() -> ReviewedEventCallMatcher {
        ReviewedEventCallMatcher::Operation("event.publish".to_owned())
    }

    #[test]
    fn omitted_site_follows_unique_observation_but_never_resolves_ambiguity() {
        for site in [0x1000, 0x2040] {
            let calls = [call(site)];
            assert_eq!(
                select_route_call(&calls, &matcher(), None, "publish")
                    .unwrap()
                    .site,
                Some(site)
            );
        }
        let calls = [call(0x1000), call(0x2040)];
        assert!(
            select_route_call(&calls, &matcher(), None, "publish")
                .unwrap_err()
                .contains("ambiguous")
        );
        assert_eq!(
            select_route_call(&calls, &matcher(), Some(0x2040), "publish")
                .unwrap()
                .site,
            Some(0x2040)
        );
        assert!(select_route_call(&calls, &matcher(), Some(0x3000), "publish").is_err());
        assert!(
            select_route_call::<crate::function_workspace::FunctionCallFact>(
                &[],
                &matcher(),
                None,
                "publish"
            )
            .is_err()
        );
    }

    #[test]
    fn inferred_dispatches_are_current_bounded_and_deterministic() {
        let mut calls = vec![call(0x2040), call(0x1000)];
        let sites = |calls: &[crate::function_workspace::FunctionCallFact]| {
            select_route_calls(calls, &matcher(), None, "publish")
                .unwrap()
                .into_iter()
                .map(|call| call.site.unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(sites(&calls), [0x1000, 0x2040]);
        calls.push(call(0x1080));
        assert_eq!(sites(&calls), [0x1000, 0x1080, 0x2040]);
        assert_eq!(
            select_route_calls(&calls, &matcher(), Some(&[0x2040]), "publish")
                .unwrap()
                .len(),
            1
        );
        assert!(select_route_calls(&calls, &matcher(), Some(&[]), "publish").is_err());
        assert!(
            select_route_calls(&calls, &matcher(), Some(&[0x1000, 0x1000]), "publish").is_err()
        );
        calls.push(call(0x1000));
        assert!(select_route_calls(&calls, &matcher(), None, "publish").is_err());
        let calls = (0..=MAX_ROUTE_CALLS as u32).map(call).collect::<Vec<_>>();
        assert!(
            select_route_calls(&calls, &matcher(), None, "publish")
                .unwrap_err()
                .contains("limit")
        );
    }

    #[test]
    fn omitted_sites_do_not_hide_indirect_or_unlocated_observations() {
        let mut candidate = call(0x1000);
        candidate.direct = false;
        for candidate in [candidate, {
            let mut call = call(0x1000);
            call.site = None;
            call
        }] {
            assert!(
                select_route_call(
                    std::slice::from_ref(&candidate),
                    &matcher(),
                    None,
                    "publish"
                )
                .is_err()
            );
            assert!(select_route_calls(&[candidate], &matcher(), None, "publish").is_err());
        }
    }

    #[test]
    fn stored_flow_calls_obey_the_same_ambiguity_and_provenance_rules() {
        let stored = |site: Option<u32>, direct| -> crate::artifacts::StoredCall {
            serde_json::from_value(serde_json::json!({
                "kind": "internal", "target": "vendor::publish", "site": site,
                "direct": direct, "tail": false, "result_modeled": false,
                "semantic_operation": "event.publish", "project_candidates": [],
                "argument_shapes": 1, "arguments": [], "argument_exact": [],
                "argument_result_provenance": [], "argument_bindings": [], "typed_arguments": []
            }))
            .unwrap()
        };
        let calls = [stored(Some(0x1000), true), stored(Some(0x2000), true)];
        assert!(select_route_call(&calls, &matcher(), None, "publish").is_err());
        assert_eq!(
            select_route_call(&calls, &matcher(), Some(0x2000), "publish")
                .unwrap()
                .site,
            Some(0x2000)
        );
        assert_eq!(
            select_route_calls(&calls, &matcher(), None, "publish")
                .unwrap()
                .len(),
            2
        );
        for call in [stored(None, true), stored(Some(0x1000), false)] {
            assert!(
                select_route_call(std::slice::from_ref(&call), &matcher(), None, "publish")
                    .is_err()
            );
            assert!(select_route_calls(&[call], &matcher(), None, "publish").is_err());
        }
    }
}
