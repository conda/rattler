use super::*;

#[test]
fn raw_script_headers_and_metadata() {
    let shebang = format!("#!{SCRIPT_PREFIX_PLACEHOLDER}/bin/python\n");
    let cases: &[(&[u8], &[u8], bool)] = &[
        (b"#!python\nbody\n", b"body\n", true),
        (b"#!pythonw\r\nbody\r\n", b"body\r\n", true),
        (b"#!python -O\n\xff\xfe", b"\xff\xfe", true),
        (b"#!python", b"", true),
        (b"#!pythonw", b"", true),
        (
            b"#!/bin/sh\necho hello\n",
            b"#!/bin/sh\necho hello\n",
            false,
        ),
        (b"#!py", b"#!py", false),
        (b"", b"", false),
    ];
    for &(source, body, rewritten) in cases {
        let root = tempfile::tempdir().unwrap();
        let path = PathBuf::from("python-scripts/probe");
        let entry = write_script_entry(root.path(), path.clone(), source, Some(0o644)).unwrap();
        let bytes = std::fs::read(root.path().join(&path)).unwrap();
        let expected = if rewritten {
            [shebang.as_bytes(), body].concat()
        } else {
            body.to_vec()
        };
        assert_eq!(bytes, expected);
        assert_eq!(entry.size_in_bytes, Some(bytes.len() as u64));
        assert_eq!(
            entry.sha256,
            Some(rattler_digest::compute_bytes_digest::<Sha256>(&bytes))
        );
        assert_eq!(entry.prefix_placeholder.is_some(), rewritten);
        if let Some(placeholder) = entry.prefix_placeholder {
            assert_eq!(placeholder.file_mode, FileMode::Text);
            assert_eq!(placeholder.placeholder, SCRIPT_PREFIX_PLACEHOLDER);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Compare to a normal file created with the same umask, rather than
            // changing the process-global umask or assuming a particular value.
            use std::os::unix::fs::OpenOptionsExt;
            let reference = root.path().join("reference");
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(if rewritten { 0o777 } else { 0o666 })
                .open(&reference)
                .unwrap();
            assert_eq!(
                std::fs::metadata(root.path().join(path))
                    .unwrap()
                    .permissions()
                    .mode(),
                std::fs::metadata(reference).unwrap().permissions().mode(),
            );
        }
    }
}
