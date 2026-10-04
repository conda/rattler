import {
    Config,
    JsGateway,
    MatchSpec,
    PackageRecord,
    RepoDataRecord,
} from "../pkg";
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
 * The result of {@link Gateway.query}: the matching records, with the non-fatal
 * warnings encountered during the query attached. The warnings are also
 * forwarded to the `onWarning` callback (or `console.warn` when none is set) as
 * they are recorded, so they surface even when this field is ignored.
 *
 * @public
 */
export type GatewayQueryResult = RepoDataRecord[] & {
    /** Non-fatal warnings encountered during the query. */
    warnings: string[];
};

/**
 * Options for {@link Gateway.fromConfig}. Everything else is taken from the
 * configuration.
 *
 * @public
 */
export type GatewayFromConfigOptions = Pick<
    GatewayOptions,
    "fetch" | "onWarning"
>;

/**
 * A virtual package (e.g. `__cuda`) to find the reverse dependencies of with
 * {@link Gateway.whoNeeds}.
 *
 * @public
 */
export type VirtualPackageTarget = {
    /** The name of the virtual package, e.g. `__cuda`. */
    name: string;
    /** The version of the virtual package, e.g. `12.4`. */
    version: string;
    /** The build string of the virtual package. Defaults to `"0"`. */
    buildString?: string;
};

/**
 * The package to find the reverse dependencies of with {@link Gateway.whoNeeds}.
 *
 * A package name matches every dependency on that name, regardless of its
 * version or build constraints. A record or virtual package only matches
 * dependencies whose match spec accepts it.
 *
 * @public
 */
export type WhoNeedsTarget =
    | string
    | PackageRecord
    | RepoDataRecord
    | VirtualPackageTarget;

/**
 * The run export field through which a {@link Dependent} references the queried
 * package.
 *
 * @public
 */
export type RunExportKind =
    | "weak"
    | "strong"
    | "noarch"
    | "weak_constrains"
    | "strong_constrains";

/**
 * A record that references the package queried through {@link Gateway.whoNeeds}.
 *
 * @public
 */
export type Dependent = {
    /** The record that references the queried package. */
    record: RepoDataRecord;
    /**
     * The dependency string through which the record references the queried
     * package.
     */
    dependency: string;
} & (
    | {
          /** The field of the record the dependency comes from. */
          kind: "depends" | "constrains";
      }
    | {
          /** The dependency comes from an optional feature in `extra_depends`. */
          kind: "extra_depends";
          /**
           * The name of the optional feature; the reference only applies when
           * that extra is enabled.
           */
          extra: string;
      }
    | {
          /** The dependency comes from the run exports of the record. */
          kind: "run_export";
          /** The run export field the dependency comes from. */
          runExportKind: RunExportKind;
      }
);

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

    /**
     * Constructs a Gateway whose channel and concurrency settings come from a
     * shared rattler configuration: `repodata-config` selects the enabled
     * repodata formats (with its per-channel overrides) and
     * `concurrency.downloads` limits the number of concurrent requests.
     *
     * Requests are always made through `fetch`, so the networking keys of the
     * configuration (mirrors, proxies, TLS and authentication) are not
     * applied.
     *
     * @example
     *
     * ```ts
     * const gateway = Gateway.fromConfig(
     *     Config.fromToml(`
     *         [repodata-config]
     *         disable-sharded = true
     *     `),
     * );
     * ```
     *
     * @param config - The configuration to apply
     * @param options - The options that are not part of the configuration
     */
    public static fromConfig(
        config: Config,
        options?: GatewayFromConfigOptions,
    ): Gateway {
        const gateway = Object.create(Gateway.prototype) as Gateway;
        gateway.native = JsGateway.fromConfig(
            config,
            options?.fetch,
            options?.onWarning,
        );
        return gateway;
    }

    /**
     * Clears the in-memory repodata cache of a channel, so that subsequent
     * queries fetch its repodata again.
     *
     * @param channel - The channel to clear the cache of
     * @param platforms - The platforms to clear. When omitted, the cache of
     *   every platform of the channel is cleared.
     */
    public clearRepodataCache(channel: string, platforms?: Platform[]): void {
        this.native.clearRepodataCache(channel, platforms);
    }

    /**
     * Returns the reverse dependencies of `target`: the records of the given
     * channels and platforms that reference it through their `depends`,
     * `constrains`, `extra_depends` or run exports. The `kind` of each result
     * tells which field matched.
     *
     * Every record of the queried platforms is scanned, so this reads far more
     * repodata than {@link Gateway.query}; the scanned records are dropped again
     * right away and only the matches are kept. Channels are always read
     * through their full repodata, regardless of the sharding configuration,
     * and CEP-42 `channel_relations` are not followed.
     *
     * @example
     *
     * ```ts
     * const dependents = await gateway.whoNeeds(
     *     ["conda-forge"],
     *     ["linux-64", "noarch"],
     *     "polars",
     * );
     * ```
     *
     * @param channels - The channels to query
     * @param platforms - The platforms to query
     * @param target - The package to find the reverse dependencies of: a
     *   package name, a record, or a virtual package.
     */
    public async whoNeeds(
        channels: string[],
        platforms: Platform[],
        target: WhoNeedsTarget,
    ): Promise<Dependent[]> {
        let dependents: Promise<unknown[]>;
        if (typeof target === "string") {
            dependents = this.native.whoNeedsName(channels, platforms, target);
        } else if (target instanceof RepoDataRecord) {
            dependents = this.native.whoNeedsRepoDataRecord(
                channels,
                platforms,
                target,
            );
        } else if (target instanceof PackageRecord) {
            dependents = this.native.whoNeedsRecord(
                channels,
                platforms,
                target,
            );
        } else {
            dependents = this.native.whoNeedsVirtualPackage(
                channels,
                platforms,
                target.name,
                target.version,
                target.buildString ?? "0",
            );
        }
        return (await dependents) as Dependent[];
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
     *   every version of that package. Package names may be globs (`foo*`) or
     *   anchored regexes (`^foo.*$`).
     * @param options - Per-query options
     */
    public async query(
        channels: string[],
        platforms: Platform[],
        specs: (string | MatchSpec)[],
        options?: GatewayRecordsQueryOptions,
    ): Promise<GatewayQueryResult> {
        const output = (await this.native.query(
            channels,
            platforms,
            // The native call consumes the specs it is given, so hand it
            // copies and leave the caller's `MatchSpec` objects usable.
            specs.map((spec) =>
                typeof spec === "string"
                    ? new MatchSpec(spec, { exactNamesOnly: false })
                    : spec.clone(),
            ),
            options?.recursive ?? false,
        )) as { records: RepoDataRecord[]; warnings: string[] };
        const result = output.records as GatewayQueryResult;
        result.warnings = output.warnings;
        return result;
    }
}
