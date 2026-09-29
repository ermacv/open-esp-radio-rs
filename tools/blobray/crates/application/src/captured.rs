//! Captured executables read in process, identified by content: their
//! container members and bytes.
use crate::in_process::Executable;
use crate::*;

/// One object of a captured executable, borrowed for a visit.
pub(crate) struct Member<'a> {
    pub id: ObjectId,
    /// The object's bytes, or why the container holds none: a thin archive
    /// member names an external file, and a member range may be malformed.
    pub bytes: std::result::Result<&'a dyn ByteSource, Diagnostic>,
}

/// Container framing of a captured executable.
pub(crate) struct Container {
    pub kind: ContainerKind,
    /// Framing that stops member enumeration; later members are unknown.
    pub framing: Option<Diagnostic>,
}

type MemberVisit<'v> = dyn FnMut(Member<'_>, &mut dyn RunControl) -> Result<()> + 'v;

/// Present every object of `executable` to `visit` in container order.
pub(crate) fn visit_members(
    executable: &Executable,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
    visit: &mut MemberVisit<'_>,
) -> Result<Container> {
    let source: &[u8] = executable.bytes();
    let mut cursor = match MemberCursor::new(&source, control) {
        Ok(cursor) => cursor,
        Err(error) if error.code == ErrorCode::Integrity => {
            return Ok(Container {
                kind: if source.starts_with(b"!<thin>\n") {
                    ContainerKind::ThinArchive
                } else {
                    ContainerKind::Archive
                },
                framing: Some(Diagnostic {
                    code: DiagnosticCode::MalformedContainer,
                    context: "archive header; membership unknown".into(),
                    message: error.message,
                }),
            });
        }
        Err(error) => return Err(error),
    };
    let kind = cursor.kind;
    loop {
        let member = match cursor.next(memory, control) {
            Ok(Some(member)) => member,
            Ok(None) => break,
            Err(error) if error.code == ErrorCode::Integrity => {
                return Ok(Container {
                    kind,
                    framing: Some(Diagnostic {
                        code: DiagnosticCode::MalformedContainer,
                        context: "archive member; remaining membership unknown".into(),
                        message: error.message,
                    }),
                });
            }
            Err(error) => return Err(error),
        };
        let mut position = control.position();
        position.member = Some(member.ordinal);
        position.table = None;
        position.entry = None;
        control.set_position(position);
        let id = ObjectId {
            artifact: executable.id().clone(),
            location: match kind {
                ContainerKind::Archive | ContainerKind::ThinArchive => {
                    ObjectLocation::ArchiveMember {
                        ordinal: member.ordinal,
                    }
                }
                _ => ObjectLocation::Standalone,
            },
        };
        match member.payload {
            Some((offset, length)) => match SourceRange::new(&source, offset, length) {
                Ok(range) => visit(
                    Member {
                        id,
                        bytes: Ok(&range),
                    },
                    control,
                )?,
                Err(error) => visit(
                    Member {
                        id,
                        bytes: Err(Diagnostic {
                            code: DiagnosticCode::MalformedObject,
                            context: "member payload".into(),
                            message: error.message,
                        }),
                    },
                    control,
                )?,
            },
            None => visit(
                Member {
                    id,
                    bytes: Err(Diagnostic {
                        code: DiagnosticCode::MissingMember,
                        context: "thin member".into(),
                        message: "a thin archive names its member files; give the members \
                                  themselves"
                            .into(),
                    }),
                },
                control,
            )?,
        }
    }
    Ok(Container {
        kind,
        framing: None,
    })
}
