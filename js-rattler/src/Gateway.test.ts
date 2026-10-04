import { describe, expect, it } from "@jest/globals";
import { Gateway } from "./Gateway";
import { Platform } from "./Platform";
import { isRattlerError } from "./RattlerError";
import { MatchSpec } from "./MatchSpec";
import { RepoDataRecord } from "./RepoDataRecord";
import { PackageRecord } from "./PackageRecord";
import { Config } from "./Config";

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
            expect(records[0]).toBeInstanceOf(RepoDataRecord);
            expect(records[0].version.source).toBe("1.0");
            expect(records[0].build).toBe("h123_0");
            expect(records[0].fileName).toBe("foo-1.0-h123_0.conda");
            expect(records[0].url).toBe(
                "https://example.com/test-channel/noarch/foo-1.0-h123_0.conda",
            );
            expect(records.warnings).toEqual([]);
        });

        it("accepts MatchSpec objects with name globs", async () => {
            const gateway = new Gateway({
                channelConfig: plainOnly,
                fetch: () =>
                    Promise.resolve(
                        new Response(repodata, {
                            status: 200,
                            headers: { "content-type": "application/json" },
                        }),
                    ),
            });

            const spec = new MatchSpec("f* >=1", { exactNamesOnly: false });
            const records = await gateway.query(
                ["https://example.com/test-channel"],
                ["noarch"],
                [spec],
            );

            // The caller's spec stays usable after the query.
            expect(spec.toString()).toBe("f* >=1");
            expect(spec.matchesRepoDataRecord(records[0])).toBe(true);
            expect(
                await gateway.query(
                    ["https://example.com/test-channel"],
                    ["noarch"],
                    ["f*"],
                ),
            ).toHaveLength(1);

            expect(records.map((record) => record.fileName)).toEqual([
                "foo-1.0-h123_0.conda",
            ]);
            expect(records[0].channel).toBe(
                "https://example.com/test-channel/",
            );
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
        it("marks invalid specs with PARSE_MATCH_SPEC", async () => {
            const gateway = new Gateway();

            const error: unknown = await gateway
                .query(["https://example.com/channel"], ["noarch"], [">=1"])
                .then(
                    () => null,
                    (err: unknown) => err,
                );

            expect(isRattlerError(error)).toBe(true);
            if (isRattlerError(error)) {
                expect(error.code).toBe("PARSE_MATCH_SPEC");
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
    describe("whoNeeds", () => {
        const record = (
            name: string,
            fields: Record<string, unknown> = {},
        ): Record<string, unknown> => ({
            name,
            version: "1.0",
            build: "h123_0",
            build_number: 0,
            subdir: "noarch",
            depends: [],
            ...fields,
        });
        const repodata = JSON.stringify({
            info: { subdir: "noarch" },
            packages: {},
            "packages.conda": {
                "bar-1.0-h123_0.conda": record("bar"),
                "foo-1.0-h123_0.conda": record("foo", {
                    depends: ["bar >=1"],
                }),
                "old-1.0-h123_0.conda": record("old", {
                    depends: ["bar <1"],
                }),
                "pinned-1.0-h123_0.conda": record("pinned", {
                    constrains: ["bar >=2"],
                }),
                "extra-1.0-h123_0.conda": record("extra", {
                    extra_depends: { speedups: ["bar"] },
                }),
                "gpu-1.0-h123_0.conda": record("gpu", {
                    depends: ["__cuda >=12"],
                }),
                "unrelated-1.0-h123_0.conda": record("unrelated", {
                    depends: ["baz"],
                }),
            },
        });
        const gateway = () =>
            new Gateway({
                channelConfig: plainOnly,
                fetch: () =>
                    Promise.resolve(new Response(repodata, { status: 200 })),
            });
        const channels = ["https://example.com/who-needs"];

        it("finds every dependent of a package name", async () => {
            const dependents = await gateway().whoNeeds(
                channels,
                ["noarch"],
                "bar",
            );

            const byName = Object.fromEntries(
                dependents.map((dependent) => [
                    dependent.record.name,
                    dependent,
                ]),
            );
            expect(Object.keys(byName).sort()).toEqual([
                "extra",
                "foo",
                "old",
                "pinned",
            ]);
            expect(byName.foo.record).toBeInstanceOf(RepoDataRecord);
            expect(byName.foo.kind).toBe("depends");
            expect(byName.foo.dependency).toBe("bar >=1");
            expect(byName.pinned.kind).toBe("constrains");
            expect(byName.pinned.dependency).toBe("bar >=2");
            expect(byName.extra).toMatchObject({
                kind: "extra_depends",
                extra: "speedups",
                dependency: "bar",
            });
        });

        it("only matches dependencies accepting a record", async () => {
            const gw = gateway();
            const [bar] = await gw.query(channels, ["noarch"], ["bar"]);

            const fromRepoDataRecord = await gw.whoNeeds(
                channels,
                ["noarch"],
                bar,
            );
            const fromPackageRecord = await gw.whoNeeds(
                channels,
                ["noarch"],
                new PackageRecord(bar.toJson()),
            );

            for (const dependents of [fromRepoDataRecord, fromPackageRecord]) {
                expect(
                    dependents.map((dependent) => dependent.record.name).sort(),
                ).toEqual(["extra", "foo"]);
            }
        });

        it("matches virtual packages", async () => {
            const dependents = await gateway().whoNeeds(channels, ["noarch"], {
                name: "__cuda",
                version: "12.4",
            });
            expect(
                dependents.map((dependent) => dependent.record.name),
            ).toEqual(["gpu"]);

            expect(
                await gateway().whoNeeds(channels, ["noarch"], {
                    name: "__cuda",
                    version: "11.8",
                }),
            ).toEqual([]);
        });

        it("rejects invalid targets with error codes", async () => {
            const error: unknown = await gateway()
                .whoNeeds(channels, ["noarch"], "not a name!")
                .then(
                    () => null,
                    (err: unknown) => err,
                );
            expect(isRattlerError(error)).toBe(true);
            if (isRattlerError(error)) {
                expect(error.code).toBe("PARSE_PACKAGE_NAME");
            }
        });
    });
    describe("clearRepodataCache", () => {
        it("refetches the repodata of a cleared channel", async () => {
            const seen: string[] = [];
            const gateway = new Gateway({
                channelConfig: plainOnly,
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
            const channel = "https://example.com/cleared";
            const query = () => gateway.query([channel], ["noarch"], ["foo"]);

            await query();
            const fetchedOnce = seen.length;
            expect(fetchedOnce).toBeGreaterThanOrEqual(1);

            await query();
            expect(seen.length).toBe(fetchedOnce);

            // Clearing another platform keeps noarch cached.
            gateway.clearRepodataCache(channel, ["linux-64"]);
            await query();
            expect(seen.length).toBe(fetchedOnce);

            gateway.clearRepodataCache(channel);
            await query();
            expect(seen.length).toBe(2 * fetchedOnce);
        });

        it("rejects invalid platforms", () => {
            expect(() =>
                new Gateway().clearRepodataCache("conda-forge", [
                    "not-a-platform" as Platform,
                ]),
            ).toThrow();
        });
    });
    describe("fromConfig", () => {
        it("applies the repodata config", async () => {
            const seen: string[] = [];
            const gateway = Gateway.fromConfig(
                Config.fromToml(`
                    [repodata-config]
                    disable-zstd = true
                    disable-bzip2 = true
                    disable-sharded = true
                `),
                {
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
                },
            );

            await gateway.query(
                ["https://example.com/from-config"],
                ["noarch"],
                ["foo"],
            );

            expect(gateway).toBeInstanceOf(Gateway);
            expect(seen).toEqual([
                "https://example.com/from-config/noarch/repodata.json",
            ]);
        });
    });
});
