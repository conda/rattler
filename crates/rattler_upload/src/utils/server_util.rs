use regex::Regex;
use std::sync::OnceLock;
use url::Url;

/// Simplified server type without options
#[derive(Debug, Clone, PartialEq)]
pub enum SimpleServerType {
    Quetz,
    Artifactory,
    Prefix,
    Anaconda,
    #[cfg(feature = "s3")]
    S3,
    CondaForge,
    Unknown,
}

/// Server patterns
struct ServerPatterns {
    prefix: Regex,
    anaconda: Regex,
    quetz: Regex,
    artifactory: Regex,
    #[cfg(feature = "s3")]
    s3: Regex,
    conda_forge: Regex,
}

impl ServerPatterns {
    fn new() -> Self {
        Self {
            // Prefix.dev patterns
            prefix: Regex::new(r"^https?://(?:www\.)?prefix\.dev/").unwrap(),
            
            // Anaconda patterns
            anaconda: Regex::new(r"^https?://(?:upload\.)?anaconda\.org/").unwrap(),
            
            // Quetz patterns (generic quetz server with /api/channels/)
            quetz: Regex::new(r"^https?://[^/]+/api/channels/").unwrap(),
            
            // Artifactory patterns (contains /artifactory/ in path)
            artifactory: Regex::new(r"^https?://[^/]+/artifactory/").unwrap(),
            
            #[cfg(feature = "s3")]
            // S3 patterns
            s3: Regex::new(r"^(?:https?://[^.]+\.s3(?:\.[^.]+)?\.amazonaws\.com|s3://)").unwrap(),
            
            // Conda-forge patterns
            conda_forge: Regex::new(r"^https?://(?:www\.)?github\.com/conda-forge/").unwrap(),
        }
    }
}

static PATTERNS: OnceLock<ServerPatterns> = OnceLock::new();

fn get_server_patterns() -> &'static ServerPatterns {
    PATTERNS.get_or_init(ServerPatterns::new)
}

/// Determine server type from host URL
/// 
/// # Arguments
/// * `host` - The host URL to analyze
/// 
/// # Returns
/// * `SimpleServerType` - The detected server type or Unknown
/// 
/// ```
pub fn check_server_type(host: &str) -> SimpleServerType {
    let patterns = get_server_patterns();
    
    // 1. Check Prefix.dev (most specific)
    if patterns.prefix.is_match(host) {
        return SimpleServerType::Prefix;
    }

    // 2. Check Anaconda.org
    if patterns.anaconda.is_match(host) {
        return SimpleServerType::Anaconda;
    }

    // 3. Check Conda-forge (GitHub)
    if patterns.conda_forge.is_match(host) {
        return SimpleServerType::CondaForge;
    }

    // 4. Check S3
    #[cfg(feature = "s3")]
    if patterns.s3.is_match(host) {
        return SimpleServerType::S3;
    }

    // 5. Check Artifactory (contains /artifactory/ in path)
    if patterns.artifactory.is_match(host) {
        return SimpleServerType::Artifactory;
    }

    // 6. Check Quetz (generic pattern with /api/channels/)
    if patterns.quetz.is_match(host) {
        return SimpleServerType::Quetz;
    }

    // 7. Unknown server type
    SimpleServerType::Unknown
}