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
    let host = match host_url.host_str() {
        Some(host) => host,
        None => return SimpleServerType::Unknown,
    };

    // 1. Check Prefix.dev
    if host.contains("prefix.dev") {
        return SimpleServerType::Prefix;
    }

    // 2. Check Anaconda.org
    if host.contains("anaconda.org") {
        return SimpleServerType::Anaconda;
    }

    // 3. Check Conda-forge (GitHub)
    if host.contains("conda-forge") {
        return SimpleServerType::CondaForge;
    }

    // 4. Check S3
    #[cfg(feature = "s3")]
    if host_url.scheme() == "s3" || 
       (host.contains("s3") && host.contains("amazonaws.com")) {
        return SimpleServerType::S3;
    }

    // 5. Check Artifactory
    if host_url.path().contains("/artifactory/") {
        return SimpleServerType::Artifactory;
    }

    // 6. Check Quetz 
    if host_url.path().contains("/api/channels/") {
        return SimpleServerType::Quetz;
    }

    // 7. Unknown server type
    SimpleServerType::Unknown
}

// Extract Quetz base_url and channel from the host
pub fn extract_quetz_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    // Extract base URL (scheme + host)
    let mut base_url = url.clone();
    base_url.set_path("");
    base_url.set_query(None);

    // Parse path to find channel in /api/channels/CHANNEL pattern
    let path_segments: Vec<&str> = url.path_segments()
        .ok_or("Cannot extract path segments")?
        .collect();

    // Look for /api/channels/CHANNEL pattern
    if let Some(api_pos) = path_segments.iter().position(|&s| s == "api") {
        if path_segments.get(api_pos + 1) == Some(&"channels") {
            if let Some(&channel) = path_segments.get(api_pos + 2) {
                return Ok((base_url, channel.to_string()));
            }
        }
    }

    // Default to "main" if no channel found
    Ok((base_url, "main".to_string()))
}

// Extract Artifactory base_url and channel from host
pub fn extract_artifactory_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let path_segments: Vec<&str> = url.path_segments()
        .ok_or("Cannot extract path segments")?
        .collect();

    // Look for /artifactory/CHANNEL pattern
    if let Some(artifactory_pos) = path_segments.iter().position(|&s| s == "artifactory") {
        if let Some(&channel) = path_segments.get(artifactory_pos + 1) {
            let mut base_url = url.clone();
            base_url.set_path("");
            return Ok((base_url, channel.to_string()));
        }
    }

    Err("Invalid Artifactory URL format".into())
}

// Extract Prefix base_url and channel from host
pub fn extract_prefix_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let path_segments: Vec<&str> = url.path_segments()
        .unwrap_or_else(|| "".split('/'))
        .filter(|s| !s.is_empty())
        .collect();

    let mut base_url = url.clone();
    base_url.set_path("");

    // Look for API upload pattern: /api/v1/upload/CHANNEL
    if path_segments.len() >= 4 && 
       path_segments[0] == "api" && 
       path_segments[1] == "v1" && 
       path_segments[2] == "upload" {
        return Ok((base_url, path_segments[3].to_string()));
    }

    // Look for direct channel pattern: /CHANNEL
    if let Some(&channel) = path_segments.first() {
        return Ok((base_url, channel.to_string()));
    }

    // Default to conda-forge
    Ok((base_url, "conda-forge".to_string()))
}

// Extract Anaconda base_url and channel from host
pub fn extract_anaconda_info(url: &Url) -> Result<(Url, Vec<String>), Box<dyn std::error::Error>> {
    let mut base_url = url.clone();
    base_url.set_path("");

    let path_segments: Vec<&str> = url.path_segments()
        .unwrap_or_else(|| "".split('/'))
        .filter(|s| !s.is_empty())
        .collect();

    // Extract channel from first path segment, default to "main"
    let channel = if let Some(&first_segment) = path_segments.first() {
        vec![first_segment.to_string()]
    } else {
        vec!["main".to_string()]
    };

    Ok((base_url, channel))
}

#[cfg(feature = "s3")]
// Extract S3 base_url and channel from host
pub fn extract_s3_info(url: &Url) -> Result<(Url, Url, String), Box<dyn std::error::Error>> {
    if url.scheme() == "s3" {
        // S3 URI format: s3://bucket-name/channel-name
        let host = url.host_str().ok_or("No host in S3 URL")?;
        let path_segments: Vec<&str> = url.path_segments()
            .unwrap_or_else(|| "".split('/'))
            .filter(|s| !s.is_empty())
            .collect();

        let channel_name = path_segments.first()
            .map(|s| s.to_string())
            .unwrap_or_else(|| "main".to_string());

        let base_url = Url::parse("https://s3.amazonaws.com")?;
        let channel = Url::parse(&format!("s3://{}/{}", host, channel_name))?;
        let region = "eu-central-1".to_string();

        return Ok((base_url, channel, region));
    } else if url.scheme().starts_with("http") && url.host_str().unwrap_or("").contains("s3") {
        // HTTP(S) format: https://bucket.s3.region.amazonaws.com/channel
        let host = url.host_str().ok_or("No host in URL")?;
        let host_parts: Vec<&str> = host.split('.').collect();

        if host_parts.len() >= 4 && host_parts[1] == "s3" && host_parts.last() == Some(&"com") {
            let bucket = host_parts[0];
            let region = if host_parts.len() > 4 { 
                host_parts[2].to_string() 
            } else { 
                "eu-central-1".to_string() 
            };

            let path_segments: Vec<&str> = url.path_segments()
                .unwrap_or_else(|| "".split('/'))
                .filter(|s| !s.is_empty())
                .collect();

            let base_url = url.clone();
            let channel_name = path_segments.first()
                .map(|s| s.to_string())
                .unwrap_or_else(|| "main".to_string());
            let channel = Url::parse(&format!("s3://{}/{}", bucket, channel_name))?;

            return Ok((base_url, channel, region));
        }
    }

    Err("Invalid S3 URL format".into())
}

// Extract S3 base_url and channel from host
pub fn extract_conda_forge_info(url: &Url) -> Result<(Url, String), Box<dyn std::error::Error>> {
    let mut base_url = url.clone();
    base_url.set_path("");

    // Extract channel from path - first path segment or "main" as default
    let channel = url.path_segments()
        .and_then(|mut segments| segments.next())
        .filter(|s| !s.is_empty())
        .unwrap_or("main")
        .to_string();

    Ok((base_url, channel))
}
