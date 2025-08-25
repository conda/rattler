#!/bin/bash

# Script to build rattler-bin and run test_login binary

echo "Building test_login binary in release mode..."
cd crates/rattler-bin && cargo build --release --features cli-tools && cd ../../

# Check if build was successful
if [ $? -eq 0 ]; then
    echo "✅ Build successful!"
    echo ""
    echo "Running test_login..."
    echo "========================"
    cd crates/rattler && cargo run --bin test_login && cd ../../
else
    echo "❌ Build failed!"
    exit 1
fi

echo ""
echo "Script completed."