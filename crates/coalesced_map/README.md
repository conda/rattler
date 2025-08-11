# coalesced_map

[![Crates.io](https://img.shields.io/crates/v/coalesced_map.svg)](https://crates.io/crates/coalesced_map)
[![Documentation](https://docs.rs/coalesced_map/badge.svg)](https://docs.rs/coalesced_map)
[![Build Status](https://github.com/conda/rattler/workflows/CI/badge.svg)](https://github.com/conda/rattler/actions)
[![License](https://img.shields.io/crates/l/coalesced_map.svg)](https://github.com/conda/rattler/blob/main/LICENSE)

A thread-safe, deduplicating map that ensures expensive computations are executed only once per key, even when multiple concurrent requests are made.

This map is designed for scenarios where multiple async tasks might request the same resource simultaneously. Instead of performing duplicate work, the `CoalescedMap` ensures that only the first request for a given key executes the initialization function, while subsequent concurrent requests wait for and receive the same result.
