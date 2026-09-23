//! Bounded argument-sensitive accessor discovery. Each contextual observation
//! retains the caller chain and actual symbolic arguments. It is independent
//! of function summaries used for executable replacement claims.
use super::*;

const MAX_CONTEXTS: usize = 128;
const MAX_DEPTH: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct CallContext {
    site: u32,
    target: u32,
    arguments: Box<[SymbolicValue]>,
}

impl CallContext {
    pub(super) fn from_event(event: &crate::DraftReferenceEvent) -> Option<Self> {
        match event {
            crate::DraftReferenceEvent::Call {
                site,
                target,
                arguments,
                ..
            }
            | crate::DraftReferenceEvent::TailCall {
                site,
                target,
                arguments,
                ..
            } => Some(Self {
                site: *site,
                target: *target,
                arguments: arguments.clone(),
            }),
            _ => None,
        }
    }
    pub(super) fn key(&self) -> String {
        format!("{}:{}:{:?}", self.site, self.target, self.arguments)
    }
    fn payload(&self, path: &[String]) -> serde_json::Value {
        serde_json::json!({"kind":"call-context","site":self.site,"target":self.target,"caller_path":path,"arguments":self.arguments.iter().map(SymbolicValue::canonical).collect::<Vec<_>>(),"argument_provenance":format!("{:?}",self.arguments)})
    }
}

#[derive(Default)]
pub(super) struct Explorer {
    queue: BTreeMap<(Vec<String>, String), CallContext>,
    scheduled: usize,
}

impl Explorer {
    pub(super) fn enqueue(
        &mut self,
        owner: &DiscoveryFunction,
        calls: &BTreeMap<String, CallContext>,
        output: &mut Vec<serde_json::Value>,
    ) {
        self.enqueue_path(vec![owner.canonical()], calls, output);
    }
    fn enqueue_path(
        &mut self,
        path: Vec<String>,
        calls: &BTreeMap<String, CallContext>,
        output: &mut Vec<serde_json::Value>,
    ) {
        for call in calls.values() {
            let mut payload = call.payload(&path);
            if path.len() > MAX_DEPTH || self.scheduled >= MAX_CONTEXTS {
                payload["kind"] = serde_json::json!("call-context-frontier");
                payload["reason"] = serde_json::json!(format!(
                    "context budget: {MAX_CONTEXTS} contexts, {MAX_DEPTH} caller levels; retry with a narrower symbol selection"
                ));
            } else {
                self.queue.insert((path.clone(), call.key()), call.clone());
                if self.queue.len() + self.scheduled > MAX_CONTEXTS {
                    let ((deferred_path, _), deferred) =
                        self.queue.pop_last().expect("bounded frontier");
                    let mut frontier = deferred.payload(&deferred_path);
                    frontier["kind"] = serde_json::json!("call-context-frontier");
                    frontier["reason"] = serde_json::json!(
                        "context budget; deterministic lexicographic scheduling; retry with a narrower symbol selection"
                    );
                    output.push(frontier);
                }
            }
            output.push(payload);
        }
    }
    pub(super) fn run(
        mut self,
        source: &str,
        symbols: &[artifact::ArtifactSymbolDefinition],
        map: &MmioMap,
        output: &mut Vec<serde_json::Value>,
        mut consume: impl FnMut(&DiscoveryFunction, &LocatedObservableEvent, usize) + Send,
    ) -> Result<()> {
        // Match the root explorer's explicit stack bound for recursive symbolic
        // expressions; only one context worker is alive per artifact.
        thread::scope(|scope| {
            thread::Builder::new().name("mmio-contexts".to_owned()).stack_size(DISCOVERY_WORKER_STACK_BYTES).spawn_scoped(scope, || {
                while let Some(((mut path, _), call)) = self.queue.pop_first() {
                    self.scheduled += 1;
                    let mut targets = symbols.iter().filter(|symbol| symbol.addresses_resolved && symbol.address == u64::from(call.target));
                    let target = targets.next();
                    if target.is_none() || targets.next().is_some() {
                        let mut frontier = call.payload(&path);
                        frontier["kind"] = serde_json::json!("call-context-frontier");
                        frontier["reason"] = serde_json::json!("target is absent, relocatable or ambiguous in the selected code; arguments remain available");
                        output.push(frontier);
                        continue;
                    }
                    let symbol = target.expect("unique target");
                    let function = DiscoveryFunction {source:source.to_owned(),member:symbol.member.clone(),symbol:symbol.name.clone()};
                    let arguments: crate::Rv32CallArguments = std::array::from_fn(|index| call.arguments.get(index).cloned().unwrap_or_else(|| SymbolicValue::input(index as u8)));
                    let exploration = explore_symbol_with_arguments(symbol, map, &direct::StructuralRelocatedCalls::default(), &StructuralPointerContext::default(), Some(&arguments));
                    for (event, occurrences) in &exploration.events { consume(&function, event, *occurrences); }
                    let context = call.payload(&path);
                    path.push(function.canonical());
                    for mut observation in exploration.observations.into_values() {
                        observation["function"] = serde_json::json!(function.canonical());
                        observation["call_context"] = context.clone();
                        output.push(observation);
                    }
                    for (scope, reason) in exploration.diagnostics {
                        output.push(serde_json::json!({"kind":"call-context-frontier","function":function.canonical(),"call_context":context,"scope":scope,"reason":reason}));
                    }
                    self.enqueue_path(path, &exploration.calls, output);
                }
            })?.join().map_err(|_| crate::Error::invalid("MMIO context worker panicked"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_accessor_discovers_read_and_write_with_caller_arguments() {
        let symbol = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "accessor",
                0x100c,
            ),
            member: None,
            name: "accessor".to_owned(),
            address: 0x100c,
            bytes: vec![
                0x83, 0x25, 0x05, 0x00, 0x23, 0x22, 0xb5, 0x00, 0x67, 0x80, 0x00, 0x00,
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let map = discovery_map(
            &MmioMap {
                registers: Vec::new(),
                regions: Vec::new(),
            },
            &[DiscoveryRange {
                name: "dev".to_owned(),
                start: 0x4000,
                end: 0x4100,
            }],
        );
        let owner = DiscoveryFunction {
            source: "fixture".to_owned(),
            member: None,
            symbol: "caller".to_owned(),
        };
        let caller = artifact::ArtifactSymbolDefinition {
            identity: artifact::ArtifactSymbolDefinition::synthetic_identity(
                module_path!(),
                &(None),
                "caller",
                0x1000,
            ),
            member: None,
            name: "caller".to_owned(),
            address: 0x1000,
            bytes: vec![
                0x37, 0x45, 0x00, 0x00, 0xef, 0x00, 0x80, 0x00, 0x67, 0x80, 0x00, 0x00,
            ],
            addresses_resolved: true,
            memory_regions: Default::default(),
            relocations: Vec::new(),
        };
        let root = explore_symbol(
            &caller,
            &map,
            &direct::StructuralRelocatedCalls::default(),
            &StructuralPointerContext::default(),
        );
        assert_eq!(root.calls.len(), 1);
        let mut explorer = Explorer::default();
        let mut output = Vec::new();
        explorer.enqueue(&owner, &root.calls, &mut output);
        let mut aggregates = Vec::new();
        explorer
            .run(
                "fixture",
                &[symbol],
                &map,
                &mut output,
                |_, event, count| aggregates.push((event.clone(), count)),
            )
            .unwrap();
        assert_eq!(aggregates.len(), 2);
        let accesses = output
            .iter()
            .filter(|item| item["kind"] == "instruction-mmio")
            .collect::<Vec<_>>();
        assert_eq!(accesses.len(), 2, "{output:#?}");
        assert!(
            accesses
                .iter()
                .any(|item| item["address"] == 0x4000 && item["access"] == "Read")
        );
        assert!(
            accesses
                .iter()
                .any(|item| item["address"] == 0x4004 && item["access"] == "Write")
        );
        assert!(
            accesses
                .iter()
                .all(|item| item["call_context"]["caller_path"][0] == "fixture:caller")
        );
    }

    #[test]
    fn bounded_frontier_selection_is_independent_of_worker_completion_order() {
        let owner = DiscoveryFunction {
            source: "fixture".to_owned(),
            member: None,
            symbol: "caller".to_owned(),
        };
        let mut forward = Explorer::default();
        let mut reverse = Explorer::default();
        for (explorer, reversed) in [(&mut forward, false), (&mut reverse, true)] {
            let mut values = (0..MAX_CONTEXTS + 2).collect::<Vec<_>>();
            if reversed {
                values.reverse();
            }
            let mut output = Vec::new();
            for value in values {
                let call = CallContext {
                    site: value as u32,
                    target: 0x1100,
                    arguments: vec![SymbolicValue::Constant(value as u32)].into_boxed_slice(),
                };
                explorer.enqueue(&owner, &BTreeMap::from([(call.key(), call)]), &mut output);
            }
            assert_eq!(explorer.queue.len(), MAX_CONTEXTS);
            assert_eq!(
                output
                    .iter()
                    .filter(|item| item["kind"] == "call-context-frontier")
                    .count(),
                2
            );
        }
        assert_eq!(forward.queue, reverse.queue);
    }
}
