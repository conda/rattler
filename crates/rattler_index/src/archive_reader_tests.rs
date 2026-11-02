use std::io::{self, BufRead, Cursor, Read};

use bytes::Bytes;
use rattler_conda_types::compression_level::CompressionLevel;
use rattler_package_streaming::write::{write_conda_package, write_tar_bz2_package};

use super::*;

const INDEX_JSON: &[u8] = br#"{
    "name": "reader-test", "version": "1.0", "build": "0", "build_number": 0,
    "subdir": "noarch", "depends": ["python >=3.10"]
}"#;

#[derive(Clone, Copy)]
enum Format {
    Conda,
    TarBz2,
}

impl Format {
    fn filename(self) -> &'static str {
        match self {
            Self::Conda => "reader-test-1.0-0.conda",
            Self::TarBz2 => "reader-test-1.0-0.tar.bz2",
        }
    }
}

fn archive(format: Format) -> Vec<u8> {
    let source = tempfile::tempdir().unwrap();
    fs::create_dir(source.path().join("info")).unwrap();
    let index = source.path().join("info/index.json");
    let payload = source.path().join("payload.bin");
    fs::write(&index, INDEX_JSON).unwrap();
    fs::write(&payload, [0, b'\r', b'\n', 0xff].repeat(4096)).unwrap();
    let mut output = Cursor::new(Vec::new());
    match format {
        Format::Conda => write_conda_package(
            &mut output,
            source.path(),
            &[index, payload],
            CompressionLevel::Default,
            None,
            "reader-test-1.0-0",
            None,
            None,
        ),
        Format::TarBz2 => write_tar_bz2_package(
            &mut output,
            source.path(),
            &[index, payload],
            CompressionLevel::Default,
            None,
            None,
        ),
    }
    .unwrap();
    output.into_inner()
}

struct ShortReads<'a> {
    remaining: &'a [u8],
    position: usize,
    interrupt_at: Option<usize>,
    fail_at: Option<(usize, io::ErrorKind)>,
}

impl Read for ShortReads<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        let available = self.fill_buf()?;
        let count = output.len().min(available.len());
        output[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl BufRead for ShortReads<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.interrupt_at.is_some_and(|at| self.position >= at) {
            self.interrupt_at = None;
            return Err(io::ErrorKind::Interrupted.into());
        }
        if let Some((at, kind)) = self.fail_at
            && self.position >= at
        {
            return Err(io::Error::new(kind, "injected archive read failure"));
        }
        Ok(&self.remaining[..self.remaining.len().min(7)])
    }

    fn consume(&mut self, count: usize) {
        self.remaining = &self.remaining[count..];
        self.position += count;
    }
}

fn record(format: Format, indexed: bool, reader: impl BufRead) -> io::Result<PackageRecord> {
    match (format, indexed) {
        (Format::Conda, false) => package_record_from_conda_reader(reader),
        (Format::TarBz2, false) => package_record_from_tar_bz2_reader(reader),
        (Format::Conda, true) => {
            indexed_package_record_from_conda_reader(reader).map(|indexed| indexed.record)
        }
        (Format::TarBz2, true) => {
            indexed_package_record_from_tar_bz2_reader(reader).map(|indexed| indexed.record)
        }
    }
}

#[test]
fn archive_records_preserve_short_reads_and_interruptions() {
    for format in [Format::Conda, Format::TarBz2] {
        let bytes = archive(format);
        let expected =
            package_record_from_index_json(&bytes, &mut Cursor::new(INDEX_JSON)).unwrap();
        for indexed in [false, true] {
            for interrupt_at in [None, Some(7)] {
                let reader = ShortReads {
                    remaining: &bytes,
                    position: 0,
                    interrupt_at,
                    fail_at: None,
                };
                assert_eq!(record(format, indexed, reader).unwrap(), expected);
            }
        }
    }
}

#[test]
fn archive_records_propagate_read_failures() {
    for format in [Format::Conda, Format::TarBz2] {
        let bytes = archive(format);
        for indexed in [false, true] {
            for kind in [io::ErrorKind::Other, io::ErrorKind::WouldBlock] {
                let reader = ShortReads {
                    remaining: &bytes,
                    position: 0,
                    interrupt_at: None,
                    fail_at: Some((7, kind)),
                };
                let error = record(format, indexed, reader).unwrap_err();
                assert_eq!(error.kind(), kind);
                assert_eq!(error.to_string(), "injected archive read failure");
            }
        }
    }
}

#[test]
fn archive_records_from_segmented_buffers() {
    for format in [Format::Conda, Format::TarBz2] {
        let bytes = archive(format);
        let expected =
            package_record_from_index_json(&bytes, &mut Cursor::new(INDEX_JSON)).unwrap();
        let buffer = bytes
            .chunks(7)
            .map(Bytes::copy_from_slice)
            .collect::<opendal::Buffer>();
        let actual = parse_package_buffer(buffer, format.filename()).unwrap();
        assert_eq!(actual.record, expected);
    }
}
