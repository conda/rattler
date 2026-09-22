#!/usr/bin/env nu

# Paths
let tmp = ($env.RUNNER_TEMP? | default $env.TEMP? | default "/tmp")
let data_dir = $"($tmp)/rustfs-data"
let log_file = $"($tmp)/rustfs.log"
# Keep the `rc` aliases out of the user's home directory so running this locally
# does not clobber a real configuration.
let rc_config_dir = $"($tmp)/rc-config"
let bucket_name = $"tmp-(random int 0..1000000)"

# Create directories
mkdir $data_dir $rc_config_dir

# Credentials
let access_key = ($env.RUSTFS_ACCESS_KEY? | default "rustfs")
let secret_key = ($env.RUSTFS_SECRET_KEY? | default "rustfs123")

# Read the `Cache-Control` header of an anonymously accessible URL.
def cache-control [url: string]: nothing -> string {
    let header = (
        ^curl -sS -I $url
        | lines
        | where {|line| ($line | str downcase | str starts-with "cache-control:") }
    )
    if ($header | is-empty) {
        ""
    } else {
        $header | first | split row ":" | get 1 | str trim
    }
}

def check-cache-control [url: string, expected: string, label: string] {
    let actual = (cache-control $url)
    if $actual != $expected {
        print $"DEBUG: Full headers for ($label):"
        print (^curl -sS -I $url)
        error make {msg: $"Expected ($label) to have '($expected)' but got '($actual)'"}
    }
    print $"✓ ($label) has correct cache control \('($expected)')"
}

# Start RustFS in background as a job
print "== Starting RustFS server..."
let rustfs_job = job spawn {
    (^rustfs server $data_dir
        --address ":9000"
        --access-key $access_key
        --secret-key $secret_key
        out+err> $log_file)
}

# wait up to 120s (60 × 2s) for RustFS to be ready
if not (seq 0 59 | any {|_|
    try { http get http://localhost:9000/health/ready | ignore; true } catch { sleep 2sec; false }
}) {
    print (open $log_file)
    error make {msg: "RustFS failed to start within 120 seconds"}
}
print "RustFS server is up and running..."

# Configure rc client and bucket
print $"== Configuring bucket ($bucket_name)..."
with-env {RC_CONFIG_DIR: $rc_config_dir} {
    ^rc alias set rustfs http://localhost:9000 $access_key $secret_key --bucket-lookup path
    ^rc bucket create $"rustfs/($bucket_name)"
    ^rc bucket anonymous set download $"rustfs/($bucket_name)"
}

print "== Upload packages to RustFS"
(^rattler
    upload s3
    --channel $"s3://($bucket_name)"
    --access-key-id $access_key
    --secret-access-key $secret_key
    --region "us-east-1"
    --endpoint-url "http://localhost:9000"
    --addressing-style path
    test-data/packages/empty-0.1.0-h4616a5c_0.conda
)

print "== Index the channel"
(^rattler-index
    s3
    $"s3://($bucket_name)"
    --access-key-id $access_key
    --secret-access-key $secret_key
    --region "us-east-1"
    --endpoint-url "http://localhost:9000"
    --addressing-style path
)

print "== Verify cache control headers are set correctly"
# repodata and the shard index get a 5-minute cache (300 seconds)
for name in ["repodata.json" "repodata.json.zst" "repodata_shards.msgpack.zst"] {
    check-cache-control $"http://localhost:9000/($bucket_name)/noarch/($name)" "public, max-age=300" $name
}

# Individual shard files are content-addressed, so they get an immutable cache (1 year).
# `rc object list` reports keys relative to the bucket, so they can be used as-is.
let shard_keys = (
    with-env {RC_CONFIG_DIR: $rc_config_dir} {
        (^rc object list --json $"rustfs/($bucket_name)/noarch/shards/"
            | from json
            | get items
            | each {|item| $item.key })
    }
)
if ($shard_keys | is-empty) {
    print "⚠ No shard files found to check"
} else {
    let first_shard = ($shard_keys | first)
    (check-cache-control
        $"http://localhost:9000/($bucket_name)/($first_shard)"
        "public, max-age=31536000, immutable"
        $first_shard)
}

print "== Test package can be installed from the channel ==="
with-env {
  AWS_ACCESS_KEY_ID: $access_key
  AWS_SECRET_ACCESS_KEY: $secret_key
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
