use memchr::memmem;
use memmap2::Mmap;
use std::path::Path;

use crate::metadata::CustomPrefixPlaceholder;

pub fn text_prefix_replacement(
    placeholder: &CustomPrefixPlaceholder,
    start: usize,
    _end: usize,
    size: usize,
    file: &Mmap,
    mount_point: &Path,
) -> Vec<u8> {
    if start >= file.len() {
        return vec![];
    }

    let old_prefix = placeholder.placeholder.as_bytes();
    let new_prefix_str = mount_point.to_string_lossy();
    let new_prefix = new_prefix_str.as_bytes();

    assert!(
        new_prefix.len() <= old_prefix.len(),
        "New prefix is longer than placeholder"
    );

    let mut replaced = Vec::with_capacity(file.len());
    let mut last_pos = 0;

    for &offset in &placeholder.offsets {
        if offset > file.len() {
            continue;
        }

        replaced.extend_from_slice(&file[last_pos..offset]);
        replaced.extend_from_slice(new_prefix);
        last_pos = offset + old_prefix.len();
    }

    if last_pos < file.len() {
        replaced.extend_from_slice(&file[last_pos..]);
    }

    let end = start.saturating_add(size).min(replaced.len());
    replaced[start..end].to_vec()
}

pub fn binary_prefix_replacement(
    placeholder: &CustomPrefixPlaceholder,
    start: usize,
    end: usize,
    _size: usize,
    file: &Mmap,
    mount_point: &Path,
) -> Vec<u8> {
    let new_prefix_str = mount_point.to_string_lossy();
    let new_prefix = new_prefix_str.as_bytes();
    let length_placeholder = placeholder.placeholder.len();
    let length_prefix = new_prefix.len();

    assert!(
        length_prefix <= length_placeholder,
        "New prefix is longer than placeholder"
    );

    let length_change = length_placeholder - length_prefix;

    if start >= end || start >= file.len() {
        return vec![];
    }

    let length = end - start;
    let mut buffer = vec![0u8; length];
    let mut buffer_pos = 0;

    let mut next_placeholder_index = match placeholder.offsets.binary_search(&start) {
        Ok(index) | Err(index) => index,
    };

    let mut unfinished_replacements = if next_placeholder_index >= 1 {
        let placeholders_before = &placeholder.offsets[0..next_placeholder_index];
        find_unfinished_replacements(&file[0..start], placeholders_before)
    } else {
        0
    };

    let actual_start = if unfinished_replacements >= 1 {
        start + (unfinished_replacements * length_change)
    } else {
        start
    };

    let mut file_pos = actual_start;

    while file_pos < end && buffer_pos < length {
        let next_placeholder = if next_placeholder_index < placeholder.offsets.len() {
            placeholder.offsets[next_placeholder_index]
        } else {
            end
        };

        if file_pos == next_placeholder && next_placeholder < end {
            next_placeholder_index += 1;

            let copy_len = length_prefix.min(length - buffer_pos);
            buffer[buffer_pos..buffer_pos + copy_len].copy_from_slice(&new_prefix[..copy_len]);
            buffer_pos += copy_len;
            unfinished_replacements += 1;

            if buffer_pos >= length {
                return buffer;
            }

            file_pos += length_placeholder;

            if file_pos >= file.len() || file_pos >= end {
                break;
            }

            let following_placeholder = if next_placeholder_index < placeholder.offsets.len() {
                placeholder.offsets[next_placeholder_index]
            } else {
                end
            };

            while file_pos < file.len()
                && file_pos < end
                && file_pos < following_placeholder
                && file[file_pos] != b'\x00'
                && buffer_pos < length
            {
                buffer[buffer_pos] = file[file_pos];
                buffer_pos += 1;
                file_pos += 1;
            }

            if file_pos < file.len() && file_pos < end && file[file_pos] == b'\x00' {
                buffer_pos += unfinished_replacements * length_change;
                unfinished_replacements = 0;
            }
        } else if file[file_pos] == b'\x00' && next_placeholder < end && unfinished_replacements > 0
        {
            buffer_pos += unfinished_replacements * length_change;
            unfinished_replacements = 0;
        } else {
            buffer[buffer_pos] = file[file_pos];
            buffer_pos += 1;
            file_pos += 1;
        }
    }
    buffer
}

pub fn find_unfinished_replacements(file_before: &[u8], offsets: &[usize]) -> usize {
    if offsets.is_empty() {
        return 0;
    }

    let last_nul_byte = memmem::rfind(file_before, b"\x00").unwrap_or_default();

    if offsets.last().unwrap() < &last_nul_byte {
        return 0;
    }

    let mut unfinished_replacements = 0;
    for &offset in offsets.iter().rev() {
        if offset >= last_nul_byte {
            unfinished_replacements += 1;
        } else {
            return unfinished_replacements;
        }
    }
    unfinished_replacements
}
