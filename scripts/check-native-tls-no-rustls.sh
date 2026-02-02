#!/bin/bash
# Check that no package pulls in rustls when using native-tls feature
# (except rattler_s3 which requires rustls due to AWS SDK limitations)

set -euxo pipefail

SKIP_PACKAGES="rattler_s3"

# Get all workspace packages
packages=$(cargo metadata --no-deps --format-version 1 | jq -r '.packages[].name')

failed=0
checked=0
skipped=0

for package in $packages; do
    # Skip packages that are known to require rustls
    if echo "$SKIP_PACKAGES" | grep -qw "$package"; then
        echo "SKIP: $package (known rustls dependency)"
        ((skipped++))
        continue
    fi

    ((checked++))

    # Check if the package has native-tls feature
    has_native_tls=$(cargo metadata --no-deps --format-version 1 | jq -r --arg pkg "$package" '.packages[] | select(.name == $pkg) | .features | has("native-tls")')

    if [ "$has_native_tls" = "true" ]; then
        # Run cargo tree with native-tls feature (prod dependencies only)
        output=$(cargo tree -i rustls --no-default-features --features native-tls --package "$package" --locked --edges=normal 2>&1 || true)
    else
        # Run cargo tree without native-tls feature (prod dependencies only)
        output=$(cargo tree -i rustls --no-default-features --package "$package" --locked --edges=normal 2>&1 || true)
    fi

    if echo "$output" | grep -q "^rustls"; then
        echo "FAIL: $package has rustls dependency"
        echo "$output" | head -20
        echo ""
        ((failed++))
    else
        echo "OK:   $package"
    fi
done

echo ""
echo "Summary: $checked checked, $failed failed, $skipped skipped"

if [ $failed -gt 0 ]; then
    exit 1
fi
