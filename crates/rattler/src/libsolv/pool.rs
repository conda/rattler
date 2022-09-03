use crate::libsolv::repo::Repo;
use crate::libsolv::solver::Solver;
use crate::libsolv::{c_string, ffi};
use rattler::MatchSpec;
use std::convert::TryInto;
use std::ffi::CString;
use std::ops::{Deref, DerefMut};
use std::os::raw::c_void;
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
        unsafe {
            ffi::pool_free(self.0.as_mut());
            let ptr = (*self.0.as_ptr()).debugcallbackdata;
            if !ptr.is_null() {
                // Free the callbackdata by reconstructing it
                let _: Box<Box<dyn Fn(&str)>> = Box::from_raw(ptr as *mut _);
            }
        }
    }
}

#[no_mangle]
extern "C" fn log_callback(
    _pool: *mut ffi::Pool,
    user_data: *mut c_void,
    _level: i32,
    str: *const i8,
) {
    unsafe {
        // Get the box back
        let closure: &mut Box<dyn FnMut(&str) -> bool> =
            &mut *(user_data as *mut std::boxed::Box<dyn for<'r> std::ops::FnMut(&'r str) -> bool>);
        // Convert the string
        let str = CStr::from_ptr(str);
        // Call the callback
        closure(str.to_str().expect("utf-8 error"));
    }
}

/// Logging verbosity for libsolv
pub enum Verbosity {
    None,
    Low,
    Medium,
    Extreme,
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

    /// Add debug callback to the pool
    pub fn set_debug_callback<F: FnMut(&str) + 'static>(&mut self, callback: F) {
        let box_callback: Box<Box<dyn FnMut(&str) + 'static>> = Box::new(Box::new(callback));
        unsafe {
            // Sets the debug callback into the pool
            // Double box because file because the Box<Fn> is a fat pointer and have a different
            // size compared to c_void
            ffi::pool_setdebugcallback(
                self.as_ptr().as_ptr(),
                Some(log_callback),
                Box::into_raw(box_callback) as *mut _,
            );
        }
    }

    /// Set debug level for libsolv
    pub fn set_debug_level(&mut self, verbosity: Verbosity) {
        let verbosity: libc::c_int = match verbosity {
            Verbosity::None => 0,
            Verbosity::Low => 1,
            Verbosity::Medium => 2,
            Verbosity::Extreme => 3,
        };
        unsafe {
            ffi::pool_setdebuglevel(self.as_ptr().as_ptr(), verbosity);
        }
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

/// Interns string like types into a `Pool` returning a `StringId`
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
        (id != 0).then(|| StringId(id))
    }
}

/// Interns an instance of `Self` into a [`Pool`] returning a handle (or `Id`) to the actual data.
/// Interning reduces memory usage by pooling data together which is considered to be equal, sharing
/// the same `Id`. However, a `Pool` also only releases memory when explicitly asked to do so or on
/// destruction.
pub trait Intern {
    type Id;

    /// Interns the type in the [`Pool`]
    fn intern(&self, pool: &mut PoolRef) -> Self::Id;
}

/// Enables retrieving the `Id` of previously interned instances of `Self` through the `Intern`
/// trait.
pub trait FindInterned: Intern {
    /// Finds a previously interned instance in the specified [`Pool`]
    fn find_interned_id(&self, pool: &PoolRef) -> Option<Self::Id>;
}

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct StringId(pub(super) ffi::Id);

impl StringId {
    /// Resolves to the interned type returns a string reference.
    ///
    /// ```rust
    /// let pool = Pool::default();
    /// let string = "Hello, world!";
    /// let id = string.intern(pool);
    /// assert_eq!(id.resolve(pool), Some(string));
    /// ```
    ///
    /// # Safety
    ///
    /// This function does not result in undefined behavior if an Id is passsed that was not
    /// interned by the passed `pool`. However, if the `pool` is different from the one that
    /// returned the `StringId` while interning the result might be unexpected.
    pub fn resolve<'a>(&self, pool: &'a PoolRef) -> Option<&'a str> {
        if self.0 >= pool.as_ref().ss.nstrings {
            None
        } else {
            // Safe because we check if the stringpool can actually contain the given id.
            let c_str = unsafe { ffi::pool_id2str(pool.as_ptr().as_ptr(), self.0) };
            let c_str = unsafe { CStr::from_ptr(c_str) }
                .to_str()
                .expect("utf-8 parse error");
            Some(c_str)
        }
    }
}

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

impl Intern for String {
    type Id = StringId;

    fn intern(&self, pool: &mut PoolRef) -> Self::Id {
        intern_str(pool, self)
    }
}

impl Intern for StringId {
    type Id = Self;

    fn intern(&self, _: &mut PoolRef) -> Self::Id {
        *self
    }
}

impl<T: Intern> Intern for &T {
    type Id = T::Id;

    fn intern(&self, pool: &mut PoolRef) -> Self::Id {
        (*self).intern(pool)
    }
}

impl FindInterned for String {
    fn find_interned_id(&self, pool: &PoolRef) -> Option<Self::Id> {
        find_intern_str(pool, self)
    }
}

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
    use std::ffi::{CStr, CString};

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
        let pool2 = Pool::default();
        let to_intern = "foobar";
        // Intern the string
        let id = to_intern.intern(&mut pool);
        // Get it back
        let outcome = id.resolve(&pool);
        assert_eq!(to_intern, outcome.unwrap());

        let outcome2 = id.resolve(&pool2);
        assert!(outcome2.is_none());
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
            assert_eq!(in_s, outcome.unwrap());
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

    #[test]
    fn test_pool_callback() {
        let mut pool = Pool::default();
        let (tx, rx) = std::sync::mpsc::sync_channel(10);
        // Set the debug level
        pool.set_debug_level(super::Verbosity::Extreme);
        pool.set_debug_callback(move |msg| {
            tx.send(msg.to_owned()).unwrap();
        });

        // Log something in the pool
        let msg = CString::new("foo").unwrap();
        unsafe { super::ffi::pool_debug(pool.as_ptr().as_ptr(), 1 << 5, msg.as_ptr()) };

        assert_eq!(rx.recv().unwrap(), "foo");
    }
}
