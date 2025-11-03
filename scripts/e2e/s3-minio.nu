#!/usr/bin/env nu

# Paths
let tmp = ($env.RUNNER_TEMP? | default $env.TEMP? | default "/tmp")
let bin_dir = $tmp
let data_dir = $"($tmp)/minio-data"
let log_file = $"($tmp)/minio.log"
let pid_file = $"($tmp)/minio.pid"
let bucket_name = $"tmp-(random int 0..1000000)"

# Create directories
mkdir $bin_dir $data_dir

# Credentials
let root_user = ($env.MINIO_ACCESS_KEY? | default "minio")
let root_password = ($env.MINIO_SECRET_KEY? | default "minio123")

# Start MinIO in background as a job
print "== Starting Minio server..."
let minio_job = job spawn {
    with-env {
        MINIO_ROOT_USER: $root_user
        MINIO_ROOT_PASSWORD: $root_password
    } {
        ^minio server $data_dir --address ":9000" out+err> $log_file
    }
}

# wait up to 120s (60 × 2s) for MinIO to be ready
if not (seq 0 59 | any {|_|
    try { http get http://localhost:9000/minio/health/live | ignore; true } catch { sleep 2sec; false }
}) {
    error make {msg: "MinIO failed to start within 120 seconds"}
}
print "Minio server is up and running..."

# Configure mc client and bucket
print $"== Configuring bucket ($bucket_name)..."
^mc alias set minio http://localhost:9000 $root_user $root_password
^mc mb $"minio/($bucket_name)"
^mc policy set download $"minio/($bucket_name)"

print "== Upload packages to Minio"
(^rattler
    upload s3
    --channel $"s3://($bucket_name)"
    --access-key-id $root_user
    --secret-access-key $root_password
    --region "us-east-1"
    --endpoint-url "http://localhost:9000"
    --addressing-style path
    test-data/packages/empty-0.1.0-h4616a5c_0.conda
)

print "== Index the channel"
(^rattler-index
    s3
    $"s3://($bucket_name)"
    --access-key-id $root_user
    --secret-access-key $root_password
    --region "us-east-1"
    --endpoint-url "http://localhost:9000"
    --addressing-style path
)

print "== Verify cache control headers are set correctly"
# Check repodata.json has 5-minute cache (300 seconds)
let repodata_cache = (^mc stat --json $"minio/($bucket_name)/noarch/repodata.json" | from json | get metadata."Cache-Control"?)
if $repodata_cache != "public, max-age=300" {
    error make {msg: $"Expected repodata.json to have 'public, max-age=300' but got '($repodata_cache)'"}
}
print "✓ repodata.json has correct cache control (5 minutes)"

# Check repodata.json.zst has 5-minute cache (300 seconds)
let repodata_zst_cache = (^mc stat --json $"minio/($bucket_name)/noarch/repodata.json.zst" | from json | get metadata."Cache-Control"?)
if $repodata_zst_cache != "public, max-age=300" {
    error make {msg: $"Expected repodata.json.zst to have 'public, max-age=300' but got '($repodata_zst_cache)'"}
}
print "✓ repodata.json.zst has correct cache control (5 minutes)"

# Check shard index has 5-minute cache
let shard_index_cache = (^mc stat --json $"minio/($bucket_name)/noarch/repodata_shards.msgpack.zst" | from json | get metadata."Cache-Control"?)
if $shard_index_cache != "public, max-age=300" {
    error make {msg: $"Expected repodata_shards.msgpack.zst to have 'public, max-age=300' but got '($shard_index_cache)'"}
}
print "✓ repodata_shards.msgpack.zst has correct cache control (5 minutes)"

# Check individual shard files have immutable cache (1 year)
let shard_files = (^mc ls --json $"minio/($bucket_name)/noarch/shards/" | lines | each { |line| $line | from json | get key })
if ($shard_files | length) > 0 {
    let first_shard = ($shard_files | first)
    let shard_cache = (^mc stat --json $"minio/($bucket_name)/noarch/shards/($first_shard)" | from json | get metadata."Cache-Control"?)
    if $shard_cache != "public, max-age=31536000, immutable" {
        error make {msg: $"Expected shard files to have 'public, max-age=31536000, immutable' but got '($shard_cache)'"}
    }
    print "✓ Shard files have correct cache control (immutable, 1 year)"
} else {
    print "⚠ No shard files found to check"
}

print "== Test package can be installed from the channel ==="
with-env {
  AWS_ACCESS_KEY_ID: $root_user
  AWS_SECRET_ACCESS_KEY: $root_password
  AWS_REGION: "us-east-1"
  AWS_ENDPOINT_URL: "http://localhost:9000"
} {
  (^rattler
      create
      --dry-run
      -c $"s3://($bucket_name)"
      empty==0.1.0
  )
}
