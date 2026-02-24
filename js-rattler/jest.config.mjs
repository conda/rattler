import { createDefaultEsmPreset } from "ts-jest";

export default {
    ...createDefaultEsmPreset(),
    // Solve tests fetch live repodata over the network; 5 s (Jest default) is
    // too tight on CI runners with variable latency.
    testTimeout: 60_000,
};
