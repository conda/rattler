#!/bin/bash
# Check that no package pulls in rustls when all features except rustls-tls are enabled
# (except rattler_s3 which requires rustls due to AWS SDK limitations)

set -euo pipefail

SKIP_PACKAGES="rattler_s3"

# Get workspace metadata once
metadata=$(cargo metadata --no-deps --format-version 1)

# Get all workspace packages
packages=$(echo "$metadata" | jq -r '.packages[].name')

failed=0
checked=0
skipped=0

for package in $packages; do
    # Skip packages that are known to require rustls
    if echo "$SKIP_PACKAGES" | grep -qw "$package"; then
        echo "SKIP: $package (known rustls dependency)"
        ((++skipped))
        continue
    fi

    ((++checked))

    # Get all features except rustls-tls, s3, gcp, and default for this package
    features=$(echo "$metadata" | jq -r --arg pkg "$package" '
        .packages[] | select(.name == $pkg) | .features | keys[] | select(. != "rustls-tls" and . != "default" and . != "s3" and . != "gcp")
    ' | tr '\n' ',' | sed 's/,$//')

    # Run cargo tree with all features except skipped features (prod dependencies only)
    if [ -n "$features" ]; then
        output=$(cargo tree -i rustls --no-default-features --features "$features" --package "$package" --locked --edges=normal 2>&1 || true)
    else
        output=$(cargo tree -i rustls --no-default-features --package "$package" --locked --edges=normal 2>&1 || true)
    fi

    if echo "$output" | grep -q "^rustls"; then
        echo "FAIL: $package has rustls dependency"
        echo "$output" | head -20
        echo ""
        ((++failed))
    else
        echo "OK:   $package"
    fi
done

echo ""
echo "Summary: $checked checked, $failed failed, $skipped skipped"

if [ $failed -gt 0 ]; then
    exit 1
fi
