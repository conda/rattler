/**
 * The two sections of a conda package: `info` holds the metadata under `info/`,
 * `pkg` the package payload.
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
