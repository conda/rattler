use std::{os::unix::ffi::OsStrExt, path::PathBuf};
use memchr::memmem;
use memmap2::Mmap;
use rattler_conda_types::package::PrefixPlaceholder;


/// Replace the prefix with a mount_point without having to actually write the prefix's
// TODO: This rendition of this replacement is not as performant as it could be, rethinking this strategy would be nice
pub fn text_prefix_replacement(
    placeholder: &PrefixPlaceholder, 
    start: usize, 
    end: usize, 
    _size: usize, 
    file: &Mmap, 
    mount_point: &PathBuf,
) -> Vec<u8> {
    // check if the prefix placeholder is there 

    let new_prefix = mount_point.as_os_str().as_bytes();
    let length_placeholder = placeholder.placeholder.len();
    let length_prefix = new_prefix.len();
    let length_change = length_placeholder - length_prefix;
    
    let offsets = placeholder.get_or_collect_offsets(file);
    let total_replacements = offsets.len();
    let transformed_size = file.len() - (total_replacements * length_change);

    let actual_end = end.min(transformed_size);
    let actual_start = start.min(transformed_size);
    
    // early return when the length of the asked section doesnt exist
    if actual_start >= actual_end {
        return vec![]
    }

    let length = actual_end - actual_start;
    let mut buffer = vec![0u8; length];
    let mut buffer_pos = 0;
    
    let mut file_pos = 0;
    let mut transformed_pos = 0;
    let mut placeholder_index = 0;

    while file_pos < file.len() && buffer_pos < length {
        if placeholder_index < total_replacements
            && file_pos == placeholder.offsets.unwrap()[placeholder_index] {
            for i in 0..length_prefix {
                if transformed_pos >= actual_start && transformed_pos < actual_end {
                    buffer[buffer_pos] = new_prefix[i];
                    buffer_pos += 1;
                }
                transformed_pos += 1;
                if buffer_pos >= length {
                    return buffer
                }
            }
            file_pos += length_placeholder;
            placeholder_index += 1;
        } else {
            if transformed_pos >= actual_start && transformed_pos < actual_end {
                buffer[buffer_pos] = file[file_pos];
                buffer_pos += 1;
            }
            transformed_pos += 1;
            file_pos += 1;
        }
    }
    buffer
}

/// Replace the prefix for the new prefix in binary files
// This function could also use some performance improvement (not checking every character individually mainly)
pub fn binary_prefix_replacement(
    placeholder: &PrefixPlaceholder,
    start: usize,
    end: usize,
    _size: usize,
    file: &Mmap,
    mount_point: &PathBuf
) -> Vec<u8> {

    let new_prefix = mount_point.as_os_str().as_bytes();
    let length_placeholder = placeholder.placeholder.len();
    let length_prefix = new_prefix.len();
    
    // Handle underflow: use checked subtraction or i64
    if length_prefix > length_placeholder {
        panic!("New prefix is longer than placeholder");
    }
    let length_change = length_placeholder - length_prefix;
    
    // Fix: proper bounds check
    if start >= end || start >= file.len() {
        return vec![]
    }
    
    let length = end - start;
    let mut buffer = vec![0u8; length];
    let mut buffer_pos = 0;

    let offsets = placeholder.get_or_collect_offsets(file);

    let mut next_placeholder_index = match offsets.binary_search(&start){
        Ok(index) => index,
        Err(index) => index
    };

    // should be actual start
    let mut unfinished_replacements = if next_placeholder_index >= 1 {
        let placeholders_before = &offsets[0..next_placeholder_index];
        find_unfinished_replacements(file[0..start].to_vec(), placeholders_before.to_vec())
    } else {
        0
    };

    let actual_start = if unfinished_replacements >= 1 {
        start + ( unfinished_replacements * length_change ) 
    } else {
        start
    };

    let mut file_pos = actual_start;

    let total_replacements = offsets.len();

    while file_pos < end && buffer_pos < length {
        let next_placeholder = if next_placeholder_index < total_replacements {
            placeholder.offsets[next_placeholder_index]
        } else {
            end
        };

        // Only process if we've reached a placeholder within our range
        if file_pos == next_placeholder && next_placeholder < end {
            next_placeholder_index += 1;
            
            // Copy the new prefix
            let copy_len = length_prefix.min(length - buffer_pos);
            buffer[buffer_pos..buffer_pos + copy_len].copy_from_slice(&new_prefix[..copy_len]);
            buffer_pos += copy_len;
            unfinished_replacements += 1;
            
            if buffer_pos >= length {
                return buffer;
            }

            // Skip the old placeholder in the file
            file_pos += length_placeholder;
            
            if file_pos >= file.len() || file_pos >= end {
                break;
            }

            // Get next placeholder position for boundary checking
            let following_placeholder = if next_placeholder_index < total_replacements {
                placeholder.offsets[next_placeholder_index]
            } else {
                end
            };

            // Copy until null byte, next placeholder, or end
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

            // If we hit a null byte, copy it & add the padding after the string content
            if file_pos < file.len()
                && file_pos < end
                && file[file_pos] == b'\x00'
                {
                    // buffer[buffer_pos] = b'\x00';
                    // file_pos += 1;
                    // buffer_pos = file_pos - actual_start;
                    // check if there are unfinished replacements and add the correct amount of null bytes to the buffer
                    
                    buffer_pos += unfinished_replacements * length_change;
                    unfinished_replacements = 0;
                    
                }
        } else if file[file_pos] == b'\x00' && next_placeholder < end && unfinished_replacements > 0{
            println!("{unfinished_replacements:?}");
            // buffer_pos += 1; // the already existing null byte
            buffer_pos += unfinished_replacements * length_change;
            unfinished_replacements = 0;
        }else {
            // Regular copy
            buffer[buffer_pos] = file[file_pos];
            buffer_pos += 1;
            file_pos += 1;
        }
    }    
    buffer
}


fn find_unfinished_replacements(file_before: Vec<u8>, offsets: Vec<usize>) -> usize {
    // there is at least one offset before 
    let last_nul_byte = match memmem::rfind(&file_before, b"\x00") {
        Some(last_nul_byte) =>  {last_nul_byte},
        None => 0
    };
    if offsets.last().unwrap() < &last_nul_byte {
        // the last 0 byte is after the last prefix meaning there is no unfinished replacement
        return 0
    } 
    let mut unfinished_replacements = 0;
    let reversed_offsets: Vec<usize> = offsets.into_iter().rev().collect();
    for offset in reversed_offsets {
        if offset >= last_nul_byte {
            unfinished_replacements += 1;
        } else {
            return unfinished_replacements
        }
    };
    unfinished_replacements
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf};
    use memmap2::MmapOptions;
    use rattler_conda_types::package::FileMode;
    use paths::PrefixPlaceholder;
    use crate::prefix_replacement::{binary_prefix_replacement, find_unfinished_replacements, text_prefix_replacement};

    #[test]
    fn test_find_one_unfinished_replacements(){
        let file_before = b"01ABCD2\x0034ABCD5";
        let offsets = vec![2, 10];

        let expected_unfinished_replacements = 1;
        let created_unfinished_replacements = find_unfinished_replacements(file_before.to_vec(), offsets.clone());
        assert_eq!(expected_unfinished_replacements, created_unfinished_replacements, "unfinished replacements failed for {:?}, expected unfinished replacements {expected_unfinished_replacements:?}, actual unfinished_replacements {created_unfinished_replacements:?}", &offsets);
    }

    #[test]
    fn test_find_two_unfinished_replacements_no_null_byte(){
        let file_before = b"01ABCD234ABCD5";
        let offsets = vec![2, 9];

        let expected_unfinished_replacements = 2;
        let created_unfinished_replacements = find_unfinished_replacements(file_before.to_vec(), offsets.clone());
        assert_eq!(expected_unfinished_replacements, created_unfinished_replacements, "unfinished replacements failed for {:?}, expected unfinished replacements {expected_unfinished_replacements:?}, actual unfinished_replacements {created_unfinished_replacements:?}", &offsets);
    }
    
    fn do_text_test(placeholder: &str, prefix: &str, before: &[u8], expected: &[u8], start: usize, end: usize) {
        let mut placeholder_obj = PrefixPlaceholder::new(
            FileMode::Text,
            placeholder.as_bytes().to_vec(),
        );
        let size = before.len();

        let mut file = MmapOptions::new().len(before.len()).map_anon().unwrap();
        file[0..before.len()].copy_from_slice(before);
        let file = file.make_read_only().unwrap();
        placeholder_obj.fill_offsets(&file);
        let mount_point = PathBuf::from(prefix);

        let created_buffer = text_prefix_replacement(&placeholder_obj, start, end, size, &file, &mount_point);
        assert_eq!(created_buffer, expected, "replacement failed for {before:?} to expected: {expected:?}, {start} to {end}");
    }


    fn do_binary_test(placeholder: &str, prefix: &str, before: &[u8], expected: &[u8], start: usize, end: usize) {
        let mut placeholder_obj = PrefixPlaceholder::new(
            FileMode::Binary,
            placeholder.as_bytes().to_vec(),
        );
        let size = before.len();

        let mut file = MmapOptions::new().len(before.len()).map_anon().unwrap();
        file[0..before.len()].copy_from_slice(before);
        let file = file.make_read_only().unwrap();
        placeholder_obj.fill_offsets(&file);
        let mount_point = PathBuf::from(prefix);

        let created_buffer = binary_prefix_replacement(&placeholder_obj, start, end, size, &file, &mount_point);
        assert_eq!(created_buffer, expected, "replacement failed for {before:?} to expected: {expected:?}, {start} to {end}");
    }

    #[test]
    fn test_binary_replacement_full_file_multiple_placeholders() {
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"\x00\x00ABCDZ\x00\x00\x00ABCDEFABCDEF\x00\x00\x00ABCDMNOPQRSABCDMNOPQRSABCDMNOPQRS\x00\x00";
        let start = 0;
        let end = before.len(); 
        
        let expected = b"\x00\x00XYZ\x00\x00\x00\x00\x00XYEFXYEF\x00\x00\x00\x00\x00\x00\x00XYMNOPQRSXYMNOPQRSXYMNOPQRS\x00\x00\x00\x00\x00\x00\x00\x00";
        do_binary_test(placeholder, prefix, before, expected, start, end);
    }

    #[test]
    fn test_text_prefix_replacement_full_file() {
        do_text_test("ABCD", "XY", b"01ABCD23456ABCD7890", b"01XY23456XY7890", 0, b"01ABCD23456ABCD7890".len());
    }

    #[test]
    fn test_binary_prefix_replacement_full_file() {
        do_binary_test("ABCD", "XY", b"01ABCD23\x00456ABCD78\x0090", b"01XY23\x00\x00\x00456XY78\x00\x00\x0090", 0, b"01ABCD23\x00456ABCD78\x0090".len());
    }

    #[test]
    fn test_text_prefix_replacement_partial_range() {
        // Replace only a portion of the file
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"ABCD0ABCD5ABCD0ABCD5ABCD";
        let start = 2;
        let end = 9; // Only process middle section
        
        let expected = b"0XY5XY0";
        do_text_test(placeholder, prefix, before, expected, start, end);
    }

    #[test]
    fn test_binary_prefix_replacement_partial_range() {
        // Replace only a portion of the file
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"ABCD\x000ABCD\x005ABCD\x000ABCD\x005ABCD\x00";
        let start = 5;
        let end = 10; // Only process middle section
        
        let expected = b"0XY\x00\x00";
        do_binary_test(placeholder, prefix, before, expected, start, end);
    }

    #[test]
    fn test_text_prefix_replacement_start_after_prefix() {
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"ABCD01234ABCD56789";
        let expected = b"34XY56789";
        
        let mut placeholder_obj = PrefixPlaceholder::new(
            FileMode::Text,
            placeholder.as_bytes().to_vec(),
        );
        let start = 5;
        let end = before.len();
        let size = before.len(); 

        let mut file = MmapOptions::new().len(end).map_anon().unwrap();
        file[0..end].copy_from_slice(before);
        let file = file.make_read_only().unwrap();
        placeholder_obj.fill_offsets(&file);
        let mount_point = PathBuf::from(prefix);

        let created_buffer = text_prefix_replacement(&placeholder_obj, start, end, size, &file, &mount_point);
        assert_eq!(created_buffer, expected, "Start after prefix failed");
    }

    #[test]
    fn test_binary_prefix_replacement_start_after_prefix() {
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"ABCD01234ABCD\x0056789";
        let expected = b"34XY\x00\x00\x00\x00\x0056789";
        
        let mut placeholder_obj = PrefixPlaceholder::new(
            FileMode::Binary,
            placeholder.as_bytes().to_vec(),
        );
        let start = 5;
        let end = before.len();
        let size = before.len(); 

        let mut file = MmapOptions::new().len(end).map_anon().unwrap();
        file[0..end].copy_from_slice(before);
        let file = file.make_read_only().unwrap();
        placeholder_obj.fill_offsets(&file);
        let mount_point = PathBuf::from(prefix);

        let created_buffer = 
        binary_prefix_replacement(&placeholder_obj, start, end, size, &file, &mount_point);
        assert_eq!(created_buffer, expected, "Start after prefix failed");
    }

    #[test]
    fn test_text_prefix_replacement_start_between_placeholders() {
        // Start in the middle, between two placeholders
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"ABCD0123ABCD5678ABCD";
        let expected = b"3XY5678XY"; // Starting at position 7
        
        let mut placeholder_obj = PrefixPlaceholder::new(
            FileMode::Text,
            placeholder.as_bytes().to_vec(),
        );
        let start = 5;
        let end = before.len();
        let size = end - start;

        let mut file = MmapOptions::new().len(end).map_anon().unwrap();
        file[0..end].copy_from_slice(before);
        let file = file.make_read_only().unwrap();
        placeholder_obj.fill_offsets(&file);
        let mount_point = PathBuf::from(prefix);

        let created_buffer = text_prefix_replacement(&placeholder_obj, start, end, size, &file, &mount_point);
        assert_eq!(created_buffer, expected, "Start between placeholders failed");
    }
    #[test]
    fn test_binary_prefix_replacement_start_between_placeholders() {
        // Start in the middle, between two placeholders
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"ABCD012\x003ABCD5678ABCD";
        let expected = b"012\x00\x00\x003XY5678XY\x00\x00\x00\x00"; // Starting at position 7
        
        let mut placeholder_obj = PrefixPlaceholder::new(
            FileMode::Binary,
            placeholder.as_bytes().to_vec(),
        );
        let start = 2;
        let end = before.len();
        let size = end - start;

        let mut file = MmapOptions::new().len(end).map_anon().unwrap();
        file[0..end].copy_from_slice(before);
        let file = file.make_read_only().unwrap();
        placeholder_obj.fill_offsets(&file);
        let mount_point = PathBuf::from(prefix);

        let created_buffer = binary_prefix_replacement(&placeholder_obj, start, end, size, &file, &mount_point);
        assert_eq!(created_buffer, expected, "Start between placeholders failed");
    }

    #[test]
    fn test_text_prefix_replacement_start_at_placeholder() {
        // Start exactly at a placeholder position
        do_text_test("ABCD", "XY", b"01234ABCD6789ABCD", b"XY6789XY", 5, b"01234ABCD6789ABCD".len());
    }

    #[test]
    fn test_binary_prefix_replacement_start_at_placeholder() {
        // Start exactly at a placeholder position
        do_binary_test("ABCD", "XY", b"01234ABCD\x006789ABCD\x00", b"XY\x00\x00\x006789XY\x00\x00\x00", 5, b"01234ABCD\x006789ABCD\x00".len());
    }

    #[test]
    fn test_text_prefix_replacement_no_placeholders() {
        do_text_test("ABCD", "XY", b"0123456789", b"0123456789", 0, b"0123456789".len());
    }

    #[test]
    fn test_binary_prefix_replacement_no_placeholders() {
        do_binary_test("ABCD", "XY", b"0123456789", b"0123456789", 0, b"0123456789".len());
    }

    #[test]
    fn test_text_prefix_replacement_only_placeholder() {
        do_text_test("ABCD", "XY", b"ABCD", b"XY", 0, b"ABCD".len());
    }

    #[test]
    fn test_binary_prefix_replacement_only_placeholder() {
        do_binary_test("ABCD", "XY", b"ABCD", b"XY\x00\x00", 0, b"ABCD".len());
    }

    #[test]
    fn test_text_prefix_replacement_start_with_placeholder() {
        do_text_test("ABCD", "XY", b"ABCD01234", b"XY01234", 0, b"ABCD01234".len());
    }

    #[test]
    fn test_binary_prefix_replacement_start_with_placeholder() {
        do_binary_test("ABCD", "XY", b"ABCD\x0001234", b"XY\x00\x00\x0001234", 0, b"ABCD\x0001234".len());
    }

    #[test]
    fn test_text_prefix_replacement_end_with_placeholder() {
        do_text_test("ABCD", "XY", b"01234ABCD", b"01234XY", 0, b"01234ABCD".len());
    }

    #[test]
    fn test_binary_prefix_replacement_end_with_placeholder() {
        do_binary_test("ABCD", "XY", b"01234ABCD", b"01234XY\x00\x00", 0, b"01234ABCD".len());
    }

    #[test]
    fn test_text_prefix_replacement_consecutive_placeholders() {
        do_text_test("ABCD", "XY", b"ABCDABCD", b"XYXY", 0, b"ABCDABCD".len());
    }

     #[test]
    fn test_binary_prefix_replacement_consecutive_placeholders() {
        do_binary_test("ABCD", "XY", b"ABCDABCD", b"XYXY\x00\x00\x00\x00", 0, b"ABCDABCD".len());
    }

    #[test]
    fn test_text_prefix_replacement_same_length() {
        do_text_test("ABCD", "WXYZ", b"01ABCD6789012ABCD7890", b"01WXYZ6789012WXYZ7890", 0, b"01ABCD6789012ABCD7890".len());
    }

    #[test]
    fn test_binary_prefix_replacement_same_length() {
        do_binary_test("ABCD", "WXYZ", b"01ABCD6789012ABCD7890", b"01WXYZ6789012WXYZ7890", 0, b"01ABCD6789012ABCD7890".len());
    }

    #[test]
    fn test_text_prefix_replacement_empty_file() {
        do_text_test("ABCD", "XY", b"", b"", 0, b"".len());
    }

    #[test]
    fn test_binary_prefix_replacement_empty_file() {
        do_binary_test("ABCD", "XY", b"", b"", 0, b"".len());
    }

    #[test]
    fn test_text_prefix_replacement_single_char_placeholder() {
        do_text_test("X", "A", b"0X2X4X6X8", b"0A2A4A6A8", 0, b"0X2X4X6X8".len());
    }

    #[test]
    fn test_binary_prefix_replacement_single_char_placeholder() {
        do_binary_test("X", "A", b"0X2X4X6X8", b"0A2A4A6A8", 0, b"0X2X4X6X8".len());
    }

    #[test]
    fn test_text_prefix_replacement_many_placeholders() {
        let mut before = Vec::new();
        let mut expected = Vec::new();
        for i in 0..10 {
            before.extend_from_slice(format!("{:02}ABCD", i).as_bytes());
            expected.extend_from_slice(format!("{:02}XY", i).as_bytes());
        }
        do_text_test("ABCD", "XY", &before, &expected, 0, before.len());
    }

    #[test]
    fn test_binary_prefix_replacement_many_placeholders() {
        let mut before = Vec::new();
        let mut expected = Vec::new();
        for i in 0..10 {
            before.extend_from_slice(format!("{:02}ABCD\x00", i).as_bytes());
            expected.extend_from_slice(format!("{:02}XY\x00\x00\x00", i).as_bytes());
        }
        do_binary_test("ABCD", "XY", &before, &expected, 0, before.len());
    }

    #[test]
    fn test_text_prefix_replacement_longer_prefix() {
        // Test with a longer replacement (should still work if placeholder is longer)
        do_text_test("ABCDEFGH", "XYZ", b"01ABCDEFGH234ABCDEFGH567", b"01XYZ234XYZ567", 0, b"01ABCDEFGH234ABCDEFGH567".len());
    }

    #[test]
    fn test_binary_prefix_replacement_longer_prefix() {
        // Test with a longer replacement (should still work if placeholder is longer)
        do_binary_test("ABCDEFGH", "XYZ", b"01ABCDEFGH234ABCDEFGH567", b"01XYZ234XYZ567\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00", 0, b"01ABCDEFGH234ABCDEFGH567".len());
    }

    #[test]
    fn test_text_prefix_replacement_three_char_to_one() {
        do_text_test("ABC", "X", b"00ABC11ABC22ABC33", b"00X11X22X33", 0, b"00ABC11ABC22ABC33".len());
    }

    #[test]
    fn test_binary_prefix_replacement_three_char_to_one() {
        do_binary_test("ABC", "X", b"00ABC11ABC22ABC33", b"00X11X22X33\x00\x00\x00\x00\x00\x00", 0, b"00ABC11ABC22ABC33".len());
    }

    #[test]
    fn test_text_prefix_replacement_with_special_chars() {
        let before = b"{\n  \"path\": \"ABCD/file\",\n  \"root\": \"ABCD\"\n}";
        do_text_test("ABCD", "XY", before, b"{\n  \"path\": \"XY/file\",\n  \"root\": \"XY\"\n}", 0, before.len());
    }

    #[test]
    fn test_text_prefix_replacement_placeholder_at_boundary() {
        // Placeholder right at the end boundary
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"01234567ABCD";
        let start = 0;
        let end = 12; // Includes the placeholder
        
        do_text_test(placeholder, prefix, before, b"01234567XY", start, end);
    }

    #[test]
    fn test_binary_prefix_replacement_placeholder_at_boundary() {
        // Placeholder right at the end boundary
        let placeholder = "ABCD";
        let prefix = "XY";
        let before = b"01234567ABCD";
        let start = 0;
        let end = 12; // Includes the placeholder
        
        do_binary_test(placeholder, prefix, before, b"01234567XY\x00\x00", start, end);
    }
}
