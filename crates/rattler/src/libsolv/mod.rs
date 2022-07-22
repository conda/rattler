use serde_with::As;
use std::convert::TryInto;
use std::ffi::{CStr, CString};
use std::ptr::NonNull;

mod ffi;

/// Wrapper for libsolv Pool
pub struct Pool(NonNull<ffi::Pool>);

/// Wrapper for the StringId of libsolv
#[derive(Copy, Clone)]
pub struct StringId(ffi::Id);

impl Default for Pool {
    fn default() -> Self {
        // Safe because the pool create failure is handled with expect
        Self(NonNull::new(unsafe { ffi::pool_create() }).expect("could not create libsolv pool"))
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        // Safe because we know that the pool exists at this point
        unsafe { ffi::pool_free(self.0.as_mut()) }
    }
}

/// Interns from Target to Id
trait Intern {
    type Id;

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
    use crate::libsolv::{ffi, Pool, StringId};

    #[test]
    fn test_pool_creation() {
        let pool = Pool::default();
        drop(pool);
    }

    #[test]
    fn test_pool_string_interning() {
        let mut pool = Pool::default();
        let to_intern = "foobar";
        let id = to_intern.intern(&mut pool);
        let outcome = id.resolve(&pool);
        assert_eq!(to_intern, outcome);
    }

    #[test]
    fn test_pool_string_interning_utf8() {
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
