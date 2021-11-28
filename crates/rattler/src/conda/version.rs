use std::num::ParseIntError;
use std::str::FromStr;

macro_rules! regex {
    ($re:literal $(,)?) => {{
        static RE: once_cell::sync::OnceCell<regex::Regex> = once_cell::sync::OnceCell::new();
        RE.get_or_init(|| regex::Regex::new($re).unwrap())
    }};
}

pub struct Version {}

pub struct ParseVersionError {
    version: String,
    kind: ParseVersionKind
}

impl ParseVersionError {
    pub fn new(text: impl Into<String>, kind: ParseVersionKind) -> Self {
        Self {
            version: text.into(),
            kind
        }
    }
}

pub enum ParseVersionKind {
    Empty,
    InvalidCharacters,
    EpochMustBeInteger(ParseIntError),
    DuplicateEpochSeparator,
    DuplicateLocalVersionSeparator,
}

impl FromStr for Version {
    type Err = ParseVersionError;

    // Implementation taken from https://github.com/ilastik/conda/blob/master/conda/resolve.py

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Trim the version
        let lowered = s.trim().to_lowercase();
        if lowered.is_empty() {
            return Err(ParseVersionError::new(s, ParseVersionKind::Empty));
        }

        // Ensure the string only contains valid characters
        let version_check_re = regex!(r#"^[\*\.\+!_0-9a-z]+$'"#);
        if !version_check_re.is_match(&lowered) {
            return Err(ParseVersionError::new(s, ParseVersionKind::InvalidCharacters));
        }

        // Find epoch
        let (epoch, rest) = if let Some((epoch, rest)) = lowered.split_once('!') {
            let epoch = epoch.parse().map_err(|e| ParseVersionError::new(s, ParseVersionKind::EpochMustBeInteger(e)))?;
            (epoch, rest)
        } else {
            (0, s)
        };

        // Ensure the rest of the string no longer contains an epoch
        if rest.find('!').is_some() {
            return Err(ParseVersionError::new(s, ParseVersionKind::DuplicateEpochSeparator));
        }

        // Find local version string
        let (local, rest) = if let Some((rest, local)) = rest.rsplit_once('!') {
            (local, rest)
        } else {
            ("", s)
        };

        // Ensure the rest of the string no longer contains a local version separator
        if rest.find('+').is_some() {
            return Err(ParseVersionError::new(s, ParseVersionKind::DuplicateLocalVersionSeparator));
        }

        let local_split = local.split(&['.', '_']);
        let version_split = rest.split(&['.', '_']);

        let version_split_re = regex!(r#"^[\*\.\+!_0-9a-z]+$'"#);
        fn split<'a>(split_iter: impl Iterator<Item=&'a str>) {

        }

        // HIER https://github.com/ilastik/conda/blob/b08d6e7166908922dd99c297d3f4fc751ab27a2a/conda/resolve.py#L183

        Ok(())
    }
}
