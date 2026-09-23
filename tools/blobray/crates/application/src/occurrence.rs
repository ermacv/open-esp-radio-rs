//! Shared physical acquisition for knowledge admission and data delivery.
//! Captures are borrowed within one callback; claim semantics remain with callers.
use crate::*;
fn invalid(message: &str) -> Error {
    Error::new(ErrorCode::InvalidRequest, message)
}
pub(crate) struct CapturedObject<'a> {
    pub payload: &'a ArtifactId,
    pub bytes: &'a dyn ByteSource,
    /// Thin-member bytes have their own retained CAS root.
    pub detached: bool,
    occurrence: &'a KnowledgeOccurrence,
}
impl CapturedObject<'_> {
    pub fn with_prepared<T>(
        &self,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
        consume: impl FnOnce(
            &mut blobray_artifacts::PreparedObject<'_, '_>,
            &mut dyn RunControl,
        ) -> Result<T>,
    ) -> Result<T> {
        blobray_artifacts::with_prepared_object(self.bytes, self.payload, memory, c, |object, c| {
            if let Some(symbol) = &self.occurrence.symbol {
                object.validate_data_symbol(&self.occurrence.object, symbol)?;
            }
            consume(object, c)
        })
    }
}
pub(crate) fn with_source<T>(
    project: &Project,
    occurrence: &KnowledgeOccurrence,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
    consume: impl FnOnce(CapturedObject<'_>, &mut dyn RunControl) -> Result<T>,
) -> Result<T> {
    let payload = match &occurrence.source {
        FunctionSource::Input { input } => {
            let scope = if let Some(symbol) = &occurrence.symbol {
                InspectionScope::Symbol {
                    input: *input,
                    symbol: symbol.clone(),
                }
            } else {
                InspectionScope::Object {
                    input: *input,
                    object: occurrence.object.clone(),
                }
            };
            let mut probe = crate::selection::Probe::new(&scope);
            project.read_inventory(Some(&occurrence.revision), memory, c, &mut probe)?;
            if !probe.found {
                return Err(Error::new(
                    ErrorCode::NotFound,
                    "occurrence absent from selected revision",
                ));
            }
            probe
                .binding
                .and_then(|b| b.payload)
                .ok_or_else(|| invalid("object bytes unavailable"))?
        }
        FunctionSource::Image { image } => {
            let image = project.image(image, c)?;
            if image.manifest.plan.recipe.revision != occurrence.revision
                || occurrence.object
                    != (ObjectId {
                        artifact: image.manifest.elf.clone(),
                        location: ObjectLocation::Standalone,
                    })
            {
                return Err(invalid("image and occurrence differ"));
            }
            image.manifest.elf
        }
    };
    let container = project.open_payload(&occurrence.object.artifact, c)?;
    let run = |bytes: &dyn ByteSource, detached, c: &mut dyn RunControl| {
        consume(
            CapturedObject {
                payload: &payload,
                bytes,
                detached,
                occurrence,
            },
            c,
        )
    };
    match occurrence.object.location {
        ObjectLocation::Standalone => run(&container, false, c),
        ObjectLocation::ArchiveMember { ordinal } => {
            let mut cursor = MemberCursor::new(&container, c)?;
            while let Some(member) = cursor.next(memory, c)? {
                if member.ordinal == ordinal {
                    return if let Some((offset, length)) = member.payload {
                        run(&SourceRange::new(&container, offset, length)?, false, c)
                    } else {
                        run(&project.open_payload(&payload, c)?, true, c)
                    };
                }
            }
            Err(invalid("captured member missing"))
        }
    }
}
