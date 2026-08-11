import { describe, expect, it } from "@jest/globals";
import { Gateway } from "./Gateway";

describe("Gateway", () => {
    describe("constructor", () => {
        it("works without arguments", () => {
            expect(() => new Gateway()).not.toThrowError();
            expect(() => new Gateway(null)).not.toThrowError();
            expect(() => new Gateway(undefined)).not.toThrowError();
        });
        it("throws on invalid arguments", () => {
            expect(() => new Gateway(true as any)).toThrowError();
        });
        it("accepts an empty object", () => {
            expect(() => new Gateway({})).not.toThrowError();
        });
        it("accepts null for maxConcurrentRequests", () => {
            expect(
                () =>
                    new Gateway({
                        maxConcurrentRequests: null,
                    }),
            ).not.toThrowError();
        });
        it("accepts empty channelConfig", () => {
            expect(
                () =>
                    new Gateway({
                        channelConfig: {},
                    }),
            ).not.toThrowError();
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
            ).not.toThrowError();
        });
    });
    describe("names", () => {
        const gateway = new Gateway();

        it("returns unsupported repodata revisions alongside names", async () => {
            const gateway = new Gateway();
            const native = gateway.native as unknown as {
                names: () => Promise<unknown>;
            };
            native.names = async () => ({
                names: ["demo"],
                notices: [],
                unsupportedRepodataRevisions: [
                    {
                        channel: "https://example.com/first/",
                        subdir: "linux-64",
                        supportedRevision: "v3",
                        advertisedRevision: "v1",
                        message: null,
                    },
                    {
                        channel: "https://example.com/second/",
                        subdir: "noarch",
                        supportedRevision: "v3",
                        advertisedRevision: "v4",
                        message: "new layout",
                    },
                ],
            });

            const result = await gateway.names([], []);
            expect(Array.from(result)).toEqual(["demo"]);
            expect(result.names).toBe(result);
            expect(result.unsupportedRepodataRevisions).toEqual([
                {
                    channel: "https://example.com/first/",
                    subdir: "linux-64",
                    supportedRevision: "v3",
                    advertisedRevision: "v1",
                    message: null,
                },
                {
                    channel: "https://example.com/second/",
                    subdir: "noarch",
                    supportedRevision: "v3",
                    advertisedRevision: "v4",
                    message: "new layout",
                },
            ]);
        });

        it("can query prefix.dev", () => {
            return gateway
                .names(
                    ["https://prefix.dev/emscripten-forge-dev"],
                    ["noarch", "emscripten-wasm32"],
                )
                .then((names) => {
                    expect(names.length).toBeGreaterThanOrEqual(177);
                    expect(names.unsupportedRepodataRevisions).toEqual([]);
                });
        });
    });
});
