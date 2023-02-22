use super::{c_string, ffi, repo::Repo, solvable::SolvableId, solver::Solver};
use crate::libsolv::wrapper::ffi::Id;
use rattler_conda_types::MatchSpec;
use std::{
    convert::TryInto,
    ffi::{CStr, CString},
    os::raw::c_void,
    ptr::NonNull,
};

/// Wrapper for libsolv Pool, the interning datastructure used by libsolv
///
/// The wrapper functions as an owned pointer, guaranteed to be non-null and freed
/// when the Pool is dropped
#[repr(transparent)]
pub struct Pool(NonNull<ffi::Pool>);

impl Default for Pool {
    fn default() -> Self {
        let pool_ptr = unsafe { ffi::pool_create() };
        Self(NonNull::new(pool_ptr).expect("pool_create returned a null pointer"))
    }
}

/// Destroy c side of things when pool is dropped
impl Drop for Pool {
    fn drop(&mut self) {
        // Safe because we know that the pool is never freed manually
        unsafe {
            // Free the registered Rust callback
            let ptr = (*self.0.as_ptr()).debugcallbackdata;
            if !ptr.is_null() {
                let _: Box<BoxedLogCallback> = Box::from_raw(ptr as *mut _);
            }

            // Free the pool itself
            ffi::pool_free(self.0.as_mut());
        }
    }
}

/// A boxed closure used for log callbacks
type BoxedLogCallback = Box<dyn FnMut(&str, i32) + 'static>;

#[no_mangle]
extern "C" fn log_callback(
    _pool: *mut ffi::Pool,
    user_data: *mut c_void,
    flags: i32,
    str: *const i8,
) {
    unsafe {
        // Get the box back
        let closure: &mut BoxedLogCallback = &mut *(user_data as *mut BoxedLogCallback);
        // Convert the string
        let str = CStr::from_ptr(str);
        // Call the callback
        closure(str.to_str().expect("utf-8 error"), flags);
    }
}

/// Logging verbosity for libsolv
pub enum Verbosity {
    None,
    Low,
    Medium,
    Extreme,
}

impl Pool {
    /// Returns a raw pointer to the wrapped `ffi::Pool`, to be used for calling ffi functions
    /// that require access to the pool (and for nothing else)
    pub(super) fn raw_ptr(&self) -> *mut ffi::Pool {
        self.0.as_ptr()
    }

    /// Returns a reference to the wrapped `ffi::Pool`.
    pub fn as_ref(&self) -> &ffi::Pool {
        // Safe because the pool is guaranteed to exist until it is dropped
        unsafe { self.0.as_ref() }
    }

    /// Swaps two solvables inside the libsolv pool
    pub fn swap_solvables(&self, s1: SolvableId, s2: SolvableId) {
        let pool = self.as_ref();
        let solvables =
            unsafe { std::slice::from_raw_parts_mut(pool.solvables, pool.nsolvables as _) };

        solvables.swap(s1.0 as _, s2.0 as _);
    }

    /// Interns a REL_EQ relation between `id1` and `id2`
    pub fn rel_eq(&self, id1: Id, id2: Id) -> Id {
        unsafe { ffi::pool_rel2id(self.0.as_ptr(), id1, id2, ffi::REL_EQ as i32, 1) }
    }

    /// Interns the provided matchspec
    pub fn conda_matchspec(&self, matchspec: &CStr) -> Id {
        unsafe { ffi::pool_conda_matchspec(self.0.as_ptr(), matchspec.as_ptr()) }
    }

    /// Add debug callback to the pool
    pub fn set_debug_callback<F: FnMut(&str, i32) + 'static>(&self, callback: F) {
        let box_callback: Box<BoxedLogCallback> = Box::new(Box::new(callback));
        unsafe {
            // Sets the debug callback into the pool
            // Double box because file because the Box<Fn> is a fat pointer and have a different
            // size compared to c_void
            ffi::pool_setdebugcallback(
                self.0.as_ptr(),
                Some(log_callback),
                Box::into_raw(box_callback) as *mut _,
            );
        }
    }

    /// Set debug level for libsolv
    pub fn set_debug_level(&self, verbosity: Verbosity) {
        let verbosity: libc::c_int = match verbosity {
            Verbosity::None => 0,
            Verbosity::Low => 1,
            Verbosity::Medium => 2,
            Verbosity::Extreme => 3,
        };
        unsafe {
            ffi::pool_setdebuglevel(self.0.as_ptr(), verbosity);
        }
    }

    /// Set the provided repo to be considered as a source of installed packages
    pub fn set_installed(&self, repo: &Repo) {
        unsafe { ffi::pool_set_installed(self.0.as_ptr(), repo.raw_ptr()) }
    }

    /// Create the solver
    pub fn create_solver(&self) -> Solver {
        let solver = NonNull::new(unsafe { ffi::solver_create(self.0.as_ptr()) })
            .expect("solver_create returned a nullptr");
        Solver::new(solver)
    }

    /// Create the whatprovides on the pool which is needed for solving
    pub fn create_whatprovides(&self) {
        unsafe {
            ffi::pool_createwhatprovides(self.0.as_ptr());
        }
    }
}

/// Interns string like types into a `Pool` returning a `StringId`
fn intern_str<T: AsRef<str>>(pool: &Pool, str: T) -> StringId {
    let c_str = CString::new(str.as_ref()).expect("the provided string contained a NUL byte");
    let length = c_str.as_bytes().len();
    let c_str = c_str.as_c_str();

    // Safe because the function accepts any string
    unsafe {
        StringId(ffi::pool_strn2id(
            pool.0.as_ptr(),
            c_str.as_ptr(),
            length.try_into().expect("string too large"),
            1,
        ))
    }
}

/// Finds a previously interned string or returns `None` if it wasn't found.
fn find_intern_str<T: AsRef<str>>(pool: &Pool, str: T) -> Option<StringId> {
    let c_str = CString::new(str.as_ref()).expect("the provided string contained a NUL byte");
    let length = c_str.as_bytes().len();
    let c_str = c_str.as_c_str();

    // Safe because the function accepts any string
    unsafe {
        let id = ffi::pool_strn2id(
            pool.0.as_ptr(),
            c_str.as_ptr(),
            length.try_into().expect("string too large"),
            0,
        );
        (id != 0).then_some(StringId(id))
    }
}

/// Interns an instance of `Self` into a [`Pool`] returning a handle (or `Id`) to the actual data.
/// Interning reduces memory usage by pooling data together which is considered to be equal, sharing
/// the same `Id`. However, a `Pool` also only releases memory when explicitly asked to do so or on
/// destruction.
pub trait Intern {
    type Id;

    /// Interns the type in the [`Pool`]
    fn intern(&self, pool: &Pool) -> Self::Id;
}

/// Enables retrieving the `Id` of previously interned instances of `Self` through the `Intern`
/// trait.
pub trait FindInterned: Intern {
    /// Finds a previously interned instance in the specified [`Pool`]
    fn find_interned_id(&self, pool: &Pool) -> Option<Self::Id>;
}

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct StringId(pub(super) Id);

impl StringId {
    /// Resolves to the interned type returns a string reference.
    ///
    /// # Safety
    ///
    /// This function does not result in undefined behavior if an Id is passsed that was not
    /// interned by the passed `pool`. However, if the `pool` is different from the one that
    /// returned the `StringId` while interning the result might be unexpected.
    pub fn resolve<'a>(&self, pool: &'a Pool) -> Option<&'a str> {
        if self.0 >= pool.as_ref().ss.nstrings {
            None
        } else {
            // Safe because we check if the stringpool can actually contain the given id.
            let c_str = unsafe { ffi::pool_id2str(pool.0.as_ptr(), self.0) };
            let c_str = unsafe { CStr::from_ptr(c_str) }
                .to_str()
                .expect("utf-8 parse error");
            Some(c_str)
        }
    }
}

impl<'s> Intern for &'s str {
    type Id = StringId;

    fn intern(&self, pool: &Pool) -> Self::Id {
        intern_str(pool, self)
    }
}

impl<'s> FindInterned for &'s str {
    fn find_interned_id(&self, pool: &Pool) -> Option<Self::Id> {
        find_intern_str(pool, self)
    }
}

impl Intern for String {
    type Id = StringId;

    fn intern(&self, pool: &Pool) -> Self::Id {
        intern_str(pool, self)
    }
}

impl Intern for StringId {
    type Id = Self;

    fn intern(&self, _: &Pool) -> Self::Id {
        *self
    }
}

impl<T: Intern> Intern for &T {
    type Id = T::Id;

    fn intern(&self, pool: &Pool) -> Self::Id {
        (*self).intern(pool)
    }
}

impl FindInterned for String {
    fn find_interned_id(&self, pool: &Pool) -> Option<Self::Id> {
        find_intern_str(pool, self)
    }
}

impl From<StringId> for Id {
    fn from(id: StringId) -> Self {
        id.0
    }
}

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct MatchSpecId(Id);
impl Intern for MatchSpec {
    type Id = MatchSpecId;

    fn intern(&self, pool: &Pool) -> Self::Id {
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
        unsafe { MatchSpecId(ffi::pool_conda_matchspec(pool.0.as_ptr(), c_str.as_ptr())) }
    }
}

/// Conversion to [`Id`]
impl From<MatchSpecId> for Id {
    fn from(id: MatchSpecId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod test {
    use std::ffi::CString;

    use super::super::pool::{Intern, Pool};
    use rattler_conda_types::{ChannelConfig, MatchSpec};

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
        let pool = Pool::default();
        spec.intern(&pool);
        // Don't think libsolv has an API to get it back
    }

    #[test]
    fn test_pool_callback() {
        let pool = Pool::default();
        let (tx, rx) = std::sync::mpsc::sync_channel(10);
        // Set the debug level
        pool.set_debug_level(super::Verbosity::Extreme);
        pool.set_debug_callback(move |msg, _level| {
            tx.send(msg.to_owned()).unwrap();
        });

        // Log something in the pool
        let msg = CString::new("foo").unwrap();
        unsafe { super::ffi::pool_debug(pool.0.as_ptr(), 1 << 5, msg.as_ptr()) };

        assert_eq!(rx.recv().unwrap(), "foo");
    }
}
