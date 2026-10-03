/**
 * The two sections of a conda package: `info` holds the metadata under
 * `info/`, `pkg` the package payload.
 *
 * @public
 */
export declare type ArchiveSection = "info" | "pkg";

/**
 * What a tar entry of a package section is.
 *
 * @public
 */
export declare type ArchiveEntryKind =
    | "file"
    | "directory"
    | "symlink"
    | "hardlink"
    | "other";

/**
 * A tar entry of a package section, as listed by `PackageArchive.listFiles`.
 *
 * @public
 */
export declare type ArchiveEntry = {
    /** The package-relative path of the entry. */
    path: string;
    /** The size of the entry in bytes. Zero for links and directories. */
    size: number;
    kind: ArchiveEntryKind;
    /** The target of a symbolic or hard link. */
    linkTarget?: string;
};

/**
 * The parsed `info/index.json` of a package: the metadata the package was
 * built with, before any repodata patches the channel applies.
 *
 * @public
 */
export declare type IndexJson = {
    name: string;
    version: string;
    build: string;
    build_number: number;
    arch?: string;
    platform?: string;
    subdir?: string;
    noarch?: NoArchType;
    depends?: string[];
    constrains?: string[];
    extra_depends?: Record<string, string[]>;
    features?: string;
    flags?: string[];
    track_features?: string[];
    license?: string;
    license_family?: string;
    purls?: string[];
    python_site_packages_path?: string;
    repodata_revision?: number;
    timestamp?: number;
};

/**
 * The parsed `info/about.json` of a package. URL fields hold one URL as a
 * string and several as an array.
 *
 * @public
 */
export declare type AboutJson = {
    channels?: string[];
    description?: string;
    dev_url?: string | string[];
    doc_url?: string | string[];
    home?: string | string[];
    license?: string;
    license_family?: string;
    source_url?: string;
    summary?: string;
    /** Free-form metadata, such as conda-forge's `recipe-maintainers`. */
    extra?: Record<string, unknown>;
};

/**
 * How a file of the payload is placed into an environment.
 *
 * @public
 */
export declare type PathType = "hardlink" | "softlink" | "directory";

/**
 * One file of the package payload as `info/paths.json` describes it.
 *
 * @public
 */
export declare type PathsEntry = {
    _path: string;
    path_type: PathType;
    /** `true` when the file must be copied rather than linked. */
    no_link?: boolean;
    sha256?: string;
    size_in_bytes?: number;
    /** Present when the build prefix is baked into the file and has to be replaced on install. */
    prefix_placeholder?: string;
    file_mode?: "binary" | "text";
};

/**
 * The parsed `info/paths.json` of a package, listing every file of the
 * payload.
 *
 * @public
 */
export declare type PathsJson = {
    paths_version: number;
    paths: PathsEntry[];
};

/**
 * The parsed `info/run_exports.json` of a package: the dependencies that
 * packages built against this one acquire.
 *
 * @public
 */
export declare type RunExportsJson = {
    weak?: string[];
    strong?: string[];
    noarch?: string[];
    weak_constrains?: string[];
    strong_constrains?: string[];
};
