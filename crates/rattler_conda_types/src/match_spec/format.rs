//! One renderer for every textual form of a [`MatchSpec`].
//!
//! Legacy `Display`, condition leaves, and the canonical format all render
//! through [`SpecView::fmt`], parameterized by a [`DisplayContext`]. Adding a
//! field to [`MatchSpec`] means extending [`Field`] and the order tables.
//!
//! [`to_canonical_string`] refuses states the grammar cannot represent while
//! rendering, attributing the error to the offending field. Round-trip
//! fidelity of the output is enforced by the property tests in
//! `tests/matchspec_proptest.rs`.

use std::fmt::{self, Display, Write};
use std::str::FromStr;

use itertools::Itertools;
use rattler_digest::{Md5Hash, Sha256Hash};
use rattler_redaction::redact_credentials_from_url;
use url::Url;

use super::condition::MatchSpecCondition;
use super::matcher::StringMatcher;
use super::package_name_matcher::PackageNameMatcher;
use super::parse::{escape_bracket_value, is_valid_extra_group_name};
use super::{CanonicalMatchSpecError, MatchSpec, NamelessMatchSpec};
use crate::flags::is_valid_matchspec_flag;
use crate::{Channel, ChannelConfig, Platform, VersionSpec, build_spec::BuildNumberSpec};

/// The dialect a match spec is rendered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayStyle {
    /// The historic positional format produced by `Display`. Infallible and
    /// best-effort: layout and field order stay stable for existing callers
    /// and values are quoted losslessly when possible (a value holding both
    /// quote characters is not). Not a serialization format; use
    /// [`MatchSpec::to_canonical_string`] for verified output.
    Legacy,
    /// The stable all-bracket representation produced by
    /// [`MatchSpec::to_canonical_string`].
    Canonical,
}

/// Where a match spec is being rendered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SpecPosition {
    /// A stand-alone match spec.
    TopLevel,
    /// A leaf inside a `when="..."` condition: the compact
    /// `{name}{operator}{version}` form is preferred and a nested `when`
    /// cannot be represented.
    ConditionLeaf,
}

/// Parameterizes [`SpecView::fmt`], the single match-spec renderer.
#[derive(Debug, Clone, Copy)]
pub(crate) struct DisplayContext {
    pub style: DisplayStyle,
    pub position: SpecPosition,
}

impl DisplayContext {
    pub(crate) const LEGACY: Self = Self {
        style: DisplayStyle::Legacy,
        position: SpecPosition::TopLevel,
    };
    pub(crate) const CANONICAL: Self = Self {
        style: DisplayStyle::Canonical,
        position: SpecPosition::TopLevel,
    };

    /// The same style, rendered as a condition leaf.
    fn condition_leaf(style: DisplayStyle) -> Self {
        Self {
            style,
            position: SpecPosition::ConditionLeaf,
        }
    }

    /// The separator between bracket fields.
    fn field_separator(self) -> &'static str {
        match self.style {
            DisplayStyle::Legacy => ", ",
            DisplayStyle::Canonical => ",",
        }
    }

    /// The separator between elements of `extras=[..]` and `flags=[..]`.
    fn list_separator(self) -> &'static str {
        self.field_separator()
    }
}

/// A formatting error that can also carry a canonical representability
/// failure, so both dialects run through the same rendering code.
pub(crate) enum FormatError {
    Fmt(fmt::Error),
    Canonical(CanonicalMatchSpecError),
}

impl From<fmt::Error> for FormatError {
    fn from(error: fmt::Error) -> Self {
        Self::Fmt(error)
    }
}

impl From<CanonicalMatchSpecError> for FormatError {
    fn from(error: CanonicalMatchSpecError) -> Self {
        Self::Canonical(error)
    }
}

impl FormatError {
    /// Converts into a plain formatting error. Only valid for the legacy
    /// dialect, which cannot produce canonical errors.
    pub(crate) fn into_fmt_error(self) -> fmt::Error {
        match self {
            Self::Fmt(error) => error,
            Self::Canonical(_) => {
                debug_assert!(false, "the legacy dialect cannot fail canonically");
                fmt::Error
            }
        }
    }
}

/// Every renderable match-spec field except the package name (which is
/// positional in all dialects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Version,
    Build,
    BuildNumber,
    FileName,
    Extras,
    Flags,
    Channel,
    Subdir,
    Namespace,
    Md5,
    Sha256,
    Url,
    License,
    LicenseFamily,
    When,
    TrackFeatures,
}

/// Bracket fields of the legacy top-level format, in their historic order.
/// `version`, `build`, `channel`, `subdir` and `namespace` prefer their
/// positional spot and fall back to these brackets when the positional
/// grammar cannot represent them faithfully (see [`LegacyPlacement`]).
const LEGACY_BRACKET_FIELDS: &[Field] = &[
    Field::Extras,
    Field::Flags,
    Field::Md5,
    Field::Sha256,
    Field::BuildNumber,
    Field::Version,
    Field::Build,
    Field::FileName,
    Field::Url,
    Field::License,
    Field::LicenseFamily,
    Field::TrackFeatures,
    Field::Channel,
    Field::Subdir,
    Field::Namespace,
    Field::When,
];

/// Bracket fields of a legacy condition leaf. The grammar cannot represent a
/// nested `when`, but the infallible legacy dialect still renders it: the
/// leaf parser rejects the key, so the output fails to parse loudly instead
/// of silently dropping the condition.
const LEGACY_CONDITION_LEAF_FIELDS: &[Field] = &[
    Field::Version,
    Field::Build,
    Field::BuildNumber,
    Field::Channel,
    Field::Subdir,
    Field::Namespace,
    Field::Extras,
    Field::Flags,
    Field::Md5,
    Field::Sha256,
    Field::FileName,
    Field::Url,
    Field::License,
    Field::LicenseFamily,
    Field::TrackFeatures,
    Field::When,
];

/// Bracket fields of the canonical format, in their stable documented order.
/// Only the package name is positional in this dialect.
const CANONICAL_BRACKET_FIELDS: &[Field] = &[
    Field::Version,
    Field::Build,
    Field::BuildNumber,
    Field::FileName,
    Field::Extras,
    Field::Flags,
    Field::Channel,
    Field::Subdir,
    Field::Namespace,
    Field::Md5,
    Field::Sha256,
    Field::Url,
    Field::License,
    Field::LicenseFamily,
    Field::When,
    Field::TrackFeatures,
];

/// Which dual-representation fields the legacy positional prefix consumed,
/// so the bracket section skips them. Fields stay positional only when the
/// positional grammar reproduces them faithfully; otherwise they fall back to
/// their bracket form.
#[derive(Debug, Default, Clone, Copy)]
struct LegacyPlacement {
    channel: bool,
    subdir: bool,
    namespace: bool,
    version: bool,
    build: bool,
}

impl LegacyPlacement {
    /// Whether `field` was consumed by the positional prefix.
    fn is_positional(self, field: Field) -> bool {
        match field {
            Field::Channel => self.channel,
            Field::Subdir => self.subdir,
            Field::Namespace => self.namespace,
            Field::Version => self.version,
            Field::Build => self.build,
            _ => false,
        }
    }
}

/// Whether a version or build rendering can occupy a positional slot without
/// being re-tokenized by an earlier parse stage: whitespace splits the
/// version/build slots, and the listed characters are eaten by the
/// channel/namespace colon split, comment stripping, the semicolon check, or
/// bracket detection.
fn is_safe_positional_value(text: &str) -> bool {
    !text.chars().any(char::is_whitespace) && !text.contains([':', '#', ';', '[', ']'])
}

/// A positional build must additionally avoid the version-group separators:
/// `python ==1 ,*` merges the build back into the version group on reparse.
fn is_safe_positional_build(text: &str) -> bool {
    is_safe_positional_value(text) && !text.contains([',', '|'])
}

/// Mirrors the classification in [`StringMatcher`] and
/// [`PackageNameMatcher`] parsing: `^...$` parses as a regex, anything else
/// containing `*` as a glob.
fn classifies_as_regex(text: &str) -> bool {
    text.len() >= 2 && text.starts_with('^') && text.ends_with('$')
}

/// Whether the matcher's rendered text reparses as the same matcher variant.
/// A programmatically constructed `Exact("cuda*")` renders as `cuda*`, which
/// classifies as a glob; no text can represent it.
fn string_matcher_is_canonical(matcher: &StringMatcher) -> bool {
    let text = matcher.to_string();
    let regex = classifies_as_regex(&text);
    let glob = !regex && text.contains('*');
    match matcher {
        StringMatcher::Exact(_) => !regex && !glob,
        StringMatcher::Glob(_) => glob,
        StringMatcher::Regex(_) => regex,
    }
}

/// Writes the positional package name for the canonical dialect, refusing
/// matchers whose text would not reparse as the same matcher.
fn fmt_canonical_name(f: &mut dyn Write, name: &PackageNameMatcher) -> Result<(), FormatError> {
    if !name_matcher_is_canonical(name) {
        return Err(CanonicalMatchSpecError::UnrepresentableName(name.to_string()).into());
    }
    write!(f, "{name}")?;
    Ok(())
}

/// Whether name text survives the condition tokenizer, which splits leaves
/// on parentheses and on `and`/`or` at word boundaries (preceded and
/// followed by whitespace, a parenthesis, or the ends of the token). A
/// trailing operator word only counts when the name is `terminal`: followed
/// by a version or a bracket section it no longer sits on a boundary.
fn name_is_condition_safe(text: &str, terminal: bool) -> bool {
    if text.contains(['(', ')']) {
        return false;
    }
    for word in ["and", "or"] {
        let mut search_start = 0;
        while let Some(found) = text[search_start..].find(word) {
            let position = search_start + found;
            let boundary_before = position == 0
                || text[..position]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_whitespace);
            let after = text[position + word.len()..].chars().next();
            let boundary_after = match after {
                None => terminal,
                Some(character) => character.is_whitespace(),
            };
            if boundary_before && boundary_after {
                return false;
            }
            search_start = position + 1;
        }
    }
    true
}

/// Whether the name matcher's text survives the positional name slot and
/// reparses as the same matcher. Exact names have a safe grammar; glob and
/// regex text must keep its classification and avoid the characters earlier
/// tokenization stages would eat (for regexes, an inner `$` would also end
/// the name early).
fn name_matcher_is_canonical(name: &PackageNameMatcher) -> bool {
    match name {
        PackageNameMatcher::Exact(_) => true,
        PackageNameMatcher::Glob(glob) => {
            let text = glob.as_str();
            text.contains('*') && !classifies_as_regex(text) && is_safe_positional_value(text)
        }
        PackageNameMatcher::Regex(regex) => {
            let text = regex.as_str();
            classifies_as_regex(text)
                && !text.contains([':', '#', ';', '[', ']'])
                && !text[..text.len() - 1].contains('$')
        }
    }
}

/// Whether rendering this channel as its bare name reconstructs it: there is
/// no explicit platform selector, and parsing the name under the default
/// channel alias yields the same base URL. URL and path channels whose name
/// is only derived fail this check and render as their full base URL instead.
fn channel_renders_by_name(channel: &Channel) -> bool {
    if channel.platforms.is_some() {
        return false;
    }
    let Some(name) = channel.name.as_deref() else {
        return false;
    };
    if !is_safe_positional_token(name) {
        return false;
    }
    // A name whose last segment is a platform (`conda-forge/linux-64`) would
    // be split into channel and subdir on reparse; the bracket URL form keeps
    // its trailing slash and survives.
    if name
        .rsplit_once('/')
        .is_some_and(|(_, last)| Platform::from_str(last).is_ok())
    {
        return false;
    }
    // Only the channel alias matters for name resolution; the root dir is
    // used for path channels alone, which never pass the URL comparison.
    let config = ChannelConfig::default_with_root_dir(std::path::PathBuf::new());
    Channel::try_from_name(name, &config)
        .is_some_and(|derived| derived.base_url == channel.base_url)
}

/// Whether a channel name or namespace can occupy a positional slot without
/// being re-tokenized as something else (comments, bracket sections, quotes,
/// version constraints, or the `::` separator itself).
fn is_safe_positional_token(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|c| {
            !c.is_whitespace()
                && !matches!(
                    c,
                    '#' | ';'
                        | ':'
                        | '['
                        | ']'
                        | '"'
                        | '\''
                        | '\\'
                        | '('
                        | ')'
                        | '='
                        | '<'
                        | '>'
                        | '!'
                        | '~'
                        | ','
                )
        })
}

/// Renders a channel as its base URL plus any explicit platform selector.
/// The canonical dialect redacts credentials; the legacy dialect historically
/// renders values raw.
fn channel_url_value(channel: &Channel, redact: bool) -> String {
    let mut value = if redact {
        let mut redacted = redact_credentials_from_url(channel.base_url.url()).to_string();
        // Redacting a token path swallows the trailing slash channel URLs
        // carry; restore it so the rendered URL stays normalized and a second
        // render produces the same text.
        if !redacted.ends_with('/') {
            redacted.push('/');
        }
        redacted
    } else {
        channel.base_url.url().to_string()
    };
    if let Some(platforms) = channel.platforms.as_ref() {
        value.push('[');
        value.push_str(&platforms.iter().format(",").to_string());
        value.push(']');
    }
    value
}

/// A borrowed, name-optional view over the fields shared by [`MatchSpec`] and
/// [`NamelessMatchSpec`], so both types render through the same code.
pub(crate) struct SpecView<'a> {
    pub name: Option<&'a PackageNameMatcher>,
    pub version: Option<&'a VersionSpec>,
    pub build: Option<&'a StringMatcher>,
    pub build_number: Option<&'a BuildNumberSpec>,
    pub file_name: Option<&'a str>,
    pub extras: Option<&'a [String]>,
    pub flags: Option<&'a [StringMatcher]>,
    pub channel: Option<&'a Channel>,
    pub subdir: Option<&'a str>,
    pub namespace: Option<&'a str>,
    pub md5: Option<&'a Md5Hash>,
    pub sha256: Option<&'a Sha256Hash>,
    pub url: Option<&'a Url>,
    pub license: Option<&'a str>,
    pub license_family: Option<&'a str>,
    pub condition: Option<&'a MatchSpecCondition>,
    pub track_features: Option<&'a [String]>,
}

impl<'a> From<&'a MatchSpec> for SpecView<'a> {
    fn from(spec: &'a MatchSpec) -> Self {
        Self {
            name: Some(&spec.name),
            version: spec.version.as_ref(),
            build: spec.build.as_ref(),
            build_number: spec.build_number.as_ref(),
            file_name: spec.file_name.as_deref(),
            extras: spec.extras.as_deref(),
            flags: spec.flags.as_deref(),
            channel: spec.channel.as_deref(),
            subdir: spec.subdir.as_deref(),
            namespace: spec.namespace.as_deref(),
            md5: spec.md5.as_ref(),
            sha256: spec.sha256.as_ref(),
            url: spec.url.as_ref(),
            license: spec.license.as_deref(),
            license_family: spec.license_family.as_deref(),
            condition: spec.condition.as_ref(),
            track_features: spec.track_features.as_deref(),
        }
    }
}

impl<'a> From<&'a NamelessMatchSpec> for SpecView<'a> {
    fn from(spec: &'a NamelessMatchSpec) -> Self {
        Self {
            name: None,
            version: spec.version.as_ref(),
            build: spec.build.as_ref(),
            build_number: spec.build_number.as_ref(),
            file_name: spec.file_name.as_deref(),
            extras: spec.extras.as_deref(),
            flags: spec.flags.as_deref(),
            channel: spec.channel.as_deref(),
            subdir: spec.subdir.as_deref(),
            namespace: spec.namespace.as_deref(),
            md5: spec.md5.as_ref(),
            sha256: spec.sha256.as_ref(),
            url: spec.url.as_ref(),
            license: spec.license.as_deref(),
            license_family: spec.license_family.as_deref(),
            condition: spec.condition.as_ref(),
            track_features: spec.track_features.as_deref(),
        }
    }
}

impl SpecView<'_> {
    /// Renders this spec according to `ctx`. This is the single formatting
    /// entry point behind the `Display` implementations, condition-leaf
    /// rendering, and [`MatchSpec::to_canonical_string`].
    pub(crate) fn fmt(&self, f: &mut dyn Write, ctx: DisplayContext) -> Result<(), FormatError> {
        let placement = self.placement(ctx);
        let bracket_fields = match ctx.style {
            DisplayStyle::Legacy => match ctx.position {
                SpecPosition::TopLevel => LEGACY_BRACKET_FIELDS,
                SpecPosition::ConditionLeaf => LEGACY_CONDITION_LEAF_FIELDS,
            },
            DisplayStyle::Canonical => CANONICAL_BRACKET_FIELDS,
        };
        let renders_brackets = bracket_fields
            .iter()
            .any(|&field| self.has(field) && !placement.is_positional(field));

        match (ctx.style, ctx.position) {
            (DisplayStyle::Legacy, SpecPosition::TopLevel) => {
                self.fmt_legacy_prefix(f, placement, renders_brackets)?;
            }
            (DisplayStyle::Canonical, SpecPosition::TopLevel) => {
                if let Some(name) = self.name {
                    fmt_canonical_name(f, name)?;
                }
            }
            (style, SpecPosition::ConditionLeaf) => {
                if style == DisplayStyle::Canonical && self.condition.is_some() {
                    // A leaf cannot carry its own `when`: the grammar has no
                    // syntax for nested conditions.
                    return Err(CanonicalMatchSpecError::NestedWhen.into());
                }

                if let Some(name) = self.name {
                    if style == DisplayStyle::Canonical {
                        // The condition tokenizer splits leaves on
                        // parentheses and on bare `and`/`or` words, so name
                        // text that contains them cannot be a leaf. A
                        // trailing `and`/`or` only counts when nothing
                        // follows the name in the rendering.
                        let terminal = if self.is_simple_condition_leaf() {
                            self.version.is_none()
                        } else {
                            !renders_brackets
                        };
                        if !name_is_condition_safe(&name.to_string(), terminal) {
                            return Err(CanonicalMatchSpecError::UnrepresentableConditionLeaf(
                                name.to_string(),
                            )
                            .into());
                        }
                        fmt_canonical_name(f, name)?;
                    } else {
                        write!(f, "{name}")?;
                    }
                }
                if self.is_simple_condition_leaf() {
                    if let Some(version) = self.version {
                        write!(f, "{version}")?;
                    }
                    return Ok(());
                }
            }
        }

        let mut first = true;
        for &field in bracket_fields {
            if !self.has(field) || placement.is_positional(field) {
                continue;
            }
            if first {
                f.write_char('[')?;
                first = false;
            } else {
                f.write_str(ctx.field_separator())?;
            }
            self.fmt_field(f, ctx, field)?;
        }
        if !first {
            f.write_char(']')?;
        }

        Ok(())
    }

    /// Decides which dual-representation fields the legacy top-level prefix
    /// consumes. Every other context consumes nothing positionally.
    fn placement(&self, ctx: DisplayContext) -> LegacyPlacement {
        if ctx.style != DisplayStyle::Legacy || ctx.position != SpecPosition::TopLevel {
            return LegacyPlacement::default();
        }

        // `{name}::` only when the name reconstructs the channel, `/{subdir}`
        // only when the parser splits it back off (a known platform), and the
        // `:{namespace}:` slot needs a positional channel.
        let channel = self.name.is_some() && self.channel.is_some_and(channel_renders_by_name);
        // A version rendering with whitespace (some lenient version groups)
        // or tokenizer characters would re-split into version and build.
        let version = self
            .version
            .is_some_and(|version| is_safe_positional_value(&version.to_string()));
        LegacyPlacement {
            channel,
            subdir: channel
                && self
                    .subdir
                    .is_some_and(|subdir| Platform::from_str(subdir).is_ok()),
            namespace: channel && self.namespace.is_some_and(is_safe_positional_token),
            version,
            // Positional only after a positional version (the old
            // `name * build` placeholder reparsed as `version: Any`).
            build: version
                && self
                    .build
                    .is_some_and(|build| is_safe_positional_build(&build.to_string())),
        }
    }

    /// The positional `channel/subdir::name version build` prefix of the
    /// legacy dialect. `renders_brackets` tells the nameless form whether a
    /// bracket section follows, so it can drop the `*` version placeholder
    /// whenever anything else identifies the spec.
    fn fmt_legacy_prefix(
        &self,
        f: &mut dyn Write,
        placement: LegacyPlacement,
        renders_brackets: bool,
    ) -> fmt::Result {
        let Some(name) = self.name else {
            match self.version {
                Some(version) if placement.version => write!(f, "{version}")?,
                // Only emit the historic `*` placeholder when nothing else
                // renders. It reparses as `version: Any`, which matches the
                // same set as `version: None`.
                None if !renders_brackets => f.write_char('*')?,
                _ => {}
            }
            if placement.build
                && let Some(build) = self.build
            {
                write!(f, " {build}")?;
            }
            return Ok(());
        };

        if placement.channel {
            let channel = self.channel.expect("placement checked by caller");
            write!(f, "{}", channel.name())?;
            if placement.subdir {
                let subdir = self.subdir.expect("placement checked by caller");
                write!(f, "/{subdir}")?;
            }
        }

        if placement.namespace {
            let namespace = self.namespace.expect("placement checked by caller");
            write!(f, ":{namespace}:")?;
        } else if placement.channel {
            f.write_str("::")?;
        }

        write!(f, "{name}")?;

        if placement.version
            && let Some(version) = self.version
        {
            write!(f, " {version}")?;
        }

        if placement.build
            && let Some(build) = self.build
        {
            write!(f, " {build}")?;
        }

        Ok(())
    }

    /// Whether `field` has a value and should be rendered.
    fn has(&self, field: Field) -> bool {
        match field {
            Field::Version => self.version.is_some(),
            Field::Build => self.build.is_some(),
            Field::BuildNumber => self.build_number.is_some(),
            Field::FileName => self.file_name.is_some(),
            Field::Extras => self.extras.is_some(),
            Field::Flags => self.flags.is_some(),
            Field::Channel => self.channel.is_some(),
            Field::Subdir => self.subdir.is_some(),
            Field::Namespace => self.namespace.is_some(),
            Field::Md5 => self.md5.is_some(),
            Field::Sha256 => self.sha256.is_some(),
            Field::Url => self.url.is_some(),
            Field::License => self.license.is_some(),
            Field::LicenseFamily => self.license_family.is_some(),
            Field::When => self.condition.is_some(),
            Field::TrackFeatures => self.track_features.is_some(),
        }
    }

    /// Renders a single `key=value` bracket field. Only called for fields
    /// where [`SpecView::has`] returned true.
    fn fmt_field(
        &self,
        f: &mut dyn Write,
        ctx: DisplayContext,
        field: Field,
    ) -> Result<(), FormatError> {
        match field {
            Field::Version => {
                let version = self.version.expect("presence checked by caller");
                write_scalar(f, ctx, "version", version)
            }
            Field::Build => {
                let build = self.build.expect("presence checked by caller");
                if ctx.style == DisplayStyle::Canonical && !string_matcher_is_canonical(build) {
                    return Err(
                        CanonicalMatchSpecError::UnrepresentableBuild(build.to_string()).into(),
                    );
                }
                write_scalar(f, ctx, "build", build)
            }
            Field::BuildNumber => {
                let build_number = self.build_number.expect("presence checked by caller");
                write_scalar(f, ctx, "build_number", build_number)
            }
            Field::FileName => {
                let file_name = self.file_name.expect("presence checked by caller");
                write_scalar(f, ctx, "fn", &file_name)
            }
            Field::Extras => {
                let extras = self.extras.expect("presence checked by caller");
                if ctx.style == DisplayStyle::Canonical {
                    let invalid = extras
                        .iter()
                        .find(|extra| !is_valid_extra_group_name(extra))
                        .cloned()
                        .or_else(|| extras.is_empty().then(String::new));
                    if let Some(extra) = invalid {
                        return Err(CanonicalMatchSpecError::UnrepresentableExtra(extra).into());
                    }
                }
                write_list(f, ctx, "extras", extras.iter(), |extra| {
                    is_valid_extra_group_name(extra)
                })
            }
            Field::Flags => {
                let flags = self.flags.expect("presence checked by caller");
                if ctx.style == DisplayStyle::Canonical {
                    let invalid = flags
                        .iter()
                        .map(ToString::to_string)
                        .zip(flags)
                        .find(|(text, flag)| {
                            !is_valid_matchspec_flag(text) || !string_matcher_is_canonical(flag)
                        })
                        .map(|(text, _)| text)
                        .or_else(|| flags.is_empty().then(String::new));
                    if let Some(flag) = invalid {
                        return Err(CanonicalMatchSpecError::UnrepresentableFlag(flag).into());
                    }
                }
                write_list(f, ctx, "flags", flags.iter(), |flag| {
                    is_valid_matchspec_flag(flag)
                })
            }
            Field::Channel => {
                let channel = self.channel.expect("presence checked by caller");
                match ctx.style {
                    // A bracket channel renders by name only when the name
                    // reconstructs it faithfully; otherwise the full URL (and
                    // platform selector) is used so nothing is lost.
                    DisplayStyle::Legacy => {
                        if channel_renders_by_name(channel) {
                            write_scalar(f, ctx, "channel", &channel.name())
                        } else {
                            write_scalar(f, ctx, "channel", &channel_url_value(channel, false))
                        }
                    }
                    DisplayStyle::Canonical => {
                        let value = canonical_channel_value(channel)?;
                        write!(f, "channel={}", canonical_bracket_value(&value)?)?;
                        Ok(())
                    }
                }
            }
            Field::Subdir => {
                let subdir = self.subdir.expect("presence checked by caller");
                write_scalar(f, ctx, "subdir", &subdir)
            }
            Field::Namespace => {
                let namespace = self.namespace.expect("presence checked by caller");
                write_scalar(f, ctx, "namespace", &namespace)
            }
            Field::Md5 => {
                let md5 = self.md5.expect("presence checked by caller");
                write_scalar(f, ctx, "md5", &hex::encode(md5))
            }
            Field::Sha256 => {
                let sha256 = self.sha256.expect("presence checked by caller");
                write_scalar(f, ctx, "sha256", &hex::encode(sha256))
            }
            Field::Url => {
                let url = self.url.expect("presence checked by caller");
                match ctx.style {
                    DisplayStyle::Legacy => write_scalar(f, ctx, "url", url),
                    // The canonical dialect never serializes credentials.
                    DisplayStyle::Canonical => {
                        write_scalar(f, ctx, "url", &redact_credentials_from_url(url))
                    }
                }
            }
            Field::License => {
                let license = self.license.expect("presence checked by caller");
                write_scalar(f, ctx, "license", &license)
            }
            Field::LicenseFamily => {
                let license_family = self.license_family.expect("presence checked by caller");
                write_scalar(f, ctx, "license_family", &license_family)
            }
            Field::When => {
                let condition = self.condition.expect("presence checked by caller");
                // `when` historically unescapes its outer scalar before
                // parsing the nested condition, so both dialects use the
                // escaped double-quoted form.
                let mut rendered = String::new();
                condition.fmt_with(&mut rendered, ctx.style)?;
                write!(f, "when=\"{}\"", escape_bracket_value(&rendered))?;
                Ok(())
            }
            Field::TrackFeatures => {
                let track_features = self.track_features.expect("presence checked by caller");
                if ctx.style == DisplayStyle::Canonical {
                    let invalid = track_features
                        .iter()
                        .find(|feature| feature.is_empty() || feature.contains([',', ' ']))
                        .cloned()
                        .or_else(|| track_features.is_empty().then(String::new));
                    if let Some(feature) = invalid {
                        return Err(
                            CanonicalMatchSpecError::UnrepresentableTrackFeature(feature).into(),
                        );
                    }
                }
                write_scalar(f, ctx, "track_features", &track_features.iter().format(" "))
            }
        }
    }

    /// Whether this spec can be emitted as the compact
    /// `{name}{operator}{version}` form inside a `when=` condition.
    fn is_simple_condition_leaf(&self) -> bool {
        if !matches!(self.name, Some(PackageNameMatcher::Exact(_))) {
            return false;
        }
        if self.condition.is_some()
            || self.build.is_some()
            || self.build_number.is_some()
            || self.file_name.is_some()
            || self.extras.is_some()
            || self.flags.is_some()
            || self.channel.is_some()
            || self.subdir.is_some()
            || self.namespace.is_some()
            || self.md5.is_some()
            || self.sha256.is_some()
            || self.url.is_some()
            || self.license.is_some()
            || self.license_family.is_some()
            || self.track_features.is_some()
        {
            return false;
        }
        match self.version {
            None => true,
            // The compact form requires the rendered version to start with a
            // version-constraint operator character so the parser can split
            // `{name}` from `{version}`. This excludes e.g. `StartsWith`
            // (renders `1.2.*`) and the wildcard `Any` (`*`). It must also
            // survive tokenization; see `is_safe_positional_value`.
            Some(version) => {
                let text = version.to_string();
                text.chars()
                    .next()
                    .is_some_and(|c| matches!(c, '>' | '<' | '=' | '!' | '~'))
                    && is_safe_positional_value(&text)
                    // Parentheses would be tokenized as condition grouping.
                    && !text.contains(['(', ')'])
            }
        }
    }
}

/// Writes a `key=[..]` list field. Elements rejected by `is_valid_bare` are
/// quoted, so they reach the parser as a single element and fail validation
/// loudly instead of silently splitting.
fn write_list<T: Display>(
    f: &mut dyn Write,
    ctx: DisplayContext,
    key: &str,
    elements: impl Iterator<Item = T>,
    mut is_valid_bare: impl FnMut(&str) -> bool,
) -> Result<(), FormatError> {
    write!(f, "{key}=[")?;
    for (index, element) in elements.enumerate() {
        if index > 0 {
            f.write_str(ctx.list_separator())?;
        }
        let text = element.to_string();
        if is_valid_bare(&text) {
            f.write_str(&text)?;
        } else {
            match pick_quote_delimiter(&text) {
                Some(delimiter) => write!(f, "{delimiter}{text}{delimiter}")?,
                // No delimiter can hold the value; the raw form fails loudly
                // at parse time.
                None => write!(f, "\"{text}\"")?,
            }
        }
    }
    f.write_char(']')?;
    Ok(())
}

/// Writes one scalar `key=value` field.
///
/// Scalars are quoted with a delimiter that keeps the value intact: the
/// parser stores most quoted values verbatim (only `when=` and `flags=`
/// unescape), so escaping would change the value on round-trip. Canonical
/// refuses values no delimiter can hold; infallible legacy falls back to a
/// raw quoted form that fails loudly at parse time.
fn write_scalar(
    f: &mut dyn Write,
    ctx: DisplayContext,
    key: &str,
    value: &dyn Display,
) -> Result<(), FormatError> {
    match ctx.style {
        DisplayStyle::Legacy => {
            let value = value.to_string();
            match pick_quote_delimiter(&value) {
                Some(delimiter) => write!(f, "{key}={delimiter}{value}{delimiter}")?,
                // No delimiter can hold the value, and raw output could
                // re-tokenize into something that parses. Emit an escaped
                // value (so it always tokenizes as a single pair) under a key
                // the parser rejects: the failure is loud no matter what the
                // value contains.
                None => write!(
                    f,
                    "unrepresentable-{key}=\"{}\"",
                    escape_bracket_value(&value)
                )?,
            }
        }
        DisplayStyle::Canonical => write!(f, "{key}={}", canonical_bracket_value(value)?)?,
    }
    Ok(())
}

impl MatchSpecCondition {
    /// Renders this condition in `style` with only the parentheses needed to
    /// preserve the precedence and shape of this left-associative AST.
    pub(crate) fn fmt_with(
        &self,
        f: &mut dyn Write,
        style: DisplayStyle,
    ) -> Result<(), FormatError> {
        self.fmt_with_parent(f, style, 0, false)
    }

    fn fmt_with_parent(
        &self,
        f: &mut dyn Write,
        style: DisplayStyle,
        parent_precedence: u8,
        is_right_child: bool,
    ) -> Result<(), FormatError> {
        let precedence = match self {
            Self::MatchSpec(_) => 3,
            Self::And(_, _) => 2,
            Self::Or(_, _) => 1,
        };
        let needs_parentheses = precedence < parent_precedence
            || (is_right_child && precedence == parent_precedence && precedence < 3);

        if needs_parentheses {
            f.write_char('(')?;
        }
        match self {
            Self::MatchSpec(spec) => {
                SpecView::from(&**spec).fmt(f, DisplayContext::condition_leaf(style))?;
            }
            Self::And(lhs, rhs) => {
                lhs.fmt_with_parent(f, style, precedence, false)?;
                f.write_str(" and ")?;
                rhs.fmt_with_parent(f, style, precedence, true)?;
            }
            Self::Or(lhs, rhs) => {
                lhs.fmt_with_parent(f, style, precedence, false)?;
                f.write_str(" or ")?;
                rhs.fmt_with_parent(f, style, precedence, true)?;
            }
        }
        if needs_parentheses {
            f.write_char(')')?;
        }

        Ok(())
    }
}

/// Picks a quote delimiter that lets `value` be emitted verbatim, or `None`
/// when neither quote character can hold it: both quotes occur unescaped, or
/// a trailing backslash would swallow the closing quote.
fn pick_quote_delimiter(value: &str) -> Option<char> {
    fn contains_unescaped(value: &str, delimiter: char) -> bool {
        let mut escaped = false;
        for character in value.chars() {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == delimiter {
                return true;
            }
        }
        false
    }

    let has_odd_trailing_backslash_run = value
        .chars()
        .rev()
        .take_while(|&character| character == '\\')
        .count()
        % 2
        == 1;
    if has_odd_trailing_backslash_run {
        return None;
    }
    let delimiter_is_safe = |delimiter| !contains_unescaped(value, delimiter);

    if value.contains("\\'") && delimiter_is_safe('"') {
        Some('"')
    } else if delimiter_is_safe('\'') && value.contains(['\\', '"']) {
        Some('\'')
    } else if delimiter_is_safe('"') {
        Some('"')
    } else if delimiter_is_safe('\'') {
        Some('\'')
    } else {
        None
    }
}

/// Formats a scalar value for canonical `MatchSpec` bracket syntax, or errors
/// when no quote delimiter can hold the value losslessly.
fn canonical_bracket_value(value: impl Display) -> Result<String, CanonicalMatchSpecError> {
    let value = value.to_string();
    match pick_quote_delimiter(&value) {
        Some(delimiter) => Ok(format!("{delimiter}{value}{delimiter}")),
        None => Err(CanonicalMatchSpecError::UnrepresentableScalar(value)),
    }
}

/// Renders a channel without losing its base URL or explicit platform
/// selectors, and without serializing credentials.
fn canonical_channel_value(channel: &Channel) -> Result<String, CanonicalMatchSpecError> {
    if channel.platforms.as_ref().is_some_and(Vec::is_empty) {
        // `url[]` cannot be distinguished from an omitted selector by the
        // parser.
        return Err(CanonicalMatchSpecError::EmptyChannelPlatforms);
    }

    Ok(channel_url_value(channel, true))
}
/// Renders `spec` canonically. Representability is checked while rendering;
/// round-trip fidelity of the output is enforced by the property tests in
/// `tests/matchspec_proptest.rs`.
pub(crate) fn to_canonical_string(spec: &MatchSpec) -> Result<String, CanonicalMatchSpecError> {
    let mut rendered = String::new();
    SpecView::from(spec)
        .fmt(&mut rendered, DisplayContext::CANONICAL)
        .map_err(|error| match error {
            FormatError::Canonical(error) => error,
            FormatError::Fmt(_) => unreachable!("writing to a String cannot fail"),
        })?;
    Ok(rendered)
}
