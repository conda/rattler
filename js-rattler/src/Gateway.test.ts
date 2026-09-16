import { describe, expect, it } from "@jest/globals";
import { Gateway } from "./Gateway";
import { Platform } from "./Platform";
import { isRattlerError } from "./RattlerError";

// Disable all repodata variants so the gateway requests exactly one URL per
// subdir: the plain `repodata.json`.
const plainOnly = {
    default: {
        shardedEnabled: false,
        zstdEnabled: false,
        bz2Enabled: false,
    },
};

describe("Gateway", () => {
    describe("constructor", () => {
        it("works without arguments", () => {
            expect(() => new Gateway()).not.toThrow();
            expect(() => new Gateway(null)).not.toThrow();
            expect(() => new Gateway(undefined)).not.toThrow();
        });
        it("throws on invalid arguments", () => {
            expect(() => new Gateway(true as any)).toThrow();
        });
        it("accepts an empty object", () => {
            expect(() => new Gateway({})).not.toThrow();
        });
        it("accepts null for maxConcurrentRequests", () => {
            expect(
                () =>
                    new Gateway({
                        maxConcurrentRequests: null,
                    }),
            ).not.toThrow();
        });
        it("accepts empty channelConfig", () => {
            expect(
                () =>
                    new Gateway({
                        channelConfig: {},
                    }),
            ).not.toThrow();
        });
        it("accepts perChannel channelConfig", () => {
            expect(
                () =>
                    new Gateway({
                        channelConfig: {
                            default: {},
                            perChannel: {
                                "https://prefix.dev": {
                                    bz2Enabled: false,
                                    shardedEnabled: false,
                                    zstdEnabled: false,
                                },
                            },
                        },
                    }),
            ).not.toThrow();
        });
    });
    describe("names", () => {
        const gateway = new Gateway();
        it("can query prefix.dev", () => {
            return gateway
                .names(
                    ["https://prefix.dev/emscripten-forge-dev"],
                    ["noarch", "emscripten-wasm32"],
                )
                .then((names) => {
                    expect(names.length).toBeGreaterThanOrEqual(177);
                });
        });
    });
    describe("query", () => {
        it("can query prefix.dev", async () => {
            const gateway = new Gateway();
            const records = await gateway.query(
                ["https://prefix.dev/emscripten-forge-dev"],
                ["noarch", "emscripten-wasm32"],
                ["regex"],
            );
            expect(records.length).toBeGreaterThanOrEqual(1);
            for (const record of records) {
                expect(record.name).toBe("regex");
                expect(record.url).toContain(
                    "https://prefix.dev/emscripten-forge-dev/",
                );
            }
        });
    });
    describe("custom fetch", () => {
        const repodata = JSON.stringify({
            info: { subdir: "noarch" },
            packages: {},
            "packages.conda": {
                "foo-1.0-h123_0.conda": {
                    name: "foo",
                    version: "1.0",
                    build: "h123_0",
                    build_number: 0,
                    subdir: "noarch",
                    depends: [],
                    timestamp: 1700000000000,
                },
            },
        });

        it("routes requests through the provided fetch", async () => {
            const seen: Request[] = [];
            const globalFetch = globalThis.fetch;
            const gateway = new Gateway({
                channelConfig: plainOnly,
                fetch: (request) => {
                    seen.push(request);
                    return Promise.resolve(
                        new Response(repodata, {
                            status: 200,
                            headers: { "content-type": "application/json" },
                        }),
                    );
                },
            });

            const records = await gateway.query(
                ["https://example.com/test-channel"],
                ["noarch"],
                ["foo"],
            );

            // The custom fetch is scoped to the gateway instance and must
            // not leak into the global fetch.
            expect(globalThis.fetch).toBe(globalFetch);

            expect(seen.length).toBeGreaterThanOrEqual(1);
            for (const request of seen) {
                expect(request.method).toBe("GET");
                expect(request.url).toContain("/test-channel/noarch/");
            }
            expect(records).toHaveLength(1);
            expect(records[0].name).toBe("foo");
            expect(records[0].version).toBe("1.0");
            expect(records[0].build).toBe("h123_0");
            expect(records[0].fn).toBe("foo-1.0-h123_0.conda");
            expect(records[0].url).toBe(
                "https://example.com/test-channel/noarch/foo-1.0-h123_0.conda",
            );
            expect(records.warnings).toEqual([]);
        });

        it("returns gateway warnings on the query result", async () => {
            // Repodata that points at a CEP-42 base channel that fails to
            // load, which surfaces as a non-fatal warning on the result.
            const relatedRepodata = JSON.stringify({
                info: {
                    subdir: "noarch",
                    channel_relations: {
                        base: "https://example.com/missing-base",
                    },
                },
                packages: {},
                "packages.conda": {},
            });
            const gateway = new Gateway({
                channelConfig: plainOnly,
                fetch: (request) => {
                    if (request.url.includes("/missing-base/")) {
                        return Promise.resolve(
                            new Response("nope", { status: 500 }),
                        );
                    }
                    return Promise.resolve(
                        new Response(relatedRepodata, { status: 200 }),
                    );
                },
            });

            const records = await gateway.query(
                ["https://example.com/test-channel"],
                ["noarch"],
                ["foo"],
            );

            expect(records).toHaveLength(0);
            expect(records.warnings.length).toBeGreaterThanOrEqual(1);
        });

        it("routes each gateway to its own fetch", async () => {
            const seenA: string[] = [];
            const seenB: string[] = [];
            const respond = (seen: string[]) => (request: Request) => {
                seen.push(request.url);
                return Promise.resolve(
                    new Response(repodata, {
                        status: 200,
                        headers: { "content-type": "application/json" },
                    }),
                );
            };
            const gatewayA = new Gateway({
                channelConfig: plainOnly,
                fetch: respond(seenA),
            });
            const gatewayB = new Gateway({
                channelConfig: plainOnly,
                fetch: respond(seenB),
            });

            await gatewayA.query(
                ["https://example.com/channel-a"],
                ["noarch"],
                ["foo"],
            );
            await gatewayB.query(
                ["https://example.com/channel-b"],
                ["noarch"],
                ["foo"],
            );

            expect(seenA.length).toBeGreaterThanOrEqual(1);
            expect(seenB.length).toBeGreaterThanOrEqual(1);
            expect(seenA.every((url) => url.includes("/channel-a/"))).toBe(
                true,
            );
            expect(seenB.every((url) => url.includes("/channel-b/"))).toBe(
                true,
            );
        });

        it("reports http errors from the provided fetch", async () => {
            const gateway = new Gateway({
                channelConfig: plainOnly,
                fetch: () =>
                    Promise.resolve(new Response("nope", { status: 500 })),
            });

            await expect(
                gateway.query(
                    ["https://example.com/broken-channel"],
                    ["noarch"],
                    ["foo"],
                ),
            ).rejects.toBeDefined();
        });
    });
    describe("error codes", () => {
        it("marks a missing channel with SUBDIR_NOT_FOUND", async () => {
            const gateway = new Gateway({
                channelConfig: plainOnly,
                fetch: () =>
                    Promise.resolve(new Response(null, { status: 404 })),
            });

            const error: unknown = await gateway
                .query(
                    ["https://example.com/missing-channel"],
                    ["noarch"],
                    ["foo"],
                )
                .then(
                    () => null,
                    (err: unknown) => err,
                );

            expect(isRattlerError(error)).toBe(true);
            if (isRattlerError(error)) {
                expect(error.code).toBe("SUBDIR_NOT_FOUND");
            }
        });

        it("marks fetch failures with FETCH", async () => {
            const gateway = new Gateway({
                channelConfig: plainOnly,
                fetch: () =>
                    Promise.resolve(new Response("nope", { status: 500 })),
            });

            const error: unknown = await gateway
                .query(
                    ["https://example.com/broken-channel"],
                    ["noarch"],
                    ["foo"],
                )
                .then(
                    () => null,
                    (err: unknown) => err,
                );

            expect(isRattlerError(error)).toBe(true);
            if (isRattlerError(error)) {
                expect(error.code).toBe("FETCH");
                expect(error.message).toContain("500");
            }
        });

        it("marks invalid platforms with PARSE_PLATFORM", async () => {
            const gateway = new Gateway();

            const error: unknown = await gateway
                .query(
                    ["https://example.com/channel"],
                    ["not-a-platform" as Platform],
                    ["foo"],
                )
                .then(
                    () => null,
                    (err: unknown) => err,
                );

            expect(isRattlerError(error)).toBe(true);
            if (isRattlerError(error)) {
                expect(error.code).toBe("PARSE_PLATFORM");
            }
        });
    });
    describe("onWarning", () => {
        it("routes gateway warnings to the callback", async () => {
            const relatedRepodata = JSON.stringify({
                info: {
                    subdir: "noarch",
                    channel_relations: {
                        base: "https://example.com/missing-base",
                    },
                },
                packages: {},
                "packages.conda": {},
            });
            const warnings: string[] = [];
            const gateway = new Gateway({
                channelConfig: plainOnly,
                fetch: (request) => {
                    if (request.url.includes("/missing-base/")) {
                        return Promise.resolve(
                            new Response("nope", { status: 500 }),
                        );
                    }
                    return Promise.resolve(
                        new Response(relatedRepodata, { status: 200 }),
                    );
                },
                onWarning: (message) => {
                    warnings.push(message);
                },
            });

            await gateway.query(
                ["https://example.com/test-channel"],
                ["noarch"],
                ["foo"],
            );

            expect(warnings.length).toBeGreaterThanOrEqual(1);
        });
    });
});
