# coalesced_map

A thread-safe, deduplicating map that ensures expensive computations are executed only once per key, even when multiple concurrent requests are made.

This map is designed for scenarios where multiple async tasks might request the same resource simultaneously. Instead of performing duplicate work, the `CoalescedMap` ensures that only the first request for a given key executes the initialization function, while subsequent concurrent requests wait for and receive the same result.