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
            conda_forge: Regex::new(r"^https?://[^/]*conda-forge").unwrap(),
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
        
        // 2. Extract channel 
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

// Extract Anaconda base_url and channel from host
pub fn extract_anaconda_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let url_str = url.as_str();
    let anaconda_pattern = Regex::new(r"^(https?://(?:upload\.)?anaconda\.org)(?:/([^/]+))?").unwrap();
    
    if let Some(captures) = anaconda_pattern.captures(url_str) {
        // 1. Extract base_url
        let base_url = Url::parse(&url.origin().ascii_serialization())?;
        
        // 2. Extract channel - defaults to "main"
        let channel = captures.get(2)
            .map(|m| m.as_str().to_string())
            .unwrap_or_else(|| "main".to_string());
        
        Ok((base_url, channel))
    } else {
        Err("Invalid Anaconda.org URL format".into())
    }
}

// Extract S3 base_url and channel from host
pub fn extract_s3_info(url: &Url) -> Result<(Url, String, String), Box<dyn std::error::Error>> {
    let url_str = url.as_str();
    
    // Handle both HTTP(S) and S3:// protocols
    if url_str.starts_with("s3://") {
        // S3 URI format: s3://bucket-name/channel-name
        let s3_pattern = Regex::new(r"^s3://([^/]+)(?:/([^/]+))?").unwrap();
        if let Some(captures) = s3_pattern.captures(url_str) {
            let channel = captures.get(2)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "main".to_string()); // Default channel
            let base_url = Url::parse("https://s3.amazonaws.com")?; // Default S3 endpoint
            let region = "eu-central-1".to_string(); // Default region for s3:// URLs
            return Ok((base_url, channel, region));
        }
    } else {
        // HTTP(S) format: https://bucket.s3.region.amazonaws.com/channel
        let s3_pattern = Regex::new(r"^(https?://([^.]+)\.s3(?:\.([^.]+))?\.amazonaws\.com)(?:/([^/]+))?").unwrap();
        if let Some(captures) = s3_pattern.captures(url_str) {
            let base_url_str = captures.get(1).unwrap().as_str();
            let region = captures.get(3)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "eu-central-1".to_string());
            let channel = captures.get(4)
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| "main".to_string());
            
            let base_url = Url::parse(base_url_str)?;
            return Ok((base_url, channel, region));
        }
    }
    
    Err("Invalid S3 URL format".into())
}

// Extract S3 base_url and channel from host
pub fn extract_conda_forge_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let base_url = Url::parse(&url.origin().ascii_serialization())
        .map_err(|e| format!("Failed to parse base URL: {}", e))?;
    
    // Extract channel from path - first path segment or "main" as default
    let channel = url.path_segments()
        .and_then(|mut segments| segments.next())
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();
    
    Ok((base_url, channel))
}