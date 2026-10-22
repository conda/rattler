# package_streaming

Helpers for downloading and extracting conda package archives.

To read several files from one (possibly remote) package, prefer
[`PackageArchive`](package_archive.md), which opens the package once and shares
the work between reads.

::: rattler.package_streaming
    options:
      members:
        - extract
        - extract_tar_bz2
        - download_to_path
        - download_bytes
        - download_to_writer
        - download_and_extract
        - fetch_raw_package_file_from_url
