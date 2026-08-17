//! Lazy detail request and cache state.

use crate::{FunctionDetailSummary, RegisterDetailSummary};

use super::{BrowserState, Section};

impl BrowserState {
    pub(in crate::tui) fn request_function_detail(&mut self) -> Option<String> {
        if self.section != Section::Functions {
            return None;
        }
        let identity = self
            .snapshot
            .functions
            .get(self.selected())?
            .identity
            .clone();
        self.requested_function_details
            .insert(identity.clone())
            .then_some(identity)
    }

    pub(in crate::tui) fn function_detail(&self, identity: &str) -> Option<&FunctionDetailSummary> {
        self.function_details.get(identity).map(Box::as_ref)
    }

    pub(in crate::tui) fn function_detail_finished(
        &mut self,
        identity: String,
        detail: Option<FunctionDetailSummary>,
    ) {
        if let Some(detail) = detail {
            self.function_details.insert(identity, Box::new(detail));
        }
    }

    pub(in crate::tui) fn request_register_detail(&mut self) -> Option<u32> {
        if self.section != Section::Registers {
            return None;
        }
        let address = self
            .snapshot
            .registers
            .registers
            .get(self.selected())?
            .address;
        self.requested_register_details
            .insert(address)
            .then_some(address)
    }

    pub(in crate::tui) fn register_detail(&self, address: u32) -> Option<&RegisterDetailSummary> {
        self.register_details.get(&address).map(Box::as_ref)
    }

    pub(in crate::tui) fn register_detail_finished(
        &mut self,
        address: u32,
        detail: Option<RegisterDetailSummary>,
    ) {
        if let Some(detail) = detail {
            self.register_details.insert(address, Box::new(detail));
        }
    }
}
