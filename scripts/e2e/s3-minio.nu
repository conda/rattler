#!/usr/bin/env nu

# Paths
let tmp = ($env.RUNNER_TMP? | default $env.TMP)
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
let args = [
  upload s3
  --channel $"s3://($bucket_name)"
  --access-key-id $root_user
  --secret-access-key $root_password
  --region "us-east-1"
  --endpoint-url "http://localhost:9000"
  --force-path-style true
  test-data/packages/empty-0.1.0-h4616a5c_0.conda
]

^rattler ...$args

print "== Index the channel ==="
let args = [
  s3
  $"s3://($bucket_name)"
  --access-key-id $root_user
  --secret-access-key $root_password
  --region "us-east-1"
  --endpoint-url "http://localhost:9000"
  --force-path-style true
]

^rattler-index ...$args

print "== Test package can be installed from the channel ==="
let args = [
  create
  --dry-run
  -c $"s3://($bucket_name)"
  empty==0.1.0
]

with-env {
  AWS_ACCESS_KEY_ID: $root_user
  AWS_SECRET_ACCESS_KEY: $root_password
  AWS_REGION: "us-east-1"
  AWS_ENDPOINT_URL: "http://localhost:9000"
} {
  ^rattler ...$args
}