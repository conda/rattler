import { describe, expect, it } from "@jest/globals";
import { Channel } from "./Channel";
import { isRattlerError } from "./RattlerError";

describe("Channel", () => {
    it("resolves a channel name against the default alias", () => {
        const channel = new Channel("conda-forge");
        expect(channel.name).toBe("conda-forge");
        expect(channel.baseUrl).toBe("https://conda.anaconda.org/conda-forge/");
        expect(channel.platforms).toBeUndefined();
        expect(channel.toString()).toBe(
            "https://conda.anaconda.org/conda-forge/",
        );
    });
    it("resolves a channel name against a custom alias", () => {
        const channel = new Channel("conda-forge", {
            channelAlias: "https://prefix.dev/",
        });
        expect(channel.name).toBe("conda-forge");
        expect(channel.baseUrl).toBe("https://prefix.dev/conda-forge/");
    });
    it("resolves names against an alias with a path", () => {
        for (const channelAlias of [
            "https://repo.example.com/conda",
            "https://repo.example.com/conda/",
        ]) {
            const channel = new Channel("conda-forge", { channelAlias });
            expect(channel.name).toBe("conda-forge");
            expect(channel.baseUrl).toBe(
                "https://repo.example.com/conda/conda-forge/",
            );

            const underAlias = new Channel(
                "https://repo.example.com/conda/conda-forge",
                { channelAlias },
            );
            expect(underAlias.name).toBe("conda-forge");

            // A sibling path that merely starts with the alias' last segment
            // is not under the alias.
            const sibling = new Channel(
                "https://repo.example.com/condaxyz/conda-forge",
                { channelAlias },
            );
            expect(sibling.name).not.toBe("xyz/conda-forge");
        }
    });
    it("parses a url", () => {
        const channel = new Channel("https://prefix.dev/conda-forge");
        expect(channel.name).toBe("conda-forge");
        expect(channel.baseUrl).toBe("https://prefix.dev/conda-forge/");
    });
    it("parses a url within the channel alias", () => {
        const channel = new Channel("https://conda.anaconda.org/conda-forge/");
        expect(channel.name).toBe("conda-forge");
    });
    it("parses explicit platforms", () => {
        const channel = new Channel("conda-forge[linux-64,noarch]");
        expect(channel.platforms).toEqual(["linux-64", "noarch"]);
    });
    it("returns platform urls", () => {
        const channel = new Channel("conda-forge");
        expect(channel.platformUrl("linux-64")).toBe(
            "https://conda.anaconda.org/conda-forge/linux-64/",
        );
    });
    it("throws a PARSE_CHANNEL error for unknown platforms", () => {
        let error: unknown;
        try {
            new Channel("conda-forge[not-a-platform]");
        } catch (e) {
            error = e;
        }
        expect(isRattlerError(error)).toBe(true);
        expect((error as { code: string }).code).toBe("PARSE_CHANNEL");
    });
});
