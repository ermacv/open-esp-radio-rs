//! Mechanical TOML decoding for reviewed function/context packs.

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table};

use super::{
    FunctionPack, FunctionReviewStatus, ReviewedBrokerSubscriptionRoute, ReviewedContext,
    ReviewedContextField, ReviewedEventCallMatcher, ReviewedEventCaseHandler,
    ReviewedEventDelivery, ReviewedEventReplay, ReviewedEventRoute, ReviewedEventStateModel,
    ReviewedEventTerminal, ReviewedFunction, ReviewedFunctionArgument, ReviewedFunctionInput,
    ReviewedFunctionSignature, ReviewedLogicalType, ReviewedMemoryObject, ReviewedPath,
    ReviewedPrecondition, ReviewedSelectorEventRoute, ReviewedStaticEventCallbackRoute,
    ReviewedTypeBinding, ReviewedTypeField,
};
use crate::Result;

const MAX_REVIEWED_EVENT_ROUTES: usize = 256;
const MAX_REVIEWED_ROUTE_SITES: usize = 64;
const MAX_REVIEWED_ROUTE_CHAIN: usize = 64;

pub(super) fn parse(document: &DocumentMut) -> Result<FunctionPack> {
    Ok(FunctionPack {
        id: required_string(document.as_item(), "id", "function pack")?,
        inputs: document
            .get("inputs")
            .and_then(Item::as_array_of_tables)
            .map(parse_inputs)
            .transpose()?
            .unwrap_or_default(),
        functions: document
            .get("functions")
            .and_then(Item::as_array_of_tables)
            .map(parse_functions)
            .transpose()?
            .unwrap_or_default(),
        types: document
            .get("types")
            .and_then(Item::as_array_of_tables)
            .map(parse_types)
            .transpose()?
            .unwrap_or_default(),
        event_routes: document
            .get("event-routes")
            .and_then(Item::as_array_of_tables)
            .map(parse_event_routes)
            .transpose()?
            .unwrap_or_default(),
    })
}

fn parse_event_routes(tables: &ArrayOfTables) -> Result<Vec<ReviewedEventRoute>> {
    if tables.len() > MAX_REVIEWED_EVENT_ROUTES {
        return Err(crate::Error::invalid(format!(
            "function pack contains {} event routes; maximum is {MAX_REVIEWED_EVENT_ROUTES}",
            tables.len()
        )));
    }
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("event-routes[{index}]");
            match required_table_string(table, "kind", &context)?.as_str() {
                "selector-delivery" => {
                    let mut one = ArrayOfTables::new();
                    one.push(table.clone());
                    Ok(parse_selector_event_routes(&one)?
                        .pop()
                        .expect("one selector route was parsed"))
                }
                "static-event-callback" => parse_static_event_callback_route(table, &context),
                "broker-subscription" => parse_broker_subscription_route(table, &context),
                kind => Err(crate::Error::invalid(format!(
                    "{context}.kind must be selector-delivery, static-event-callback, or broker-subscription, got {kind:?}"
                ))),
            }
        })
        .collect()
}

fn parse_selector_event_routes(tables: &ArrayOfTables) -> Result<Vec<ReviewedEventRoute>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("event-routes[{index}]");
            let case_fields = [
                optional_string(table, "case-handler-profile"),
                optional_string(table, "case-handler-source"),
                optional_string(table, "case-handler-function"),
            ];
            let case_handler = match case_fields {
                [None, None, None] => None,
                [Some(profile), Some(source), Some(function)] => Some(ReviewedEventCaseHandler {
                    profile,
                    source,
                    function,
                }),
                _ => {
                    return Err(crate::Error::invalid(format!(
                        "{context} case handler requires profile, source, and function together"
                    )));
                }
            };
            let terminal_fields = [
                optional_string(table, "terminal-profile"),
                optional_string(table, "terminal-source"),
                optional_string(table, "terminal-function"),
            ];
            let terminal = match terminal_fields {
                [None, None, None] => None,
                [Some(profile), Some(source), Some(function)] => Some(ReviewedEventTerminal {
                    profile,
                    source,
                    function,
                }),
                _ => {
                    return Err(crate::Error::invalid(format!(
                        "{context} terminal requires profile, source, and function together"
                    )));
                }
            };
            let replay_fields = [
                optional_string(table, "replay-manifest"),
                optional_string(table, "replay-source"),
                optional_string(table, "replay-evidence"),
                optional_string(table, "replay-producer-phase"),
                optional_string(table, "replay-consumer-phase"),
                optional_string(table, "replay-state-observation"),
                optional_string(table, "replay-state-model"),
            ];
            let replay = match replay_fields {
                [None, None, None, None, None, None, None] => None,
                [
                    Some(manifest),
                    Some(source),
                    Some(evidence),
                    Some(producer_phase),
                    Some(consumer_phase),
                    Some(state_observation),
                    Some(state_model),
                ] => {
                    let state_model = match state_model.as_str() {
                        "counted-latch" => ReviewedEventStateModel::CountedLatch,
                        _ => {
                            return Err(crate::Error::invalid(format!(
                                "{context}.replay-state-model must be counted-latch"
                            )));
                        }
                    };
                    Some(ReviewedEventReplay {
                        manifest: manifest.into(),
                        source,
                        evidence: evidence.into(),
                        producer_phase,
                        consumer_phase,
                        state_observation,
                        state_model,
                    })
                }
                _ => {
                    return Err(crate::Error::invalid(format!(
                        "{context} replay requires manifest, source, evidence, producer/consumer phases, and state observation/model together"
                    )));
                }
            };
            Ok(ReviewedEventRoute::SelectorDelivery(ReviewedSelectorEventRoute {
                id: required_table_string(table, "id", &context)?,
                profile: required_table_string(table, "profile", &context)?,
                source: required_table_string(table, "source", &context)?,
                dispatcher: required_table_string(table, "dispatcher", &context)?,
                mechanism: required_table_string(table, "mechanism", &context)?,
                selector_role: required_table_string(table, "selector-role", &context)?,
                selector_value: required_integer(table, "selector-value", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.selector-value must fit u32"))
                    .map_err(crate::Error::invalid)?,
                receiver: optional_string(table, "receiver"),
                execution_context: required_table_string(table, "execution-context", &context)?,
                consumer_profile: required_table_string(table, "consumer-profile", &context)?,
                consumer_source: required_table_string(table, "consumer-source", &context)?,
                consumer_entry: required_table_string(table, "consumer-entry", &context)?,
                delivery: ReviewedEventDelivery {
                    operation: required_table_string(table, "delivery-operation", &context)?,
                    output_role: required_table_string(table, "delivery-output-role", &context)?,
                    selector_offset: required_integer(table, "delivery-selector-offset", &context)?
                        .try_into()
                        .map_err(|_| format!("{context}.delivery-selector-offset must fit u32"))
                        .map_err(crate::Error::invalid)?,
                    selector_width: required_integer(table, "delivery-selector-width", &context)?
                        .try_into()
                        .map_err(|_| format!("{context}.delivery-selector-width must fit u8"))
                        .map_err(crate::Error::invalid)?,
                    encoding: required_table_string(table, "delivery-encoding", &context)?,
                },
                case_handler,
                terminal,
                replay,
                rationale: required_table_string(table, "rationale", &context)?,
            }))
        })
        .collect()
}

fn parse_static_event_callback_route(table: &Table, context: &str) -> Result<ReviewedEventRoute> {
    Ok(ReviewedEventRoute::StaticEventCallback(
        ReviewedStaticEventCallbackRoute {
            id: required_table_string(table, "id", context)?,
            profile: required_table_string(table, "profile", context)?,
            source: required_table_string(table, "source", context)?,
            dispatcher: required_table_string(table, "dispatcher", context)?,
            mechanism: required_table_string(table, "mechanism", context)?,
            execution_context: required_table_string(table, "execution-context", context)?,
            dispatch_call: parse_call_matcher(table, "dispatch", context)?,
            dispatch_sites: optional_u32_array(table, "dispatch-sites", context)?,
            upstream_chain: required_string_array(table, "upstream-chain", context)?,
            upstream_sites: optional_u32_array(table, "upstream-sites", context)?,
            dispatch_object_argument: required_u8(table, "dispatch-object-argument", context)?,
            dispatch_queue_argument: required_u8(table, "dispatch-queue-argument", context)?,
            binding_profile: required_table_string(table, "binding-profile", context)?,
            binding_source: required_table_string(table, "binding-source", context)?,
            binding_entry: required_table_string(table, "binding-entry", context)?,
            binding_call: parse_call_matcher(table, "binding", context)?,
            binding_site: optional_u32(table, "binding-site", context)?,
            binding_object_argument: required_u8(table, "binding-object-argument", context)?,
            binding_callback_argument: required_u8(table, "binding-callback-argument", context)?,
            delivery_profile: required_table_string(table, "delivery-profile", context)?,
            delivery_source: required_table_string(table, "delivery-source", context)?,
            delivery_entry: required_table_string(table, "delivery-entry", context)?,
            receive_call: parse_call_matcher(table, "receive", context)?,
            receive_site: optional_u32(table, "receive-site", context)?,
            receive_queue_argument: required_u8(table, "receive-queue-argument", context)?,
            run_call: parse_call_matcher(table, "run", context)?,
            run_site: optional_u32(table, "run-site", context)?,
            run_event_argument: required_u8(table, "run-event-argument", context)?,
            callback_profile: required_table_string(table, "callback-profile", context)?,
            callback_source: required_table_string(table, "callback-source", context)?,
            callback_function: required_table_string(table, "callback-function", context)?,
            rationale: required_table_string(table, "rationale", context)?,
        },
    ))
}

fn parse_broker_subscription_route(table: &Table, context: &str) -> Result<ReviewedEventRoute> {
    let case_handler = parse_case_handler(table, context)?.ok_or_else(|| {
        crate::Error::invalid(format!(
            "{context} broker subscription requires a case handler"
        ))
    })?;
    Ok(ReviewedEventRoute::BrokerSubscription(
        ReviewedBrokerSubscriptionRoute {
            id: required_table_string(table, "id", context)?,
            profile: required_table_string(table, "profile", context)?,
            source: required_table_string(table, "source", context)?,
            dispatcher: required_table_string(table, "dispatcher", context)?,
            mechanism: required_table_string(table, "mechanism", context)?,
            execution_context: required_table_string(table, "execution-context", context)?,
            dispatch_call: parse_call_matcher(table, "dispatch", context)?,
            dispatch_site: optional_u32(table, "dispatch-site", context)?,
            dispatch_selector_argument: required_u8(table, "dispatch-selector-argument", context)?,
            selector_role: required_table_string(table, "selector-role", context)?,
            selector_value: required_u32(table, "selector-value", context)?,
            dispatch_payload_argument: required_u8(table, "dispatch-payload-argument", context)?,
            payload_role: required_table_string(table, "payload-role", context)?,
            payload_value: table
                .get("payload-value")
                .map(|_| required_table_string(table, "payload-value", context))
                .transpose()?,
            domain: super::ReviewedEventDomainWitness {
                profile: required_table_string(table, "domain-profile", context)?,
                source: required_table_string(table, "domain-source", context)?,
                entry: required_table_string(table, "domain-entry", context)?,
                call: parse_call_matcher(table, "domain", context)?,
                call_site: optional_u32(table, "domain-site", context)?,
                dispatch_argument: required_u8(table, "domain-dispatch-argument", context)?,
                call_object_argument: required_u8(table, "domain-call-object-argument", context)?,
                call_selector_argument: required_u8(
                    table,
                    "domain-call-selector-argument",
                    context,
                )?,
                selector_value: required_u32(table, "domain-selector-value", context)?,
            },
            binding_profile: required_table_string(table, "binding-profile", context)?,
            binding_source: required_table_string(table, "binding-source", context)?,
            binding_entry: required_table_string(table, "binding-entry", context)?,
            binding_call: parse_call_matcher(table, "binding", context)?,
            binding_site: optional_u32(table, "binding-site", context)?,
            binding_domain_argument: required_u8(table, "binding-domain-argument", context)?,
            binding_object_argument: required_u8(table, "binding-object-argument", context)?,
            binding_callback_store_site: optional_u32(
                table,
                "binding-callback-store-site",
                context,
            )?,
            binding_callback_store_offset: required_integer(
                table,
                "binding-callback-store-offset",
                context,
            )?,
            callback_profile: required_table_string(table, "callback-profile", context)?,
            callback_source: required_table_string(table, "callback-source", context)?,
            callback_function: required_table_string(table, "callback-function", context)?,
            callback_selector_argument: required_u8(table, "callback-selector-argument", context)?,
            case_handler,
            case_handler_site: optional_u32(table, "case-handler-site", context)?,
            terminal: parse_terminal(table, context)?,
            rationale: required_table_string(table, "rationale", context)?,
        },
    ))
}

fn parse_call_matcher(
    table: &Table,
    prefix: &str,
    context: &str,
) -> Result<ReviewedEventCallMatcher> {
    match (
        optional_string(table, &format!("{prefix}-operation")),
        optional_string(table, &format!("{prefix}-function")),
    ) {
        (Some(operation), None) => Ok(ReviewedEventCallMatcher::Operation(operation)),
        (None, Some(function)) => Ok(ReviewedEventCallMatcher::Function(function)),
        _ => Err(crate::Error::invalid(format!(
            "{context} requires exactly one {prefix} operation or function"
        ))),
    }
}

fn parse_case_handler(table: &Table, context: &str) -> Result<Option<ReviewedEventCaseHandler>> {
    match [
        optional_string(table, "case-handler-profile"),
        optional_string(table, "case-handler-source"),
        optional_string(table, "case-handler-function"),
    ] {
        [None, None, None] => Ok(None),
        [Some(profile), Some(source), Some(function)] => Ok(Some(ReviewedEventCaseHandler {
            profile,
            source,
            function,
        })),
        _ => Err(crate::Error::invalid(format!(
            "{context} case handler requires profile, source, and function together"
        ))),
    }
}

fn parse_terminal(table: &Table, context: &str) -> Result<Option<ReviewedEventTerminal>> {
    match [
        optional_string(table, "terminal-profile"),
        optional_string(table, "terminal-source"),
        optional_string(table, "terminal-function"),
    ] {
        [None, None, None] => Ok(None),
        [Some(profile), Some(source), Some(function)] => Ok(Some(ReviewedEventTerminal {
            profile,
            source,
            function,
        })),
        _ => Err(crate::Error::invalid(format!(
            "{context} terminal requires profile, source, and function together"
        ))),
    }
}

fn parse_types(tables: &ArrayOfTables) -> Result<Vec<ReviewedLogicalType>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("types[{index}]");
            Ok(ReviewedLogicalType {
                id: required_table_string(table, "id", &context)?,
                name: required_table_string(table, "name", &context)?,
                description: optional_string(table, "description"),
                bindings: table
                    .get("bindings")
                    .and_then(Item::as_array_of_tables)
                    .map(|bindings| parse_type_bindings(bindings, &context))
                    .transpose()?
                    .unwrap_or_default(),
                fields: table
                    .get("fields")
                    .and_then(Item::as_array_of_tables)
                    .map(|fields| parse_type_fields(fields, &context))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_type_bindings(
    tables: &ArrayOfTables,
    logical_type: &str,
) -> Result<Vec<ReviewedTypeBinding>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{logical_type}.bindings[{index}]");
            let object = match required_table_string(table, "kind", &context)?.as_str() {
                "argument" => ReviewedMemoryObject::Argument {
                    function: required_table_string(table, "function", &context)?,
                    index: required_integer(table, "argument", &context)?
                        .try_into()
                        .map_err(|_| format!("{context}.argument must fit u8"))
                        .map_err(crate::Error::invalid)?,
                },
                "global" => ReviewedMemoryObject::Global {
                    member: optional_string(table, "member"),
                    symbol: required_table_string(table, "symbol", &context)?,
                },
                "dereferenced" => ReviewedMemoryObject::Dereferenced {
                    pointer: Box::new(parse_pointer_object(table, &context)?),
                    pointer_offset: required_integer(table, "pointer-offset", &context)?,
                },
                "absolute" => ReviewedMemoryObject::Absolute {
                    address_space: required_table_string(table, "address-space", &context)?,
                    address: required_integer(table, "address", &context)?
                        .try_into()
                        .map_err(|_| format!("{context}.address must fit u32"))
                        .map_err(crate::Error::invalid)?,
                },
                kind => {
                    return Err(crate::Error::invalid(format!(
                        "invalid memory object kind {kind:?} in {context}"
                    )));
                }
            };
            Ok(ReviewedTypeBinding {
                profile: required_table_string(table, "profile", &context)?,
                source: required_table_string(table, "source", &context)?,
                name: required_table_string(table, "name", &context)?,
                object,
            })
        })
        .collect()
}

fn parse_pointer_object(table: &Table, context: &str) -> Result<ReviewedMemoryObject> {
    match required_table_string(table, "pointer-kind", context)?.as_str() {
        "argument" => Ok(ReviewedMemoryObject::Argument {
            function: required_table_string(table, "pointer-function", context)?,
            index: required_integer(table, "pointer-argument", context)?
                .try_into()
                .map_err(|_| format!("{context}.pointer-argument must fit u8"))
                .map_err(crate::Error::invalid)?,
        }),
        "global" => Ok(ReviewedMemoryObject::Global {
            member: optional_string(table, "pointer-member"),
            symbol: required_table_string(table, "pointer-symbol", context)?,
        }),
        "absolute" => Ok(ReviewedMemoryObject::Absolute {
            address_space: required_table_string(table, "pointer-address-space", context)?,
            address: required_integer(table, "pointer-address", context)?
                .try_into()
                .map_err(|_| format!("{context}.pointer-address must fit u32"))
                .map_err(crate::Error::invalid)?,
        }),
        kind => Err(crate::Error::invalid(format!(
            "invalid pointer memory object kind {kind:?} in {context}"
        ))),
    }
}

fn parse_type_fields(tables: &ArrayOfTables, logical_type: &str) -> Result<Vec<ReviewedTypeField>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{logical_type}.fields[{index}]");
            Ok(ReviewedTypeField {
                offset: required_integer(table, "offset", &context)?,
                width: required_integer(table, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.width must fit u8"))
                    .map_err(crate::Error::invalid)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                display_type: optional_string(table, "display-type"),
                description: optional_string(table, "description"),
            })
        })
        .collect()
}

fn parse_inputs(tables: &ArrayOfTables) -> Result<Vec<ReviewedFunctionInput>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("inputs[{index}]");
            Ok(ReviewedFunctionInput {
                profile: required_table_string(table, "profile", &context)?,
                source: required_table_string(table, "source", &context)?,
                sha256: required_table_string(table, "artifact-sha256", &context)?,
            })
        })
        .collect()
}

fn parse_functions(tables: &ArrayOfTables) -> Result<Vec<ReviewedFunction>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("functions[{index}]");
            Ok(ReviewedFunction {
                profile: required_table_string(table, "profile", &context)?,
                source: required_table_string(table, "source", &context)?,
                identity: required_table_string(table, "identity", &context)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                role: optional_string(table, "role"),
                summary: optional_string(table, "summary"),
                signature: table
                    .get("signature")
                    .and_then(Item::as_table)
                    .map(|signature| parse_function_signature(signature, &context))
                    .transpose()?,
                accept_incomplete: optional_bool(table, "accept-incomplete", &context)?
                    .unwrap_or(false),
                preconditions: table
                    .get("preconditions")
                    .and_then(Item::as_array_of_tables)
                    .map(|values| parse_preconditions(values, &context))
                    .transpose()?
                    .unwrap_or_default(),
                paths: table
                    .get("paths")
                    .and_then(Item::as_array_of_tables)
                    .map(|values| parse_paths(values, &context))
                    .transpose()?
                    .unwrap_or_default(),
                contexts: table
                    .get("contexts")
                    .and_then(Item::as_array_of_tables)
                    .map(|contexts| parse_contexts(contexts, &context))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_function_signature(table: &Table, function: &str) -> Result<ReviewedFunctionSignature> {
    let context = format!("{function}.signature");
    Ok(ReviewedFunctionSignature {
        arguments: table
            .get("arguments")
            .and_then(Item::as_array_of_tables)
            .map(|arguments| {
                arguments
                    .iter()
                    .enumerate()
                    .map(|(position, argument)| -> Result<ReviewedFunctionArgument> {
                        let argument_context = format!("{context}.arguments[{position}]");
                        Ok(ReviewedFunctionArgument {
                            index: required_integer(argument, "index", &argument_context)?
                                .try_into()
                                .map_err(|_| {
                                    crate::Error::invalid(format!(
                                        "{argument_context}.index must fit u8"
                                    ))
                                })?,
                            name: required_table_string(argument, "name", &argument_context)?,
                            abi: required_table_string(argument, "abi", &argument_context)?,
                            role: optional_string(argument, "role"),
                        })
                    })
                    .collect()
            })
            .transpose()?
            .unwrap_or_default(),
        return_abi: optional_string(table, "return-abi"),
        return_role: optional_string(table, "return-role"),
    })
}

fn parse_preconditions(
    tables: &ArrayOfTables,
    function: &str,
) -> Result<Vec<ReviewedPrecondition>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{function}.preconditions[{index}]");
            Ok(ReviewedPrecondition {
                id: required_table_string(table, "id", &context)?,
                expression: required_table_string(table, "expression", &context)?,
                rationale: required_table_string(table, "rationale", &context)?,
            })
        })
        .collect()
}

fn parse_paths(tables: &ArrayOfTables, function: &str) -> Result<Vec<ReviewedPath>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{function}.paths[{index}]");
            Ok(ReviewedPath {
                id: required_table_string(table, "id", &context)?,
                class: required_table_string(table, "class", &context)?,
                summary: required_table_string(table, "summary", &context)?,
                evidence: required_table_string(table, "evidence", &context)?,
            })
        })
        .collect()
}

fn parse_contexts(tables: &ArrayOfTables, function: &str) -> Result<Vec<ReviewedContext>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{function}.contexts[{index}]");
            Ok(ReviewedContext {
                argument: required_integer(table, "argument", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.argument must fit u8"))
                    .map_err(crate::Error::invalid)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                type_name: optional_string(table, "type-name"),
                fields: table
                    .get("fields")
                    .and_then(Item::as_array_of_tables)
                    .map(|fields| parse_fields(fields, &context))
                    .transpose()?
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn parse_fields(tables: &ArrayOfTables, context: &str) -> Result<Vec<ReviewedContextField>> {
    tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            let context = format!("{context}.fields[{index}]");
            Ok(ReviewedContextField {
                offset: required_integer(table, "offset", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.offset must fit i32"))
                    .map_err(crate::Error::invalid)?,
                width: required_integer(table, "width", &context)?
                    .try_into()
                    .map_err(|_| format!("{context}.width must fit u8"))
                    .map_err(crate::Error::invalid)?,
                status: parse_status(table, &context)?,
                name: optional_string(table, "name"),
                display_type: optional_string(table, "display-type"),
                description: optional_string(table, "description"),
            })
        })
        .collect()
}

fn parse_status(table: &Table, context: &str) -> Result<FunctionReviewStatus> {
    Ok(match optional_string(table, "status").as_deref() {
        Some("reviewed") => FunctionReviewStatus::Reviewed,
        Some("ignored") => FunctionReviewStatus::Ignored,
        None | Some("unreviewed") => {
            return Err(crate::Error::invalid(format!(
                "{context} is a sparse review overlay and requires status = \"reviewed\" or \"ignored\"; omit unreviewed generated observations"
            )));
        }
        Some(status) => {
            return Err(crate::Error::invalid(format!(
                "invalid review status {status:?} in {context}"
            )));
        }
    })
}

fn required_string(item: &Item, key: &str, context: &str) -> Result<String> {
    item.get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn required_table_string(table: &Table, key: &str, context: &str) -> Result<String> {
    optional_string(table, key)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires string {key:?}")))
}

fn optional_string(table: &Table, key: &str) -> Option<String> {
    table.get(key).and_then(Item::as_str).map(str::to_owned)
}

fn required_integer(table: &Table, key: &str, context: &str) -> Result<i64> {
    table
        .get(key)
        .and_then(Item::as_integer)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires integer {key:?}")))
}

fn required_u8(table: &Table, key: &str, context: &str) -> Result<u8> {
    required_integer(table, key, context)?
        .try_into()
        .map_err(|_| crate::Error::invalid(format!("{context}.{key} must fit u8")))
}

fn required_u32(table: &Table, key: &str, context: &str) -> Result<u32> {
    required_integer(table, key, context)?
        .try_into()
        .map_err(|_| crate::Error::invalid(format!("{context}.{key} must fit u32")))
}

fn optional_u32(table: &Table, key: &str, context: &str) -> Result<Option<u32>> {
    table
        .get(key)
        .map(|_| required_u32(table, key, context))
        .transpose()
}

fn optional_u32_array(table: &Table, key: &str, context: &str) -> Result<Option<Vec<u32>>> {
    table
        .get(key)
        .map(|_| required_u32_array(table, key, context))
        .transpose()
}

fn required_string_array(table: &Table, key: &str, context: &str) -> Result<Vec<String>> {
    let array = table
        .get(key)
        .and_then(Item::as_array)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires array {key:?}")))?;
    if array.len() > MAX_REVIEWED_ROUTE_CHAIN {
        return Err(crate::Error::invalid(format!(
            "{context}.{key} contains {} entries; maximum is {MAX_REVIEWED_ROUTE_CHAIN}",
            array.len()
        )));
    }
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                crate::Error::invalid(format!("{context}.{key}[{index}] must be a string"))
            })
        })
        .collect()
}

fn required_u32_array(table: &Table, key: &str, context: &str) -> Result<Vec<u32>> {
    let array = table
        .get(key)
        .and_then(Item::as_array)
        .ok_or_else(|| crate::Error::invalid(format!("{context} requires array {key:?}")))?;
    if array.len() > MAX_REVIEWED_ROUTE_SITES {
        return Err(crate::Error::invalid(format!(
            "{context}.{key} contains {} entries; maximum is {MAX_REVIEWED_ROUTE_SITES}",
            array.len()
        )));
    }
    array
        .iter()
        .enumerate()
        .map(|(index, value)| {
            value
                .as_integer()
                .and_then(|value| value.try_into().ok())
                .ok_or_else(|| {
                    crate::Error::invalid(format!("{context}.{key}[{index}] must fit u32"))
                })
        })
        .collect()
}

fn optional_bool(table: &Table, key: &str, context: &str) -> Result<Option<bool>> {
    table
        .get(key)
        .map(|item| {
            item.as_bool()
                .ok_or_else(|| crate::Error::invalid(format!("{context}.{key} must be a boolean")))
        })
        .transpose()
}
