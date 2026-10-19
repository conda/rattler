/**
 * Any value that can be represented in JSON.
 *
 * @public
 */
export declare type JsonValue =
    | string
    | number
    | boolean
    | null
    | JsonValue[]
    | { [key: string]: JsonValue };

/**
 * The contents of the `info/index.json` file of a conda package. It holds the
 * same information as the package's repodata record, before any repodata
 * patches are applied.
 *
 * @public
 */
export declare type IndexJson = {
    name: string;
    version: string;
    build: string;
    build_number: number;
    arch?: string | null;
    constrains?: string[];
    depends: string[];
    extra_depends?: Record<string, string[]>;
    features?: string | null;
    flags?: string[];
    license?: string | null;
    license_family?: string | null;
    noarch?: NoArchType;
    platform?: string | null;
    /** Package URLs (purls) of the sources the package was built from. */
    purls?: string[];
    python_site_packages_path?: string | null;
    /** The repodata revision required to represent this package. */
    repodata_revision?: number;
    subdir?: string | null;
    /** The build time in milliseconds since the unix epoch. */
    timestamp?: number | null;
    track_features?: string;
};

/**
 * The contents of the `info/about.json` file of a conda package.
 *
 * The url fields hold a single url as a string and several urls as an array.
 *
 * @public
 */
export declare type AboutJson = {
    channels?: string[];
    description?: string | null;
    dev_url?: string | string[];
    doc_url?: string | string[];
    extra?: Record<string, JsonValue>;
    home?: string | string[];
    license?: string | null;
    license_family?: string | null;
    source_url?: string | null;
    summary?: string | null;
};

/**
 * The type of an entry in `info/paths.json`.
 *
 * @public
 */
export declare type PathType = "hardlink" | "softlink" | "directory";

/**
 * Whether a file with a prefix placeholder is a text or a binary file.
 *
 * @public
 */
export declare type FileMode = "text" | "binary";

/**
 * A group of placeholder occurrences in one text encoding, as proposed by the
 * draft CEP for prefix replacement offsets. Text files list the byte offset of
 * each occurrence, binary files list the offsets of each occurrence and of the
 * end of its c-string.
 *
 * @public
 */
export declare type PrefixOffsetGroup = {
    encoding: "utf-8" | "utf-16-le" | "utf-16-be" | "utf-32-le" | "utf-32-be";
    ranges: number[] | number[][];
};

/**
 * A single file, symlink or directory of a conda package.
 *
 * @public
 */
export declare type PathsEntry = {
    /** The path of the entry relative to the root of the environment. */
    _path: string;
    path_type: PathType;
    /** Whether the file must be copied instead of linked. Defaults to `false`. */
    no_link?: boolean;
    /** The hex encoded SHA256 hash of the file. */
    sha256?: string;
    size_in_bytes?: number;
    /** The mode of the file if it contains a prefix placeholder. */
    file_mode?: FileMode;
    /** The prefix that must be replaced when the file is installed. */
    prefix_placeholder?: string;
    /** The locations of the prefix placeholder in the file, if recorded. */
    offsets?: PrefixOffsetGroup[];
    /** The length of the shebang line, which is replaced separately. */
    shebang_length?: number;
};

/**
 * The contents of the `info/paths.json` file of a conda package.
 *
 * @public
 */
export declare type PathsJson = {
    paths: PathsEntry[];
    paths_version: number;
};

/**
 * The contents of the `info/run_exports.json` file of a conda package. Each
 * field holds match specs that are added to the packages that depend on this
 * package at build time.
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
