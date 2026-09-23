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

    pub(in crate::tui) fn request_register_detail(&mut self) -> Option<String> {
        if self.section != Section::Registers {
            return None;
        }
        let subject = self
            .snapshot
            .registers
            .register_at(self.selected())?
            .id
            .clone();
        self.requested_register_details
            .insert(subject.clone())
            .then_some(subject)
    }

    pub(in crate::tui) fn register_detail(&self, subject: &str) -> Option<&RegisterDetailSummary> {
        self.register_details.get(subject).map(Box::as_ref)
    }

    pub(in crate::tui) fn register_detail_finished(
        &mut self,
        subject: String,
        generation: u64,
        detail: Option<RegisterDetailSummary>,
    ) {
        if generation != self.snapshot.generation {
            self.requested_register_details.remove(&subject);
            self.message = Some("Register detail belongs to an earlier workspace generation; requesting current evidence".to_owned());
            return;
        }
        if let Some(detail) = detail {
            if self
                .snapshot
                .registers
                .inventory
                .snapshot()
                .is_none_or(|snapshot| snapshot.id() != detail.inventory_snapshot)
            {
                self.requested_register_details.remove(&subject);
                self.message = Some("Register detail belongs to a different inventory snapshot; requesting current evidence".to_owned());
                return;
            }
            self.register_details.insert(subject, Box::new(detail));
        }
    }
}
