import { describe, expect, it } from "@jest/globals";
import { Config } from "./Config";
import { isRattlerError } from "./RattlerError";

describe("Config", () => {
    it("has defaults", () => {
        const config = new Config();
        expect(config.defaultChannels).toBeUndefined();
        expect(config.concurrencyDownloads).toBe(50);
        expect(config.concurrencySolves).toBeGreaterThanOrEqual(1);
    });

    it("parses a TOML string", () => {
        const config = Config.fromToml(`
            default-channels = ["conda-forge", "https://prefix.dev/bioconda"]

            [concurrency]
            downloads = 7
        `);
        expect(config.defaultChannels).toEqual([
            "conda-forge",
            "https://prefix.dev/bioconda",
        ]);
        expect(config.concurrencyDownloads).toBe(7);
    });

    it("reports unused keys", () => {
        const { config, unusedKeys } = Config.fromTomlWithUnusedKeys(`
            default-channels = ["conda-forge"]
            not-a-key = true
        `);
        expect(config).toBeInstanceOf(Config);
        expect(config.defaultChannels).toEqual(["conda-forge"]);
        expect(unusedKeys).toEqual(["not-a-key"]);
    });

    it("merges configurations", () => {
        const merged = Config.fromToml(
            `default-channels = ["conda-forge"]`,
        ).merge(Config.fromToml(`concurrency.downloads = 3`));
        expect(merged.defaultChannels).toEqual(["conda-forge"]);
        expect(merged.concurrencyDownloads).toBe(3);
    });

    it("marks invalid TOML with PARSE_CONFIG", () => {
        let error: unknown = null;
        try {
            Config.fromToml("default-channels = [");
        } catch (err) {
            error = err;
        }
        expect(isRattlerError(error)).toBe(true);
        if (isRattlerError(error)) {
            expect(error.code).toBe("PARSE_CONFIG");
        }
    });
});
