use crate::libsolv::repo::Repo;
use crate::libsolv::solver::Solver;
use crate::libsolv::{c_string, ffi};
use rattler::MatchSpec;
use std::convert::TryInto;
use std::ffi::{CStr, CString};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;

/// Wrapper for libsolv Pool which is an interning datastructure used by libsolv.
#[repr(transparent)]
pub struct Pool(pub(super) NonNull<ffi::Pool>);

/// A `PoolRef` is a wrapper around an `ffi::Pool` that provides a safe abstraction over its
/// functionality.
///
/// A `PoolRef` can not be constructed by itself but is instead returned by dereferencing a
/// [`Pool`].
#[repr(transparent)]
pub struct PoolRef(ffi::Pool);

impl Deref for Pool {
    type Target = PoolRef;

    fn deref(&self) -> &Self::Target {
        unsafe { self.0.cast().as_ref() }
    }
}

impl DerefMut for Pool {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { self.0.cast().as_mut() }
    }
}

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

impl PoolRef {
    /// Returns a pointer to the wrapped `ffi::Pool`
    pub(super) fn as_ptr(&self) -> NonNull<ffi::Pool> {
        unsafe { NonNull::new_unchecked(self as *const Self as *mut Self).cast() }
    }

    /// Returns a reference to the wrapped `ffi::Pool`.
    pub(super) fn as_ref(&self) -> &ffi::Pool {
        // Safe because RepoRef is a transparent wrapper around ffi::Repo
        unsafe { std::mem::transmute(self) }
    }

    /// Create repo from a pool
    pub fn create_repo<S: AsRef<str>>(&mut self, url: S) -> Repo {
        unsafe {
            let c_url = c_string(url);
            Repo::new(
                NonNull::new(ffi::repo_create(self.as_ptr().as_mut(), c_url.as_ptr()))
                    .expect("libsolv repo_create returned nullptr"),
            )
        }
    }

    /// Create the solver
    pub fn create_solver(&mut self) -> Solver {
        let solver = NonNull::new(unsafe { ffi::solver_create(self.as_ptr().as_mut()) })
            .expect("solver_create returned a nullptr");
        Solver::new(solver)
    }

    /// Create the whatprovides on the pool which is needed for solving
    pub fn create_whatprovides(&mut self) {
        // Safe because pointer must exist
        unsafe {
            ffi::pool_createwhatprovides(self.as_ptr().as_mut());
        }
    }
}

/// Intern string like types
fn intern_str<T: AsRef<str>>(pool: &mut PoolRef, str: T) -> StringId {
    // Safe because conversion is valid
    let c_str = CString::new(str.as_ref()).expect("could never be null because of trait-bound");
    let length = c_str.as_bytes().len();
    let c_str = c_str.as_c_str();

    // Safe because pool exists and function accepts any string
    unsafe {
        StringId(ffi::pool_strn2id(
            pool.as_ptr().as_mut(),
            c_str.as_ptr(),
            length.try_into().expect("string too large"),
            1,
        ))
    }
}

/// Finds a previously interned string or returns `None` if it wasn't found.
fn find_intern_str<T: AsRef<str>>(pool: &PoolRef, str: T) -> Option<StringId> {
    // Safe because conversion is valid
    let c_str = CString::new(str.as_ref()).expect("could never be null because of trait-bound");
    let length = c_str.as_bytes().len();
    let c_str = c_str.as_c_str();

    // Safe because pool exists and function accepts any string
    unsafe {
        let id = ffi::pool_strn2id(
            pool.as_ptr().as_ptr(),
            c_str.as_ptr(),
            length.try_into().expect("string too large"),
            0,
        );
        if id == 0 {
            None
        } else {
            Some(StringId(id))
        }
    }
}

/// Interns from Target type to Id
pub trait Intern {
    type Id;

    /// Interns the type in the [`Pool`]
    fn intern(&self, pool: &mut PoolRef) -> Self::Id;
}

pub trait FindInterned: Intern {
    /// Finds a previously interned instance in the specified [`Pool`]
    fn find_interned_id(&self, pool: &PoolRef) -> Option<Self::Id>;
}

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct StringId(pub(super) ffi::Id);

impl StringId {
    /// Resolve to the interned type returns a string reference
    pub fn resolve<'a>(&self, pool: &'a PoolRef) -> &'a str {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            let c_str = ffi::pool_id2str(pool.as_ptr().as_ptr(), self.0);
            CStr::from_ptr(c_str).to_str().expect("utf-8 parse error")
        }
    }
}

/// Intern implementation for string reference
impl<'s> Intern for &'s str {
    type Id = StringId;

    fn intern(&self, pool: &mut PoolRef) -> Self::Id {
        intern_str(pool, self)
    }
}

impl<'s> FindInterned for &'s str {
    fn find_interned_id(&self, pool: &PoolRef) -> Option<Self::Id> {
        find_intern_str(pool, self)
    }
}

/// Intern implementation for owned Strings
impl<'s> Intern for &'s String {
    type Id = StringId;

    fn intern(&self, pool: &mut PoolRef) -> Self::Id {
        intern_str(pool, self)
    }
}

impl<'s> FindInterned for &'s String {
    fn find_interned_id(&self, pool: &PoolRef) -> Option<Self::Id> {
        find_intern_str(pool, self)
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

    fn intern(&self, pool: &mut PoolRef) -> Self::Id {
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
        unsafe {
            MatchSpecId(ffi::pool_conda_matchspec(
                pool.as_ptr().as_mut(),
                c_str.as_ptr(),
            ))
        }
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
