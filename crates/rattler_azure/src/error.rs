/// Why a channel URL, host, name, endpoint key or location was rejected.
#[derive(Debug, thiserror::Error)]
pub enum AzureUrlError {
    #[error("no host in Azure blob URL")]
    NoHost,

    #[error(
        "Azure blob URL must not contain userinfo (`user:pass@host`), userinfo is invalid in blob URLs"
    )]
    UserInfoNotAllowed,

    #[error("`{authority}` is not a valid Azure host: {reason}; expected `host` or `host:port`")]
    InvalidHostAuthority { authority: String, reason: String },

    #[error(
        "`{0}` has no storage account label: a host-style key needs a dotted host whose first \
         label is the account; write `<host>/<account>` to name the account in the path"
    )]
    InvalidHost(String),

    #[error("`{0}` is not a valid Azure endpoint key: write `<host>` or `<host>/<account>`")]
    InvalidKey(String),

    #[error(
        "`{0}` is not a known Azure blob endpoint and no `azure-options` entry matches it (ports \
         and path-style account names must match exactly); add an entry for `{0}` (host-style) \
         or `{0}/<account>` (path-style)"
    )]
    UnconfiguredHost(String),

    #[error(
        "`{0}` is not a known Azure blob endpoint and no `azure-options` entry matches it (ports \
         and path-style account names must match exactly); the host does not start with an \
         account name, so add a path-style entry for `{0}/<account>`"
    )]
    UnconfiguredHostWithoutAccount(String),

    #[error(
        "`{0}` is not a valid Azure storage account name: account names are 3-24 characters of \
         lowercase letters and digits only"
    )]
    InvalidAccountName(String),

    #[error(
        "`{0}` is not a valid Azure blob container name: container names are 3-63 characters of \
         lowercase letters, digits and hyphens, must start and end with a letter or digit, and \
         must not contain consecutive hyphens"
    )]
    InvalidContainerName(String),

    #[error("`{value}` is not a valid URL")]
    InvalidUrl {
        value: String,
        #[source]
        source: url::ParseError,
    },

    #[error(
        "Azure blob channel URL segment `{0}` is a relative path segment; a channel URL must name \
         the container it addresses directly, so write the path without `.` or `..`"
    )]
    DotSegmentInPath(String),

    #[error(
        "Azure blob channel URL segment `{0}` percent-decodes to a path separator; write `/` \
         unencoded between segments"
    )]
    EncodedSeparatorInPath(String),

    #[error(
        "Azure blob channel URL path `{path}` has an empty segment; a doubled `/` names nothing, \
         so write the path with single separators"
    )]
    EmptyPathSegment { path: String },

    #[error(
        "Azure blob channel URL segment `{segment}` contains `{escape}`, which is not a valid \
         percent-escape; write a literal `%` as `%25`"
    )]
    MalformedPercentEscape { segment: String, escape: String },

    /// Blob names are UTF-8. Decoding lossily would substitute U+FFFD and silently
    /// address a different blob than the URL names.
    #[error(
        "Azure blob channel URL segment `{segment}` percent-decodes to bytes that are not UTF-8, \
         so it cannot name a blob"
    )]
    NonUtf8Path {
        segment: String,
        #[source]
        source: std::str::Utf8Error,
    },

    #[error("Azure blob channel URL must use the `az://` scheme, got `{0}`")]
    InvalidScheme(String),

    #[error(
        "{0:?} contains a control character or a leading or trailing space, which URL parsing \
         would silently drop; remove it"
    )]
    StrippedCharacter(String),
}
