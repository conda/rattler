# Concurrent S3 Indexing Implementation Plan

## Goal
Implement race-condition-resistant indexing for rattler_index when multiple processes index the same S3 bucket concurrently.

## Algorithm
1. **Collect ETags**: Get ETags/last-modified for all critical repodata files
2. **Read with validation**: Read existing repodata.json - if ETag doesn't match collected value, restart at step 1
3. **Index packages**: Determine which packages to add/remove and index new packages
4. **Write with conditions**: Write all repodata files using collected ETags as conditions
5. **Handle race**: If write fails with ConditionNotMatch, restart at step 1

---

## Stage 1: Create Metadata Tracking Structure
**Goal**: Simple struct to hold ETags/last-modified for critical files
**Status**: Not Started

### Tasks:
- [ ] Remove `PackageRegistry` struct
- [ ] Create `RepodataFileMetadata` struct:
  ```rust
  struct RepodataFileMetadata {
      etag: Option<String>,
      last_modified: Option<DateTime<Utc>>,
  }
  ```
- [ ] Create helper function `collect_repodata_metadata(op: &Operator, subdir: &str, has_patch: bool) -> RepodataFileMetadata` that:
  - Calls `op.stat()` on the relevant repodata file
  - Returns metadata without reading file contents

**Success Criteria**:
- Can efficiently collect metadata for repodata files
- Handles non-existent files (None values)

**Tests**:
- Test metadata collection for existing file
- Test metadata collection for non-existent file

---

## Stage 2: Implement Guarded Read
**Goal**: Read repodata.json with ETag validation
**Status**: Not Started

### Tasks:
- [ ] Modify repodata reading logic to:
  1. Accept `RepodataFileMetadata` parameter
  2. Use `read_with().if_match()` or `if_unmodified_since()` based on metadata
  3. Return `ConditionNotMatch` error if validation fails
- [ ] Remove `read_bytes_with_metadata` retry loop (we'll retry at higher level)
- [ ] Update `index_subdir` to pass metadata to read operation

**Success Criteria**:
- Read fails with `ConditionNotMatch` if ETag changed
- No automatic retries in read function

**Tests**:
- Test successful read with matching ETag
- Test read failure with mismatched ETag

---

## Stage 3: Implement Conditional Writes
**Goal**: Write repodata files with ETag conditions
**Status**: Not Started

### Tasks:
- [ ] Update `write_repodata` signature to accept `RepodataFileMetadata`
- [ ] Apply conditional writes using stored ETags:
  - `repodata.json` - `if_match` or `if_unmodified_since`
  - `repodata_from_packages.json` - `if_match` or `if_unmodified_since`
  - `repodata.json.zst` - `if_match` or `if_unmodified_since`
  - `repodata_shards.msgpack.zst` - `if_match` or `if_unmodified_since`
- [ ] Return `ConditionNotMatch` errors to caller

**Success Criteria**:
- All writes conditional on initial ETags
- Errors propagate correctly

**Tests**:
- Test write succeeds with valid ETag
- Test write fails with changed ETag

---

## Stage 4: Implement Retry Loop
**Goal**: Wrap entire operation in retry loop
**Status**: Not Started

### Tasks:
- [ ] Refactor `index_subdir` body into inner function
- [ ] Add outer retry loop:
  ```rust
  loop {
      // 1. Collect ETags
      let metadata = collect_repodata_metadata(...);

      // 2. Read with validation (fails if ETag mismatch)
      // 3. Index packages
      // 4. Write with conditions

      match result {
          Ok(_) => return Ok(()),
          Err(e) if is_condition_not_match(&e) => continue,
          Err(e) => return Err(e),
      }
  }
  ```
- [ ] Add retry count limit (use `default_retry_policy`)
- [ ] Add exponential backoff
- [ ] Add logging for retries

**Success Criteria**:
- Entire operation retries on any ConditionNotMatch
- Respects retry limits
- Clear logging

**Tests**:
- Test retry on read race condition
- Test retry on write race condition
- Test max retries exhausted

---

## Stage 5: Add Comprehensive Testing
**Goal**: Ensure concurrent operations work correctly
**Status**: Not Started

### Tasks:
- [ ] Add integration test simulating concurrent indexing
- [ ] Test scenario: Two processes index same subdir simultaneously
- [ ] Test scenario: Modification during read phase triggers retry
- [ ] Test scenario: Modification during write phase triggers retry

**Success Criteria**:
- Tests pass with concurrent indexing
- No data loss or corruption
- Retries work correctly

**Tests**:
- Concurrent indexing test
- Race condition during read
- Race condition during write

---

## Implementation Notes

### Key Design Decisions:
1. **Collect-then-validate pattern**: Always collect ETags first, then validate during read
2. **Complete retry on race**: Any `ConditionNotMatch` restarts entire operation
3. **No caching between retries**: Simple approach - redo all work on retry
4. **Conditional on all repodata files**: Guard against concurrent writes to any critical file

### Critical Files Protected:
- `repodata.json`
- `repodata_from_packages.json`
- `repodata.json.zst`
- `repodata_shards.msgpack.zst`

### Files to Modify:
- `crates/rattler_index/src/lib.rs` - Main logic, retry loop
- `crates/rattler_index/src/utils.rs` - Metadata collection helper
- `crates/rattler_index/tests/test_index.rs` - Tests

### Algorithm Flow:
```
loop {
  1. stat() all critical files → collect ETags
  2. read() repodata.json with if_match(etag)
     - If ConditionNotMatch: continue loop (restart at step 1)
  3. List packages, determine adds/removes
  4. Index new packages
  5. write() all repodata files with if_match(etag)
     - If ConditionNotMatch: continue loop (restart at step 1)
  6. Success: break loop
}
```
