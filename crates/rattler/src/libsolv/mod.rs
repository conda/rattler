use std::convert::TryInto;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::marker::PhantomData;
use std::ptr::NonNull;

mod ffi;

/// Wrapper for libsolv Pool which is an interning datastructure used by libsolv
pub struct Pool(NonNull<ffi::Pool>);

/// Representation of a repo containing package data in libsolv
/// This corresponds to a repo_data json
/// Lifetime of this object is coupled to the Pool on creation
struct Repo<'pool>(NonNull<ffi::Repo>, PhantomData<&'pool ffi::Pool>);

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct StringId(ffi::Id);

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

/// Destroy c side of things when repo is dropped
impl Drop for Repo<'_> {
    /// Safe because we have coupled Repo lifetime to Pool lifetime
    fn drop(&mut self) {
        unsafe { ffi::repo_free(self.0.as_mut(), 1) }
    }
}

impl Pool {
    /// Create repo from a pool
    fn create_repo<S: AsRef<str>>(&mut self, url: S) -> Repo {
        unsafe {
            let c_url =
                CString::new(url.as_ref()).expect("could never be null because of trait-bound");
            Repo(
                NonNull::new(ffi::repo_create(self.0.as_mut(), c_url.as_ptr()))
                    .expect("could not create repo object"),
                PhantomData,
            )
        }
    }
}

/// Interns from Target tyoe to Id
trait Intern {
    type Id;

    /// Intern the type in the [`Pool`]
    fn intern(&self, pool: &mut Pool) -> Self::Id;
}

impl StringId {
    /// Resolve to the interned type
    fn resolve<'a>(&self, pool: &'a Pool) -> &'a str {
        // Safe because the new-type wraps the ffi::id and cant be created otherwise
        unsafe {
            let c_str = ffi::pool_id2str(pool.0.as_ptr(), self.0);
            CStr::from_ptr(c_str).to_str().expect("utf-8 parse error")
        }
    }
}

/// Blanket implementation for string types
impl<T: AsRef<str>> Intern for T {
    type Id = StringId;

    fn intern(&self, pool: &mut Pool) -> Self::Id {
        // Safe because conversion is valid
        let c_str =
            CString::new(self.as_ref()).expect("could never be null because of trait-bound");
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
}

#[cfg(test)]
mod test {
    use super::Intern;
    use crate::libsolv::Pool;

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
}
