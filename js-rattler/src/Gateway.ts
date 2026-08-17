import { JsGateway, PackageRecordJson } from "../pkg";
import { Platform } from "./Platform";
import { NormalizedPackageName } from "./PackageName";

export type GatewaySourceConfig = {
    /** `true` if downloading `repodata.json.zst` is enabled. Defaults to `true`. */
    zstdEnabled?: boolean;

    /** `true` if downloading `repodata.json.bz2` is enabled. Defaults to `true`. */
    bz2Enabled?: boolean;

    /**
     * `true` if sharded repodata is available for the channel. Defaults to
     * `true`.
     */
    shardedEnabled?: boolean;
};

export type GatewayChannelConfig = {
    /**
     * The default configuration for a channel if its is not explicitly matched
     * in the `perChannel` field.
     */
    default?: GatewaySourceConfig;

    /**
     * Configuration for a specific channel.
     *
     * The key refers to the prefix of a channel so `https://prefix.dev` matches
     * any channel on `https://prefix.dev`. The key with the longest match is
     * used.
     */
    perChannel?: {
        [key: string]: GatewaySourceConfig;
    };
};

export type ChannelNotice = {
    channel: string;
    id: string;
    message: string;
    level: "info" | "warning" | "critical";
    createdAt: string | null;
    expiresAt: string | null;
    interval: number | null;
};

export type GatewayQueryOptions = {
    /** Whether CEP-6 channel notices are fetched. Defaults to `false`. */
    channelNotices?: boolean;
};

export type GatewayNamesResult = NormalizedPackageName[] & {
    /** The package names. This aliases the result array for compatibility. */
    names: NormalizedPackageName[];
    /** CEP-6 notices published by queried and CEP-42-discovered channels. */
    notices: ChannelNotice[];
};

/**
 * A fetch implementation used to execute the HTTP requests of a {@link Gateway}.
 * Compatible with the WHATWG `fetch` function.
 *
 * @public
 */
export type GatewayFetch = (request: Request) => Promise<Response>;

export type GatewayOptions = {
    /**
     * The maximum number of concurrent requests the gateway can execute. By
     * default there is no limit.
     */
    maxConcurrentRequests?: number | null;

    /** Defines how to access channels. */
    channelConfig?: GatewayChannelConfig;

    /**
     * A custom fetch implementation used for all HTTP requests made by this
     * gateway. When omitted, the global `fetch` function is used, which is the
     * right choice for browsers and plain Node.
     *
     * Set this only when requests must go through the host's own HTTP stack:
     * authentication, proxies, caching, or request mocking in tests.
     */
    fetch?: GatewayFetch;

    /**
     * A callback invoked for every warning the gateway emits, for example for
     * malformed CEP-42 channel relations. When omitted, warnings are forwarded
     * to `console.warn`.
     */
    onWarning?: (message: string) => void;
};

/**
 * Per-query options for {@link Gateway.query}.
 *
 * @public
 */
export type GatewayRecordsQueryOptions = {
    /**
     * Whether the records of dependencies are recursively fetched as well.
     * Defaults to `false`.
     */
    recursive?: boolean;
};

/**
 * A single record in the Conda repodata as returned by {@link Gateway.query}.
 * This is the `repodata.json` representation of a package extended with the
 * filename, the canonical download URL, and the channel it came from.
 *
 * @public
 */
export type RepoDataRecordJson = PackageRecordJson & {
    /** The filename of the package archive. */
    fn: string;

    /** The canonical URL from where to download this package. */
    url: string;

    /** The channel the package came from. */
    channel?: string | null;
};

/**
 * The result of {@link Gateway.query}: the matching records, with the non-fatal
 * warnings encountered during the query attached. The warnings are also
 * forwarded to the `onWarning` callback (or `console.warn` when none is set)
 * as they are recorded, so they surface even when this field is ignored.
 *
 * @public
 */
export type GatewayQueryResult = RepoDataRecordJson[] & {
    /** Non-fatal warnings encountered during the query. */
    warnings: string[];
};

/**
 * A `Gateway` provides efficient access to conda repodata.
 *
 * Repodata can be accessed through several different methods. The `Gateway`
 * implements all the nitty-gritty details of repodata access and provides a
 * simple high level API for consumers.
 *
 * The Gateway efficiently manages memory to reduce it to the bare minimum.
 *
 * Internally the gateway caches all fetched repodata records, running the same
 * query twice will return the previous results.
 *
 * @public
 */
export class Gateway {
    /** @internal */
    native: JsGateway;

    /**
     * Constructs a new Gateway object.
     *
     * @param options - The options to configure the Gateway with.
     */
    constructor(options?: GatewayOptions | null) {
        if (options && typeof options === "object") {
            const { fetch: fetchImpl, onWarning, ...rest } = options;
            this.native = new JsGateway(rest, fetchImpl, onWarning);
        } else {
            this.native = new JsGateway(options);
        }
    }

    /** Fetches CEP-6 notices for the given channels. */
    public async channelNotices(channels: string[]): Promise<ChannelNotice[]> {
        return (await this.native.channel_notices(channels)) as ChannelNotice[];
    }

    /**
     * Returns the names of the package that are available for the given
     * channels and platforms.
     *
     * @param channels - The channels to query
     * @param platforms - The platforms to query
     * @param options - Per-query options
     */
    public async names(
        channels: string[],
        platforms: Platform[],
        options?: GatewayQueryOptions,
    ): Promise<GatewayNamesResult> {
        const nativeNames = (
            this.native.names as unknown as (
                channels: string[],
                platforms: Platform[],
                channelNotices: boolean,
            ) => Promise<unknown>
        ).bind(this.native);
        const rawOutput = await nativeNames(
            channels,
            platforms,
            options?.channelNotices ?? false,
        );
        // Accept the old native array shape as well, so the TypeScript wrapper
        // remains compatible when it is loaded with an older WASM artifact.
        const output = Array.isArray(rawOutput)
            ? { names: rawOutput as NormalizedPackageName[], notices: [] }
            : (rawOutput as {
                  names: NormalizedPackageName[];
                  notices: ChannelNotice[];
              });
        const result = output.names as GatewayNamesResult;
        result.names = result;
        result.notices = output.notices;
        return result;
    }

    /**
     * Returns all records matching the given match specs in the given channels
     * and platforms.
     *
     * @param channels - The channels to query
     * @param platforms - The platforms to query
     * @param specs - The match specs to query for. A bare package name matches
     *   every version of that package.
     * @param options - Per-query options
     */
    public async query(
        channels: string[],
        platforms: Platform[],
        specs: string[],
        options?: GatewayRecordsQueryOptions,
    ): Promise<GatewayQueryResult> {
        const output = (await this.native.query(
            channels,
            platforms,
            specs,
            options?.recursive ?? false,
        )) as { records: RepoDataRecordJson[]; warnings: string[] };
        const result = output.records as GatewayQueryResult;
        result.warnings = output.warnings;
        return result;
    }
}
