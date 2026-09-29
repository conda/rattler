//! Reading complete layer files, e.g. to merge layers into a new base layer.
//! Files read this way are verified against the SHA-256 in their name.

use bytes::Bytes;
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

use crate::{
    Kind, Location, LookupError,
    format::{KEY_LEAF, VALUES_LEAF},
    source::ByteSource,
    table::{LookupTable, PackagesFile},
    write::{sha256_bytes, sha256_of_name},
};

/// Checks that `bytes` has the SHA-256 the content-addressed `name` claims.
pub fn verify_sha256(name: &str, bytes: &[u8], location: &Location) -> Result<(), LookupError> {
    let Some(expected) = sha256_of_name(name) else {
        return Err(LookupError::invalid_file(
            location,
            format!("`{name}` is not a content-addressed file name"),
        ));
    };
    let actual = sha256_bytes(bytes);
    if actual != expected {
        return Err(LookupError::DigestMismatch {
            location: Box::new(location.clone()),
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

/// Reads all filenames of a packages file that is completely in memory.
pub async fn read_packages(bytes: Bytes, location: &Location) -> Result<Vec<String>, LookupError> {
    let source = ByteSource::from_bytes(location.clone(), bytes);
    PackagesFile::from_source(source, None)
        .await?
        .read_all()
        .await
}

/// Reads all rows of a lookup table that is completely in memory: the keys
/// with their package ids, in key order. The table must be of `kind`.
pub async fn read_table(
    bytes: Bytes,
    kind: Kind,
    location: &Location,
) -> Result<Vec<(String, Vec<u32>)>, LookupError> {
    // Validate the file (metadata, columns) the same way a lookup does.
    let table = LookupTable::from_source(
        ByteSource::from_bytes(location.clone(), bytes.clone()),
        None,
    )
    .await?;
    if table.kind() != kind {
        return Err(LookupError::invalid_file(
            location,
            format!("it is a `{}` table, not a `{kind}` table", table.kind()),
        ));
    }
    drop(table);

    let builder = ParquetRecordBatchReaderBuilder::try_new(bytes)?;
    let projection =
        parquet::arrow::ProjectionMask::leaves(builder.parquet_schema(), [KEY_LEAF, VALUES_LEAF]);
    let reader = builder
        .with_projection(projection)
        .with_batch_size(8192)
        .build()?;
    let mut rows = Vec::new();
    for batch in reader {
        let batch = batch?;
        let keys = arrow_array::cast::AsArray::as_string::<i32>(batch.column(0));
        let lists = arrow_array::cast::AsArray::as_list::<i32>(batch.column(1));
        for row in 0..batch.num_rows() {
            let ids = lists.value(row);
            let ids =
                arrow_array::cast::AsArray::as_primitive::<arrow_array::types::UInt32Type>(&ids)
                    .values()
                    .to_vec();
            rows.push((keys.value(row).to_string(), ids));
        }
    }
    Ok(rows)
}
