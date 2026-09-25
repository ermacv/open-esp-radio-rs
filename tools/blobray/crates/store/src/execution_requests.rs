//! Canonical execution requests retained by identity.
//!
//! A request is stored once as a content-addressed payload. Run records,
//! worker messages and execution manifests carry only its identity, so their
//! control-message bounds do not bound the size of an execution matrix.
use super::*;
use std::io::Write;

/// Working memory admitted per encoded request byte. Requests are dominated by
/// byte arrays whose decoded form is smaller than their JSON encoding.
const REQUEST_DECODE_EXPANSION: u64 = 8;

/// Canonical bytes of a validated request and their identity.
pub fn encode_execution_request(request: &ExecutionRequest) -> Result<(ArtifactId, Vec<u8>)> {
    request.validate()?;
    let bytes = serde_json::to_vec(request).map_err(jobs::json)?;
    if bytes.len() > MAX_EXECUTION_REQUEST_BYTES {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "execution request exceeds its size bound",
        ));
    }
    Ok((ArtifactId::of_bytes(&bytes), bytes))
}

/// Decode a retained request whose bytes were verified against its identity.
pub fn decode_execution_request(
    source: &dyn ByteSource,
    memory: &WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<ExecutionRequest> {
    let length = usize::try_from(source.len())
        .ok()
        .filter(|n| *n <= MAX_EXECUTION_REQUEST_BYTES)
        .ok_or_else(|| integrity("execution request exceeds its size bound"))?;
    // Decoded requests are bounded by a small multiple of their encoding.
    let _capacity = memory.reserve(
        (length as u64).saturating_mul(REQUEST_DECODE_EXPANSION) + WORK_BLOCK as u64,
        c.position(),
    )?;
    let mut bytes = memory.bytes(length, c.position())?;
    source.read_at(0, &mut bytes, c)?;
    let request: ExecutionRequest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
    if request.schema != EXECUTION_SCHEMA {
        return Err(Error::new(
            ErrorCode::Incompatible,
            "unsupported execution request",
        ));
    }
    request.validate()?;
    Ok(request)
}

impl Staging {
    /// Retain exact bytes produced by this operation and return their identity.
    pub fn retain_bytes(&self, bytes: &[u8], c: &mut dyn RunControl) -> Result<ArtifactId> {
        let mut file = self.disk.temporary(&self.root.join("staging"))?;
        for chunk in bytes.chunks(WORK_BLOCK) {
            c.bytes(chunk.len())?;
            file.write_all(chunk).map_err(io)?;
        }
        self.retain_temporary(file, c)
    }
    pub(crate) fn has_payload(&self, id: &ArtifactId) -> bool {
        fs::symlink_metadata(self.object_path(id)).is_ok_and(|m| m.is_file())
    }
    /// The request of this operation: staged at admission or, for a replay,
    /// already retained by the project under the same identity.
    pub fn execution_request(
        &self,
        project: &Project,
        id: &ArtifactId,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionRequest> {
        let source = if self.has_payload(id) {
            self.open_payload(id, c)?
        } else {
            project.open_payload(id, c)?
        };
        decode_execution_request(&source, memory, c)
    }
}

impl Project {
    /// A retained execution request, verified against its identity.
    pub fn execution_request(
        &self,
        id: &ArtifactId,
        memory: &WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionRequest> {
        decode_execution_request(&self.open_payload(id, c)?, memory, c)
    }
}
