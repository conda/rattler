//! Data types for [CEP-6] channel notices.
//!
//! [CEP-6]: https://github.com/conda/ceps/blob/main/cep-0006.md

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

/// The importance of a channel notice.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelNoticeLevel {
    /// General information.
    #[default]
    Info,
    /// A warning that may require action from the user.
    Warning,
    /// A critical notice, such as a security advisory.
    Critical,
}

/// A notice published by a channel in its `notices.json` file.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChannelNotice {
    /// A stable identifier for the notice.
    pub id: String,
    /// The message to display to users.
    pub message: String,
    /// The importance of the notice.
    #[serde(default)]
    pub level: ChannelNoticeLevel,
    /// When the notice was created.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<Timestamp>,
    /// When the notice expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expired_at: Option<Timestamp>,
    /// The requested interval between displaying the notice.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interval: Option<u64>,
}

/// The contents of a CEP-6 `notices.json` file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChannelNotices {
    /// Notices published by the channel.
    #[serde(default)]
    pub notices: Vec<ChannelNotice>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_cep6_notices() {
        let notices: ChannelNotices = serde_json::from_str(
            r#"{
                "notices": [{
                    "id": "security-1",
                    "message": "Please update demo",
                    "level": "critical",
                    "created_at": "2025-01-01T12:00:00+00:00",
                    "expired_at": "2025-02-01T12:00:00+00:00",
                    "interval": 24
                }]
            }"#,
        )
        .unwrap();

        assert_eq!(notices.notices.len(), 1);
        assert_eq!(notices.notices[0].level, ChannelNoticeLevel::Critical);
        assert_eq!(notices.notices[0].interval, Some(24));
    }
}
