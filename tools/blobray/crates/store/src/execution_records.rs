//! Retained encoding of execution records: the logical JSONL stream of
//! `ExecutionEvidence`, one record per line, stored as a raw deflate stream.
//! Guest events repeat heavily, so the retained payload is a small fraction
//! of the logical stream; readers see exactly the logical records.
use super::*;
use flate2::{Compression, Decompress, FlushDecompress, Status, write::DeflateEncoder};
use std::io::Write;

/// Deflating writer of the retained execution record stream.
pub struct ExecutionRecordWriter<W: Write> {
    encoder: DeflateEncoder<W>,
}
impl<W: Write> ExecutionRecordWriter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            encoder: DeflateEncoder::new(writer, Compression::fast()),
        }
    }
    /// Complete the deflate stream and return the underlying writer.
    pub fn finish(self) -> std::io::Result<W> {
        self.encoder.finish()
    }
}
impl<W: Write> Write for ExecutionRecordWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.encoder.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.encoder.flush()
    }
}

/// Pull reader of retained execution records with the line bounds of
/// `JsonlCursor`. Every compressed block read and every decoded block is a
/// checkpoint, so inflation is bounded by the operation's work budget.
struct RecordCursor<'a> {
    source: &'a dyn ByteSource,
    offset: u64,
    input: Vec<u8>,
    input_start: usize,
    input_end: usize,
    output: Vec<u8>,
    output_start: usize,
    output_end: usize,
    inflater: Decompress,
    finished: bool,
    line: Vec<u8>,
}
impl<'a> RecordCursor<'a> {
    fn new(source: &'a dyn ByteSource) -> Result<Self> {
        let buffer = |size| {
            let mut bytes = Vec::new();
            bytes.try_reserve_exact(size).map_err(|_| {
                Error::new(
                    ErrorCode::ResourceLimited,
                    "record buffer allocation failed",
                )
            })?;
            bytes.resize(size, 0);
            Ok::<_, Error>(bytes)
        };
        Ok(Self {
            source,
            offset: 0,
            input: buffer(WORK_BLOCK)?,
            input_start: 0,
            input_end: 0,
            output: buffer(WORK_BLOCK)?,
            output_start: 0,
            output_end: 0,
            inflater: Decompress::new(false),
            finished: false,
            line: {
                let mut line = Vec::new();
                line.try_reserve_exact(CONTROL_MESSAGE_BYTES).map_err(|_| {
                    Error::new(
                        ErrorCode::ResourceLimited,
                        "record buffer allocation failed",
                    )
                })?;
                line
            },
        })
    }
    /// Refill decoded bytes; false at the verified end of the stream.
    fn fill(&mut self, c: &mut dyn RunControl) -> Result<bool> {
        while self.output_start == self.output_end {
            if self.finished {
                return Ok(false);
            }
            if self.input_start == self.input_end && self.offset < self.source.len() {
                c.checkpoint(1)?;
                self.input_end = (self.source.len() - self.offset).min(WORK_BLOCK as u64) as usize;
                self.source
                    .read_at(self.offset, &mut self.input[..self.input_end], c)?;
                self.offset += self.input_end as u64;
                self.input_start = 0;
            }
            c.checkpoint(1)?;
            let (read, written) = (self.inflater.total_in(), self.inflater.total_out());
            let status = self
                .inflater
                .decompress(
                    &self.input[self.input_start..self.input_end],
                    &mut self.output,
                    FlushDecompress::None,
                )
                .map_err(|_| integrity("execution records are not a deflate stream"))?;
            self.input_start += (self.inflater.total_in() - read) as usize;
            self.output_start = 0;
            self.output_end = (self.inflater.total_out() - written) as usize;
            match status {
                Status::StreamEnd => {
                    if self.input_start != self.input_end || self.offset != self.source.len() {
                        return Err(integrity("execution records have trailing bytes"));
                    }
                    self.finished = true;
                }
                Status::Ok | Status::BufError
                    if self.output_end == 0
                        && self.input_start == self.input_end
                        && self.offset == self.source.len() =>
                {
                    return Err(integrity("execution records are truncated"));
                }
                Status::Ok | Status::BufError => {}
            }
        }
        Ok(true)
    }
    fn next<T: serde::de::DeserializeOwned>(
        &mut self,
        c: &mut dyn RunControl,
    ) -> Result<Option<T>> {
        self.line.clear();
        loop {
            if self.output_start == self.output_end && !self.fill(c)? {
                return if self.line.is_empty() {
                    Ok(None)
                } else {
                    serde_json::from_slice(&self.line)
                        .map(Some)
                        .map_err(jobs::json)
                };
            }
            let available = &self.output[self.output_start..self.output_end];
            if let Some(end) = available.iter().position(|b| *b == b'\n') {
                if self.line.len() + end > CONTROL_MESSAGE_BYTES - 1 {
                    return Err(integrity("JSONL record exceeds 64 KiB"));
                }
                self.line.extend_from_slice(&available[..end]);
                self.output_start += end + 1;
                return serde_json::from_slice(&self.line)
                    .map(Some)
                    .map_err(jobs::json);
            }
            if self.line.len() + available.len() > CONTROL_MESSAGE_BYTES - 1 {
                return Err(integrity("JSONL record exceeds 64 KiB"));
            }
            self.line.extend_from_slice(available);
            self.output_start = self.output_end;
        }
    }
}

/// Decode every retained execution record in order.
pub(crate) fn visit_execution_records(
    source: &dyn ByteSource,
    c: &mut dyn RunControl,
    mut sink: impl FnMut(ExecutionEvidence, &mut dyn RunControl) -> Result<()>,
) -> Result<()> {
    let mut cursor = RecordCursor::new(source)?;
    while let Some(record) = cursor.next(c)? {
        sink(record, c)?;
    }
    Ok(())
}

/// Retained encoding of a logical JSONL record stream, for tests that forge
/// records.
#[cfg(test)]
pub(crate) fn encode_records(jsonl: &[u8]) -> Vec<u8> {
    let mut writer = ExecutionRecordWriter::new(Vec::new());
    writer.write_all(jsonl).unwrap();
    writer.finish().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(case: u32) -> ExecutionEvidence {
        ExecutionEvidence::Outcome {
            case,
            replacement: false,
            stop: ExecutionStop::Returned {
                low: Some(case),
                high: Some(0),
            },
            steps: u64::from(case),
        }
    }

    fn decode(bytes: &[u8]) -> Result<Vec<ExecutionEvidence>> {
        let mut records = vec![];
        visit_execution_records(&bytes, &mut || Ok(()), |r, _| {
            records.push(r);
            Ok(())
        })?;
        Ok(records)
    }

    #[test]
    fn records_round_trip_across_blocks_and_reject_damage() {
        let records: Vec<_> = (0..20_000).map(outcome).collect();
        let mut jsonl = vec![];
        for record in &records {
            serde_json::to_writer(&mut jsonl, record).unwrap();
            jsonl.push(b'\n');
        }
        let encoded = encode_records(&jsonl);
        assert!(encoded.len() * 4 < jsonl.len());
        assert_eq!(decode(&encoded).unwrap(), records);
        assert!(decode(&encoded[..encoded.len() - 1]).is_err());
        let mut trailing = encoded.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        assert!(decode(&jsonl).is_err());
        let long = vec![b' '; CONTROL_MESSAGE_BYTES];
        assert!(decode(&encode_records(&long)).is_err());
    }
}
