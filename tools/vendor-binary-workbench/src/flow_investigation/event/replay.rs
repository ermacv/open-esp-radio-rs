//! Validation of concrete replay evidence joined to an event route.

use crate::{Result, artifacts, execution_model};

#[derive(Debug)]
pub(super) struct ReplayRouteProof {
    pub(super) service_id: String,
    pub(super) producer_symbol: String,
    pub(super) consumer_symbol: String,
    pub(super) enqueue_site: u32,
    pub(super) dequeue_site: u32,
    pub(super) handler_site: Option<u32>,
    pub(super) state: ReplayStateProof,
}

#[derive(Debug)]
pub(super) struct ReplayStateProof {
    pub(super) id: String,
    pub(super) symbol: String,
    pub(super) address: u32,
    pub(super) width: u8,
    pub(super) producer_before: u32,
    pub(super) producer_after: u32,
    pub(super) producer_write_site: u32,
    pub(super) consumer_before: u32,
    pub(super) consumer_after: u32,
    pub(super) consumer_write_site: u32,
}

pub(super) fn load_replay_proof(
    route: &crate::function_workspace::ReviewedEventRoute,
) -> Result<Option<ReplayRouteProof>> {
    let Some(binding) = &route.replay else {
        return Ok(None);
    };
    let input = std::fs::read_to_string(&binding.evidence)
        .map_err(|error| crate::Error::read("event replay evidence", &binding.evidence, error))?;
    let evidence = artifacts::parse_replay_evidence(&input)?;
    let reviewed_manifest = std::fs::canonicalize(&binding.manifest).map_err(|error| {
        crate::Error::read("reviewed replay manifest", &binding.manifest, error)
    })?;
    let executed_manifest = std::fs::canonicalize(&evidence.manifest.path).map_err(|error| {
        crate::Error::read(
            "executed replay manifest",
            std::path::Path::new(&evidence.manifest.path),
            error,
        )
    })?;
    if reviewed_manifest != executed_manifest {
        return Err(crate::Error::invalid(format!(
            "replay evidence was executed from {}, but the reviewed route binds {}",
            executed_manifest.display(),
            reviewed_manifest.display()
        )));
    }
    let producer_index = evidence
        .phases
        .iter()
        .position(|phase| phase.name == binding.producer_phase)
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "replay evidence has no producer phase {:?}",
                binding.producer_phase
            ))
        })?;
    let consumer_index = evidence
        .phases
        .iter()
        .position(|phase| phase.name == binding.consumer_phase)
        .ok_or_else(|| {
            crate::Error::invalid(format!(
                "replay evidence has no consumer phase {:?}",
                binding.consumer_phase
            ))
        })?;
    if producer_index >= consumer_index {
        return Err(crate::Error::invalid(format!(
            "replay producer phase {:?} must precede consumer phase {:?}",
            binding.producer_phase, binding.consumer_phase
        )));
    }
    let producer = evidence.phase(&binding.producer_phase)?;
    let consumer = evidence.phase(&binding.consumer_phase)?;
    let enqueues = producer
        .fifo_lifecycle
        .iter()
        .filter_map(|event| match event {
            artifacts::StoredFifoLifecycleEvent::Enqueued {
                service_id,
                site,
                value,
                ..
            } if *value == route.selector_value => Some((service_id, *site)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let dequeues = consumer
        .fifo_lifecycle
        .iter()
        .filter_map(|event| match event {
            artifacts::StoredFifoLifecycleEvent::Dequeued {
                service_id,
                site,
                value,
                ..
            } if *value == route.selector_value => Some((service_id, *site)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let (service_id, enqueue_site) = match enqueues.as_slice() {
        [(service_id, site)] => ((*service_id).clone(), *site),
        _ => {
            return Err(crate::Error::invalid(format!(
                "producer phase {:?} has {} enqueue events for selector {:#x}, expected one",
                binding.producer_phase,
                enqueues.len(),
                route.selector_value
            )));
        }
    };
    let dequeue_site = match dequeues.as_slice() {
        [(dequeue_service, site)] if *dequeue_service == &service_id => *site,
        [(dequeue_service, _)] => {
            return Err(crate::Error::invalid(format!(
                "selector {:#x} was enqueued through {:?} but dequeued through {:?}",
                route.selector_value, service_id, dequeue_service
            )));
        }
        _ => {
            return Err(crate::Error::invalid(format!(
                "consumer phase {:?} has {} dequeue events for selector {:#x}, expected one",
                binding.consumer_phase,
                dequeues.len(),
                route.selector_value
            )));
        }
    };

    let handler_site = if let Some(handler) = &route.case_handler {
        let expected = identity_symbol(&handler.function);
        match &consumer.completion {
            artifacts::StoredReplayCompletion::GoalReached {
                goal: execution_model::ExecutionGoal::ReachSymbol { symbol },
            } if symbol == expected => {}
            _ => {
                return Err(crate::Error::invalid(format!(
                    "consumer phase {:?} did not complete by reaching handler {:?}",
                    binding.consumer_phase, expected
                )));
            }
        }
        Some(
            consumer
                .calls
                .iter()
                .find(|call| call.symbol == expected)
                .map(|call| call.site)
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "consumer phase {:?} has no ordered call to reached handler {:?}",
                        binding.consumer_phase, expected
                    ))
                })?,
        )
    } else {
        None
    };

    let state = load_replay_state(binding, producer, consumer)?;

    Ok(Some(ReplayRouteProof {
        service_id,
        producer_symbol: producer.symbol.clone(),
        consumer_symbol: consumer.symbol.clone(),
        enqueue_site,
        dequeue_site,
        handler_site,
        state,
    }))
}

fn load_replay_state(
    binding: &crate::function_workspace::ReviewedEventReplay,
    producer: &artifacts::StoredReplayPhase,
    consumer: &artifacts::StoredReplayPhase,
) -> Result<ReplayStateProof> {
    let producer_matches = producer
        .memory_observations
        .iter()
        .filter(|observation| observation.id == binding.state_observation)
        .collect::<Vec<_>>();
    let consumer_matches = consumer
        .memory_observations
        .iter()
        .filter(|observation| observation.id == binding.state_observation)
        .collect::<Vec<_>>();
    let [producer_state] = producer_matches.as_slice() else {
        return Err(crate::Error::invalid(format!(
            "replay producer phase {:?} has {} observations named {:?}, expected one",
            binding.producer_phase,
            producer_matches.len(),
            binding.state_observation
        )));
    };
    let [consumer_state] = consumer_matches.as_slice() else {
        return Err(crate::Error::invalid(format!(
            "replay consumer phase {:?} has {} observations named {:?}, expected one",
            binding.consumer_phase,
            consumer_matches.len(),
            binding.state_observation
        )));
    };
    if (
        &producer_state.symbol,
        producer_state.address,
        producer_state.width,
    ) != (
        &consumer_state.symbol,
        consumer_state.address,
        consumer_state.width,
    ) {
        return Err(crate::Error::invalid(format!(
            "replay state observation {:?} changes identity between producer and consumer",
            binding.state_observation
        )));
    }
    let mask = match producer_state.width {
        8 => 0xff,
        16 => 0xffff,
        32 => u32::MAX,
        width => {
            return Err(crate::Error::invalid(format!(
                "replay state observation {:?} has unsupported width {width}",
                binding.state_observation
            )));
        }
    };
    match binding.state_model {
        crate::function_workspace::ReviewedEventStateModel::CountedLatch => {
            let incremented = producer_state.before.wrapping_add(1) & mask;
            let decremented = consumer_state.before.wrapping_sub(1) & mask;
            if producer_state.after != incremented
                || consumer_state.before != producer_state.after
                || consumer_state.after != decremented
            {
                return Err(crate::Error::invalid(format!(
                    "counted latch {:?} must increment in producer and decrement from the same state in consumer; observed {:#x}->{:#x}, then {:#x}->{:#x}",
                    binding.state_observation,
                    producer_state.before,
                    producer_state.after,
                    consumer_state.before,
                    consumer_state.after
                )));
            }
        }
    }
    let producer_write = producer_state.writes.last().ok_or_else(|| {
        crate::Error::invalid(format!(
            "replay producer phase has no exact write for state observation {:?}",
            binding.state_observation
        ))
    })?;
    let consumer_write = consumer_state.writes.last().ok_or_else(|| {
        crate::Error::invalid(format!(
            "replay consumer phase has no exact write for state observation {:?}",
            binding.state_observation
        ))
    })?;
    if producer_write.value != producer_state.after || consumer_write.value != consumer_state.after
    {
        return Err(crate::Error::invalid(format!(
            "replay state observation {:?} final write does not match its recorded after value",
            binding.state_observation
        )));
    }
    Ok(ReplayStateProof {
        id: binding.state_observation.clone(),
        symbol: producer_state.symbol.clone(),
        address: producer_state.address,
        width: producer_state.width,
        producer_before: producer_state.before,
        producer_after: producer_state.after,
        producer_write_site: producer_write.site,
        consumer_before: consumer_state.before,
        consumer_after: consumer_state.after,
        consumer_write_site: consumer_write.site,
    })
}

fn identity_symbol(identity: &str) -> &str {
    identity
        .split_once("::")
        .map_or(identity, |(_, remainder)| remainder)
        .rsplit_once("@0x")
        .map_or_else(
            || identity.split_once("::").map_or(identity, |(_, rest)| rest),
            |(symbol, _)| symbol,
        )
}
