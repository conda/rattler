import { afterAll, beforeAll, describe, expect, it } from "@jest/globals";
import { createServer, type Server } from "node:http";
import { readFileSync } from "node:fs";
import { PackageArchive } from "./PackageArchive";
import { isRattlerError } from "./RattlerError";

// Serves the test archives with HTTP range support, the way a conda channel
// does, and records every request so the tests can check how much of an
// archive a read costs.
const files = new Map<string, Buffer>([
    [
        "/sparse-test-1.0.0-0.conda",
        readFileSync("../test-data/sparse/sparse-test-1.0.0-0.conda"),
    ],
    [
        "/symlink-test-1.0.0-0.conda",
        readFileSync("../test-data/sparse/symlink-test-1.0.0-0.conda"),
    ],
    [
        "/test-package-0.1-0.tar.bz2",
        readFileSync(
            "../test-data/test-server/repo/noarch/test-package-0.1-0.tar.bz2",
        ),
    ],
]);

type Request = { method: string; path: string; range: string | null };
const requests: Request[] = [];
let server: Server;
let base: string;
// Paths under /no-ranges/ are served by a server that ignores `Range`.
const noRanges = "/no-ranges";

beforeAll(async () => {
    server = createServer((request, response) => {
        const url = request.url ?? "";
        const ignoreRange = url.startsWith(noRanges);
        const path = ignoreRange ? url.slice(noRanges.length) : url;
        requests.push({
            method: request.method ?? "",
            path,
            range: request.headers.range ?? null,
        });
        const file = files.get(path);
        if (file === undefined) {
            response.statusCode = 404;
            response.end();
            return;
        }
        response.setHeader("accept-ranges", "bytes");
        const range = ignoreRange ? undefined : request.headers.range;
        if (range) {
            const match = /^bytes=(\d+)-(\d+)$/.exec(range);
            if (!match) {
                response.statusCode = 416;
                response.end();
                return;
            }
            const start = Number(match[1]);
            const end = Math.min(Number(match[2]), file.length - 1);
            response.statusCode = 206;
            response.setHeader(
                "content-range",
                `bytes ${start}-${end}/${file.length}`,
            );
            response.setHeader("content-length", end - start + 1);
            response.end(request.method === "HEAD" ? undefined : file.subarray(start, end + 1));
            return;
        }
        response.setHeader("content-length", file.length);
        response.end(request.method === "HEAD" ? undefined : file);
    });
    await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
    const address = server.address();
    if (address === null || typeof address === "string") {
        throw new Error("test server did not expose a TCP address");
    }
    base = `http://127.0.0.1:${address.port}`;
});

afterAll(async () => {
    await new Promise<void>((resolve) => {
        server.close(() => resolve());
    });
});

const since = () => requests.length;
const requestsSince = (mark: number) => requests.slice(mark);

describe("PackageArchive", () => {
    it("opens a .conda archive with a HEAD and one tail request", async () => {
        const mark = since();
        const archive = await PackageArchive.fromUrl(`${base}/sparse-test-1.0.0-0.conda`);
        expect(archive.archiveType).toBe("conda");
        expect(archive.size).toBe(files.get("/sparse-test-1.0.0-0.conda")!.length);
        expect(archive.url).toBe(`${base}/sparse-test-1.0.0-0.conda`);
        const made = requestsSince(mark);
        expect(made.map((r) => r.method)).toEqual(["HEAD", "GET"]);
        expect(made[1].range).toMatch(/^bytes=\d+-\d+$/);
    });

    it("skips the HEAD request when the size is known", async () => {
        const size = files.get("/sparse-test-1.0.0-0.conda")!.length;
        const mark = since();
        const archive = await PackageArchive.fromUrl(`${base}/sparse-test-1.0.0-0.conda`, size);
        expect(archive.size).toBe(size);
        expect(requestsSince(mark).map((r) => r.method)).toEqual(["GET"]);
    });

    it("reads the info section from the tail without another request", async () => {
        const archive = await PackageArchive.fromUrl(`${base}/sparse-test-1.0.0-0.conda`);
        const mark = since();
        const index = await archive.indexJson();
        expect(index.name).toBe("sparse-test");
        expect(index.version).toBe("1.0.0");
        const paths = await archive.pathsJson();
        expect(paths.paths.map((p) => p._path)).toEqual([
            "bin/first-file.txt",
            "lib/blob.bin",
            "share/last-file.txt",
        ]);
        expect(paths.paths[1].size_in_bytes).toBe(150000);
        expect(await archive.aboutJson()).toBeUndefined();
        expect(await archive.runExportsJson()).toBeUndefined();
        expect(await archive.listFiles("info")).toEqual([
            { path: "info/index.json", size: expect.any(Number), kind: "file" },
            { path: "info/paths.json", size: expect.any(Number), kind: "file" },
        ]);
        expect(requestsSince(mark)).toEqual([]);
        expect(archive.sectionSize("info")).toBeLessThan(64 * 1024);
    });

    it("fetches the payload once with a range request", async () => {
        const archive = await PackageArchive.fromUrl(`${base}/sparse-test-1.0.0-0.conda`);
        const mark = since();
        const entries = await archive.listFiles("pkg");
        expect(entries.map((e) => e.path)).toEqual([
            "bin/first-file.txt",
            "lib/blob.bin",
            "share/last-file.txt",
        ]);
        const first = await archive.readFile("bin/first-file.txt");
        expect(new TextDecoder().decode(first)).toBe("first payload file\n");
        expect(await archive.readFile("share/missing.txt")).toBeUndefined();
        const made = requestsSince(mark);
        expect(made).toHaveLength(1);
        expect(made[0].range).toMatch(/^bytes=\d+-\d+$/);
        expect(archive.sectionSize("pkg")).toBeGreaterThan(64 * 1024);
    });

    it("lists links with their targets and refuses to read them", async () => {
        const archive = await PackageArchive.fromUrl(`${base}/symlink-test-1.0.0-0.conda`);
        const entries = await archive.listFiles("pkg");
        expect(entries).toContainEqual({
            path: "lib/liblink.so",
            size: 0,
            kind: "symlink",
            linkTarget: "libreal.so.1",
        });
        expect(entries.find((e) => e.path === "lib/libhard.so")?.kind).toBe("hardlink");
        await expect(archive.readFile("lib/liblink.so")).rejects.toMatchObject({
            code: "ARCHIVE",
        });
    });

    it("falls back to the whole archive when the server ignores ranges", async () => {
        const mark = since();
        const archive = await PackageArchive.fromUrl(
            `${base}${noRanges}/sparse-test-1.0.0-0.conda`,
        );
        const index = await archive.indexJson();
        expect(index.name).toBe("sparse-test");
        const last = await archive.readFile("share/last-file.txt");
        expect(new TextDecoder().decode(last)).toBe("last payload file\n");
        // HEAD, then the one GET that returned everything.
        expect(requestsSince(mark).map((r) => r.method)).toEqual(["HEAD", "GET"]);
    });

    it("reads a .tar.bz2 archive", async () => {
        const archive = await PackageArchive.fromUrl(`${base}/test-package-0.1-0.tar.bz2`);
        expect(archive.archiveType).toBe("tar.bz2");
        const mark = since();
        const index = await archive.indexJson();
        expect(index.name).toBe("test-package");
        const info = await archive.listFiles("info");
        expect(info.map((e) => e.path)).toContain("info/recipe/meta.yaml");
        const about = await archive.aboutJson();
        expect(about).toBeDefined();
        expect(requestsSince(mark)).toHaveLength(1);
    });

    it("rejects URLs that are not conda archives", async () => {
        expect.assertions(2);
        try {
            await PackageArchive.fromUrl(`${base}/sparse-test.zip`);
        } catch (err) {
            expect(isRattlerError(err)).toBe(true);
            if (isRattlerError(err)) expect(err.code).toBe("UNSUPPORTED_ARCHIVE_TYPE");
        }
    });

    it("reports a missing archive", async () => {
        await expect(PackageArchive.fromUrl(`${base}/missing-1.0-0.conda`)).rejects.toMatchObject({
            code: "FETCH",
        });
    });

    it("rejects unknown sections", async () => {
        const archive = await PackageArchive.fromUrl(`${base}/sparse-test-1.0.0-0.conda`);
        await expect(archive.listFiles("payload" as any)).rejects.toMatchObject({
            code: "INVALID_SECTION",
        });
    });
});
