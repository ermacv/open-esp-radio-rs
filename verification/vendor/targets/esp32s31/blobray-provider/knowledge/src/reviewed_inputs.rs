//! Exact caller-owned input domains recovered for the current radio artifact.

use crate::*;

const DTM_EVENT_SYMBOL: &str = "r_sym_ble_G4zC4UNjJYmyjOsZ3vNq";
const DTM_EVENT_ADDRESS: u64 = 0x1003_4b1c;
const DTM_EVENT_SIZE: usize = 550;
const DTM_CHANNEL_DOMAIN: ReviewedMemoryValueDomain =
    ReviewedMemoryValueDomain::inclusive("esp32s31-ble-dtm-channel-0-through-39", 0, 39);

pub(super) fn caller_memory_input_domain(
    symbol: &artifact::ArtifactSymbolDefinition,
    location: &MemoryObjectLocation,
    width: u8,
) -> Option<ReviewedMemoryValueDomain> {
    let exact_body = symbol.name == DTM_EVENT_SYMBOL
        && symbol.address == DTM_EVENT_ADDRESS
        && symbol.bytes.len() == DTM_EVENT_SIZE;
    let exact_field = matches!(
        location,
        MemoryObjectLocation {
            root: MemoryObjectRoot::Argument { index: 0 },
            offset: 0x0e,
        }
    );
    (exact_body && exact_field && width == 8).then_some(DTM_CHANNEL_DOMAIN)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn dtm_event() -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: None,
            name: DTM_EVENT_SYMBOL.to_owned(),
            address: DTM_EVENT_ADDRESS,
            bytes: vec![0; DTM_EVENT_SIZE],
            addresses_resolved: true,
            memory_regions: Arc::default(),
            relocations: Vec::new(),
        }
    }

    #[test]
    fn dtm_channel_domain_requires_the_exact_current_caller_field() {
        let channel = MemoryObjectLocation {
            root: MemoryObjectRoot::Argument { index: 0 },
            offset: 0x0e,
        };
        let other_field = MemoryObjectLocation {
            root: MemoryObjectRoot::Argument { index: 0 },
            offset: 0x0f,
        };

        assert_eq!(
            caller_memory_input_domain(&dtm_event(), &channel, 8)
                .map(ReviewedMemoryValueDomain::id),
            Some("esp32s31-ble-dtm-channel-0-through-39")
        );
        assert!(caller_memory_input_domain(&dtm_event(), &other_field, 8).is_none());
        assert!(caller_memory_input_domain(&dtm_event(), &channel, 16).is_none());
    }
}
