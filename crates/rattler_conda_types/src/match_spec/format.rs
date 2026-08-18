//! A single rendering pipeline for the [`MatchSpec`] family of types.
//!
//! Every textual representation of a match spec — the historic positional
//! [`std::fmt::Display`] format, the compact form used for leaves inside
//! `when="..."` conditions, and the stable canonical V3 format — is produced
//! by one renderer that is parameterized by a [`DisplayContext`]. This keeps
//! the field inventory in one place: adding a field to [`MatchSpec`] means
//! extending [`Field`] and the per-context order tables instead of updating
//! several hand-rolled formatters.
//!
//! Canonical rendering is verified by a single round-trip through the parser
//! in [`to_canonical_string`]. The happy path therefore performs exactly one
//! parse; the per-field forensics in [`diagnose_parse_failure`] and
//! [`canonical_divergence`] only run when that round-trip fails, to attribute
//! the failure to a specific field.

use std::fmt::{self, Display, Write};
use std::str::FromStr;

use itertools::Itertools;
use rattler_digest::{Md5Hash, Sha256Hash};
use rattler_redaction::redact_credentials_from_url;
use url::Url;

use super::condition::{MatchSpecCondition, parse_condition_with_options};
use super::matcher::StringMatcher;
use super::package_name_matcher::PackageNameMatcher;
use super::parse::{escape_bracket_value, is_valid_extra_group_name};
use super::{CanonicalMatchSpecError, MatchSpec, NamelessMatchSpec};
use crate::flags::is_valid_matchspec_flag;
use crate::{
    Channel, ChannelConfig, ParseMatchSpecOptions, ParseStrictness, Platform, RepodataRevision,
    VersionSpec, build_spec::BuildNumberSpec,
};

/// The dialect a match spec is rendered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayStyle {
    /// The historic positional representation produced by `Display`. The
    /// layout and field order are kept stable for existing callers. Every
    /// bracket value is quoted with a delimiter that preserves it verbatim
    /// where one exists, but this dialect is infallible and best-effort — it
    /// is not a serialization format and not every value round-trips (e.g.
    /// values containing `]` or both quote characters). Use
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
    /// A leaf inside a `when="..."` condition, where the compact
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

/// A formatting error that also carries canonical representability failures,
/// so the canonical dialect can flow through the same rendering code as the
/// infallible legacy dialect.
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
/// `version` is always positional in this dialect; `build`, `channel`,
/// `subdir` and `namespace` prefer their positional spot and fall back to
/// these brackets when the positional grammar cannot represent them
/// faithfully (see [`LegacyPlacement`]).
const LEGACY_BRACKET_FIELDS: &[Field] = &[
    Field::Extras,
    Field::Flags,
    Field::Md5,
    Field::Sha256,
    Field::BuildNumber,
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

/// Bracket fields of a legacy condition leaf. Conditions cannot nest, so
/// `when` is absent.
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
    build: bool,
}

impl LegacyPlacement {
    /// Whether `field` was consumed by the positional prefix.
    fn is_positional(self, field: Field) -> bool {
        match field {
            Field::Channel => self.channel,
            Field::Subdir => self.subdir,
            Field::Namespace => self.namespace,
            Field::Build => self.build,
            _ => false,
        }
    }
}

/// Whether rendering this channel as its bare name reconstructs it
/// faithfully: no explicit platform selector, and parsing the name under the
/// default channel alias yields the same base URL. URL channels whose name is
/// merely derived from the URL (and path channels) fail this and render as
/// their full base URL instead.
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
    // Only the channel alias matters for name resolution; the root dir is
    // used for path channels alone, which never pass the URL comparison.
    let config = ChannelConfig::default_with_root_dir(std::path::PathBuf::new());
    Channel::from_name(name, &config).base_url == channel.base_url
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
        redact_credentials_from_url(channel.base_url.url()).to_string()
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
                    write!(f, "{name}")?;
                }
            }
            (style, SpecPosition::ConditionLeaf) => {
                if style == DisplayStyle::Canonical && self.condition.is_some() {
                    // A leaf cannot carry its own `when`: the grammar has no
                    // syntax for nested conditions.
                    return Err(CanonicalMatchSpecError::NestedWhen.into());
                }
                debug_assert!(
                    self.condition.is_none(),
                    "MatchSpec inside a `when=` condition must not itself carry a `when` clause",
                );

                if let Some(name) = self.name {
                    write!(f, "{name}")?;
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

    /// Decides which dual-representation fields the positional prefix of the
    /// legacy top-level dialect consumes. In every other context nothing is
    /// consumed positionally besides the name and the compact leaf version,
    /// which are handled directly by [`SpecView::fmt`].
    fn placement(&self, ctx: DisplayContext) -> LegacyPlacement {
        if ctx.style != DisplayStyle::Legacy || ctx.position != SpecPosition::TopLevel {
            return LegacyPlacement::default();
        }

        // A channel renders as `{name}::` only when that string reconstructs
        // the channel faithfully; a subdir rides along as `{name}/{subdir}::`
        // only when the parser will split it back off (it must be a known
        // platform); the `{channel}:{namespace}:` slot requires a positional
        // channel. Everything else falls back to bracket fields.
        let channel = self.name.is_some() && self.channel.is_some_and(channel_renders_by_name);
        LegacyPlacement {
            channel,
            subdir: channel
                && self
                    .subdir
                    .is_some_and(|subdir| Platform::from_str(subdir).is_ok()),
            namespace: channel && self.namespace.is_some_and(is_safe_positional_token),
            // A build matcher is positional only after a version (the historic
            // `name * build` placeholder reparsed with `version: Any` instead
            // of `version: None`), and only when its text survives the
            // tokenization stages that run before build parsing: the
            // channel/namespace colon split, comment stripping, the semicolon
            // check, and bracket detection.
            build: self.version.is_some()
                && self
                    .build
                    .is_some_and(|build| !build.to_string().contains([':', '#', ';', '[', ']'])),
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
                Some(version) => write!(f, "{version}")?,
                // Without a version the spec is only identified by its other
                // fields; emit the historic `*` placeholder only when nothing
                // else renders (note: it reparses as `version: Any`, which
                // matches identically to `version: None`).
                None if !renders_brackets => f.write_char('*')?,
                None => {}
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

        if let Some(version) = self.version {
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
                write_list(f, ctx, "extras", extras.iter(), |extra| {
                    is_valid_extra_group_name(extra)
                })
            }
            Field::Flags => {
                let flags = self.flags.expect("presence checked by caller");
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
        if self.build.is_some()
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
            // (renders `1.2.*`) and the wildcard `Any` (`*`).
            Some(version) => version
                .to_string()
                .chars()
                .next()
                .is_some_and(|c| matches!(c, '>' | '<' | '=' | '!' | '~')),
        }
    }
}

/// Writes a `key=[..]` list field. Elements whose text `is_valid_bare`
/// accepts render unquoted; anything else renders quoted, so an element that
/// would re-tokenize under bare rendering (a comma, whitespace, an empty or
/// invalid name) reaches the parser as a single quoted element that fails
/// validation loudly, instead of silently splitting into different elements.
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

/// Writes one scalar `key=value` field with the quoting rules of the context.
///
/// Both dialects quote every scalar with a delimiter that keeps the value
/// intact, because the parser stores most quoted bracket values verbatim
/// (only `when=` and `flags=` are unescaped on parse) — so escaping here
/// would silently mutate the value on round-trip. The canonical dialect
/// refuses values no delimiter can hold; the infallible legacy dialect falls
/// back to the historic raw double-quoted form for them, which fails loudly
/// at parse time instead of round-tripping to a different value.
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
                None => write!(f, "{key}=\"{value}\"")?,
            }
        }
        DisplayStyle::Canonical => write!(f, "{key}={}", canonical_bracket_value(value)?)?,
    }
    Ok(())
}

impl MatchSpecCondition {
    /// Renders this condition in `style`, emitting only the parentheses
    /// required to preserve precedence and the exact shape of this
    /// left-associative AST. Both dialects share this rule: extra parentheses
    /// carry no information and the output reparses to the identical tree.
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

    /// Iterates over the match-spec leaves of this condition, left to right.
    fn leaves(&self) -> impl Iterator<Item = &MatchSpec> + '_ {
        let mut stack = vec![self];
        std::iter::from_fn(move || {
            while let Some(node) = stack.pop() {
                match node {
                    Self::MatchSpec(spec) => return Some(&**spec),
                    Self::And(lhs, rhs) | Self::Or(lhs, rhs) => {
                        stack.push(rhs);
                        stack.push(lhs);
                    }
                }
            }
            None
        })
    }
}

/// Picks a quote delimiter that lets `value` be emitted verbatim.
///
/// `MatchSpec` parsing preserves ordinary scalar escapes verbatim, so the only
/// lossless quoting is a delimiter that does not occur unescaped in the value.
/// Returns `None` when neither quote character can hold the value: both quotes
/// occur unescaped, or a trailing backslash would swallow the closing quote.
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

/// The parse options a canonical string is expected to round-trip through.
fn canonical_parse_options() -> ParseMatchSpecOptions {
    ParseMatchSpecOptions::strict()
        .with_repodata_revision(RepodataRevision::V3)
        .with_exact_names_only(false)
}

/// Renders `spec` canonically and proves the result faithful by round-tripping
/// it through the parser exactly once. See
/// [`MatchSpec::to_canonical_string`] for the public contract.
pub(crate) fn to_canonical_string(spec: &MatchSpec) -> Result<String, CanonicalMatchSpecError> {
    let mut rendered = String::new();
    SpecView::from(spec)
        .fmt(&mut rendered, DisplayContext::CANONICAL)
        .map_err(|error| match error {
            FormatError::Canonical(error) => error,
            FormatError::Fmt(_) => unreachable!("writing to a String cannot fail"),
        })?;

    match MatchSpec::from_str(&rendered, canonical_parse_options()) {
        Ok(reparsed) => match canonical_divergence(spec, &reparsed) {
            None => Ok(rendered),
            Some(error) => Err(error),
        },
        Err(_) => Err(diagnose_parse_failure(spec, rendered)),
    }
}

/// Compares a spec against its reparsed canonical form and attributes the
/// first divergence to a field. Returns `None` when the round-trip is
/// faithful. Only runs when the canonical text parsed but something was lost.
fn canonical_divergence(spec: &MatchSpec, reparsed: &MatchSpec) -> Option<CanonicalMatchSpecError> {
    use CanonicalMatchSpecError as Error;

    if reparsed.name != spec.name {
        return Some(Error::UnrepresentableName(spec.name.to_string()));
    }
    if reparsed.version != spec.version {
        return Some(Error::UnrepresentableVersion(display_or_default(
            spec.version.as_ref(),
        )));
    }
    if reparsed.build != spec.build {
        return Some(Error::UnrepresentableBuild(display_or_default(
            spec.build.as_ref(),
        )));
    }
    if reparsed.build_number != spec.build_number {
        return Some(Error::UnrepresentableScalar(display_or_default(
            spec.build_number.as_ref(),
        )));
    }
    if reparsed.file_name != spec.file_name {
        return Some(Error::UnrepresentableScalar(display_or_default(
            spec.file_name.as_ref(),
        )));
    }
    if reparsed.extras != spec.extras {
        return Some(Error::UnrepresentableExtra(first_divergent_element(
            spec.extras.as_deref(),
            reparsed.extras.as_deref(),
        )));
    }
    if reparsed.flags != spec.flags {
        return Some(Error::UnrepresentableFlag(first_divergent_element(
            spec.flags.as_deref(),
            reparsed.flags.as_deref(),
        )));
    }
    match (spec.channel.as_deref(), reparsed.channel.as_deref()) {
        (None, None) => {}
        (Some(original), Some(parsed)) if channel_roundtrips(original, parsed) => {}
        (original, _) => {
            return Some(Error::UnrepresentableChannel(original.map_or_else(
                String::new,
                |channel| {
                    canonical_channel_value(channel).unwrap_or_else(|_| channel.name().to_string())
                },
            )));
        }
    }
    if reparsed.subdir != spec.subdir {
        return Some(Error::UnrepresentableScalar(display_or_default(
            spec.subdir.as_ref(),
        )));
    }
    if reparsed.namespace != spec.namespace {
        return Some(Error::UnrepresentableScalar(display_or_default(
            spec.namespace.as_ref(),
        )));
    }
    if reparsed.md5 != spec.md5 {
        return Some(Error::UnrepresentableScalar(
            spec.md5.map(hex::encode).unwrap_or_default(),
        ));
    }
    if reparsed.sha256 != spec.sha256 {
        return Some(Error::UnrepresentableScalar(
            spec.sha256.map(hex::encode).unwrap_or_default(),
        ));
    }
    if reparsed.url != spec.url.as_ref().map(redact_credentials_from_url) {
        return Some(Error::UnrepresentableScalar(display_or_default(
            spec.url.as_ref(),
        )));
    }
    if reparsed.license != spec.license {
        return Some(Error::UnrepresentableScalar(display_or_default(
            spec.license.as_ref(),
        )));
    }
    if reparsed.license_family != spec.license_family {
        return Some(Error::UnrepresentableScalar(display_or_default(
            spec.license_family.as_ref(),
        )));
    }
    if reparsed.track_features != spec.track_features {
        return Some(Error::UnrepresentableTrackFeature(first_divergent_element(
            spec.track_features.as_deref(),
            reparsed.track_features.as_deref(),
        )));
    }
    if reparsed.condition != spec.condition {
        return spec
            .condition
            .as_ref()
            .and_then(diagnose_condition)
            .or_else(|| {
                Some(Error::UnrepresentableConditionLeaf(display_or_default(
                    spec.condition.as_ref().map(RenderedCondition),
                )))
            });
    }

    None
}

/// Attributes a whole-string parse failure to a field. Only runs on the error
/// path, so the targeted per-field reparses here do not burden canonical
/// rendering.
fn diagnose_parse_failure(spec: &MatchSpec, rendered: String) -> CanonicalMatchSpecError {
    use CanonicalMatchSpecError as Error;

    // Names are the only positional token; a name whose text reads as bracket
    // or positional syntax corrupts everything after it.
    let name = spec.name.to_string();
    let expected = MatchSpec {
        name: spec.name.clone(),
        ..MatchSpec::default()
    };
    if !matches!(
        MatchSpec::from_str(&name, canonical_parse_options()),
        Ok(parsed) if parsed == expected
    ) {
        return Error::UnrepresentableName(name);
    }

    if let Some(extra) = spec
        .extras
        .iter()
        .flatten()
        .find(|extra| !is_valid_extra_group_name(extra))
    {
        return Error::UnrepresentableExtra(extra.clone());
    }
    if let Some(flag) = spec
        .flags
        .iter()
        .flatten()
        .find(|flag| !is_valid_matchspec_flag(&flag.to_string()))
    {
        return Error::UnrepresentableFlag(flag.to_string());
    }
    if let Some(feature) = spec
        .track_features
        .iter()
        .flatten()
        .find(|feature| feature.is_empty() || feature.contains([',', ' ']))
    {
        return Error::UnrepresentableTrackFeature(feature.clone());
    }

    if let Some(version) = &spec.version {
        let value = version.to_string();
        if !matches!(
            VersionSpec::from_str(&value, ParseStrictness::Strict),
            Ok(parsed) if parsed == *version
        ) {
            return Error::UnrepresentableVersion(value);
        }
    }
    if let Some(build) = &spec.build {
        let value = build.to_string();
        if !matches!(value.parse::<StringMatcher>(), Ok(parsed) if parsed == *build) {
            return Error::UnrepresentableBuild(value);
        }
    }

    if let Some(channel) = spec.channel.as_deref() {
        match canonical_channel_value(channel) {
            Err(error) => return error,
            Ok(value) => {
                // The root is irrelevant for this absolute URL, but channel
                // parsing still requires one. `temp_dir` is deterministic
                // enough here and cannot fail due to a deleted or inaccessible
                // current working directory.
                let config = ChannelConfig::default_with_root_dir(std::env::temp_dir());
                if !matches!(
                    Channel::from_str(&value, &config),
                    Ok(parsed) if channel_roundtrips(channel, &parsed)
                ) {
                    return Error::UnrepresentableChannel(value);
                }
            }
        }
    }

    if let Some(error) = spec.condition.as_ref().and_then(diagnose_condition) {
        return error;
    }

    Error::NotRoundTrippable(rendered)
}

/// Finds the first condition leaf whose canonical text does not parse back as
/// a single match-spec leaf.
fn diagnose_condition(condition: &MatchSpecCondition) -> Option<CanonicalMatchSpecError> {
    for leaf in condition.leaves() {
        let mut rendered = String::new();
        match SpecView::from(leaf).fmt(
            &mut rendered,
            DisplayContext::condition_leaf(DisplayStyle::Canonical),
        ) {
            Err(FormatError::Canonical(error)) => return Some(error),
            Err(FormatError::Fmt(_)) => unreachable!("writing to a String cannot fail"),
            Ok(()) => {}
        }

        if !matches!(
            parse_condition_with_options(&rendered, canonical_parse_options()),
            Ok((rest, MatchSpecCondition::MatchSpec(_))) if rest.trim().is_empty()
        ) {
            return Some(CanonicalMatchSpecError::UnrepresentableConditionLeaf(
                rendered,
            ));
        }
    }
    None
}

/// The parser deliberately ignores channel names when checking whether a
/// canonical channel is faithful: the canonical text carries the base URL, and
/// a reparsed channel may derive a different display name from it. File URLs
/// must round-trip exactly because their identity depends on more than the
/// URL text.
fn channel_roundtrips(original: &Channel, parsed: &Channel) -> bool {
    let canonical_base_url = redact_credentials_from_url(original.base_url.url());
    **parsed.base_url.url() == canonical_base_url
        && parsed.platforms == original.platforms
        && (original.base_url.url().scheme() != "file" || parsed == original)
}

/// Renders an optional displayable value, defaulting to the empty string.
fn display_or_default<T: Display>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

/// Renders the first element of `original` that its reparsed counterpart does
/// not reproduce, for use in error messages about list-valued fields.
fn first_divergent_element<T: Display + PartialEq>(
    original: Option<&[T]>,
    parsed: Option<&[T]>,
) -> String {
    let original = original.unwrap_or_default();
    let parsed = parsed.unwrap_or_default();
    original
        .iter()
        .enumerate()
        .find(|(index, element)| parsed.get(*index) != Some(*element))
        .map_or_else(
            || original.iter().format(",").to_string(),
            |(_, element)| element.to_string(),
        )
}

/// Adapter rendering a condition in its canonical form for error messages.
struct RenderedCondition<'a>(&'a MatchSpecCondition);

impl Display for RenderedCondition<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.fmt_with(f, DisplayStyle::Canonical) {
            Ok(()) => Ok(()),
            Err(FormatError::Fmt(error)) => Err(error),
            // This adapter is only used for conditions that already rendered
            // canonically as part of the whole spec, so a canonical failure
            // cannot occur here — but if it ever does, emit a placeholder
            // instead of a `fmt::Error`, which `to_string` turns into a panic.
            Err(FormatError::Canonical(_)) => f.write_str("<unrepresentable condition>"),
        }
    }
}
