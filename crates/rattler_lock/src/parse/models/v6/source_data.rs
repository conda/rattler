use std::borrow::Cow;

use serde::{Deserialize, Deserializer, Serialize};
use serde_with::{serde_as, DeserializeAs, SerializeAs};
use typed_path::Utf8TypedPathBuf;
use url::Url;

use crate::conda::{GitShallowSpec, PackageBuildSource};
use crate::source::{
    GitReference, GitSourceLocation, PathSourceLocation, SourceLocation, UrlSourceLocation,
};

#[serde_as]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone)]
struct SourceLocationData<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Cow<'a, Url>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Md5>>")]
    pub md5: Option<rattler_digest::Md5Hash>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<rattler_digest::Sha256Hash>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<Cow<'a, Url>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdirectory: Option<Cow<'a, str>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Cow<'a, str>>,
}

impl<'a> From<&'a SourceLocation> for SourceLocationData<'a> {
    fn from(value: &'a SourceLocation) -> Self {
        match value {
            SourceLocation::Url(location) => location.into(),
            SourceLocation::Git(location) => location.into(),
            SourceLocation::Path(location) => location.into(),
        }
    }
}

impl<'a> From<&'a UrlSourceLocation> for SourceLocationData<'a> {
    fn from(value: &'a UrlSourceLocation) -> Self {
        Self {
            url: Some(Cow::Borrowed(&value.url)),
            md5: value.md5,
            sha256: value.sha256,
            git: None,
            rev: None,
            branch: None,
            tag: None,
            subdirectory: None,
            path: None,
        }
    }
}

impl<'a> From<&'a GitSourceLocation> for SourceLocationData<'a> {
    fn from(value: &'a GitSourceLocation) -> Self {
        Self {
            url: None,
            md5: None,
            sha256: None,
            git: Some(Cow::Borrowed(&value.git)),
            rev: if let Some(GitReference::Rev(rev)) = value.rev.as_ref() {
                Some(Cow::Borrowed(rev))
            } else {
                None
            },
            branch: if let Some(GitReference::Branch(branch)) = value.rev.as_ref() {
                Some(Cow::Borrowed(branch))
            } else {
                None
            },
            tag: if let Some(GitReference::Tag(tag)) = value.rev.as_ref() {
                Some(Cow::Borrowed(tag))
            } else {
                None
            },
            subdirectory: value.subdirectory.as_deref().map(Cow::Borrowed),
            path: None,
        }
    }
}

impl<'a> From<&'a PathSourceLocation> for SourceLocationData<'a> {
    fn from(value: &'a PathSourceLocation) -> Self {
        Self {
            url: None,
            md5: None,
            sha256: None,
            git: None,
            rev: None,
            branch: None,
            tag: None,
            subdirectory: None,
            path: Some(Cow::Borrowed(value.path.as_str())),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SourceLocationError {
    #[error("must specify exactly one of `url`, `path` or `git`")]
    MissingOrMultipleSourceRoots,

    #[error("must specify none or exactly one of `branch`, `tag` or `rev`")]
    MultipleGitReferences,

    #[error("`path` can not have `subdir`")]
    PathSubdir,
}

impl<'a> TryFrom<SourceLocationData<'a>> for SourceLocation {
    type Error = SourceLocationError;

    fn try_from(value: SourceLocationData<'a>) -> Result<Self, Self::Error> {
        let SourceLocationData {
            url,
            md5,
            sha256,
            path,
            git,
            rev,
            branch,
            tag,
            subdirectory,
        } = value;

        let count = [url.is_some(), path.is_some(), git.is_some()]
            .into_iter()
            .filter(|&x| x)
            .count();
        if count != 1 {
            return Err(SourceLocationError::MissingOrMultipleSourceRoots);
        }

        if let Some(url) = url {
            let url = url.into_owned();
            Ok(SourceLocation::Url(UrlSourceLocation { url, md5, sha256 }))
        } else if let Some(path) = path {
            if subdirectory.is_some() {
                return Err(SourceLocationError::PathSubdir);
            }
            let path = path.into_owned().into();
            Ok(SourceLocation::Path(PathSourceLocation { path }))
        } else if let Some(git) = git {
            let git = git.into_owned();
            let rev = match (rev, branch, tag) {
                (Some(rev), None, None) => Some(GitReference::Rev(rev.into_owned())),
                (None, Some(branch), None) => Some(GitReference::Branch(branch.into_owned())),
                (None, None, Some(tag)) => Some(GitReference::Tag(tag.into_owned())),
                (None, None, None) => None,
                _ => return Err(SourceLocationError::MultipleGitReferences),
            };

            Ok(SourceLocation::Git(GitSourceLocation {
                git,
                rev,
                subdirectory: subdirectory.map(Cow::into_owned),
            }))
        } else {
            unreachable!("we already checked that exactly one of url, path or git is set")
        }
    }
}

#[serde_as]
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone)]
struct PackageBuildSourceData<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<Cow<'a, Url>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde_as(as = "Option<rattler_digest::serde::SerializableHash::<rattler_digest::Sha256>>")]
    pub sha256: Option<rattler_digest::Sha256Hash>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<Cow<'a, Url>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub branch: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<Cow<'a, str>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explicit_rev: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rev: Option<Cow<'a, str>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Cow<'a, str>>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub subdir: Option<Cow<'a, str>>,
}

impl<'a> From<&'a PackageBuildSource> for PackageBuildSourceData<'a> {
    fn from(value: &'a PackageBuildSource) -> Self {
        match value {
            PackageBuildSource::Git {
                url,
                spec,
                rev,
                subdir,
            } => {
                let (branch, tag, explicit_rev) = match spec {
                    Some(GitShallowSpec::Branch(branch)) => {
                        (Some(Cow::Borrowed(branch.as_str())), None, None)
                    }
                    Some(GitShallowSpec::Tag(tag)) => {
                        (None, Some(Cow::Borrowed(tag.as_str())), None)
                    }
                    Some(GitShallowSpec::Rev) => (None, None, Some(true)),
                    None => (None, None, None),
                };
                let subdir = subdir.as_ref().map(|p| Cow::Borrowed(p.as_str()));
                Self {
                    url: None,
                    sha256: None,
                    git: Some(Cow::Borrowed(url)),
                    branch,
                    tag,
                    explicit_rev,
                    path: None,
                    rev: Some(Cow::Borrowed(rev)),
                    subdir,
                }
            }
            PackageBuildSource::Url {
                url,
                sha256,
                subdir,
            } => Self {
                url: Some(Cow::Borrowed(url)),
                sha256: Some(*sha256),
                git: None,
                branch: None,
                tag: None,
                explicit_rev: None,
                rev: None,
                path: None,
                subdir: subdir.as_ref().map(|p| Cow::Borrowed(p.as_str())),
            },
            PackageBuildSource::Path { path } => Self {
                url: None,
                sha256: None,
                git: None,
                branch: None,
                tag: None,
                explicit_rev: None,
                rev: None,
                path: Some(Cow::Borrowed(path.as_str())),
                subdir: None,
            },
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PackageBuildSourceError {
    #[error("must specify exactly one of `url` or `git`")]
    MissingOrMultipleSourceRoots,

    #[error("url source must have sha256 hash")]
    MissingSha256ForUrl,

    #[error("git source must have rev")]
    MissingRevForGit,

    #[error("git source cannot have both branch and tag")]
    BranchAndTag,
}

impl<'a> TryFrom<PackageBuildSourceData<'a>> for PackageBuildSource {
    type Error = PackageBuildSourceError;

    fn try_from(value: PackageBuildSourceData<'a>) -> Result<Self, Self::Error> {
        let PackageBuildSourceData {
            url,
            sha256,
            git,
            branch,
            tag,
            rev,
            explicit_rev,
            path,
            subdir,
        } = value;

        let count = [url.is_some(), git.is_some(), path.is_some()]
            .into_iter()
            .filter(|&x| x)
            .count();
        if count != 1 {
            return Err(PackageBuildSourceError::MissingOrMultipleSourceRoots);
        }

        let path = path.map(|s| Utf8TypedPathBuf::from(&*s));
        let subdir = subdir.map(|s| Utf8TypedPathBuf::from(&*s));

        if let Some(url) = url {
            let url = url.into_owned();
            let sha256 = sha256.ok_or(PackageBuildSourceError::MissingSha256ForUrl)?;
            Ok(PackageBuildSource::Url {
                url,
                sha256,
                subdir,
            })
        } else if let Some(git) = git {
            let git = git.into_owned();
            let rev = rev
                .ok_or(PackageBuildSourceError::MissingRevForGit)?
                .into_owned();

            if branch.is_some() && tag.is_some() {
                return Err(PackageBuildSourceError::BranchAndTag);
            }

            let spec = if let Some(branch) = branch {
                Some(GitShallowSpec::Branch(branch.into_owned()))
            } else if let Some(tag) = tag {
                Some(GitShallowSpec::Tag(tag.into_owned()))
            } else if let Some(true) = explicit_rev {
                Some(GitShallowSpec::Rev)
            } else {
                None
            };

            Ok(PackageBuildSource::Git {
                url: git,
                spec,
                rev,
                subdir,
            })
        } else if let Some(path) = path {
            Ok(PackageBuildSource::Path { path })
        } else {
            unreachable!("we already checked that exactly one of url or git is set")
        }
    }
}

pub struct SourceLocationSerializer;

pub struct PackageBuildSourceSerializer;

impl SerializeAs<PackageBuildSource> for PackageBuildSourceSerializer {
    fn serialize_as<S>(source: &PackageBuildSource, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let data = PackageBuildSourceData::from(source);
        data.serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, PackageBuildSource> for PackageBuildSourceSerializer {
    fn deserialize_as<D>(deserializer: D) -> Result<PackageBuildSource, D::Error>
    where
        D: Deserializer<'de>,
    {
        PackageBuildSourceData::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}

impl SerializeAs<SourceLocation> for SourceLocationSerializer {
    fn serialize_as<S>(source: &SourceLocation, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let data = SourceLocationData::from(source);
        data.serialize(serializer)
    }
}

impl<'de> DeserializeAs<'de, SourceLocation> for SourceLocationSerializer {
    fn deserialize_as<D>(deserializer: D) -> Result<SourceLocation, D::Error>
    where
        D: Deserializer<'de>,
    {
        SourceLocationData::deserialize(deserializer)?
            .try_into()
            .map_err(serde::de::Error::custom)
    }
}
