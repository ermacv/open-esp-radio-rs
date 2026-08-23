use open_esp_radio_hil_protocol::{RxDeliveryEvidence, RxSequenceStageEvidence};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RxDeliveryAssessment {
    pub(crate) host_to_post_reorder: bool,
    pub(crate) before_mac_sequence_ordering: bool,
    pub(crate) post_reorder_to_enqueue: bool,
    pub(crate) enqueue_to_consumer: bool,
}

impl RxDeliveryAssessment {
    pub(crate) const fn exact(self) -> bool {
        !self.host_to_post_reorder && !self.post_reorder_to_enqueue && !self.enqueue_to_consumer
    }

    pub(crate) fn frontier(self) -> &'static str {
        match (
            self.host_to_post_reorder,
            self.before_mac_sequence_ordering,
            self.post_reorder_to_enqueue,
            self.enqueue_to_consumer,
        ) {
            (false, false, false, false) => "exact",
            (true, true, false, false) => "before-802.11-sequence-assignment",
            (true, false, false, false) => "at-or-before-post-reorder",
            (false, false, true, false) => "network-enqueue",
            (false, false, false, true) => "network-to-udp-consumer",
            _ => "multiple-frontiers",
        }
    }
}

pub(crate) fn assess(host_units: u64, evidence: RxDeliveryEvidence) -> RxDeliveryAssessment {
    let host_to_post_reorder = !stage_matches_host(evidence.post_reorder, host_units);
    let post = evidence.post_reorder;
    let mac = evidence.mac_order;
    let before_mac_sequence_ordering = host_to_post_reorder
        && stage_has_exact_cardinality(post, host_units)
        && post.forward_missing == post.late_recovered
        && post.late_recovered != 0
        && mac.backward_mac_forward == post.late_recovered
        && mac.backward_mac_backward == 0
        && mac.backward_mac_same == 0
        && mac.backward_mac_other_tid == 0
        && mac.backward_mac_unavailable == 0;
    let post_reorder_to_enqueue = evidence.network_queue_full != 0
        || evidence.network_invalid_length != 0
        || !same_sequence_stream(evidence.post_reorder, evidence.network_enqueued);
    let ledger = evidence.consumer_ledger;
    let enqueue_to_consumer = ledger.overflow != 0
        || ledger.enqueued_not_consumed != 0
        || ledger.skipped_before_observed != 0
        || ledger.unexpected_consumer != 0
        || u64::from(ledger.matched) != u64::from(evidence.network_enqueued.data_units)
        || !same_sequence_stream(evidence.network_enqueued, evidence.udp_consumer);
    RxDeliveryAssessment {
        host_to_post_reorder,
        before_mac_sequence_ordering,
        post_reorder_to_enqueue,
        enqueue_to_consumer,
    }
}

fn stage_has_exact_cardinality(stage: RxSequenceStageEvidence, host_units: u64) -> bool {
    let expected_highest = host_units
        .checked_sub(1)
        .and_then(|value| u32::try_from(value).ok());
    u64::from(stage.data_units) == host_units
        && stage.first == (host_units != 0).then_some(0)
        && stage.highest == expected_highest
        && stage.duplicates == 0
        && stage.backward_unclassified == 0
        && stage.data_after_terminal == 0
}

pub(crate) fn markdown(host_units: u64, evidence: RxDeliveryEvidence) -> String {
    let assessment = assess(host_units, evidence);
    let post = evidence.post_reorder;
    let enqueued = evidence.network_enqueued;
    let consumer = evidence.udp_consumer;
    let ledger = evidence.consumer_ledger;
    format!(
        "## Typed RX delivery frontier\n\n\
         - Classification: `{}`; host→post-reorder defect: `{}`; before-MAC-sequence ordering: `{}`; post-reorder→enqueue defect: `{}`; enqueue→consumer defect: `{}`\n\
         - Data units host / post-reorder / enqueued / UDP consumer: `{}` / `{}` / `{}` / `{}`\n\
         - Post-reorder gap/missing/late/duplicate/backward: `{}` / `{}` / `{}` / `{}` / `{}`; first anomaly: `{}`\n\
         - Network queue-full/invalid-length: `{}` / `{}`\n\
         - Ledger matched/pending/skipped/unexpected/overflow: `{}` / `{}` / `{}` / `{}` / `{}`; first expected/observed: `{}` / `{}`\n\
         - Reorder ingress/retries/direct then buffered/released/missing/stale/expiries/discarded/max occupied: `{}` / `{}` / `{}` then `{}` / `{}` / `{}` / `{}` / `{}` / `{}` / `{}`\n\
         - Post-reorder backward UDP with MAC backward/same/forward/other-TID/unavailable: `{}` / `{}` / `{}` / `{}` / `{}`\n\
         - Control markers post-reorder/enqueue/consumer and data after terminal: `{}/{}/{}` and `{}/{}/{}`\n\n",
        assessment.frontier(),
        assessment.host_to_post_reorder,
        assessment.before_mac_sequence_ordering,
        assessment.post_reorder_to_enqueue,
        assessment.enqueue_to_consumer,
        host_units,
        post.data_units,
        enqueued.data_units,
        consumer.data_units,
        post.gap_events,
        post.forward_missing,
        post.late_recovered,
        post.duplicates,
        post.backward_unclassified,
        display_option(post.first_anomaly),
        evidence.network_queue_full,
        evidence.network_invalid_length,
        ledger.matched,
        ledger.enqueued_not_consumed,
        ledger.skipped_before_observed,
        ledger.unexpected_consumer,
        ledger.overflow,
        display_option(ledger.first_expected),
        display_option(ledger.first_observed),
        evidence.reorder.ingress,
        evidence.reorder.ingress_retries,
        evidence.reorder.direct,
        evidence.reorder.buffered,
        evidence.reorder.released,
        evidence.reorder.missing,
        evidence.reorder.stale,
        evidence.reorder.gap_expiries,
        evidence.reorder.discarded,
        evidence.reorder.maximum_occupied,
        evidence.mac_order.backward_mac_backward,
        evidence.mac_order.backward_mac_same,
        evidence.mac_order.backward_mac_forward,
        evidence.mac_order.backward_mac_other_tid,
        evidence.mac_order.backward_mac_unavailable,
        post.control_markers,
        enqueued.control_markers,
        consumer.control_markers,
        post.data_after_terminal,
        enqueued.data_after_terminal,
        consumer.data_after_terminal,
    )
}

fn stage_matches_host(stage: RxSequenceStageEvidence, host_units: u64) -> bool {
    let expected_highest = host_units
        .checked_sub(1)
        .and_then(|value| u32::try_from(value).ok());
    u64::from(stage.data_units) == host_units
        && stage.first == (host_units != 0).then_some(0)
        && stage.highest == expected_highest
        && stage.gap_events == 0
        && stage.forward_missing == 0
        && stage.late_recovered == 0
        && stage.duplicates == 0
        && stage.backward_unclassified == 0
        && stage.data_after_terminal == 0
}

fn same_sequence_stream(left: RxSequenceStageEvidence, right: RxSequenceStageEvidence) -> bool {
    left == right
}

fn display_option(value: Option<u32>) -> String {
    value.map_or_else(|| String::from("none"), |value| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use open_esp_radio_hil_protocol::{RxConsumerLedgerEvidence, RxReorderDeliveryEvidence};

    fn exact() -> RxDeliveryEvidence {
        let stage = RxSequenceStageEvidence {
            data_units: 3,
            first: Some(0),
            highest: Some(2),
            control_markers: 1,
            ..Default::default()
        };
        RxDeliveryEvidence {
            post_reorder: stage,
            network_enqueued: stage,
            udp_consumer: stage,
            consumer_ledger: RxConsumerLedgerEvidence {
                matched: 3,
                ..Default::default()
            },
            reorder: RxReorderDeliveryEvidence {
                released: 3,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn classifies_each_delivery_edge_independently() {
        assert!(assess(3, exact()).exact());

        let mut post = exact();
        post.post_reorder.data_units = 2;
        post.network_enqueued = post.post_reorder;
        post.udp_consumer = post.post_reorder;
        post.consumer_ledger.matched = 2;
        assert_eq!(assess(3, post).frontier(), "at-or-before-post-reorder");

        let mut enqueue = exact();
        enqueue.network_queue_full = 1;
        enqueue.network_enqueued.data_units = 2;
        enqueue.udp_consumer = enqueue.network_enqueued;
        enqueue.consumer_ledger.matched = 2;
        assert_eq!(assess(3, enqueue).frontier(), "network-enqueue");

        let mut consumer = exact();
        consumer.consumer_ledger.enqueued_not_consumed = 1;
        consumer.udp_consumer.data_units = 2;
        consumer.consumer_ledger.matched = 2;
        assert_eq!(assess(3, consumer).frontier(), "network-to-udp-consumer");
    }

    #[test]
    fn forward_mac_sequence_localizes_udp_reordering_before_the_target_mac() {
        let mut evidence = exact();
        evidence.post_reorder.gap_events = 1;
        evidence.post_reorder.forward_missing = 1;
        evidence.post_reorder.late_recovered = 1;
        evidence.post_reorder.first_anomaly = Some(2);
        evidence.network_enqueued = evidence.post_reorder;
        evidence.udp_consumer = evidence.post_reorder;
        evidence.mac_order.backward_mac_forward = 1;

        assert_eq!(
            assess(3, evidence).frontier(),
            "before-802.11-sequence-assignment"
        );
    }
}
