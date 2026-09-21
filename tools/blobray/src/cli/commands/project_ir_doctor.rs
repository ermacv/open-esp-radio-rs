//! CLI rendering for application-owned IR diagnostics.
use crate::application::project_ir_doctor::IrDoctorReport;
impl IrDoctorReport {
    pub(super) fn render_human(&self) {
        outputln!(
            "Linked IR: {} — profiles={} errors={} warnings={}",
            self.status,
            self.profiles.len(),
            self.errors,
            self.warnings
        );
        for obligation in &self.coverage {
            for issue in &obligation.issues {
                outputln!("  coverage {}: {}", obligation.id, issue);
            }
        }
        for profile in &self.profiles {
            outputln!(
                "  {:<20} roots={:<20} inputs={:<20} output={:<14} functions={} decode-blockers={} registers={} fields={}",
                profile.id,
                profile.symbol_prefix.as_ref().map_or_else(
                    || profile.roots.to_owned(),
                    |prefix| format!("{}:{prefix}", profile.roots),
                ),
                profile.input_status,
                profile.output_status,
                profile.functions,
                profile.decode_blockers,
                profile.registers,
                profile.field_candidates
            );
            for diagnostic in &profile.diagnostics {
                outputln!("    {}: {}", diagnostic.kind, diagnostic.error);
            }
        }
    }
}
