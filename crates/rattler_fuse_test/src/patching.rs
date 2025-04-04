use once_cell::sync::Lazy;
use rattler_conda_types::Platform;
use regex::Regex;
use std::borrow::Cow;
use std::cmp;
use std::os::fd::AsRawFd;
use std::os::unix::fs::FileExt;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::tree_objects::PatchMode;

// Add this near the other constants at the top of the file
static NEXT_MEMORY_FD: AtomicU64 = AtomicU64::new(0x7FFF_FFFF_0000_0000);

const MAX_SHEBANG_LENGTH_LINUX: usize = 127;
const MAX_SHEBANG_LENGTH_MACOS: usize = 512;

pub enum OpenFile {
    NoPatch(std::fs::File),
    Patched(PatchedFile),
    InMemory(Vec<u8>),
}

impl OpenFile {
    pub fn fd(&self) -> u64 {
        match self {
            OpenFile::NoPatch(f) => f.as_raw_fd() as u64,
            OpenFile::Patched(f) => f.file.as_raw_fd() as u64,
            OpenFile::InMemory(_) =>
            // Use pointer address to ensure uniqueness across different in-memory files
            // Starting from a very high base value to avoid conflicts with real FDs
            {
                NEXT_MEMORY_FD.fetch_add(1, Ordering::Relaxed)
            }
        }
    }

    pub fn read_at(&mut self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        let bytes_read = match self {
            OpenFile::NoPatch(f) => f.read_at(buf, offset).unwrap(),
            OpenFile::Patched(f) => {
                f.advance(Some(offset + buf.len() as u64)).unwrap();
                f.read_at(buf, offset).unwrap()
            }
            OpenFile::InMemory(data) => {
                let end = cmp::min(data.len(), (offset as i64 + buf.len() as i64) as usize);
                let bytes_read = end - offset as usize;
                buf[..bytes_read as usize].copy_from_slice(&data[offset as usize..end]);
                bytes_read
            }
        };
        Ok(bytes_read)
    }

    pub fn size_change(&mut self) -> i64 {
        match self {
            OpenFile::NoPatch(_) | OpenFile::InMemory(_) => 0,
            OpenFile::Patched(f) => f.size_change(),
        }
    }
}

pub struct PatchedFile {
    file: std::fs::File,
    offsets: PatchOffsets,
    current_pos: Option<u64>,
    old_prefix: Vec<u8>,
    new_prefix: Rc<Vec<u8>>,
    target_platform: Platform,
}

#[derive(Debug, Default)]
struct PatchedShebang {
    original_len: usize,
    new: Vec<u8>,
}

#[derive(Debug)]
enum PatchOffsets {
    BinPatchOffsets(BinaryPatchOffsets),
    TextPatchOffsets(TextPatchOffsets),
}

#[derive(Debug)]
struct TextPatchOffsets {
    /// The difference in length between the old and new shebang
    shebang: Option<PatchedShebang>,
    /// The difference in length between the old and new prefixes
    shift: u16,
    /// The apparent offsets in the file (TODO: Include this in the package cache)
    offset: Vec<u64>,
}

#[derive(Debug)]
struct BinaryPatchOffsets {
    /// The difference in length between the old and new prefixes
    shift: u16,
    /// Each element corrosponds to a string inside the file that needs to be patched
    /// Each element of the inner vector corrosponds to an instance of the placeholder
    /// in the string, except the last element which corrosponds to the end of the string
    offset: Vec<Vec<u64>>,
    /// State which is used to keep track between buffers in advance
    advance_state: Option<Vec<u64>>,
}

impl PatchedShebang {
    pub fn shift(&self) -> i64 {
        self.new.len() as i64 - self.original_len as i64
    }
}

impl TextPatchOffsets {
    pub fn new(shift: u16) -> Self {
        Self {
            shebang: None,
            shift,
            offset: Vec::new(),
        }
    }

    /// Find the closest offset that is less than or equal to the given offset.
    pub fn start_idx(&self, pos: u64) -> Option<usize> {
        match self.offset.binary_search(&pos) {
            Ok(idx) => Some(idx),
            Err(0) => None,
            Err(idx) => {
                if idx == 0 {
                    None
                } else {
                    Some(idx - 1)
                }
            }
        }
    }

    /// Get the offset at the given index
    pub fn at(&self, idx: usize) -> Option<(u64, u64)> {
        let apparent_offset = *self.offset.get(idx)?;
        let shebang = self.shebang.as_ref().expect("Shebang not yet read");
        let shift = idx as i64 * i64::from(self.shift) + shebang.shift();
        let real_offset = apparent_offset + shift as u64;
        Some((apparent_offset, real_offset))
    }

    /// Insert a new offset into the list of offsets
    pub fn insert(&mut self, real_offset: u64) {
        let shift = self.offset.len() as u64 * u64::from(self.shift);
        let apparent_offset = real_offset - shift;

        assert!(match self.offset.last() {
            Some(&last) => last <= apparent_offset,
            None => true,
        });
        match self.offset.binary_search(&apparent_offset) {
            Ok(_) => (),
            Err(idx) => self.offset.insert(idx, apparent_offset),
        }
    }
}

impl BinaryPatchOffsets {
    pub fn new(shift: u16) -> Self {
        Self {
            shift,
            offset: Vec::new(),
            advance_state: None,
        }
    }

    /// Find the closest set of offsets that is less than or equal to the given offset.
    pub fn start_idx(&self, pos: u64) -> Option<usize> {
        let idx = match self.offset.binary_search_by_key(&pos, |v| v[0]) {
            Ok(idx) => Some(idx),
            Err(0) => None,
            Err(idx) => {
                if idx == 0 {
                    None
                } else {
                    Some(idx - 1)
                }
            }
        };
        match idx {
            Some(idx) => {
                let end_of_string = *self.offset[idx]
                    .last()
                    .expect("There should be at least two elements in the vector");
                let end_of_string =
                    end_of_string + u64::from(self.shift) * (self.offset[idx].len() - 1) as u64;
                if pos > end_of_string {
                    Some(idx + 1)
                } else {
                    Some(idx)
                }
            }
            None => None,
        }
    }

    /// Get the offset at the given index
    pub fn at(&self, idx: usize) -> Option<impl Iterator<Item = (u64, u64)> + '_> {
        let inner = self.offset.get(idx)?;
        let shift = u64::from(self.shift);
        Some(inner.iter().enumerate().map(move |(i, apparent_offset)| {
            let real_offset = apparent_offset + i as u64 * shift;
            (*apparent_offset, real_offset)
        }))
    }

    /// Insert a new offset into the list of offsets
    pub fn insert(&mut self, real_offsets: Vec<u64>) {
        let apparent_offsets: Vec<u64> = real_offsets
            .iter()
            .enumerate()
            .map(|(i, offset)| offset - i as u64 * u64::from(self.shift))
            .collect();
        assert!(match self.offset.last() {
            Some(last) => last[0] <= apparent_offsets[0],
            None => true,
        });
        match self
            .offset
            .binary_search_by_key(&apparent_offsets[0], |v| v[0])
        {
            Ok(_) => (),
            Err(idx) => self.offset.insert(idx, apparent_offsets),
        }
    }
}

impl PatchOffsets {
    pub fn size_change(&self) -> i64 {
        match self {
            PatchOffsets::BinPatchOffsets(_) => 0,
            PatchOffsets::TextPatchOffsets(ref offsets) => {
                let shebang = offsets.shebang.as_ref().expect("Shebang not yet read");
                shebang.shift() - (i64::from(offsets.shift) * offsets.offset.len() as i64)
            }
        }
    }

    pub fn shift(&self) -> u16 {
        match self {
            PatchOffsets::BinPatchOffsets(offsets) => offsets.shift,
            PatchOffsets::TextPatchOffsets(offsets) => offsets.shift,
        }
    }

    pub fn count(&self) -> usize {
        match self {
            PatchOffsets::BinPatchOffsets(offsets) => offsets.offset.iter().map(std::vec::Vec::len).sum(),
            PatchOffsets::TextPatchOffsets(offsets) => offsets.offset.len(),
        }
    }
}

impl PatchedFile {
    pub fn open(file: std::fs::File, patch_mode: &PatchMode) -> OpenFile {
        match patch_mode {
            PatchMode::Binary(old, new, target_platform) => {
                let shift = (old.len() - new.len())
                    .try_into()
                    .expect("Difference between old and new prefix must be representable as a u16");
                let patched_file = Self {
                    file,
                    offsets: PatchOffsets::BinPatchOffsets(BinaryPatchOffsets::new(shift)),
                    current_pos: Some(0),
                    old_prefix: old.clone(),
                    new_prefix: new.clone(),
                    target_platform: *target_platform,
                };
                OpenFile::Patched(patched_file)
            }
            PatchMode::Text(old, new, target_platform) => {
                let shift = (old.len() - new.len())
                    .try_into()
                    .expect("Difference between old and new prefix must be representable as a u16");
                let patched_file = Self {
                    file,
                    offsets: PatchOffsets::TextPatchOffsets(TextPatchOffsets::new(shift)),
                    current_pos: Some(0),
                    old_prefix: old.clone(),
                    new_prefix: new.clone(),
                    target_platform: *target_platform,
                };
                OpenFile::Patched(patched_file)
            }
            PatchMode::None => OpenFile::NoPatch(file),
        }
    }

    /// Get the size change that would result from applying the patch
    pub fn size_change(&mut self) -> i64 {
        self.advance(None).unwrap();
        self.offsets.size_change()
    }

    /// Scan the file for occurrences of the old prefix and update the offsets
    pub fn advance(&mut self, new_pos: Option<u64>) -> std::io::Result<()> {
        if match (self.current_pos, new_pos) {
            (None, _) => true,
            (Some(_), None) => false,
            (Some(current), Some(desired)) => {
                current >= desired + u64::from(self.offsets.shift()) * self.offsets.count() as u64
            }
        } {
            return Ok(());
        }
        let mut current_pos = match self.current_pos {
            Some(pos) => pos,
            None => unreachable!(),
        };

        let size: usize = 64 * 1024; // This must be longer than the maximum shebang length
        let mut buffer: Vec<u8> = vec![0; size + self.old_prefix.len() - 1];
        loop {
            let bytes_read = self.file.read_at(&mut buffer, current_pos)?;

            // If we've reached the end of the file, stop reading
            if bytes_read == 0 {
                self.current_pos = None;
                break;
            }
            let mut buffer = &buffer[..bytes_read];

            match self.offsets {
                PatchOffsets::TextPatchOffsets(ref mut offsets) => {
                    let mut real_offset = current_pos;
                    // Ignore the first line if it's a shebang
                    assert!(current_pos == 0 || current_pos > MAX_SHEBANG_LENGTH_MACOS as u64);
                    if current_pos == 0 {
                        offsets.shebang = Some(
                            if self.target_platform.is_unix() && buffer.starts_with(b"#!") {
                                let (first, _) = buffer
                                    .split_at(buffer.iter().position(|&c| c == b'\n').unwrap_or(0));
                                let first_line = String::from_utf8_lossy(first);
                                let prefix_placeholder = String::from_utf8_lossy(&self.old_prefix);
                                let target_prefix = String::from_utf8_lossy(&self.new_prefix);
                                let new_shebang = replace_shebang(
                                    first_line,
                                    (&prefix_placeholder, &target_prefix),
                                    &self.target_platform,
                                );
                                real_offset += first.len() as u64;
                                buffer = &buffer[first.len()..];
                                PatchedShebang {
                                    original_len: first.len(),
                                    new: new_shebang.as_bytes().to_vec(),
                                }
                            } else {
                                PatchedShebang::default()
                            },
                        );
                    }
                    // Update the offsets with any occurrences of the old prefix
                    for index in memchr::memmem::find_iter(buffer, &self.old_prefix) {
                        offsets.insert(real_offset + index as u64);
                    }
                }
                PatchOffsets::BinPatchOffsets(ref mut offsets) => {
                    let finder = memchr::memmem::Finder::new(&self.old_prefix);
                    let mut real_offset = current_pos;

                    while !buffer.is_empty() {
                        let mut end = if let Some(state) = &offsets.advance_state {
                            if state.is_empty() {
                                buffer.len()
                            } else {
                                let mut end = 0;
                                while end < buffer.len() && buffer[end] != b'\0' {
                                    end += 1;
                                }
                                end
                            }
                        } else {
                            offsets.advance_state = Some(Vec::new());
                            buffer.len()
                        };
                        let these_offsets = offsets
                            .advance_state
                            .as_mut()
                            .expect("Value is initialized at the start of the loop");

                        assert!(end <= buffer.len() && end > 0);

                        if let Some(first_index) = finder.find(&buffer[..end]) {
                            these_offsets.push(real_offset + first_index as u64);
                            // Find the end of the c-style string. The nul terminator basically.
                            end = first_index + self.old_prefix.len();
                            while end < buffer.len() && buffer[end] != b'\0' {
                                end += 1;
                            }
                            // Find all occurrences of the old prefix in the string
                            for index in
                                finder.find_iter(&buffer[first_index + self.old_prefix.len()..end])
                            {
                                these_offsets.push(
                                    (real_offset as usize
                                        + first_index
                                        + self.old_prefix.len()
                                        + index) as u64,
                                );
                            }
                            // If we've found the end of the string, add it to the offsets
                            if end < buffer.len() {
                                these_offsets.push(real_offset + end as u64);
                                let these_offsets = offsets
                                    .advance_state
                                    .take()
                                    .expect("Value is initialized at the start of the loop");
                                offsets.insert(these_offsets);
                            }
                        } else if !these_offsets.is_empty() && end != buffer.len() {
                            // We have a partial match from the previous buffer and the current buffer
                            // doesn't contain any more matches
                            these_offsets.push(real_offset + end as u64);
                            let these_offsets = offsets
                                .advance_state
                                .take()
                                .expect("Value is initialized at the start of the loop");
                            offsets.insert(these_offsets);
                        }
                        buffer = &buffer[end..];
                        real_offset += end as u64;
                    }
                }
            }

            current_pos += cmp::min(bytes_read, size) as u64;

            // Break if we've read the desired number of bytes
            if let Some(pos) = new_pos {
                if current_pos
                    >= pos + u64::from(self.offsets.shift()) * self.offsets.count() as u64
                {
                    self.current_pos = Some(current_pos);
                    break;
                }
            }
        }
        Ok(())
    }

    /// Read from the file at the given offset with prefix substitution
    ///
    /// This function requires that advance has already been called!
    pub fn read_at(&self, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
        assert!(self.current_pos.unwrap_or(u64::MAX) >= offset + buf.len() as u64);

        let mut bytes_read: usize = 0;
        match &self.offsets {
            PatchOffsets::TextPatchOffsets(offsets) => {
                let mut start_idx = if let Some(idx) = offsets.start_idx(offset) { idx } else {
                    // Read up until the first offset
                    let mut apparent_pos = offset;
                    let mut real_pos = offset;
                    let mut to_read = match offsets.at(0) {
                        Some((first_pos, first_pos2)) => {
                            assert!(first_pos == first_pos2);
                            cmp::min((first_pos - offset) as usize, buf.len())
                        }
                        None => buf.len() - bytes_read,
                    };

                    // Handle the shebang
                    if let Some(shebang) = &offsets.shebang {
                        let n = cmp::min(shebang.new.len(), buf.len() - bytes_read)
                            - real_pos as usize;
                        buf[bytes_read..bytes_read + n].copy_from_slice(
                            &shebang.new[offset as usize..n + offset as usize],
                        );
                        bytes_read += n;
                        to_read -= n;
                        apparent_pos += n as u64;
                        real_pos = if shebang.new.len() == apparent_pos as usize {
                            shebang.original_len as u64
                        } else {
                            apparent_pos
                        };
                    }

                    // Handle anything else which comes before the first offset
                    let n = self
                        .file
                        .read_at(&mut buf[bytes_read..bytes_read + to_read], real_pos)?;
                    bytes_read += n;
                    if n < to_read {
                        return Ok(bytes_read);
                    }
                    0
                };

                while bytes_read < buf.len() {
                    let (mut apparent_pos, mut real_pos) = offsets.at(start_idx).unwrap();
                    let (_, real_next) = offsets.at(start_idx + 1).unwrap_or((u64::MAX, u64::MAX));

                    // First, perform the substitution if necessary
                    let shift = offset as usize + bytes_read - apparent_pos as usize;
                    if shift < self.new_prefix.len() {
                        let new_prefix = &self.new_prefix[shift..];

                        let remaining = buf.len() - bytes_read;
                        if remaining <= new_prefix.len() {
                            // We need to copy a suffix from the target path and then stop
                            buf[bytes_read..].copy_from_slice(&new_prefix[..remaining]);
                            return Ok(buf.len());
                        }
                        buf[bytes_read..bytes_read + new_prefix.len()].copy_from_slice(new_prefix);
                        bytes_read += new_prefix.len();
                    } else {
                        // Need to skip some bytes before copying from the source
                        real_pos += shift as u64 - self.new_prefix.len() as u64;
                        apparent_pos += shift as u64 - self.new_prefix.len() as u64;
                    }
                    real_pos += self.old_prefix.len() as u64;
                    apparent_pos += self.new_prefix.len() as u64;
                    assert!(apparent_pos == offset + bytes_read as u64);

                    // The remaining bytes up until the next offset can be proxied from the file
                    let read_size =
                        cmp::min(buf.len() - bytes_read, (real_next - real_pos) as usize);
                    let n = self
                        .file
                        .read_at(&mut buf[bytes_read..bytes_read + read_size], real_pos)?;
                    bytes_read += n;
                    start_idx += 1;
                    assert!(self.current_pos.unwrap_or(u64::MAX) >= real_pos + n as u64);
                    if n < read_size || start_idx >= offsets.offset.len() {
                        return Ok(bytes_read);
                    }
                }
            }
            PatchOffsets::BinPatchOffsets(offsets) => {
                if offsets.offset.is_empty() {
                    return self.file.read_at(buf, offset);
                }

                let mut start_idx = offsets.start_idx(offset).unwrap_or_default();

                // Read up until the first offset
                let apparent_pos = offset;
                let to_read = match offsets.at(start_idx) {
                    Some(mut iter) => {
                        let (first_pos, first_pos2) = iter
                            .next()
                            .expect("Empty iterator of offsets doesn't make sense");
                        assert!(first_pos == first_pos2);
                        cmp::min(
                            first_pos as i64 - offset as i64,
                            (buf.len() - bytes_read) as i64,
                        )
                    }
                    None => (buf.len() - bytes_read) as i64,
                };
                if to_read > 0 {
                    let to_read = to_read as usize;
                    let n = self
                        .file
                        .read_at(&mut buf[bytes_read..bytes_read + to_read], apparent_pos)?;
                    bytes_read += n;
                    if n < to_read {
                        return Ok(bytes_read);
                    }
                }

                while bytes_read < buf.len() {
                    let iter_vec: Vec<(u64, u64)> = if let Some(iter) = offsets.at(start_idx) { iter.collect() } else {
                        // We've reached the end of the offsets so we can just read from the file
                        let start = offset + bytes_read as u64;
                        bytes_read += self.file.read_at(&mut buf[bytes_read..], start)?;
                        assert!(self.current_pos.is_none() || bytes_read == buf.len());
                        return Ok(bytes_read);
                    };
                    for window in iter_vec.windows(2) {
                        let (apparent_pos, mut real_pos) = window[0];
                        let (next_apparent, next_real) = window[1];
                        if next_apparent > offset + bytes_read as u64 {
                            // First, perform the substitution if necessary
                            let seek = offset + bytes_read as u64 - apparent_pos;
                            if seek <= self.new_prefix.len() as u64 {
                                let new_prefix = &self.new_prefix[seek as usize..];
                                let remaining = cmp::min(buf.len() - bytes_read, new_prefix.len());
                                buf[bytes_read..bytes_read + remaining].copy_from_slice(new_prefix);
                                bytes_read += remaining;
                                if bytes_read == buf.len() {
                                    return Ok(bytes_read);
                                }
                                real_pos += self.old_prefix.len() as u64;
                            } else {
                                real_pos += seek + u64::from(offsets.shift);
                            }

                            // The remaining bytes up until the next offset can be proxied from the file
                            let read_size =
                                cmp::min(buf.len() - bytes_read, (next_real - real_pos) as usize);
                            let n = self
                                .file
                                .read_at(&mut buf[bytes_read..bytes_read + read_size], real_pos)?;
                            bytes_read += n;
                            assert!(
                                self.current_pos.unwrap_or(u64::MAX) >= real_pos + n as u64
                            );
                        }
                    }

                    // Pad the buffer with null bytes if we've reached the end of the offsets
                    let last_apparent = iter_vec
                        .last()
                        .expect("Empty iterator of offsets doesn't make sense")
                        .0;
                    if last_apparent == offset + bytes_read as u64 {
                        let n_nulls = cmp::min(
                            buf.len() - bytes_read,
                            offsets.shift as usize * (iter_vec.len() - 1),
                        );
                        buf[bytes_read..bytes_read + n_nulls].fill(0);
                        bytes_read += n_nulls;
                    }

                    if bytes_read == buf.len() {
                        break;
                    }

                    // Read until the next cstring
                    start_idx += 1;
                    let (apparent_pos, real_pos) = if let Some(mut iter) = offsets.at(start_idx) { iter
                    .next()
                    .expect("Empty iterator of offsets doesn't make sense") } else {
                        let start = offset + bytes_read as u64;
                        bytes_read += self.file.read_at(&mut buf[bytes_read..], start)?;
                        assert!(self.current_pos.is_none() || bytes_read == buf.len());
                        return Ok(bytes_read);
                    };
                    assert!(apparent_pos == real_pos);
                    let read_size = cmp::min(
                        buf.len() - bytes_read,
                        apparent_pos as usize - (offset as usize + bytes_read),
                    );
                    let n = self.file.read_at(
                        &mut buf[bytes_read..bytes_read + read_size],
                        offset + bytes_read as u64,
                    )?;
                    assert!(n == read_size, "Read {n} bytes, expected {read_size}");
                    bytes_read += n;
                    if bytes_read == buf.len() {
                        break;
                    }
                }
            }
        }
        Ok(bytes_read)
    }
}

static SHEBANG_REGEX: Lazy<Regex> = Lazy::new(|| {
    // ^(#!      pretty much the whole match string
    // (?:[ ]*)  allow spaces between #! and beginning of
    //           the executable path
    // (/(?:\\ |[^ \n\r\t])*)  the executable is the next
    //                         text block without an
    //                         escaped space or non-space
    //                         whitespace character
    // (.*))$    the rest of the line can contain option
    //           flags and end whole_shebang group
    Regex::new(r"^(#!(?:[ ]*)(/(?:\\ |[^ \n\r\t])*)(.*))$").unwrap()
});

static PYTHON_REGEX: Lazy<Regex> = Lazy::new(|| {
    // Match string starting with `python`, and optional version number
    // followed by optional flags.
    // python matches the string `python`
    // (?:\d+(?:\.\d+)*)? matches an optional version number
    Regex::new(r"^python(?:\d+(?:\.\d+)?)?$").unwrap()
});

/// Finds if the shebang line length is valid.
fn is_valid_shebang_length(shebang: &str, platform: &Platform) -> bool {
    if platform.is_linux() {
        shebang.len() <= MAX_SHEBANG_LENGTH_LINUX
    } else if platform.is_osx() {
        shebang.len() <= MAX_SHEBANG_LENGTH_MACOS
    } else {
        true
    }
}

/// Convert a shebang to use `/usr/bin/env` to find the executable.
/// This is useful for long shebangs or shebangs with spaces.
fn convert_shebang_to_env(shebang: Cow<'_, str>) -> Cow<'_, str> {
    if let Some(captures) = SHEBANG_REGEX.captures(&shebang) {
        let path = &captures[2];
        let exe_name = path.rsplit_once('/').map_or(path, |(_, f)| f);
        if PYTHON_REGEX.is_match(exe_name) {
            Cow::Owned(format!(
                "#!/bin/sh\n'''exec' \"{}\"{} \"$0\" \"$@\" #'''",
                path, &captures[3]
            ))
        } else {
            Cow::Owned(format!("#!/usr/bin/env {}{}", exe_name, &captures[3]))
        }
    } else {
        shebang
    }
}

/// Long shebangs and shebangs with spaces are invalid.
/// Long shebangs are longer than 127 on Linux or 512 on macOS characters.
/// Shebangs with spaces are replaced with a shebang that uses `/usr/bin/env` to find the executable.
/// This function replaces long shebangs with a shebang that uses `/usr/bin/env` to find the
/// executable.
fn replace_shebang<'a>(
    shebang: Cow<'a, str>,
    old_new: (&str, &str),
    platform: &Platform,
) -> Cow<'a, str> {
    // If the new shebang would contain a space, return a `#!/usr/bin/env` shebang
    assert!(
        shebang.starts_with("#!"),
        "Shebang does not start with #! ({shebang})",
    );

    if old_new.1.contains(' ') {
        // Doesn't matter if we don't replace anything
        if !shebang.contains(old_new.0) {
            return shebang;
        }
        // we convert the shebang without spaces to a new shebang, and only then replace
        // which is relevant for the Python case
        let new_shebang = convert_shebang_to_env(shebang).replace(old_new.0, old_new.1);
        return new_shebang.into();
    }

    let shebang: Cow<'_, str> = shebang.replace(old_new.0, old_new.1).into();

    if !shebang.starts_with("#!") {
        tracing::warn!("Shebang does not start with #! ({})", shebang);
        return shebang;
    }

    if is_valid_shebang_length(&shebang, platform) {
        shebang
    } else {
        convert_shebang_to_env(shebang)
    }
}
