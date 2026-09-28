# Quick Start

`simpleSolve` resolves a set of package specs against one or more channels for a given platform, without needing a full `Gateway` setup:

```ts
--8<-- "examples/quick-start.mjs"
```

For repeated queries against the same channels, construct a [`Gateway`](../reference/index.md) instead — it caches fetched repodata so running the same query twice returns the previous results:

```ts
import { Gateway } from "@conda-org/rattler";

const gateway = new Gateway();
const names = await gateway.names(["conda-forge"], ["linux-64"]);
```

See the [Reference](../reference/index.md) section for the full API.
