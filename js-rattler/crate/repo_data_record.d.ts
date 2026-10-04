/**
 * The JSON representation of a {@link RepoDataRecord}: the `repodata.json`
 * representation of a package extended with the filename, the canonical
 * download URL, and the channel it came from.
 *
 * @public
 */
export declare type RepoDataRecordJson = PackageRecordJson & {
    /** The filename of the package archive. */
    fn: string;

    /** The canonical URL from where to download this package. */
    url: string;

    /** The channel the package came from. */
    channel?: string | null;
};
