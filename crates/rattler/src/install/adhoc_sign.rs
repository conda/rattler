//! Minimal, dependency-free ad-hoc re-signing of Mach-O binaries.
//!
//! Conda-forge macOS binaries ship with an ad-hoc code signature. Prefix
//! replacement rewrites bytes inside `__TEXT`, which invalidates that
//! signature, so the file must be re-signed before it will run (especially on
//! arm64, where an invalid signature is fatal).
//!
//! An *ad-hoc* signature is the simplest kind there is: an embedded signature
//! `SuperBlob` whose only meaningful member is a `CodeDirectory` — no CMS blob,
//! no certificate chain, no requirements. The `CodeDirectory` is mostly an
//! array of SHA-256 hashes, one per 4 KiB page of the file up to `codeLimit`
//! (the start of the signature itself).
//!
//! Because rattler's binary prefix replacement is **length-preserving** (it
//! null-pads the shortened c-string), the `__LINKEDIT` segment and the
//! `LC_CODE_SIGNATURE` load command stay at exactly the same file offset and
//! size. Re-signing therefore reduces to: *recompute the per-page hashes and
//! overwrite them in place*. Nothing else in the file moves, and the special
//! slots (entitlements, requirements) are byte-identical so their hashes are
//! unchanged. There is no outer hash over the `CodeDirectory` to fix up for an
//! ad-hoc signature, so this is all that is required.
//!
//! Scope of this minimal implementation:
//!  * Little-endian Mach-O (all Apple targets: `x86_64`, arm64) — 32- and 64-bit.
//!  * Fat/universal binaries (`FAT_MAGIC` and `FAT_MAGIC_64`).
//!  * SHA-256 code directories (the modern default).
//!
//! Anything outside that (unsigned binary, big-endian Mach-O, SHA-1 directory,
//! a layout we don't recognize) returns [`ResignOutcome::NeedsFullSign`] so the
//! caller can fall back to spawning `/usr/bin/codesign`.

use rattler_digest::{Sha256, compute_bytes_digest};

// --- Mach-O / code-signing constants ---------------------------------------

const FAT_MAGIC: u32 = 0xcafe_babe; // fat header, big-endian on disk
const FAT_MAGIC_64: u32 = 0xcafe_babf;
const MH_MAGIC_64: u32 = 0xfeed_facf; // thin Mach-O, little-endian on disk
const MH_MAGIC_32: u32 = 0xfeed_face;

const LC_CODE_SIGNATURE: u32 = 0x1d;

const CSMAGIC_EMBEDDED_SIGNATURE: u32 = 0xfade_0cc0;
const CSMAGIC_CODEDIRECTORY: u32 = 0xfade_0c02;
const CSSLOT_CODEDIRECTORY: u32 = 0;

const CS_HASHTYPE_SHA256: u8 = 2;
const CS_SHA256_LEN: usize = 32;

/// Result of attempting an in-place ad-hoc re-sign.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResignOutcome {
    /// Every Mach-O slice carried an ad-hoc signature and was re-signed in
    /// place. The buffer is fully signed and ready to write.
    Resigned,
    /// The buffer is not a Mach-O file; nothing to do.
    NotMachO,
    /// At least one slice has no existing signature or uses a layout we do not
    /// handle. The caller should fall back to `/usr/bin/codesign`.
    NeedsFullSign,
}

// --- little-endian / big-endian readers (bounds-checked) -------------------

fn read_u32_le(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u32_be(data: &[u8], off: usize) -> Option<u32> {
    data.get(off..off + 4)
        .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64_be(data: &[u8], off: usize) -> Option<u64> {
    data.get(off..off + 8)
        .map(|b| u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

/// Re-sign every Mach-O slice in `data` in place. `data` is the whole file.
pub(crate) fn adhoc_resign(data: &mut [u8]) -> ResignOutcome {
    // A slice is a (start-offset, length) region of `data` that is a thin
    // Mach-O. For a thin file there is exactly one, at offset 0.
    let slices = match collect_slices(data) {
        Some(s) => s,
        None => return ResignOutcome::NotMachO,
    };

    for (off, len) in slices {
        match resign_slice(data, off, len) {
            SliceOutcome::Resigned => {}
            SliceOutcome::NeedsFullSign => return ResignOutcome::NeedsFullSign,
        }
    }
    ResignOutcome::Resigned
}

/// Enumerate the thin Mach-O slices in the file. Returns `None` if this is not
/// a Mach-O file at all.
fn collect_slices(data: &[u8]) -> Option<Vec<(usize, usize)>> {
    // The fat header is always big-endian; a thin header is little-endian.
    match read_u32_be(data, 0)? {
        FAT_MAGIC | FAT_MAGIC_64 => {
            let is_64 = read_u32_be(data, 0)? == FAT_MAGIC_64;
            let nfat = read_u32_be(data, 4)? as usize;
            let (entry_size, off_field, size_field) = if is_64 { (32, 8, 16) } else { (20, 8, 12) };
            let mut slices = Vec::with_capacity(nfat);
            for i in 0..nfat {
                let base = 8 + i * entry_size;
                let (offset, size) = if is_64 {
                    (
                        read_u64_be(data, base + off_field)? as usize,
                        read_u64_be(data, base + size_field)? as usize,
                    )
                } else {
                    (
                        read_u32_be(data, base + off_field)? as usize,
                        read_u32_be(data, base + size_field)? as usize,
                    )
                };
                slices.push((offset, size));
            }
            Some(slices)
        }
        _ => match read_u32_le(data, 0)? {
            MH_MAGIC_64 | MH_MAGIC_32 => Some(vec![(0, data.len())]),
            _ => None,
        },
    }
}

enum SliceOutcome {
    Resigned,
    NeedsFullSign,
}

/// Re-sign a single thin Mach-O slice living at `data[off..off+len]`.
fn resign_slice(data: &mut [u8], off: usize, _len: usize) -> SliceOutcome {
    // Header: magic(4) cputype(4) cpusubtype(4) filetype(4) ncmds(4)
    //         sizeofcmds(4) flags(4) [reserved(4) for 64-bit]
    let magic = match read_u32_le(data, off) {
        Some(m) => m,
        None => return SliceOutcome::NeedsFullSign,
    };
    let header_size = match magic {
        MH_MAGIC_64 => 32,
        MH_MAGIC_32 => 28,
        _ => return SliceOutcome::NeedsFullSign, // big-endian Mach-O: unsupported
    };
    let ncmds = match read_u32_le(data, off + 16) {
        Some(n) => n as usize,
        None => return SliceOutcome::NeedsFullSign,
    };

    // Walk the load commands looking for LC_CODE_SIGNATURE.
    let (sig_off, _sig_size) = {
        let mut lc = off + header_size;
        let mut found = None;
        for _ in 0..ncmds {
            let cmd = match read_u32_le(data, lc) {
                Some(c) => c,
                None => return SliceOutcome::NeedsFullSign,
            };
            let cmdsize = match read_u32_le(data, lc + 4) {
                Some(s) if s >= 8 => s as usize,
                _ => return SliceOutcome::NeedsFullSign,
            };
            if cmd == LC_CODE_SIGNATURE {
                // linkedit_data_command: cmd(4) cmdsize(4) dataoff(4) datasize(4)
                let dataoff = match read_u32_le(data, lc + 8) {
                    Some(d) => d as usize,
                    None => return SliceOutcome::NeedsFullSign,
                };
                let datasize = match read_u32_le(data, lc + 12) {
                    Some(d) => d as usize,
                    None => return SliceOutcome::NeedsFullSign,
                };
                found = Some((off + dataoff, datasize));
                break;
            }
            lc += cmdsize;
        }
        match found {
            Some(v) => v,
            // No existing signature -> would need to insert a load command and
            // grow __LINKEDIT. Hand it to /usr/bin/codesign.
            None => return SliceOutcome::NeedsFullSign,
        }
    };

    // The embedded-signature SuperBlob (big-endian):
    //   magic(4) length(4) count(4) [ type(4) offset(4) ] * count
    if read_u32_be(data, sig_off) != Some(CSMAGIC_EMBEDDED_SIGNATURE) {
        return SliceOutcome::NeedsFullSign;
    }
    let count = match read_u32_be(data, sig_off + 8) {
        Some(c) => c as usize,
        None => return SliceOutcome::NeedsFullSign,
    };
    let mut cd_off = None;
    for i in 0..count {
        let idx = sig_off + 12 + i * 8;
        let slot_type = match read_u32_be(data, idx) {
            Some(t) => t,
            None => return SliceOutcome::NeedsFullSign,
        };
        let blob_rel = match read_u32_be(data, idx + 4) {
            Some(o) => o as usize,
            None => return SliceOutcome::NeedsFullSign,
        };
        if slot_type == CSSLOT_CODEDIRECTORY {
            cd_off = Some(sig_off + blob_rel);
            break;
        }
    }
    let cd_off = match cd_off {
        Some(o) => o,
        None => return SliceOutcome::NeedsFullSign,
    };

    // CodeDirectory (big-endian). Field offsets from the blob start:
    //   0 magic  4 length  8 version  12 flags  16 hashOffset  20 identOffset
    //   24 nSpecialSlots  28 nCodeSlots  32 codeLimit  36 hashSize(u8)
    //   37 hashType(u8)  38 platform(u8)  39 pageSize(u8, log2)  40 spare2
    //   ... 48 codeLimit64(u64) for version >= 0x20300
    if read_u32_be(data, cd_off) != Some(CSMAGIC_CODEDIRECTORY) {
        return SliceOutcome::NeedsFullSign;
    }
    let version = read_u32_be(data, cd_off + 8).unwrap_or(0);
    let hash_offset = match read_u32_be(data, cd_off + 16) {
        Some(o) => o as usize,
        None => return SliceOutcome::NeedsFullSign,
    };
    let n_code_slots = match read_u32_be(data, cd_off + 28) {
        Some(n) => n as usize,
        None => return SliceOutcome::NeedsFullSign,
    };
    let hash_size = data.get(cd_off + 36).copied().unwrap_or(0) as usize;
    let hash_type = data.get(cd_off + 37).copied().unwrap_or(0);
    let page_shift = data.get(cd_off + 39).copied().unwrap_or(0);

    // We only implement SHA-256, 32-byte slots.
    if hash_type != CS_HASHTYPE_SHA256 || hash_size != CS_SHA256_LEN {
        return SliceOutcome::NeedsFullSign;
    }

    // codeLimit is the number of bytes covered by the hashes (everything up to
    // the signature). Use the 64-bit field if present and set.
    let mut code_limit = u64::from(read_u32_be(data, cd_off + 32).unwrap_or(0));
    if version >= 0x0002_0300
        && let Some(cl64) = read_u64_be(data, cd_off + 48)
        && cl64 != 0
    {
        code_limit = cl64;
    }
    let code_limit = code_limit as usize;

    // pageSize is stored as a log2; 0 means "one hash over the whole range".
    let page = if page_shift == 0 {
        code_limit.max(1)
    } else {
        1usize << page_shift
    };

    // Sanity check: the slot count must match the covered range. If prefix
    // replacement had (impossibly) changed the length, bail to the external
    // signer rather than write a corrupt directory.
    let expected_slots = code_limit.div_ceil(page);
    if expected_slots != n_code_slots {
        return SliceOutcome::NeedsFullSign;
    }

    // Recompute each code-page hash and overwrite it in place.
    for i in 0..n_code_slots {
        let start = off + i * page;
        let end = (off + ((i + 1) * page)).min(off + code_limit);
        let page_bytes = match data.get(start..end) {
            Some(b) => b,
            None => return SliceOutcome::NeedsFullSign,
        };
        let digest = compute_bytes_digest::<Sha256>(page_bytes);

        let slot = cd_off + hash_offset + i * hash_size;
        match data.get_mut(slot..slot + hash_size) {
            Some(dst) => dst.copy_from_slice(digest.as_slice()),
            None => return SliceOutcome::NeedsFullSign,
        }
    }

    SliceOutcome::Resigned
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::Command;

    /// End-to-end: build a real ad-hoc-signed binary, mutate a byte inside the
    /// hashed region (invalidating the signature), re-sign with our code, and
    /// assert that `codesign --verify` accepts it and the program still runs.
    #[test]
    fn resign_roundtrip_matches_codesign() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("hello.c");
        let bin = dir.path().join("hello");
        std::fs::write(
            &src,
            "#include <stdio.h>\nint main(){printf(\"MARKER_ABCDEFGH\\n\");return 0;}\n",
        )
        .unwrap();

        // Compile and ad-hoc sign with the real toolchain.
        let cc = Command::new("clang")
            .arg("-o")
            .arg(&bin)
            .arg(&src)
            .status()
            .unwrap();
        assert!(cc.success(), "clang failed");
        let sign = Command::new("/usr/bin/codesign")
            .args(["--sign", "-", "--force"])
            .arg(&bin)
            .status()
            .unwrap();
        assert!(sign.success(), "initial ad-hoc sign failed");

        // Corrupt a byte inside the hashed region to invalidate the signature,
        // mimicking prefix replacement. Flip a char inside our marker string.
        let mut data = std::fs::read(&bin).unwrap();
        let pos = data
            .windows(6)
            .position(|w| w == b"MARKER")
            .expect("marker not found");
        data[pos] = b'X';

        // The signature is now invalid: prove that first.
        std::fs::write(&bin, &data).unwrap();
        let bad = Command::new("/usr/bin/codesign")
            .args(["--verify"])
            .arg(&bin)
            .status()
            .unwrap();
        assert!(
            !bad.success(),
            "expected corrupted binary to fail verification"
        );

        // Re-sign in place with our implementation.
        let outcome = adhoc_resign(&mut data);
        assert_eq!(outcome, ResignOutcome::Resigned);
        std::fs::write(&bin, &data).unwrap();

        // codesign must now accept it again...
        let ok = Command::new("/usr/bin/codesign")
            .args(["--verify", "--verbose=2"])
            .arg(&bin)
            .status()
            .unwrap();
        assert!(
            ok.success(),
            "codesign --verify rejected our re-signed binary"
        );

        // ...and it must still execute.
        let run = Command::new(&bin).output().unwrap();
        assert!(run.status.success());
        assert!(String::from_utf8_lossy(&run.stdout).contains("XARKER"));
    }
}
