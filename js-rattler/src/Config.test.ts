import { describe, expect, it, jest } from "@jest/globals";
import { Config } from "./Config";
import { Gateway } from "./Gateway";
import { isRattlerError } from "./RattlerError";

describe("Config", () => {
    it("has defaults", () => {
        const config = new Config();
        expect(config.defaultChannels).toBeUndefined();
        expect(config.concurrencyDownloads).toBe(50);
        expect(config.concurrencySolves).toBeGreaterThanOrEqual(1);
        expect(config.toJson()).toEqual({});
    });

    it("is constructed from a plain object", () => {
        const config = new Config({
            "default-channels": ["conda-forge", "https://prefix.dev/bioconda"],
            concurrency: { downloads: 7 },
        });
        expect(config.defaultChannels).toEqual([
            "conda-forge",
            "https://prefix.dev/bioconda",
        ]);
        expect(config.concurrencyDownloads).toBe(7);
    });

    it("round-trips through toJson and fromJson", () => {
        const json = {
            "default-channels": ["conda-forge"],
            mirrors: {
                "https://conda.anaconda.org/conda-forge/": [
                    "https://prefix.dev/conda-forge/",
                ],
            },
            "repodata-config": {
                "disable-zstd": true,
                "https://prefix.dev/": { "disable-sharded": true },
            },
            "tls-no-verify": true,
        };
        const config = Config.fromJson(json);
        expect(config).toBeInstanceOf(Config);
        expect(config.toJson()).toEqual(json);
        expect(Config.fromJson(config.toJson()).toJson()).toEqual(json);
    });

    it("warns about unknown keys", () => {
        const warn = jest
            .spyOn(console, "warn")
            .mockImplementation(() => undefined);
        try {
            const config = new Config({
                "default-channels": ["conda-forge"],
                "not-a-key": true,
            } as never);
            expect(config.defaultChannels).toEqual(["conda-forge"]);
            expect(warn).toHaveBeenCalledWith(
                expect.stringContaining("not-a-key"),
            );
        } finally {
            warn.mockRestore();
        }
    });

    it("rejects malformed values with INVALID_CONFIG", () => {
        let error: unknown = null;
        try {
            new Config({ concurrency: { downloads: "many" } } as never);
        } catch (err) {
            error = err;
        }
        expect(isRattlerError(error)).toBe(true);
        if (isRattlerError(error)) {
            expect(error.code).toBe("INVALID_CONFIG");
        }
    });

    it("merges configurations", () => {
        const merged = new Config({
            "default-channels": ["conda-forge"],
        }).merge(new Config({ concurrency: { downloads: 3 } }));
        expect(merged.defaultChannels).toEqual(["conda-forge"]);
        expect(merged.concurrencyDownloads).toBe(3);
    });

    describe("gatewayOptions", () => {
        it("maps the repodata and concurrency config", () => {
            const config = new Config({
                concurrency: { downloads: 4 },
                "repodata-config": {
                    "disable-bzip2": true,
                    "https://prefix.dev/": { "disable-sharded": true },
                },
            });
            expect(config.gatewayOptions()).toEqual({
                maxConcurrentRequests: 4,
                channelConfig: {
                    default: {
                        zstdEnabled: true,
                        bz2Enabled: false,
                        shardedEnabled: true,
                    },
                    perChannel: {
                        // Per-channel entries inherit the defaults.
                        "https://prefix.dev/": {
                            zstdEnabled: true,
                            bz2Enabled: false,
                            shardedEnabled: false,
                        },
                    },
                },
            });
        });

        it("configures a gateway", async () => {
            const seen: string[] = [];
            const config = new Config({
                "repodata-config": {
                    "disable-zstd": true,
                    "disable-bzip2": true,
                    "disable-sharded": true,
                },
            });
            const gateway = new Gateway({
                ...config.gatewayOptions(),
                fetch: (request) => {
                    seen.push(request.url);
                    return Promise.resolve(
                        new Response(
                            JSON.stringify({
                                info: { subdir: "noarch" },
                                packages: {},
                                "packages.conda": {},
                            }),
                            { status: 200 },
                        ),
                    );
                },
            });

            await gateway.query(
                ["https://example.com/from-config"],
                ["noarch"],
                ["foo"],
            );

            expect(seen).toEqual([
                "https://example.com/from-config/noarch/repodata.json",
            ]);
        });
    });
});
