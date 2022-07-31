use crate::libsolv::repo::{Repo, RepoOwnedPtr};
use crate::libsolv::solver::{Solver, SolverOwnedPtr};
use crate::libsolv::{c_string, ffi};
use rattler::MatchSpec;
use std::convert::TryInto;
use std::ffi::{CStr, CString};
use std::marker::PhantomData;
use std::ptr::NonNull;

/// Wrapper for libsolv Pool which is an interning datastructure used by libsolv
pub struct Pool(NonNull<ffi::Pool>);

impl Default for Pool {
    fn default() -> Self {
        // Safe because the pool create failure is handled with expect
        Self(NonNull::new(unsafe { ffi::pool_create() }).expect("could not create libsolv pool"))
    }
}

/// Destroy c side of things when pool is dropped
impl Drop for Pool {
    fn drop(&mut self) {
        // Safe because we know that the pool exists at this point
        unsafe { ffi::pool_free(self.0.as_mut()) }
    }
}

impl Pool {
    /// Create repo from a pool
    pub fn create_repo<S: AsRef<str>>(&mut self, url: S) -> Repo {
        unsafe {
            let c_url = c_string(url);
            Repo(
                RepoOwnedPtr::new(ffi::repo_create(self.0.as_mut(), c_url.as_ptr())),
                PhantomData,
            )
        }
    }

    /// Create the solver
    pub fn create_solver(&mut self) -> Solver {
        unsafe {
            Solver(
                SolverOwnedPtr::new(ffi::solver_create(self.0.as_mut())),
                PhantomData,
            )
        }
    }

    /// Create the whatprovides on the pool which is needed for solving
    pub fn create_whatprovides(&mut self) {
        // Safe because pointer must exist
        unsafe {
            ffi::pool_createwhatprovides(self.0.as_mut());
        }
    }
}

/// Intern string like types
fn intern_str<T: AsRef<str>>(pool: &mut Pool, str: T) -> StringId {
    // Safe because conversion is valid
    let c_str = CString::new(str.as_ref()).expect("could never be null because of trait-bound");
    let length = c_str.as_bytes().len();
    let c_str = c_str.as_c_str();

    // Safe because pool exists and function accepts any string
    unsafe {
        StringId(ffi::pool_strn2id(
            pool.0.as_mut(),
            c_str.as_ptr(),
            length.try_into().expect("string too large"),
            1,
        ))
    }
}

/// Interns from Target tyoe to Id
pub trait Intern {
    type Id;

    /// Intern the type in the [`Pool`]
    fn intern(&self, pool: &mut Pool) -> Self::Id;
}

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct StringId(ffi::Id);

impl StringId {
    /// Resolve to the interned type returns a string reference
    fn resolve<'a>(&self, pool: &'a Pool) -> &'a str {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            let c_str = ffi::pool_id2str(pool.0.as_ptr(), self.0);
            CStr::from_ptr(c_str).to_str().expect("utf-8 parse error")
        }
    }
}

/// Intern implementation for string reference
impl<'s> Intern for &'s str {
    type Id = StringId;

    fn intern(&self, pool: &mut Pool) -> Self::Id {
        intern_str(pool, self)
    }
}

/// Intern implementation for owned Strings
impl<'s> Intern for &'s String {
    type Id = StringId;

    fn intern(&self, pool: &mut Pool) -> Self::Id {
        intern_str(pool, self)
    }
}

/// Conversion to [`ffi::Id`]
impl From<StringId> for ffi::Id {
    fn from(id: StringId) -> Self {
        id.0
    }
}

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct MatchSpecId(ffi::Id);
impl Intern for MatchSpec {
    type Id = MatchSpecId;

    fn intern(&self, pool: &mut Pool) -> Self::Id {
        let name = self
            .name
            .as_ref()
            .expect("matchspec should have a name")
            .clone();

        // Put the matchspec in conda build form
        // This is also used by mamba to add matchspecs to libsolv
        // See: https://github.dev/mamba-org/mamba/blob/master/libmamba/src/core/match_spec.cpp
        let conda_build_form = if self.version.is_some() {
            let version = self.version.as_ref().unwrap().clone();
            if self.build.is_some() {
                format!(
                    "{} {} {}",
                    name,
                    version,
                    self.build.as_ref().unwrap().clone()
                )
            } else {
                format!("{} {}", name, version)
            }
        } else {
            name
        };

        let c_str = c_string(conda_build_form);
        unsafe { MatchSpecId(ffi::pool_conda_matchspec(pool.0.as_mut(), c_str.as_ptr())) }
    }
}

/// Conversion to [`ffi::Id`]
impl From<MatchSpecId> for ffi::Id {
    fn from(id: MatchSpecId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod test {
    use crate::libsolv::pool::{Intern, Pool};
    use rattler::{ChannelConfig, MatchSpec};

    #[test]
    fn test_pool_creation() {
        let mut pool = Pool::default();
        let repo = pool.create_repo("conda-forge");
        drop(repo);
        drop(pool);
    }

    #[test]
    fn test_pool_string_interning() {
        let mut pool = Pool::default();
        let to_intern = "foobar";
        // Intern the string
        let id = to_intern.intern(&mut pool);
        // Get it back
        let outcome = id.resolve(&pool);
        assert_eq!(to_intern, outcome);
    }

    #[test]
    fn test_pool_string_interning_utf8() {
        // Some interesting utf-8 strings to test
        let strings = [
            "いろはにほへとちりぬるを
            わかよたれそつねならむ
            うゐのおくやまけふこえて
            あさきゆめみしゑひもせす",
            "イロハニホヘト チリヌルヲ ワカヨタレソ ツネナラム ウヰノオクヤマ ケフコエテ アサキユメミシ ヱヒモセスン",
            "Pchnąć w tę łódź jeża lub ośm skrzyń fig",
            "В чащах юга жил бы цитрус? Да, но фальшивый экземпляр!",
            "Съешь же ещё этих мягких французских булок да выпей чаю"];

        let mut pool = Pool::default();
        for in_s in strings {
            let id = in_s.intern(&mut pool);
            let outcome = id.resolve(&pool);
            assert_eq!(in_s, outcome);
        }
    }

    #[test]
    fn test_matchspec_interning() {
        // Create a matchspec
        let channel_config = ChannelConfig::default();
        let spec = MatchSpec::from_str("foo=1.0=py27_0", &channel_config).unwrap();
        // Intern it into the pool
        let mut pool = Pool::default();
        spec.intern(&mut pool);
        // Don't think libsolv has an API to get it back
    }
}
