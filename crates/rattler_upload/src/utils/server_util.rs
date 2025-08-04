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
            
            // Quetz patterns
            quetz: Regex::new(r"^https?://[^/]+/api/channels/").unwrap(),
            
            // Artifactory patterns
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
pub fn check_server_type(host_url: &Url) -> SimpleServerType {
    let patterns = get_server_patterns();
    let host = host_url.as_str();
    
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

// Extract Quetz base_url and channel from the host
pub fn extract_quetz_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let url_str = url.as_str();
    let quetz_pattern = Regex::new(r"^(https?://[^/]+)(?:/api/channels/([^/]+))?").unwrap();
    
    if let Some(captures) = quetz_pattern.captures(url_str) {
        // 1. Extract base URL
        let base_url = captures.get(1).unwrap().as_str();
        let base_url = Url::parse(base_url)?;
        
        // 2. Extract channel, default to "main" if not found
        let channel = captures.get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| "main".to_string());
        
        Ok((base_url, channel))
    } else {
        Err("Invalid Quetz URL format".into())
    }
}

// Extract Artifactory base_url and channel from host
pub fn extract_artifactory_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let url_str = url.as_str();
    let artifactory_pattern = Regex::new(r"^(https?://[^/]+)/artifactory/([^/]+)(?:/(.+)/([^/]+))?").unwrap();
    
    if let Some(captures) = artifactory_pattern.captures(url_str) {
        // 1. Extract base URL - e.g., "https://artifactory.company.com"
        let base_url = captures.get(1).unwrap().as_str();
        let base_url = Url::parse(base_url)?;
        
        // 2. Extract repository/channel 
        let channel = captures.get(2).unwrap().as_str().to_string();
        
        Ok((base_url, channel))
    } else {
        Err("Invalid Artifactory URL format".into())
    }
}

// Extract Prefix base_url and channel from host
pub fn extract_prefix_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let url_str = url.as_str();
    let prefix_pattern: Regex = Regex::new(r"^https?://(?:www\.)?prefix\.dev(?:/api/v1/upload/([^/]+)|/([^/]+))?$").unwrap();
    
    if let Some(captures) = prefix_pattern.captures(url_str) {
        // 1. Extract base_url
        let base_url = Url::parse(&format!("{}://{}", url.scheme(), url.host_str().unwrap()))?;
        
        // 2. Extract channel - defaults to "conda-forge" 
        let channel: String = captures.get(1)
            .or(captures.get(2))
            .map(|m| m.as_str().to_string()) 
            .unwrap_or_else(|| "conda-forge".to_string());
        
        Ok((base_url, channel))
    } else {
        Err("Invalid Prefix.dev URL format".into())
    }
}