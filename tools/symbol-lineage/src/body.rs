//! Relocation-normalized function bodies.
//!
//! Relocatable RISC-V objects leave every relocated immediate unresolved, so
//! identical source produces identical bytes except inside those fields. A
//! body masks exactly the immediate bits named by each relocation type and
//! keeps opcodes, registers and the relocation shape. Target names are kept
//! separately: they change with obfuscation and must not affect identity.

use std::collections::HashSet;

use sha2::{Digest, Sha256};

const R_RISCV_BRANCH: u32 = 16;
const R_RISCV_JAL: u32 = 17;
const R_RISCV_CALL: u32 = 18;
const R_RISCV_CALL_PLT: u32 = 19;
const R_RISCV_GOT_HI20: u32 = 20;
const R_RISCV_TLS_GOT_HI20: u32 = 21;
const R_RISCV_TLS_GD_HI20: u32 = 22;
const R_RISCV_PCREL_HI20: u32 = 23;
const R_RISCV_PCREL_LO12_I: u32 = 24;
const R_RISCV_PCREL_LO12_S: u32 = 25;
const R_RISCV_HI20: u32 = 26;
const R_RISCV_LO12_I: u32 = 27;
const R_RISCV_LO12_S: u32 = 28;
const R_RISCV_ALIGN: u32 = 43;
const R_RISCV_RVC_BRANCH: u32 = 44;
const R_RISCV_RVC_JUMP: u32 = 45;
const R_RISCV_RVC_LUI: u32 = 46;
const R_RISCV_RELAX: u32 = 51;

const U_IMMEDIATE: u32 = 0xffff_f000;
const I_IMMEDIATE: u32 = 0xfff0_0000;
const SB_IMMEDIATE: u32 = 0xfe00_0f80;
const CB_IMMEDIATE: u16 = 0x1c7c;
const CJ_IMMEDIATE: u16 = 0x1ffc;
const CI_IMMEDIATE: u16 = 0x107c;

/// Similarity features use runs of this many 16-bit parcels.
const SHINGLE: usize = 4;

/// What one relocation refers to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Reference {
    /// A position inside the same function, relative to its start.
    Label(usize),
    /// A named symbol outside the function.
    Symbol(String),
    /// A section or unnamed symbol.
    Anonymous,
    /// A non-symbol target.
    Other,
}

/// One relocation inside a function.
#[derive(Clone, Debug)]
pub struct RelocationSite {
    /// Byte offset from the function start.
    pub offset: usize,
    /// ELF `r_type`.
    pub r_type: u32,
    /// Relocation target.
    pub reference: Reference,
}

/// A function body with name-independent identity and similarity features.
#[derive(Debug)]
pub struct Body {
    /// Masked body length in bytes.
    pub size: usize,
    /// Digest of the masked bytes and the name-free relocation shape.
    pub fingerprint: [u8; 32],
    /// Masked body as little-endian 16-bit parcels.
    pub parcels: Vec<u16>,
    /// Call targets in body order.
    pub calls: Vec<String>,
    /// Relocations in body order.
    pub relocations: Vec<RelocationSite>,
}

impl Body {
    /// Normalize `bytes` with the relocations that fall inside them.
    pub fn new(bytes: &[u8], mut relocations: Vec<RelocationSite>) -> Self {
        relocations.sort_by_key(|site| (site.offset, site.r_type));
        let mut masked = bytes.to_vec();
        let mut digest = Sha256::new();
        let mut calls = Vec::new();
        for site in &relocations {
            mask(&mut masked, site.offset, site.r_type);
            digest.update((site.offset as u64).to_le_bytes());
            digest.update(site.r_type.to_le_bytes());
            match &site.reference {
                Reference::Label(offset) => {
                    digest.update([0]);
                    digest.update((*offset as u64).to_le_bytes());
                }
                Reference::Symbol(name) => {
                    digest.update([1]);
                    if is_call(site.r_type) {
                        calls.push(name.clone());
                    }
                }
                Reference::Anonymous => digest.update([2]),
                Reference::Other => digest.update([3]),
            }
        }
        digest.update(&masked);
        let parcels = masked
            .chunks(2)
            .map(|pair| u16::from_le_bytes([pair[0], *pair.get(1).unwrap_or(&0)]))
            .collect();
        Self {
            size: bytes.len(),
            fingerprint: digest.finalize().into(),
            parcels,
            calls,
            relocations,
        }
    }

    /// Hashed parcel shingles for a cheap similarity prefilter.
    pub fn shingles(&self) -> HashSet<u64> {
        self.parcels
            .windows(SHINGLE)
            .map(|window| {
                window
                    .iter()
                    .fold(0xcbf2_9ce4_8422_2325_u64, |hash, parcel| {
                        (hash ^ u64::from(*parcel)).wrapping_mul(0x0000_0100_0000_01b3)
                    })
            })
            .collect()
    }
}

/// Whether the relocation type transfers control to its target.
pub fn is_call(r_type: u32) -> bool {
    matches!(r_type, R_RISCV_CALL | R_RISCV_CALL_PLT | R_RISCV_JAL)
}

fn mask(bytes: &mut [u8], offset: usize, r_type: u32) {
    match r_type {
        R_RISCV_ALIGN | R_RISCV_RELAX => {}
        R_RISCV_BRANCH | R_RISCV_PCREL_LO12_S | R_RISCV_LO12_S => {
            mask32(bytes, offset, SB_IMMEDIATE)
        }
        R_RISCV_JAL | R_RISCV_GOT_HI20 | R_RISCV_TLS_GOT_HI20 | R_RISCV_TLS_GD_HI20
        | R_RISCV_PCREL_HI20 | R_RISCV_HI20 => mask32(bytes, offset, U_IMMEDIATE),
        R_RISCV_PCREL_LO12_I | R_RISCV_LO12_I => mask32(bytes, offset, I_IMMEDIATE),
        R_RISCV_CALL | R_RISCV_CALL_PLT => {
            mask32(bytes, offset, U_IMMEDIATE);
            mask32(bytes, offset + 4, I_IMMEDIATE);
        }
        R_RISCV_RVC_BRANCH => mask16(bytes, offset, CB_IMMEDIATE),
        R_RISCV_RVC_JUMP => mask16(bytes, offset, CJ_IMMEDIATE),
        R_RISCV_RVC_LUI => mask16(bytes, offset, CI_IMMEDIATE),
        // Data and arithmetic relocations: the whole word is link-time.
        _ => mask32(bytes, offset, u32::MAX),
    }
}

fn mask32(bytes: &mut [u8], offset: usize, mask: u32) {
    if let Some(word) = bytes.get_mut(offset..offset + 4) {
        let value = u32::from_le_bytes([word[0], word[1], word[2], word[3]]) & !mask;
        word.copy_from_slice(&value.to_le_bytes());
    }
}

fn mask16(bytes: &mut [u8], offset: usize, mask: u16) {
    if let Some(parcel) = bytes.get_mut(offset..offset + 2) {
        let value = u16::from_le_bytes([parcel[0], parcel[1]]) & !mask;
        parcel.copy_from_slice(&value.to_le_bytes());
    }
}

/// Parts per million of the longest common parcel subsequence, relative to
/// both lengths (`2 * common / (left + right)`).
pub fn similarity_ppm(left: &[u16], right: &[u16]) -> u32 {
    if left.is_empty() && right.is_empty() {
        return 1_000_000;
    }
    let mut previous = vec![0_u32; right.len() + 1];
    let mut current = vec![0_u32; right.len() + 1];
    for parcel in left {
        for (column, other) in right.iter().enumerate() {
            current[column + 1] = if parcel == other {
                previous[column] + 1
            } else {
                previous[column + 1].max(current[column])
            };
        }
        std::mem::swap(&mut previous, &mut current);
    }
    let common = u64::from(previous[right.len()]);
    let total = (left.len() + right.len()) as u64;
    u32::try_from(2 * common * 1_000_000 / total).unwrap_or(1_000_000)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(offset: usize, r_type: u32, reference: Reference) -> RelocationSite {
        RelocationSite {
            offset,
            r_type,
            reference,
        }
    }

    #[test]
    fn call_immediates_do_not_change_identity_but_registers_do() {
        // auipc ra, 0x12 ; jalr ra, 0x34(ra)
        let first = [0x97, 0x20, 0x01, 0x00, 0xe7, 0x80, 0x40, 0x03];
        // auipc ra, 0x56 ; jalr ra, 0x78(ra)
        let second = [0x97, 0x60, 0x05, 0x00, 0xe7, 0x80, 0x80, 0x07];
        // auipc t1 instead of ra
        let third = [0x17, 0x63, 0x05, 0x00, 0xe7, 0x80, 0x80, 0x07];
        let call = |name: &str| vec![site(0, R_RISCV_CALL, Reference::Symbol(name.into()))];
        let a = Body::new(&first, call("r_sym_bt_first"));
        let b = Body::new(&second, call("r_named_callee"));
        let c = Body::new(&third, call("r_named_callee"));
        assert_eq!(a.fingerprint, b.fingerprint);
        assert_ne!(a.fingerprint, c.fingerprint);
        assert_eq!(b.calls, ["r_named_callee"]);
    }

    #[test]
    fn local_label_offsets_are_part_of_identity() {
        let bytes = [0x63, 0x04, 0x00, 0x00];
        let a = Body::new(&bytes, vec![site(0, R_RISCV_BRANCH, Reference::Label(8))]);
        let b = Body::new(&bytes, vec![site(0, R_RISCV_BRANCH, Reference::Label(12))]);
        assert_ne!(a.fingerprint, b.fingerprint);
    }

    #[test]
    fn similarity_counts_the_common_subsequence() {
        assert_eq!(similarity_ppm(&[1, 2, 3, 4], &[1, 2, 3, 4]), 1_000_000);
        assert_eq!(similarity_ppm(&[1, 2, 3, 4], &[1, 9, 3, 4]), 750_000);
        assert_eq!(similarity_ppm(&[1, 2], &[3, 4]), 0);
    }
}
