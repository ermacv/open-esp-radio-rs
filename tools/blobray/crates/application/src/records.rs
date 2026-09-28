//! Retained function records loaded into admitted working memory.
use crate::*;

pub(crate) fn load_records<'a>(
    source: &dyn ByteSource,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<RecordBuffer<'a>> {
    c.phase(RunPhase::LoadResearch)?;
    // Covers decoding before ownership transfers to the admitted record buffer.
    let _decode = memory.reserve(1024 * 1024, c.position())?;
    let mut records = RecordBuffer::new(memory);
    blobray_store::visit_jsonl(source, c, |r, c| {
        c.checkpoint(1)?;
        records.push(r, c.position())?;
        Ok(())
    })?;
    Ok(records)
}
