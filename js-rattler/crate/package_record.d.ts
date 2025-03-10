/**
 * A package record is a JSON object that contains metadata about a package.
 *
 * @public
 */
export declare type PackageRecordJson = {
    name: string;
    version: string;
    build: string;
    build_number: number;
    subdir: Platform;
    arch?: Arch;
    constrains?: string[];
    depends?: string[];
    features?: string;
    license?: string;
    license_family?: string;
    md5?: string;
    legacy_bz2_md5?: string;
    legacy_bz2_size?: number;
    sha256?: string;
    platform?: string;
    python_site_packages_path?: string;
    size?: number;
    noarch?: NoArchType;
    timestamp?: number;
    track_features?: string;
};
