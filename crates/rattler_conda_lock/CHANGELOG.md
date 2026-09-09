# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0]

### Added

- Initial release: reading and writing [CEP-37](https://github.com/conda/ceps/blob/main/cep-0037.md)
  `conda-lock.yml` files, with source-aware diagnostics and package-derived
  content hashes. Conversion to and from pixi lock files lives in
  `rattler_lock::conda_lock`.
