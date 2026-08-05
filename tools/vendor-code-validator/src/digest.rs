//! Content identity shared by discovery reports and verification evidence.

use std::{fs, path::Path};

use sha2::{Digest, Sha256};

use crate::Result;

pub(crate) fn artifact_sha256(path: &Path) -> Result<String> {
    let bytes = fs::read(path)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}
