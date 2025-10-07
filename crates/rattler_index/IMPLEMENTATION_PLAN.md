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
**Status**: Complete

### Tasks:
- [x] Remove `PackageRegistry` struct
- [x] Create `RepodataFileMetadata` struct with `new()` method
- [x] Create `RepodataMetadataCollection` struct to track all critical files:
  - `repodata.json`
  - `repodata_from_packages.json`
  - `repodata.json.zst`
  - `repodata_shards.msgpack.zst`

**Success Criteria**:
- ✅ Can efficiently collect metadata for all repodata files
- ✅ Handles non-existent files (None values)

---

## Stage 2: Implement Guarded Read
**Goal**: Read repodata.json with ETag validation
**Status**: Complete

### Tasks:
- [x] Create `read_with_metadata_check()` utility function
- [x] Uses `read_with().if_match()` or `if_unmodified_since()` based on metadata
- [x] Returns `ConditionNotMatch` error if validation fails
- [x] No automatic retries (retry at higher level)

**Success Criteria**:
- ✅ Read fails with `ConditionNotMatch` if ETag changed
- ✅ No automatic retries in read function

---

## Stage 3: Implement Conditional Writes
**Goal**: Write repodata files with ETag conditions
**Status**: Complete

### Tasks:
- [x] Update `write_repodata` signature to accept `RepodataMetadataCollection`
- [x] Remove `write_zst` and `write_shards` parameters (inferred from metadata)
- [x] Apply conditional writes using stored ETags to all critical files
- [x] Collect metadata at the start of `index_subdir`
- [x] Use metadata for conditional read of existing repodata

**Success Criteria**:
- ✅ All writes conditional on initial ETags
- ✅ Errors propagate correctly
- ✅ Read operations also guarded by metadata

---

## Stage 4: Implement Retry Loop
**Goal**: Wrap entire operation in retry loop
**Status**: Complete

### Tasks:
- [x] Refactor `index_subdir` body into `index_subdir_inner` function
- [x] Add outer retry loop with `default_retry_policy`
- [x] Detect `ConditionNotMatch` errors and retry entire operation
- [x] Add exponential backoff via retry policy
- [x] Add logging for retry attempts

**Success Criteria**:
- ✅ Entire operation retries on any ConditionNotMatch
- ✅ Respects retry limits from `default_retry_policy`
- ✅ Clear logging when retries occur
- ✅ Non-race-condition errors propagate immediately

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
