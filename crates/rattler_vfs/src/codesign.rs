use sha2::{Digest, Sha256};
use std::fmt;

const MACHO_MAGIC_64: u32 = 0xfeedfacf;
const MACHO_CIGAM_64: u32 = 0xcffaedfe;
const MACHO_MAGIC_32: u32 = 0xfeedface;
const MACHO_CIGAM_32: u32 = 0xcefaedfe;
const FAT_MAGIC: u32 = 0xcafebabe;

const LC_CODE_SIGNATURE: u32 = 0x1d;
const SUPERBLOB_MAGIC: u32 = 0xfade0cc0;
const CODE_DIRECTORY_MAGIC: u32 = 0xfade0c02;
const HASH_TYPE_SHA256: u8 = 2;

#[derive(Debug)]
pub enum CodesignError {
    TooSmall,
    NoCodeSignature,
    OutOfBounds,
    InvalidSuperBlob,
    NoCodeDirectory,
    UnsupportedHashType(u8),
}

impl fmt::Display for CodesignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooSmall => write!(f, "binary too small"),
            Self::NoCodeSignature => write!(f, "LC_CODE_SIGNATURE not found"),
            Self::OutOfBounds => write!(f, "code signature out of bounds"),
            Self::InvalidSuperBlob => write!(f, "invalid SuperBlob magic"),
            Self::NoCodeDirectory => write!(f, "CodeDirectory not found in SuperBlob"),
            Self::UnsupportedHashType(t) => {
                write!(f, "unsupported hash type {t} (expected SHA-256)")
            }
        }
    }
}

fn read_u32_be(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn read_u32(data: &[u8], offset: usize, big_endian: bool) -> u32 {
    if big_endian {
        read_u32_be(data, offset)
    } else {
        u32::from_le_bytes([
            data[offset],
            data[offset + 1],
            data[offset + 2],
            data[offset + 3],
        ])
    }
}

fn macho_info(magic: u32) -> Option<(bool, usize)> {
    match magic {
        MACHO_MAGIC_64 => Some((true, 32)),
        MACHO_CIGAM_64 => Some((false, 32)),
        MACHO_MAGIC_32 => Some((true, 28)),
        MACHO_CIGAM_32 => Some((false, 28)),
        _ => None,
    }
}

fn find_code_signature(macho: &[u8]) -> Result<(u32, u32), CodesignError> {
    if macho.len() < 28 {
        return Err(CodesignError::TooSmall);
    }
    let magic = read_u32_be(macho, 0);
    let (big_endian, header_size) = macho_info(magic).ok_or(CodesignError::TooSmall)?;
    let ncmds = read_u32(macho, 16, big_endian) as usize;
    let mut offset = header_size;
    for _ in 0..ncmds {
        if offset + 8 > macho.len() {
            return Err(CodesignError::OutOfBounds);
        }
        let cmd = read_u32(macho, offset, big_endian);
        let cmdsize = read_u32(macho, offset + 4, big_endian) as usize;
        if cmdsize < 8 || offset + cmdsize > macho.len() {
            return Err(CodesignError::OutOfBounds);
        }
        if cmd == LC_CODE_SIGNATURE {
            let dataoff = read_u32(macho, offset + 8, big_endian);
            let datasize = read_u32(macho, offset + 12, big_endian);
            return Ok((dataoff, datasize));
        }
        offset += cmdsize;
    }
    Err(CodesignError::NoCodeSignature)
}

fn find_code_directory(superblob: &[u8]) -> Result<usize, CodesignError> {
    if superblob.len() < 12 {
        return Err(CodesignError::InvalidSuperBlob);
    }
    let magic = read_u32_be(superblob, 0);
    if magic != SUPERBLOB_MAGIC {
        return Err(CodesignError::InvalidSuperBlob);
    }
    let count = read_u32_be(superblob, 8) as usize;
    for i in 0..count {
        let index_offset = 12 + i * 8;
        if index_offset + 8 > superblob.len() {
            return Err(CodesignError::OutOfBounds);
        }
        let blob_offset = read_u32_be(superblob, index_offset + 4) as usize;
        if blob_offset + 4 > superblob.len() {
            return Err(CodesignError::OutOfBounds);
        }
        let blob_magic = read_u32_be(superblob, blob_offset);
        if blob_magic == CODE_DIRECTORY_MAGIC {
            return Ok(blob_offset);
        }
    }
    Err(CodesignError::NoCodeDirectory)
}

fn resign_macho_slice(data: &mut [u8]) -> Result<(), CodesignError> {
    let (dataoff, datasize) = find_code_signature(data)?;
    let sig_start = dataoff as usize;
    let sig_end = sig_start + datasize as usize;
    if sig_end > data.len() {
        return Err(CodesignError::OutOfBounds);
    }
    let cd_offset_in_sb = find_code_directory(&data[sig_start..sig_end])?;
    let cd_abs = sig_start + cd_offset_in_sb;
    if cd_abs + 44 > data.len() {
        return Err(CodesignError::OutOfBounds);
    }
    let hash_offset = read_u32_be(data, cd_abs + 16) as usize;
    let n_code_slots = read_u32_be(data, cd_abs + 28) as usize;
    let code_limit = read_u32_be(data, cd_abs + 32) as usize;
    let hash_size = data[cd_abs + 36] as usize;
    let hash_type = data[cd_abs + 37];
    let page_size_log2 = data[cd_abs + 39];
    if hash_type != HASH_TYPE_SHA256 || hash_size != 32 {
        return Err(CodesignError::UnsupportedHashType(hash_type));
    }
    if page_size_log2 >= 64 {
        return Err(CodesignError::OutOfBounds);
    }
    if code_limit > data.len() {
        return Err(CodesignError::OutOfBounds);
    }
    let page_size = 1usize << page_size_log2;
    let hashes_start = cd_abs
        .checked_add(hash_offset)
        .ok_or(CodesignError::OutOfBounds)?;
    let hashes_end = n_code_slots
        .checked_mul(hash_size)
        .and_then(|n| hashes_start.checked_add(n))
        .ok_or(CodesignError::OutOfBounds)?;
    if hashes_end > data.len() {
        return Err(CodesignError::OutOfBounds);
    }
    for i in 0..n_code_slots {
        let page_start = i * page_size;
        let page_end = (page_start + page_size).min(code_limit);
        let hash = Sha256::digest(&data[page_start..page_end]);
        let dest = hashes_start + i * hash_size;
        data[dest..dest + hash_size].copy_from_slice(&hash);
    }
    Ok(())
}

/// Re-sign a Mach-O binary in-place after prefix replacement.
///
/// Recomputes the SHA-256 page hashes in the existing `CodeDirectory` without
/// invoking `/usr/bin/codesign`. Handles thin and fat (universal) binaries.
/// Returns `Ok(())` for non-Mach-O data without modifying it.
pub fn adhoc_resign(data: &mut [u8]) -> Result<(), CodesignError> {
    if data.len() < 4 {
        return Ok(());
    }
    let magic = read_u32_be(data, 0);
    if magic == FAT_MAGIC {
        if data.len() < 8 {
            return Err(CodesignError::TooSmall);
        }
        let nfat_arch = read_u32_be(data, 4) as usize;
        for i in 0..nfat_arch {
            let entry_offset = 8 + i * 20;
            if entry_offset + 20 > data.len() {
                return Err(CodesignError::OutOfBounds);
            }
            let slice_offset = read_u32_be(data, entry_offset + 8) as usize;
            let slice_size = read_u32_be(data, entry_offset + 12) as usize;
            if slice_offset + slice_size > data.len() {
                return Err(CodesignError::OutOfBounds);
            }
            resign_macho_slice(&mut data[slice_offset..slice_offset + slice_size])?;
        }
        Ok(())
    } else if macho_info(magic).is_some() {
        resign_macho_slice(data)
    } else {
        Ok(())
    }
}
