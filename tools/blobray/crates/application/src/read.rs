//! Read capabilities never expose store handles or writer acquisition.
use crate::*;

/// Read-only project selection. Opening it never creates or repairs a project.
/// ```compile_fail
/// # fn denied(view: blobray_application::ReadView) {
/// view.writer();
/// # }
/// ```
/// ```compile_fail
/// # fn denied(view: blobray_application::ReadView) {
/// view.recover();
/// # }
/// ```
pub struct ReadView {
    project: blobray_store::Project,
}
impl ReadView {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            project: Project::open(path)?,
        })
    }
    pub fn current(&self) -> Result<Option<RevisionId>> {
        self.project.current()
    }
    pub fn inventory(
        &self,
        revision: &RevisionId,
        memory: &WorkingMemory,
        control: &mut dyn RunControl,
        sink: &mut dyn InventorySink,
    ) -> Result<InventoryView> {
        Ok(InventoryView {
            inner: self
                .project
                .read_inventory(Some(revision), memory, control, sink)?,
        })
    }
    pub fn doctor(
        &self,
        memory: &WorkingMemory,
        control: &mut dyn RunControl,
        sink: &mut dyn DoctorSink,
    ) -> Result<DoctorSummary> {
        self.project.doctor_stream(memory, control, sink)
    }
}
/// Verified manifest lease, not a future transitive GC pin.
/// ```compile_fail
/// # fn denied(view: blobray_application::InventoryView) {
/// view.writer();
/// # }
/// ```
pub struct InventoryView {
    inner: blobray_store::SnapshotView,
}
impl InventoryView {
    pub fn revision_id(&self) -> &RevisionId {
        &self.inner.revision_id
    }
    pub fn complete(&self) -> bool {
        self.inner.complete
    }
    /// The manifest borrow cannot outlive its verified file lease.
    /// ```compile_fail
    /// fn escape(view: blobray_application::InventoryView) -> &'static dyn blobray_domain::ByteSource {
    ///     view.manifest()
    /// }
    /// ```
    pub fn manifest(&self) -> &dyn ByteSource {
        self.inner.manifest()
    }
}
