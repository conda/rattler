#!/usr/bin/env nu

let bucket_name = ($env.BUCKET? | default $"tmp-conda-rattler-(random int 0..1000000)")
let region = ($env.AWS_REGION? | default "eu-west-1")

def run [desc: string, cmd: closure] {
  print $"== ($desc)"
  do $cmd | ignore
  let code = ($env.LAST_EXIT_CODE? | default 0)
  if $code != 0 {
    print $"WARN: ($desc) failed \(exit=($code)\)"
    false
  } else { true }
}


def bucket_exists [] {
  (do { ^aws s3api head-bucket --bucket $bucket_name } | complete).exit_code == 0
}

# --- steps (don’t abort on failure) ---
let test_ok = (run $"Create bucket ($bucket_name)" {
  ^aws s3api create-bucket --bucket $bucket_name --create-bucket-configuration $"LocationConstraint=($region)"
}) and (run "Set lifecycle (1 day)" {
  ^aws s3api put-bucket-lifecycle-configuration --bucket $bucket_name --lifecycle-configuration '{ "Rules":[{"ID":"ttl-1d","Status":"Enabled","Expiration":{"Days":1},"Filter":{"Prefix":""}}] }'
}) and (run "Upload package" {
  ^rattler upload s3 --channel $"s3://($bucket_name)" test-data/packages/empty-0.1.0-h4616a5c_0.conda
}) and (run "Index channel" {
  ^rattler-index s3 $"s3://($bucket_name)"
}) and (run "Dry-run install" {
  ^rattler create --dry-run -c $"s3://($bucket_name)" empty==0.1.0
})

# --- cleanup always attempted ---
if (bucket_exists) {
  print "== Cleanup: remove bucket and all its contents"
  (do { ^aws s3 rm $"s3://($bucket_name)" --recursive } | complete) | ignore
  (do { ^aws s3 rb $"s3://($bucket_name)" } | complete) | ignore
} else {
  print "== Cleanup: bucket did not exist (skip)"
}

# --- exit non-zero if any step failed ---
if not $test_ok { exit 1 }
