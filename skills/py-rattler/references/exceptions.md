# Exceptions

All exceptions are in `rattler.exceptions`. All are subclasses of Python's built-in `Exception`.

```python
from rattler.exceptions import SolverError, InvalidVersionError, ...
```

## Parsing Errors

| Exception | Description |
|-----------|-------------|
| `InvalidVersionError` | Invalid version string |
| `InvalidVersionSpecError` | Invalid version specification |
| `InvalidMatchSpecError` | Invalid MatchSpec string |
| `InvalidPackageNameError` | Invalid package name |
| `PackageNameMatcherParseError` | Invalid package name matcher (glob/regex) |
| `InvalidChannelError` | Invalid channel URL or name |
| `InvalidUrlError` | Invalid URL |
| `InvalidHeaderNameError` | Invalid HTTP header name |
| `InvalidHeaderValueError` | Invalid HTTP header value |
| `ParsePlatformError` | Invalid platform string |
| `ParseArchError` | Invalid architecture string |
| `ParseCondaLockError` | Invalid conda lock file |
| `ParseExplicitEnvironmentSpecError` | Invalid explicit environment spec |
| `RequirementError` | Invalid requirement string |

## Operation Errors

| Exception | Description |
|-----------|-------------|
| `SolverError` | Dependency solving failed |
| `InstallerError` | Package installation failed |
| `TransactionError` | Transaction execution failed |
| `GatewayError` | Repodata gateway query failed |
| `FetchRepoDataError` | Repodata fetch failed |
| `LinkError` | Package linking failed |
| `ExtractError` | Package archive extraction failed |
| `ValidatePackageRecordsError` | Package record validation failed |

## Environment Errors

| Exception | Description |
|-----------|-------------|
| `ActivationError` | Environment activation failed |
| `ActivationScriptFormatError` | Activation script has invalid format |
| `ShellError` | Shell interaction failed |
| `EnvironmentCreationError` | Environment creation failed |
| `DetectVirtualPackageError` | Virtual package detection failed |

## Version Errors

| Exception | Description |
|-----------|-------------|
| `VersionBumpError` | Version bumping operation failed |
| `VersionExtendError` | Version extension operation failed |

## Other Errors

| Exception | Description |
|-----------|-------------|
| `AuthenticationStorageError` | Failed to query authentication storage |
| `CacheDirError` | Failed to determine cache directory |
| `ConversionError` | Type conversion failed |
| `ConvertSubdirError` | Failed to parse platform/subdir |
| `IoError` | I/O operation failed |
