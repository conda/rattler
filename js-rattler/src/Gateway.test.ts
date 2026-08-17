import { describe, expect, it } from "@jest/globals";
import { Gateway } from "./Gateway";

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
});
