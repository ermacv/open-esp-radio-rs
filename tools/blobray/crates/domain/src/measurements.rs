//! Fixed-size diagnostic counters. Measurements never participate in result identity.
use crate::*;
#[derive(Clone, Copy, Debug)]
pub enum WorkMetric {
    ArchiveEntries,
    ObjectReadBytes,
    ObjectHashBytes,
    ObjectsPrepared,
    SectionsPrepared,
    RelocationLookups,
    PublicationPasses,
    KnowledgeHistoryPasses,
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct WorkMeasurements {
    pub archive_entries: u64,
    pub object_read_bytes: u64,
    pub object_hash_bytes: u64,
    pub objects_prepared: u64,
    pub sections_prepared: u64,
    pub relocation_lookups: u64,
    pub publication_passes: u64,
    pub knowledge_history_passes: u64,
}
impl WorkMeasurements {
    pub fn add(&mut self, metric: WorkMetric, amount: u64) {
        let target = match metric {
            WorkMetric::ArchiveEntries => &mut self.archive_entries,
            WorkMetric::ObjectReadBytes => &mut self.object_read_bytes,
            WorkMetric::ObjectHashBytes => &mut self.object_hash_bytes,
            WorkMetric::ObjectsPrepared => &mut self.objects_prepared,
            WorkMetric::SectionsPrepared => &mut self.sections_prepared,
            WorkMetric::RelocationLookups => &mut self.relocation_lookups,
            WorkMetric::PublicationPasses => &mut self.publication_passes,
            WorkMetric::KnowledgeHistoryPasses => &mut self.knowledge_history_passes,
        };
        *target = target.saturating_add(amount);
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct PhaseCost {
    pub elapsed_ms: u64,
    pub work_units: u64,
}
macro_rules! phases {
    ($($field:ident : $variant:ident),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
        pub struct PhaseMeasurements { $(pub $field: PhaseCost),+ }
        impl PhaseMeasurements {
            pub fn add(&mut self, phase: RunPhase, elapsed: u64, work: u64) {
                let cost = match phase { $(RunPhase::$variant => &mut self.$field),+ };
                cost.elapsed_ms = cost.elapsed_ms.saturating_add(elapsed);
                cost.work_units = cost.work_units.saturating_add(work);
            }
        }
    };
}
phases! {
    starting: Starting, capture: Capture, read_captured: ReadCaptured, members: Members,
    elf: Elf, validate_revision: ValidateRevision, serialize: Serialize, retain: Retain,
    publish: Publish, execute: Execute, compare: Compare, materialize: Materialize,
    link: Link, validate_image: ValidateImage, analyze_function: AnalyzeFunction,
    analyze_values: AnalyzeValues, prepare_object: PrepareObject, prepare_section: PrepareSection,
    plan_investigation: PlanInvestigation, index_research: IndexResearch,
}
