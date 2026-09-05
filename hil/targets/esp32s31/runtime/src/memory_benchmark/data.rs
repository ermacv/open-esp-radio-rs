//! Source conditioning, disjoint packet geometry and complete validation.

use open_esp_radio_hil_protocol::{MemoryBenchmarkRequest, MemoryBenchmarkStop};

const GUARD: u8 = 0xa5;
pub(super) const OFFSET: usize = 36;
const ALIGNMENT: usize = 64;
pub(super) const MAX_FRAMES: usize = 32;
// Payload budget plus a prefix and worst-case cache-line rounding per frame.
// These are diagnostic allocation bounds, not silicon memory limits.
pub(super) const ARENA_CAPACITY: usize = 49152 + MAX_FRAMES * (OFFSET + ALIGNMENT);

#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub(super) bytes: usize,
    pub(super) frames: usize,
    pub(super) stride: usize,
}

impl Layout {
    pub(super) fn new(request: MemoryBenchmarkRequest) -> Option<Self> {
        if !request.validate() {
            return None;
        }
        let bytes = usize::from(request.bytes);
        let frames = usize::from(request.frames);
        // At least one trailing guard byte, followed by line rounding. Adjacent
        // frames never share a source cache line or a destination guard.
        let stride = (OFFSET + bytes + 1).next_multiple_of(ALIGNMENT);
        (stride * frames <= ARENA_CAPACITY).then_some(Self {
            bytes,
            frames,
            stride,
        })
    }

    pub(super) fn prepare(self, source: &mut [u8], destination: &mut [u8], iteration: u16) {
        destination.fill(GUARD);
        for (frame, (source, destination)) in source
            .chunks_mut(self.stride)
            .zip(destination.chunks_mut(self.stride))
            .take(self.frames)
            .enumerate()
        {
            fill_source(&mut source[..self.bytes], frame as u8, iteration);
            // Poison every payload byte independently of the guard pattern.
            // Even a one-byte frame equal to GUARD must detect a skipped copy.
            for (destination, source) in destination[OFFSET..OFFSET + self.bytes]
                .iter_mut()
                .zip(&source[..self.bytes])
            {
                *destination = !source;
            }
        }
    }

    pub(super) fn copy(self, source: &[u8], destination: &mut [u8]) {
        for (source, destination) in source
            .chunks(self.stride)
            .zip(destination.chunks_mut(self.stride))
            .take(self.frames)
        {
            destination[OFFSET..OFFSET + self.bytes].copy_from_slice(&source[..self.bytes]);
        }
    }

    pub(super) fn verify(
        self,
        source: &[u8],
        destination: &[u8],
    ) -> Result<(), MemoryBenchmarkStop> {
        for (source, destination) in source
            .chunks(self.stride)
            .zip(destination.chunks(self.stride))
            .take(self.frames)
        {
            verify(&source[..self.bytes], destination, OFFSET)?;
        }
        if destination[self.frames * self.stride..]
            .iter()
            .any(|byte| *byte != GUARD)
        {
            return Err(MemoryBenchmarkStop::GuardCorrupted);
        }
        Ok(())
    }
}

fn fill_source(source: &mut [u8], frame: u8, iteration: u16) {
    for (index, byte) in source.iter_mut().enumerate() {
        *byte = (index as u8)
            .wrapping_mul(37)
            .wrapping_add(((index >> 8) as u8).wrapping_mul(17))
            .wrapping_add(frame.wrapping_mul(71))
            .wrapping_add(iteration as u8);
    }
}

pub(super) fn verify(
    source: &[u8],
    destination: &[u8],
    offset: usize,
) -> Result<(), MemoryBenchmarkStop> {
    let end = offset + source.len();
    if destination[offset..end] != *source {
        return Err(MemoryBenchmarkStop::DataMismatch);
    }
    if destination[..offset]
        .iter()
        .chain(&destination[end..])
        .any(|byte| *byte != GUARD)
    {
        return Err(MemoryBenchmarkStop::GuardCorrupted);
    }
    Ok(())
}
