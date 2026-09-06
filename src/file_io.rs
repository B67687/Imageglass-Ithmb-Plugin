//! Shared file I/O helpers for .ithmb reading.
//!
//! Centralises the metadata-check + read + validation pattern used by both
//! [`crate::codec`] (metadata path) and [`crate::decode`] (decode path).

use std::io::Read;

use crate::MAX_FILE_SIZE_BYTES;
use crate::types::IGStatus;

/// Check that `path` exists and its size does not exceed [`MAX_FILE_SIZE_BYTES`].
///
/// Returns the file size on success so callers can avoid a second metadata call.
fn check_file_size(path: &str) -> Result<u64, IGStatus> {
    let metadata = std::fs::metadata(path).map_err(|_| IGStatus::IoError)?;
    let file_size = metadata.len();
    if file_size > MAX_FILE_SIZE_BYTES {
        return Err(IGStatus::DecodeFailed);
    }
    Ok(file_size)
}

/// Read an entire .ithmb file into memory.
///
/// Performs a pre-size check via [`MAX_FILE_SIZE_BYTES`], reads the full file,
/// and verifies the result is at least 4 bytes (the minimum for a valid prefix).
pub(crate) fn read_ithmb_file(path: &str) -> Result<Vec<u8>, IGStatus> {
    let _file_size = check_file_size(path)?;
    let bytes = std::fs::read(path).map_err(|_| IGStatus::IoError)?;
    if bytes.len() < 4 {
        return Err(IGStatus::DecodeFailed);
    }
    Ok(bytes)
}

/// Read only the 4-byte format prefix from an .ithmb file.
///
/// Performs a pre-size check via [`MAX_FILE_SIZE_BYTES`], then reads exactly
/// 4 bytes from the start of the file.  Avoids loading the entire file into
/// memory when only the prefix is needed (e.g. metadata lookup).
///
/// Returns the 4-byte prefix and the total file size (needed by callers to
/// populate `IGImageInfo::file_size_bytes`).
pub(crate) fn read_ithmb_prefix(path: &str) -> Result<([u8; 4], u64), IGStatus> {
    let file_size = check_file_size(path)?;
    let mut file = std::fs::File::open(path).map_err(|_| IGStatus::IoError)?;
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf)
        .map_err(|_| IGStatus::DecodeFailed)?;
    Ok((buf, file_size))
}
