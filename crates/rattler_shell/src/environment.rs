//! Explicit, native-string environment snapshots for child processes.

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
};
#[cfg(windows)]
use windows_sys::Win32::Globalization::{CSTR_EQUAL, CompareStringOrdinal};

/// A complete environment, preserving unset, empty and non-Unicode values.
/// On Windows, inserting or removing a variable reconciles all case aliases.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EnvironmentSnapshot {
    variables: HashMap<OsString, OsString>,
}

impl EnvironmentSnapshot {
    /// Captures the process environment explicitly.
    pub fn from_system() -> Self {
        std::env::vars_os().collect()
    }

    /// Looks up a variable using the host's environment-name semantics.
    pub fn get(&self, name: impl AsRef<OsStr>) -> Option<&OsStr> {
        let name = name.as_ref();
        self.variables
            .get(name)
            .or_else(|| {
                if cfg!(windows) {
                    self.variables
                        .iter()
                        .find(|(key, _)| same_name(key, name))
                        .map(|(_, value)| value)
                } else {
                    None
                }
            })
            .map(OsString::as_os_str)
    }

    /// Sets a variable, replacing every existing alias on Windows.
    pub fn insert(&mut self, name: impl Into<OsString>, value: impl Into<OsString>) {
        let name = name.into();
        #[cfg(windows)]
        self.remove(&name);
        self.variables.insert(name, value.into());
    }

    /// Unsets a variable, including every existing alias on Windows.
    pub fn remove(&mut self, name: impl AsRef<OsStr>) {
        let name = name.as_ref();
        if cfg!(windows) {
            self.variables.retain(|key, _| !same_name(key, name));
        } else {
            self.variables.remove(name);
        }
    }

    /// Iterates over native-string names and values for `Command::envs`.
    pub fn iter(&self) -> impl Iterator<Item = (&OsStr, &OsStr)> {
        self.variables
            .iter()
            .map(|(key, value)| (key.as_os_str(), value.as_os_str()))
    }

    /// The Unicode subset used to generate shell activation statements.
    /// The complete native-string snapshot is still inherited by the shell.
    pub fn unicode_variables(&self) -> HashMap<String, String> {
        self.iter()
            .filter_map(|(key, value)| Some((key.to_str()?.to_owned(), value.to_str()?.to_owned())))
            .collect()
    }
}

impl<K: Into<OsString>, V: Into<OsString>> FromIterator<(K, V)> for EnvironmentSnapshot {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(values: T) -> Self {
        let mut environment = Self::default();
        for (key, value) in values {
            environment.insert(key, value);
        }
        environment
    }
}

fn same_name(left: &OsStr, right: &OsStr) -> bool {
    #[cfg(unix)]
    {
        left == right
    }
    #[cfg(windows)]
    {
        if left == right {
            return true;
        }
        if let (Some(left), Some(right)) = (left.to_str(), right.to_str())
            && left.is_ascii()
            && right.is_ascii()
        {
            return left.eq_ignore_ascii_case(right);
        }
        let left: Vec<u16> = left.encode_wide().collect();
        let right: Vec<u16> = right.encode_wide().collect();
        let (Ok(left_len), Ok(right_len)) = (i32::try_from(left.len()), i32::try_from(right.len()))
        else {
            return false;
        };
        // SAFETY: both pointers reference initialized UTF-16 slices of the
        // specified lengths, which stay alive for the comparison.
        unsafe {
            CompareStringOrdinal(left.as_ptr(), left_len, right.as_ptr(), right_len, 1)
                == CSTR_EQUAL
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn aliases_replace_and_unset_the_same_variable() {
        let mut environment: EnvironmentSnapshot =
            [("Path", "one"), ("PATH", "two")].into_iter().collect();
        assert_eq!(environment.get("pAtH"), Some(OsStr::new("two")));
        assert_eq!(environment.iter().count(), 1);
        environment.remove("path");
        assert!(environment.get("PATH").is_none());

        environment.insert("Älias", "present");
        assert_eq!(environment.get("äLIAS"), Some(OsStr::new("present")));
        environment.remove("älias");
        assert!(environment.get("Älias").is_none());
    }
}
