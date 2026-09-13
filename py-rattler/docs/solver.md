# solve

`exclude_newer` accepts a fixed datetime or a minimum age (`timedelta`). By
default, filtering uses the channel's `indexed_timestamp`, falls back to the
build `timestamp`, and excludes records missing both. Records exactly at the
cutoff remain eligible.

Use `timestamp_policy="allow-missing"` to include records missing both timestamps,
or `timestamp_policy="require-indexed-timestamp"` to require publication metadata
without falling back to build time. The policy applies only when `exclude_newer`
is set and is shared by all packages and channels.

::: rattler.solver.solver
