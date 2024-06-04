use super::{
    super::{c_string, wrapper::ffi::Id},
    ffi,
    repo::Repo,
    solvable::SolvableId,
    solver::Solver,
};
use rattler_conda_types::MatchSpec;
use std::ffi::c_char;
use std::{
    convert::TryInto,
    ffi::{CStr, CString},
    os::raw::c_void,
    ptr::NonNull,
};

/// The type of distribution that the pool is being used for
/// Note: rattler only supports conda
#[repr(u32)]
pub enum DistType {
    Rpm = ffi::DISTTYPE_RPM,
    Debian = ffi::DISTTYPE_DEB,
    Arch = ffi::DISTTYPE_ARCH,
    Haiku = ffi::DISTTYPE_HAIKU,
    Conda = ffi::DISTTYPE_CONDA,
}

/// Wrapper for libsolv Pool, the interning datastructure used by libsolv
///
/// The wrapper functions as an owned pointer, guaranteed to be non-null and freed
/// when the Pool is dropped
///
/// Additional note: interning reduces memory usage by only storing unique instances of the provided
/// data, which then share the same `Id`. This `Pool` releases memory when explicitly asked and upon
/// destruction
#[repr(transparent)]
pub struct Pool(NonNull<ffi::Pool>);

impl Default for Pool {
    fn default() -> Self {
        let pool_ptr = unsafe { ffi::pool_create() };
        let self_obj = Self(NonNull::new(pool_ptr).expect("pool_create returned null"));
        self_obj.set_disttype(DistType::Conda);
        self_obj
    }
}

/// Destroy c side of things when pool is dropped
impl Drop for Pool {
    fn drop(&mut self) {
        // Safe because we know that the pool is never freed manually
        unsafe {
            // Free the registered Rust callback, if present
            let ptr = (*self.raw_ptr()).debugcallbackdata;
            if !ptr.is_null() {
                let _: Box<BoxedLogCallback> = Box::from_raw(ptr.cast());
            }

            // Free the pool itself
            ffi::pool_free(self.0.as_mut());
        }
    }
}

/// A boxed closure used for log callbacks
type BoxedLogCallback = Box<dyn FnMut(&str, i32) + 'static>;

/// The callback that is actually registered on the pool (it must be a function pointer)
#[no_mangle]
extern "C" fn log_callback(
    _pool: *mut ffi::Pool,
    user_data: *mut c_void,
    flags: i32,
    str: *const c_char,
) {
    unsafe {
        // We have previously stored a `BoxedLogCallback` in `user_data`, so now we can retrieve it
        // and run it
        let closure: &mut BoxedLogCallback = &mut *(user_data.cast::<BoxedLogCallback>());
        let str = CStr::from_ptr(str);
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

        // Safe because `pool.solvables` is the start of an array of length `pool.nsolvables`
        let solvables =
            unsafe { std::slice::from_raw_parts_mut(pool.solvables, pool.nsolvables as _) };

        solvables.swap(s1.0 as _, s2.0 as _);
    }

    /// Interns a `REL_EQ` relation between `id1` and `id2`
    pub fn rel_eq(&self, id1: Id, id2: Id) -> Id {
        unsafe { ffi::pool_rel2id(self.raw_ptr(), id1, id2, ffi::REL_EQ as i32, 1) }
    }

    /// Interns the provided matchspec
    pub fn conda_matchspec(&self, matchspec: &CStr) -> Id {
        unsafe { ffi::pool_conda_matchspec(self.raw_ptr(), matchspec.as_ptr()) }
    }

    /// Add debug callback to the pool
    pub fn set_debug_callback<F: FnMut(&str, i32) + 'static>(&self, callback: F) {
        let box_callback: Box<BoxedLogCallback> = Box::new(Box::new(callback));
        unsafe {
            // Sets the debug callback into the pool
            // Double box because file because the Box<Fn> is a fat pointer and have a different
            // size compared to c_void
            ffi::pool_setdebugcallback(
                self.raw_ptr(),
                Some(log_callback),
                Box::into_raw(box_callback).cast(),
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
            ffi::pool_setdebuglevel(self.raw_ptr(), verbosity);
        }
    }

    /// Set the provided repo to be considered as a source of installed packages
    ///
    /// Panics if the repo does not belong to this pool
    pub fn set_installed(&self, repo: &Repo<'_>) {
        repo.ensure_belongs_to_pool(self);
        unsafe { ffi::pool_set_installed(self.raw_ptr(), repo.raw_ptr()) }
    }

    /// Set the disttype for this pool. This is used to determine how to interpret the
    /// fields of a solvable. Note that rattler currently only supports the conda disttype.
    pub fn set_disttype(&self, disttype: DistType) {
        unsafe { ffi::pool_setdisttype(self.raw_ptr(), disttype as i32) };
    }

    /// Create the solver
    pub fn create_solver(&self) -> Solver<'_> {
        let solver = NonNull::new(unsafe { ffi::solver_create(self.raw_ptr()) })
            .expect("solver_create returned a nullptr");

        // Safe because we know the solver ptr is valid
        unsafe { Solver::new(self, solver) }
    }

    /// Create the whatprovides on the pool which is needed for solving
    pub fn create_whatprovides(&self) {
        unsafe {
            ffi::pool_createwhatprovides(self.raw_ptr());
        }
    }

    pub fn intern_matchspec(&self, match_spec: &MatchSpec) -> MatchSpecId {
        let c_str = c_string(match_spec.to_string());
        unsafe { MatchSpecId(ffi::pool_conda_matchspec(self.raw_ptr(), c_str.as_ptr())) }
    }

    /// Interns string like types into a `Pool` returning a `StringId`
    pub fn intern_str<T: Into<Vec<u8>>>(&self, str: T) -> StringId {
        let c_str = CString::new(str).expect("the provided string contained a NUL byte");
        let length = c_str.as_bytes().len();
        let c_str = c_str.as_c_str();

        // Safe because the function accepts any string
        unsafe {
            StringId(ffi::pool_strn2id(
                self.raw_ptr(),
                c_str.as_ptr(),
                length.try_into().expect("string too large"),
                1,
            ))
        }
    }

    /// Finds a previously interned string or returns `None` if it wasn't found
    pub fn find_interned_str<T: AsRef<str>>(&self, str: T) -> Option<StringId> {
        let c_str = CString::new(str.as_ref()).expect("the provided string contained a NUL byte");
        let length = c_str.as_bytes().len();
        let c_str = c_str.as_c_str();

        // Safe because the function accepts any string
        unsafe {
            let id = ffi::pool_strn2id(
                self.raw_ptr(),
                c_str.as_ptr(),
                length.try_into().expect("string too large"),
                0,
            );
            (id != 0).then_some(StringId(id))
        }
    }

    /// Returns a string describing the last error associated to this pool, or "no error" if there
    /// were no errors
    pub fn last_error(&self) -> String {
        // Safe, because `pool_errstr` is guaranteed to return a valid string even in the absence
        // of errors
        let err = unsafe { CStr::from_ptr(ffi::pool_errstr(self.raw_ptr())) };
        err.to_string_lossy().into_owned()
    }
}

/// Wrapper for the [`StringId`] of libsolv
#[derive(Copy, Clone)]
pub struct StringId(pub(super) Id);

impl StringId {
    /// Resolves the id to the interned string, if present in the pool
    ///
    /// Note: string ids are basically indexes in an array, so using a [`StringId`] from one pool in
    /// a different one will either return `None` (if the id can't be found) or it will return
    /// whatever string is found at the index
    pub fn resolve<'a>(&self, pool: &'a Pool) -> Option<&'a str> {
        if self.0 < pool.as_ref().ss.nstrings {
            // Safe because we know the string is in the pool
            let c_str = unsafe { ffi::pool_id2str(pool.0.as_ptr(), self.0) };
            let c_str = unsafe { CStr::from_ptr(c_str) }
                .to_str()
                .expect("utf-8 parse error");
            Some(c_str)
        } else {
            None
        }
    }
}

impl From<StringId> for Id {
    fn from(id: StringId) -> Self {
        id.0
    }
}

/// Wrapper for the match spec type of libsolv
#[derive(Copy, Clone)]
pub struct MatchSpecId(Id);

/// Conversion to [`Id`]
impl From<MatchSpecId> for Id {
    fn from(id: MatchSpecId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod test {
    use std::ffi::CString;

    use super::super::pool::Pool;
    use rattler_conda_types::{MatchSpec, ParseStrictness};

    #[test]
    fn test_pool_string_interning() {
        let pool = Pool::default();
        let pool2 = Pool::default();
        let to_intern = "foobar";
        // Intern the string
        let id = pool.intern_str(to_intern);
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

        let pool = Pool::default();
        for in_s in strings {
            let id = pool.intern_str(in_s);
            let outcome = id.resolve(&pool);
            assert_eq!(in_s, outcome.unwrap());
        }
    }

    #[test]
    fn test_matchspec_interning() {
        // Create a matchspec
        let spec = MatchSpec::from_str("foo=1.0=py27_0", ParseStrictness::Lenient).unwrap();
        // Intern it into the pool
        let pool = Pool::default();
        pool.intern_matchspec(&spec);
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
