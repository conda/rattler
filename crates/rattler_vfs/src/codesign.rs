//! In-memory ad-hoc Mach-O code re-signing.
//!
//! After binary prefix replacement, the page hashes in the embedded `CodeDirectory`
//! are stale. This module recomputes them in-place without invoking `/usr/bin/codesign`.

use sha2::{Digest, Sha256};
use std::fmt;

// Mach-O magic values
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

/// Determine endianness and header size from Mach-O magic.
/// Returns `(big_endian, header_size)` or `None` if not a Mach-O.
///
/// When the first 4 bytes are read as big-endian:
/// - MAGIC values mean the binary IS big-endian (bytes match the constant directly)
/// - CIGAM values mean the binary is little-endian (bytes are swapped)
fn macho_info(magic: u32) -> Option<(bool, usize)> {
    match magic {
        MACHO_MAGIC_64 => Some((true, 32)),
        MACHO_CIGAM_64 => Some((false, 32)),
        MACHO_MAGIC_32 => Some((true, 28)),
        MACHO_CIGAM_32 => Some((false, 28)),
        _ => None,
    }
}

/// Walk Mach-O load commands, return `(dataoff, datasize)` from `LC_CODE_SIGNATURE`.
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

/// Parse a `SuperBlob`, return the offset of the `CodeDirectory` blob within it.
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

/// Re-sign a single Mach-O binary (thin, not fat) in-place.
fn resign_macho_slice(data: &mut [u8]) -> Result<(), CodesignError> {
    let (dataoff, datasize) = find_code_signature(data)?;
    let sig_start = dataoff as usize;
    let sig_end = sig_start + datasize as usize;
    if sig_end > data.len() {
        return Err(CodesignError::OutOfBounds);
    }

    let cd_offset_in_sb = find_code_directory(&data[sig_start..sig_end])?;
    let cd_abs = sig_start + cd_offset_in_sb;

    // Parse CodeDirectory fields (big-endian)
    if cd_abs + 44 > data.len() {
        return Err(CodesignError::OutOfBounds);
    }
    let hash_offset = read_u32_be(data, cd_abs + 16) as usize;
    let n_code_slots = read_u32_be(data, cd_abs + 28) as usize;
    let code_limit = read_u32_be(data, cd_abs + 32) as usize;
    let hash_size = data[cd_abs + 36] as usize;
    let hash_type = data[cd_abs + 37];
    let page_size_log2 = data[cd_abs + 39];

    if hash_type != HASH_TYPE_SHA256 {
        return Err(CodesignError::UnsupportedHashType(hash_type));
    }
    if hash_size != 32 {
        return Err(CodesignError::UnsupportedHashType(hash_type));
    }
    if page_size_log2 >= 64 {
        return Err(CodesignError::OutOfBounds);
    }
    if code_limit > data.len() {
        return Err(CodesignError::OutOfBounds);
    }
    let page_size = 1usize << page_size_log2;

    // Verify bounds for the hash table (check for overflow)
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

    // Recompute page hashes
    for i in 0..n_code_slots {
        let page_start = i * page_size;
        let page_end = std::cmp::min(page_start + page_size, code_limit);
        let hash = Sha256::digest(&data[page_start..page_end]);
        let dest = hashes_start + i * hash_size;
        data[dest..dest + hash_size].copy_from_slice(&hash);
    }

    Ok(())
}

/// Re-sign a Mach-O binary in-place after prefix replacement.
///
/// Handles both thin and fat (universal) binaries. For non-Mach-O data, returns `Ok(())`.
pub fn adhoc_resign(data: &mut [u8]) -> Result<(), CodesignError> {
    if data.len() < 4 {
        return Ok(());
    }

    let magic = read_u32_be(data, 0);

    if magic == FAT_MAGIC {
        // Fat binary: re-sign each architecture slice
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
        // Not a Mach-O binary, nothing to do
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal Mach-O 64-bit binary with a valid code signature.
    ///
    /// `content_pages` are 4096-byte pages of content (the last may be shorter).
    /// The header + load command occupy the first 48 bytes of the first page.
    fn build_test_macho(content_pages: &[&[u8]], identifier: &str, big_endian: bool) -> Vec<u8> {
        let page_size: usize = 4096;

        // Build content: header page + extra content pages
        let mut content = vec![0u8; page_size]; // first page (header lives here)

        // Write mach_header_64
        // MAGIC on disk = big-endian binary, CIGAM on disk = little-endian binary
        let (magic, write_u32): (u32, fn(u32) -> [u8; 4]) = if big_endian {
            (MACHO_MAGIC_64, u32::to_be_bytes)
        } else {
            (MACHO_CIGAM_64, u32::to_le_bytes)
        };
        content[0..4].copy_from_slice(&magic.to_be_bytes());
        content[4..8].copy_from_slice(&write_u32(0x0100000c)); // cputype arm64
        content[12..16].copy_from_slice(&write_u32(2)); // MH_EXECUTE
        content[16..20].copy_from_slice(&write_u32(1)); // ncmds = 1
        content[20..24].copy_from_slice(&write_u32(16)); // sizeofcmds

        // Append content pages
        for page in content_pages {
            let mut padded = vec![0u8; page_size];
            let copy_len = page.len().min(page_size);
            padded[..copy_len].copy_from_slice(&page[..copy_len]);
            content.extend_from_slice(&padded);
        }

        let code_limit = content.len();
        let n_code_slots = code_limit.div_ceil(page_size);

        // Build CodeDirectory
        let ident_bytes = identifier.as_bytes();
        let ident_offset: u32 = 44; // right after the fixed fields
        let hash_offset: u32 = ident_offset + ident_bytes.len() as u32 + 1; // +1 for null
        let cd_length = hash_offset as usize + n_code_slots * 32; // 32 = SHA-256 hash size

        let mut cd = vec![0u8; cd_length];
        // magic (big-endian)
        cd[0..4].copy_from_slice(&CODE_DIRECTORY_MAGIC.to_be_bytes());
        // length
        cd[4..8].copy_from_slice(&(cd_length as u32).to_be_bytes());
        // version
        cd[8..12].copy_from_slice(&0x20400u32.to_be_bytes());
        // flags = adhoc
        cd[12..16].copy_from_slice(&0x2u32.to_be_bytes());
        // hashOffset
        cd[16..20].copy_from_slice(&hash_offset.to_be_bytes());
        // identOffset
        cd[20..24].copy_from_slice(&ident_offset.to_be_bytes());
        // nSpecialSlots = 0
        cd[24..28].copy_from_slice(&0u32.to_be_bytes());
        // nCodeSlots
        cd[28..32].copy_from_slice(&(n_code_slots as u32).to_be_bytes());
        // codeLimit
        cd[32..36].copy_from_slice(&(code_limit as u32).to_be_bytes());
        // hashSize
        cd[36] = 32;
        // hashType = SHA-256
        cd[37] = HASH_TYPE_SHA256;
        // platform
        cd[38] = 0;
        // pageSize = log2(4096) = 12
        cd[39] = 12;

        // identifier string (null-terminated)
        let ident_start = ident_offset as usize;
        cd[ident_start..ident_start + ident_bytes.len()].copy_from_slice(ident_bytes);
        cd[ident_start + ident_bytes.len()] = 0;

        // Compute page hashes
        for i in 0..n_code_slots {
            let page_start = i * page_size;
            let page_end = std::cmp::min(page_start + page_size, code_limit);
            let hash = Sha256::digest(&content[page_start..page_end]);
            let dest = hash_offset as usize + i * 32;
            cd[dest..dest + 32].copy_from_slice(&hash);
        }

        // Build SuperBlob: 1 blob (the CodeDirectory)
        let sb_length = 12 + 8 + cd.len(); // header + 1 index entry + cd
        let mut superblob = Vec::with_capacity(sb_length);
        superblob.extend_from_slice(&SUPERBLOB_MAGIC.to_be_bytes());
        superblob.extend_from_slice(&(sb_length as u32).to_be_bytes());
        superblob.extend_from_slice(&1u32.to_be_bytes()); // count = 1
        // BlobIndex: type=0 (CodeDirectory), offset=20
        superblob.extend_from_slice(&0u32.to_be_bytes());
        superblob.extend_from_slice(&20u32.to_be_bytes()); // offset from superblob start
        superblob.extend_from_slice(&cd);

        // Write LC_CODE_SIGNATURE load command at offset 32
        let dataoff = code_limit as u32;
        let datasize = superblob.len() as u32;
        let write_u32_fn: fn(u32) -> [u8; 4] = write_u32;
        content[32..36].copy_from_slice(&write_u32_fn(LC_CODE_SIGNATURE));
        content[36..40].copy_from_slice(&write_u32_fn(16)); // cmdsize
        content[40..44].copy_from_slice(&write_u32_fn(dataoff));
        content[44..48].copy_from_slice(&write_u32_fn(datasize));

        // Append the signature
        content.extend_from_slice(&superblob);

        // Fix page 0 hash (it now includes the load command we just wrote)
        let hash = Sha256::digest(&content[0..page_size]);
        let sig_start = code_limit;
        let cd_start = sig_start + 20; // SuperBlob header(12) + BlobIndex(8)
        let hashes_start = cd_start + hash_offset as usize;
        content[hashes_start..hashes_start + 32].copy_from_slice(&hash);

        content
    }

    fn build_test_macho_32bit(content_pages: &[&[u8]], identifier: &str) -> Vec<u8> {
        let page_size: usize = 4096;
        let header_size: usize = 28; // 32-bit header is 28 bytes

        let mut content = vec![0u8; page_size];

        // mach_header (32-bit, little-endian) — CIGAM on disk = little-endian
        content[0..4].copy_from_slice(&MACHO_CIGAM_32.to_be_bytes());
        content[4..8].copy_from_slice(&12u32.to_le_bytes()); // cputype ARM
        content[12..16].copy_from_slice(&2u32.to_le_bytes()); // MH_EXECUTE
        content[16..20].copy_from_slice(&1u32.to_le_bytes()); // ncmds = 1
        content[20..24].copy_from_slice(&16u32.to_le_bytes()); // sizeofcmds

        for page in content_pages {
            let mut padded = vec![0u8; page_size];
            let copy_len = page.len().min(page_size);
            padded[..copy_len].copy_from_slice(&page[..copy_len]);
            content.extend_from_slice(&padded);
        }

        let code_limit = content.len();
        let n_code_slots = code_limit.div_ceil(page_size);

        // Build CodeDirectory (same as 64-bit, always big-endian)
        let ident_bytes = identifier.as_bytes();
        let ident_offset: u32 = 44;
        let hash_offset: u32 = ident_offset + ident_bytes.len() as u32 + 1;
        let cd_length = hash_offset as usize + n_code_slots * 32;

        let mut cd = vec![0u8; cd_length];
        cd[0..4].copy_from_slice(&CODE_DIRECTORY_MAGIC.to_be_bytes());
        cd[4..8].copy_from_slice(&(cd_length as u32).to_be_bytes());
        cd[8..12].copy_from_slice(&0x20400u32.to_be_bytes());
        cd[12..16].copy_from_slice(&0x2u32.to_be_bytes());
        cd[16..20].copy_from_slice(&hash_offset.to_be_bytes());
        cd[20..24].copy_from_slice(&ident_offset.to_be_bytes());
        cd[24..28].copy_from_slice(&0u32.to_be_bytes());
        cd[28..32].copy_from_slice(&(n_code_slots as u32).to_be_bytes());
        cd[32..36].copy_from_slice(&(code_limit as u32).to_be_bytes());
        cd[36] = 32;
        cd[37] = HASH_TYPE_SHA256;
        cd[39] = 12;
        let ident_start = ident_offset as usize;
        cd[ident_start..ident_start + ident_bytes.len()].copy_from_slice(ident_bytes);

        for i in 0..n_code_slots {
            let page_start = i * page_size;
            let page_end = std::cmp::min(page_start + page_size, code_limit);
            let hash = Sha256::digest(&content[page_start..page_end]);
            let dest = hash_offset as usize + i * 32;
            cd[dest..dest + 32].copy_from_slice(&hash);
        }

        let sb_length = 12 + 8 + cd.len();
        let mut superblob = Vec::with_capacity(sb_length);
        superblob.extend_from_slice(&SUPERBLOB_MAGIC.to_be_bytes());
        superblob.extend_from_slice(&(sb_length as u32).to_be_bytes());
        superblob.extend_from_slice(&1u32.to_be_bytes());
        superblob.extend_from_slice(&0u32.to_be_bytes());
        superblob.extend_from_slice(&20u32.to_be_bytes());
        superblob.extend_from_slice(&cd);

        // LC_CODE_SIGNATURE at offset 28 (after 32-bit header)
        let dataoff = code_limit as u32;
        let datasize = superblob.len() as u32;
        content[header_size..header_size + 4].copy_from_slice(&LC_CODE_SIGNATURE.to_le_bytes());
        content[header_size + 4..header_size + 8].copy_from_slice(&16u32.to_le_bytes());
        content[header_size + 8..header_size + 12].copy_from_slice(&dataoff.to_le_bytes());
        content[header_size + 12..header_size + 16].copy_from_slice(&datasize.to_le_bytes());

        content.extend_from_slice(&superblob);

        // Fix page 0 hash
        let hash = Sha256::digest(&content[0..page_size]);
        let sig_start = code_limit;
        let cd_start = sig_start + 20;
        let hashes_start = cd_start + hash_offset as usize;
        content[hashes_start..hashes_start + 32].copy_from_slice(&hash);

        content
    }

    /// Verify that all page hashes in the `CodeDirectory` match the actual content.
    fn verify_page_hashes(data: &[u8]) -> bool {
        let magic = read_u32_be(data, 0);
        let slices: Vec<(usize, usize)> = if magic == FAT_MAGIC {
            let nfat = read_u32_be(data, 4) as usize;
            (0..nfat)
                .map(|i| {
                    let off = 8 + i * 20;
                    (
                        read_u32_be(data, off + 8) as usize,
                        read_u32_be(data, off + 12) as usize,
                    )
                })
                .collect()
        } else {
            vec![(0, data.len())]
        };

        for (slice_off, slice_size) in slices {
            let s = &data[slice_off..slice_off + slice_size];
            let (dataoff, _) = find_code_signature(s).unwrap();
            let sig_start = dataoff as usize;
            let cd_off = find_code_directory(&s[sig_start..]).unwrap();
            let cd_abs = sig_start + cd_off;

            let hash_offset = read_u32_be(s, cd_abs + 16) as usize;
            let n_code_slots = read_u32_be(s, cd_abs + 28) as usize;
            let code_limit = read_u32_be(s, cd_abs + 32) as usize;
            let page_size = 1usize << s[cd_abs + 39];

            for i in 0..n_code_slots {
                let page_start = i * page_size;
                let page_end = std::cmp::min(page_start + page_size, code_limit);
                let expected = Sha256::digest(&s[page_start..page_end]);
                let stored_start = cd_abs + hash_offset + i * 32;
                if s[stored_start..stored_start + 32] != *expected {
                    return false;
                }
            }
        }
        true
    }

    // --- Parsing tests ---

    #[test]
    fn test_find_code_signature() {
        let data = build_test_macho(&[b"hello"], "test", false);
        let (dataoff, datasize) = find_code_signature(&data).unwrap();
        assert!(dataoff > 0);
        assert!(datasize > 0);
        assert_eq!(dataoff as usize + datasize as usize, data.len());
    }

    #[test]
    fn test_find_code_signature_missing() {
        // Binary with 0 load commands
        let mut data = vec![0u8; 64];
        data[0..4].copy_from_slice(&MACHO_MAGIC_64.to_be_bytes());
        // ncmds = 0
        assert!(matches!(
            find_code_signature(&data),
            Err(CodesignError::NoCodeSignature)
        ));
    }

    #[test]
    fn test_find_code_signature_too_small() {
        assert!(matches!(
            find_code_signature(&[0; 4]),
            Err(CodesignError::TooSmall)
        ));
    }

    #[test]
    fn test_find_code_directory() {
        let data = build_test_macho(&[b"hello"], "test", false);
        let (dataoff, datasize) = find_code_signature(&data).unwrap();
        let sig = &data[dataoff as usize..(dataoff + datasize) as usize];
        let cd_offset = find_code_directory(sig).unwrap();
        let cd_magic = read_u32_be(sig, cd_offset);
        assert_eq!(cd_magic, CODE_DIRECTORY_MAGIC);
    }

    #[test]
    fn test_find_code_directory_missing() {
        // SuperBlob with 0 blobs
        let mut sb = vec![0u8; 12];
        sb[0..4].copy_from_slice(&SUPERBLOB_MAGIC.to_be_bytes());
        sb[4..8].copy_from_slice(&12u32.to_be_bytes());
        sb[8..12].copy_from_slice(&0u32.to_be_bytes());
        assert!(matches!(
            find_code_directory(&sb),
            Err(CodesignError::NoCodeDirectory)
        ));
    }

    // --- Re-signing tests ---

    #[test]
    fn test_resign_updates_page_hashes() {
        let mut data = build_test_macho(&[b"original content"], "test", false);
        assert!(verify_page_hashes(&data));

        // Modify content in the second page (first extra content page)
        data[4096] = 0xff;
        data[4097] = 0xfe;
        assert!(!verify_page_hashes(&data));

        adhoc_resign(&mut data).unwrap();
        assert!(verify_page_hashes(&data));
    }

    #[test]
    fn test_resign_preserves_size() {
        let mut data = build_test_macho(&[b"content"], "test", false);
        let original_size = data.len();
        data[4096] = 0xff;
        adhoc_resign(&mut data).unwrap();
        assert_eq!(data.len(), original_size);
    }

    #[test]
    fn test_resign_noop_for_non_macho() {
        let mut data = vec![0u8; 100];
        data[0..4].copy_from_slice(b"ELF!");
        assert!(adhoc_resign(&mut data).is_ok());
    }

    #[test]
    fn test_resign_noop_for_empty() {
        let mut data = vec![];
        assert!(adhoc_resign(&mut data).is_ok());
    }

    #[test]
    fn test_resign_multiple_modified_pages() {
        let page_a = vec![0xaau8; 4096];
        let page_b = vec![0xbbu8; 4096];
        let page_c = vec![0xccu8; 4096];
        let mut data = build_test_macho(
            &[page_a.as_slice(), page_b.as_slice(), page_c.as_slice()],
            "multi",
            false,
        );
        assert!(verify_page_hashes(&data));

        // Modify pages 1 and 3 (content pages at offsets 4096 and 12288)
        data[4096] = 0x00;
        data[12288] = 0x00;
        assert!(!verify_page_hashes(&data));

        adhoc_resign(&mut data).unwrap();
        assert!(verify_page_hashes(&data));
    }

    #[test]
    fn test_resign_partial_last_page() {
        // Content page with only 100 bytes (less than 4096)
        let short_page = vec![0x42u8; 100];
        let mut data = build_test_macho(&[short_page.as_slice()], "partial", false);
        assert!(verify_page_hashes(&data));

        data[4096] = 0xff;
        adhoc_resign(&mut data).unwrap();
        assert!(verify_page_hashes(&data));
    }

    #[test]
    fn test_resign_fat_binary() {
        // Build two thin binaries
        let thin_a = build_test_macho(&[b"arch-a"], "fat-test", false);
        let thin_b = build_test_macho(&[b"arch-b"], "fat-test", false);

        // Align slices to 4096 boundaries
        let slice_a_offset = 4096usize; // after fat header
        let slice_a_size = thin_a.len();
        let slice_b_offset = (slice_a_offset + slice_a_size + 4095) & !4095; // align to 4096
        let slice_b_size = thin_b.len();
        let total_size = slice_b_offset + slice_b_size;

        let mut fat = vec![0u8; total_size];
        // Fat header (big-endian)
        fat[0..4].copy_from_slice(&FAT_MAGIC.to_be_bytes());
        fat[4..8].copy_from_slice(&2u32.to_be_bytes()); // 2 architectures

        // fat_arch[0]
        fat[8..12].copy_from_slice(&0x01000007u32.to_be_bytes()); // x86_64
        fat[16..20].copy_from_slice(&(slice_a_offset as u32).to_be_bytes());
        fat[20..24].copy_from_slice(&(slice_a_size as u32).to_be_bytes());
        fat[24..28].copy_from_slice(&12u32.to_be_bytes()); // align = 2^12

        // fat_arch[1]
        fat[28..32].copy_from_slice(&0x0100000cu32.to_be_bytes()); // arm64
        fat[36..40].copy_from_slice(&(slice_b_offset as u32).to_be_bytes());
        fat[40..44].copy_from_slice(&(slice_b_size as u32).to_be_bytes());
        fat[44..48].copy_from_slice(&12u32.to_be_bytes());

        // Copy slices
        fat[slice_a_offset..slice_a_offset + slice_a_size].copy_from_slice(&thin_a);
        fat[slice_b_offset..slice_b_offset + slice_b_size].copy_from_slice(&thin_b);

        assert!(verify_page_hashes(&fat));

        // Modify content in both slices
        fat[slice_a_offset + 4096] = 0xff;
        fat[slice_b_offset + 4096] = 0xfe;
        assert!(!verify_page_hashes(&fat));

        adhoc_resign(&mut fat).unwrap();
        assert!(verify_page_hashes(&fat));
    }

    #[test]
    fn test_resign_32bit_macho() {
        let mut data = build_test_macho_32bit(&[b"32bit content"], "test32");
        assert!(verify_page_hashes(&data));

        data[4096] = 0xff;
        adhoc_resign(&mut data).unwrap();
        assert!(verify_page_hashes(&data));
    }

    #[test]
    fn test_resign_big_endian_macho() {
        let mut data = build_test_macho(&[b"be content"], "test-be", true);
        assert!(verify_page_hashes(&data));

        data[4096] = 0xff;
        adhoc_resign(&mut data).unwrap();
        assert!(verify_page_hashes(&data));
    }

    // --- Malformed binary edge cases ---

    #[test]
    fn test_resign_invalid_page_size_log2() {
        let mut data = build_test_macho(&[b"test"], "test", false);
        // Find CodeDirectory and corrupt page_size_log2
        let (dataoff, _) = find_code_signature(&data).unwrap();
        let cd_off = find_code_directory(&data[dataoff as usize..]).unwrap();
        let cd_abs = dataoff as usize + cd_off;
        data[cd_abs + 39] = 64; // page_size_log2 = 64 would overflow 1usize << 64
        assert!(matches!(
            resign_macho_slice(&mut data),
            Err(CodesignError::OutOfBounds)
        ));
    }

    #[test]
    fn test_resign_code_limit_exceeds_data() {
        let mut data = build_test_macho(&[b"test"], "test", false);
        let (dataoff, _) = find_code_signature(&data).unwrap();
        let cd_off = find_code_directory(&data[dataoff as usize..]).unwrap();
        let cd_abs = dataoff as usize + cd_off;
        // Set code_limit to way beyond data length
        let huge_limit = (data.len() as u32 + 99999).to_be_bytes();
        data[cd_abs + 32..cd_abs + 36].copy_from_slice(&huge_limit);
        assert!(matches!(
            resign_macho_slice(&mut data),
            Err(CodesignError::OutOfBounds)
        ));
    }

    #[test]
    fn test_resign_zero_code_slots() {
        let mut data = build_test_macho(&[b"test"], "test", false);
        let (dataoff, _) = find_code_signature(&data).unwrap();
        let cd_off = find_code_directory(&data[dataoff as usize..]).unwrap();
        let cd_abs = dataoff as usize + cd_off;
        // Set n_code_slots to 0
        data[cd_abs + 28..cd_abs + 32].copy_from_slice(&0u32.to_be_bytes());
        // Should succeed (nothing to resign)
        assert!(resign_macho_slice(&mut data).is_ok());
    }
}
