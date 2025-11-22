import { JsGateway } from "../pkg";
import { Platform } from "./Platform";
import { NormalizedPackageName } from "./PackageName";

/**
 * Specifies which cache layers to clear.
 *
 * @public
 */
export type CacheClearMode =
    /** Clear only the in-memory cache */
    | "memory"
    /** Clear both in-memory and on-disk cache (Node.js only, not available in browsers) */
    | "memory-and-disk";

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

export type GatewayOptions = {
    /**
     * The maximum number of concurrent requests the gateway can execute. By
     * default there is no limit.
     */
    maxConcurrentRequests?: number | null;

    /** Defines how to access channels. */
    channelConfig?: GatewayChannelConfig;
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
        this.native = new JsGateway(options);
    }

    /**
     * Returns the names of the package that are available for the given
     * channels and platforms.
     *
     * @param channels - The channels to query
     * @param platforms - The platforms to query
     */
    public async names(
        channels: string[],
        platforms: Platform[],
    ): Promise<NormalizedPackageName[]> {
        return (await this.native.names(
            channels,
            platforms,
        )) as NormalizedPackageName[];
    }

    /**
     * Clears the cache for the given channel.
     *
     * Any subsequent query will re-fetch any required data from the source.
     *
     * @param channel - The channel URL or name to clear the cache for
     * @param subdirs - Optional array of platform subdirectories to clear (e.g., ["linux-64", "noarch"]).
     *                 If not provided, all subdirectories for the channel are cleared.
     * @param mode - Optional cache clear mode: "memory" (default) or "memory-and-disk".
     *              Note: "memory-and-disk" is only available in Node.js, not in browsers.
     *
     * @example
     * ```typescript
     * const gateway = new Gateway();
     *
     * // Clear only in-memory cache for all subdirectories
     * gateway.clearRepoDataCache("conda-forge");
     *
     * // Clear specific subdirectories
     * gateway.clearRepoDataCache("conda-forge", ["linux-64", "noarch"]);
     *
     * // Clear both in-memory and on-disk cache (Node.js only)
     * gateway.clearRepoDataCache("conda-forge", ["linux-64"], "memory-and-disk");
     * ```
     */
    public clearRepoDataCache(
        channel: string,
        subdirs?: Platform[] | null,
        mode?: CacheClearMode | null,
    ): void {
        this.native.clearRepoDataCache(channel, subdirs ?? undefined, mode ?? undefined);
    }
}
