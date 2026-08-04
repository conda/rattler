import { JsGateway } from "../pkg";
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
}
