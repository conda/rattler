#![cfg_attr(not(feature = "dangerous-shutdown-guard"), forbid(unsafe_code))]
#![cfg_attr(feature = "dangerous-shutdown-guard", deny(unsafe_code))]
#![doc(
    html_root_url = "https://docs.rs/pyo3-log/0.2.1/pyo3-log/",
    test(attr(deny(warnings))),
    test(attr(allow(unknown_lints, non_local_definitions)))
)]
#![warn(missing_docs)]

//! A bridge from Rust to Python logging
//!
//! The library can be used to install a [logger][log::Log] into Rust that will send the messages
//! over to the Python [logging](https://docs.python.org/3/library/logging.html). This can be
//! useful when writing a native Python extension module in Rust and it is desirable to log from
//! the Rust side too.
//!
//! The library internally depends on the [`pyo3`] crate. This is not exposed through the public
//! API and it should work from extension modules not using [`pyo3`] directly. It'll nevertheless
//! still bring the dependency in, so this might be considered if the module doesn't want to use
//! it.
//!
//! # Simple usage
//!
//! Each extension module has its own global variables, therefore the used logger is also
//! independent of other Rust native extensions. Therefore, it is up to each one to set a logger
//! for itself if it wants one.
//!
//! By using [`init`] function from a place that's run only once (maybe from the top-level module
//! of the extension), the logger is registered and the log messages (eg. [`info`][log::info]) send
//! their messages over to the Python side.
//!
//! ```rust
//! use log::info;
//! use pyo3::prelude::*;
//!
//! #[pyfunction]
//! fn log_something() {
//!     info!("Something!");
//! }
//!
//! #[pymodule]
//! fn my_module(m: Bound<'_, PyModule>) -> PyResult<()> {
//!     pyo3_log::init();
//!
//!     m.add_wrapped(wrap_pyfunction!(log_something))?;
//!     Ok(())
//! }
//! ```
//!
//! The following example is how this would be performed with the new declarative inline module syntax
//! introduced in PyO3 0.23 and above.
//!
//! ```rust
//! # mod test_declarative_example {
//! use pyo3::prelude::*;
//! use log::info;
//!
//! #[pymodule]
//! mod my_module{
//!     use super::*;
//!
//!    #[pymodule_init]
//!     fn init(_m: &Bound<'_, PyModule>) -> PyResult<()> {
//!         pyo3_log::init();
//!         Ok(())
//!     }
//!
//!     #[pyfunction]
//!     fn log_something() {
//!         info!("Something!");
//!     }
//! }
//! # }
//! ```
//!
//! # Performance, Filtering and Caching
//!
//! Ideally, the logging system would always consult the Python loggers to know which messages
//! should or should not be logged. However, one of the reasons of using Rust instead of Python is
//! performance. Part of that is giving up the GIL in long-running computations to let other
//! threads run at the same time.
//!
//! Therefore, acquiring the GIL and calling into the Python interpreter on each
//! [`trace`][log::trace] message only to figure out it is not to be logged would be prohibitively
//! slow. There are two techniques employed here.
//!
//! First, level filters are applied before consulting the Python side. By default, only the
//! [`Debug`][Level::Debug] level and more severe is considered to be sent over to Python. This can
//! be overridden using the [`filter`][Logger::filter] and [`filter_target`][Logger::filter_target]
//! methods.
//!
//! Second, the Python loggers and their effective log levels are cached on the Rust side on the
//! first use of the given module. This means that on a disabled level, only the first logging
//! attempt in the given module will acquire GIL while the future ones will short-circuit before
//! ever reaching Python.
//!
//! This is good for performance, but could lead to the incorrect messages to be logged or not
//! logged in certain situations ‒ if Rust logs before the Python logging system is set up properly
//! or when it is reconfigured at runtime.
//!
//! For these reasons it is possible to turn caching off on construction of the logger (at the cost
//! of performance) and to clear the cache manually through the [`ResetHandle`].
//!
//! To tune the caching and filtering, the logger needs to be created manually:
//!
//! ```rust
//! # use log::LevelFilter;
//! # use pyo3::prelude::*;
//! # use pyo3_log::{Caching, Logger};
//! #
//! # fn main() -> PyResult<()> {
//! # Python::attach(|py| {
//! let handle = Logger::new(py, Caching::LoggersAndLevels)?
//!     .filter(LevelFilter::Trace)
//!     .filter_target("my_module::verbose_submodule".to_owned(), LevelFilter::Warn)
//!     .install()
//!     .expect("Someone installed a logger before us :-(");
//!
//! // Some time in the future when logging changes, reset the caches:
//! handle.reset();
//! # Ok(())
//! # })
//! # }
//! ```
//!
//! # Mapping
//!
//! The logging `target` is mapped into the name of the logger on the Python side, replacing all
//! `::` occurrences with `.` (both form hierarchy in their respective language).
//!
//! Log levels are mapped to the same-named ones. The [`Trace`][Level::Trace] doesn't exist on the
//! Python side, but is mapped to a level with value 5.
//!
//! # Interaction with Python GIL
//!
//! Under the hook, the logging routines call into Python. That means they need to acquire the
//! Global Interpreter Lock of Python.
//!
//! This has several consequences. One of them is the above mentioned performance considerations.
//!
//! The other is a risk of deadlocks if threads are used from within the extension code without
//! releasing the GIL.
//!
//! ```rust
//! use std::thread;
//! use log::info;
//! use pyo3::prelude::*;
//!
//! #[pyfunction]
//! fn deadlock() {
//!     info!("This logs fine");
//!
//!     let background_thread = thread::spawn(|| {
//!         info!("This'll deadlock");
//!     });
//!
//!     background_thread.join().unwrap();
//! }
//! # let _ = deadlock;
//! ```
//!
//! The above code will deadlock, because the `info` call in the background thread needs the GIL
//! that's held by the deadlock function. One needs to give up the GIL to let the other threads
//! run, something like this:
//!
//! ```rust
//! use std::thread;
//! use log::info;
//! use pyo3::prelude::*;
//!
//! #[pyfunction]
//! fn dont_deadlock(py: Python<'_>) {
//!     info!("This logs fine");
//!
//!     py.detach(|| {
//!         let background_thread = thread::spawn(|| {
//!             info!("This'll not deadlock");
//!         });
//!
//!         background_thread.join().unwrap();
//!     });
//! }
//! # let _ = dont_deadlock;
//! ```

use std::cmp;
use std::collections::HashMap;
#[cfg(feature = "dangerous-shutdown-guard")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use arc_swap::ArcSwap;
use log::{Level, LevelFilter, Log, Metadata, Record, SetLoggerError};
use pyo3::prelude::*;
use pyo3::types::PyTuple;

/// Set once the Python interpreter starts shutting down.
///
/// Once the interpreter begins finalization, calling into it is no longer safe ‒ doing so can lead
/// to crashes or hangs. We register an [`atexit`](https://docs.python.org/3/library/atexit.html)
/// hook that flips this flag so we can stop forwarding log messages in time.
#[cfg(feature = "dangerous-shutdown-guard")]
static PYTHON_FINALIZING: AtomicBool = AtomicBool::new(false);

/// The atexit hook. Marks the interpreter as on its way out.
#[cfg(feature = "dangerous-shutdown-guard")]
#[pyfunction]
fn pyo3_log_atexit() {
    PYTHON_FINALIZING.store(true, Ordering::SeqCst);
}

/// Registers the [`pyo3_log_atexit`] hook, idempotently.
///
/// Multiple loggers (or repeated construction) would otherwise register the hook more than once.
/// The flag here keeps it to a single registration per process.
#[cfg(feature = "dangerous-shutdown-guard")]
fn register_atexit(py: Python<'_>) -> PyResult<()> {
    static REGISTERED: AtomicBool = AtomicBool::new(false);
    if REGISTERED.swap(true, Ordering::SeqCst) {
        return Ok(());
    }

    let atexit = py.import("atexit")?;
    let hook = wrap_pyfunction!(pyo3_log_atexit, py)?;
    atexit.call_method1("register", (hook,))?;
    Ok(())
}

/// Checks whether it is safe to call into the Python interpreter.
///
/// Returns `false` if either the interpreter is not initialized (according to the
/// [`Py_IsInitialized`][pyo3::ffi::Py_IsInitialized] FFI call) or our atexit hook has fired,
/// signalling that finalization has begun. In either case, calling into Python is unsafe and the
/// log message must be dropped.
#[cfg(feature = "dangerous-shutdown-guard")]
fn interpreter_usable() -> bool {
    if PYTHON_FINALIZING.load(Ordering::SeqCst) {
        return false;
    }
    #[allow(unsafe_code)]
    // SAFETY: Py_IsInitialized is safe to call at any time, including before initialization and
    // after finalization. It only reads an interpreter status flag and takes no arguments.
    let initialized = unsafe { pyo3::ffi::Py_IsInitialized() } != 0;
    initialized
}

/// A handle into a [`Logger`], able to reset its caches.
///
/// This handle can be used to manipulate a [`Logger`] even after it has been installed. It's main
/// purpose is to reset the internal caches, for example if the logging settings on the Python side
/// changed.
#[derive(Clone, Debug)]
pub struct ResetHandle(Arc<ArcSwap<CacheNode>>);

impl ResetHandle {
    /// Reset the internal logger caches.
    ///
    /// This removes all the cached loggers and levels (if there were any). Future logging calls
    /// may cache them again, using the current Python logging settings.
    pub fn reset(&self) {
        // Overwrite whatever is in the cache directly. This must win in case of any collisions
        // (the caching uses compare_and_swap to let the reset win).
        self.0.store(Default::default());
    }
}

/// What the [`Logger`] can cache.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
#[derive(Default)]
pub enum Caching {
    /// Disables caching.
    ///
    /// Every time a log message passes the filters, the code goes to the Python side to check if
    /// the message shall be logged.
    Nothing,

    /// Caches the Python `Logger` objects.
    ///
    /// The logger objects (which should stay the same during the lifetime of a Python application)
    /// are cached. However, the log levels are not. This means there's some amount of calling of
    /// Python code saved during a logging call, but the GIL still needs to be acquired even if the
    /// message doesn't eventually get output anywhere.
    Loggers,

    /// Caches both the Python `Logger` and their respective effective log levels.
    ///
    /// Therefore, once a `Logger` has been cached, it is possible to decide on the Rust side if a
    /// message would get logged or not. If the message is not to be logged, no Python code is
    /// called and the GIL doesn't have to be acquired.
    #[default]
    LoggersAndLevels,
}

#[derive(Debug)]
struct CacheEntry {
    filter: LevelFilter,
    logger: Py<PyAny>,
}

impl CacheEntry {
    fn clone_ref(&self, py: Python<'_>) -> Self {
        CacheEntry {
            filter: self.filter,
            logger: self.logger.clone_ref(py),
        }
    }
}

#[derive(Debug, Default)]
struct CacheNode {
    local: Option<CacheEntry>,
    children: HashMap<String, Arc<CacheNode>>,
}

impl CacheNode {
    fn store_to_cache_recursive<'a, P>(
        &self,
        py: Python<'_>,
        mut path: P,
        entry: CacheEntry,
    ) -> Arc<Self>
    where
        P: Iterator<Item = &'a str>,
    {
        let mut me = CacheNode {
            children: self.children.clone(),
            local: self.local.as_ref().map(|e| e.clone_ref(py)),
        };
        match path.next() {
            Some(segment) => {
                let child = me.children.entry(segment.to_owned()).or_default();
                *child = child.store_to_cache_recursive(py, path, entry);
            }
            None => me.local = Some(entry),
        }
        Arc::new(me)
    }
}

/// The `Logger`
///
/// The actual `Logger` that can be installed into the Rust side and will send messages over to
/// Python.
///
/// It can be either created directly and then installed, passed to other aggregating log systems,
/// or the [`init`] or [`try_init`] functions may be used if defaults are good enough.
#[derive(Debug)]
pub struct Logger {
    /// Filter used as a fallback if none of the `filters` match.
    top_filter: LevelFilter,

    /// Mapping of filters to modules.
    ///
    /// The most specific one will be used, falling back to `top_filter` if none matches. Stored as
    /// full paths, with `::` separaters (eg. before converting them from Rust to Python).
    filters: HashMap<String, LevelFilter>,

    /// The prefix to prepend to all log targets
    prefix: Option<String>,

    /// The imported Python `logging` module.
    logging: Py<PyModule>,

    /// Caching configuration.
    caching: Caching,

    /// The cache with loggers and level filters.
    ///
    /// The nodes form a tree ‒ each one potentially holding a cache entry (or not) and might have
    /// some children.
    ///
    /// When updating, the whole path from the root is cloned in a copy-on-write manner and the Arc
    /// here is switched. In case of collisions (eg. someone already replaced the root since
    /// starting the update), the update is just thrown away.
    cache: Arc<ArcSwap<CacheNode>>,
}

impl Logger {
    /// Creates a new logger.
    ///
    /// It defaults to having a filter for [`Debug`][LevelFilter::Debug].
    pub fn new(py: Python<'_>, caching: Caching) -> PyResult<Self> {
        let logging = py.import("logging")?;
        #[cfg(feature = "dangerous-shutdown-guard")]
        register_atexit(py)?;
        Ok(Self {
            top_filter: LevelFilter::Debug,
            filters: HashMap::new(),
            prefix: None,
            logging: logging.into(),
            caching,
            cache: Default::default(),
        })
    }

    /// Installs this logger as the global one.
    ///
    /// When installing, it also sets the corresponding [maximum level][log::set_max_level],
    /// constructed using the filters in this logger.
    pub fn install(self) -> Result<ResetHandle, SetLoggerError> {
        let handle = self.reset_handle();
        let level = cmp::max(
            self.top_filter,
            self.filters
                .values()
                .copied()
                .max()
                .unwrap_or(LevelFilter::Off),
        );
        log::set_boxed_logger(Box::new(self))?;
        log::set_max_level(level);
        Ok(handle)
    }

    /// Provides the reset handle of this logger.
    ///
    /// Note that installing the logger also returns a reset handle. This function is available if,
    /// for example, the logger will be passed to some other logging system that connects multiple
    /// loggers together.
    pub fn reset_handle(&self) -> ResetHandle {
        ResetHandle(Arc::clone(&self.cache))
    }

    /// Configures the default logging filter.
    ///
    /// Log messages will be filtered according a filter. If one provided by a
    /// [`filter_target`][Logger::filter_target] matches, it takes preference. If none matches,
    /// this one is used.
    ///
    /// The default filter if none set is [`Debug`][LevelFilter::Debug].
    pub fn filter(mut self, filter: LevelFilter) -> Self {
        self.top_filter = filter;
        self
    }

    /// Sets a filter for a specific target, overriding the default.
    ///
    /// This'll match targets with the same name and all the children in the module hierarchy. In
    /// case multiple match, the most specific one wins.
    ///
    /// With this configuration, modules will log in the following levels:
    ///
    /// ```rust
    /// # use log::LevelFilter;
    /// # use pyo3_log::Logger;
    ///
    /// Logger::default()
    ///     .filter(LevelFilter::Warn)
    ///     .filter_target("xy".to_owned(), LevelFilter::Debug)
    ///     .filter_target("xy::aa".to_owned(), LevelFilter::Trace);
    /// ```
    ///
    /// * `whatever` => `Warn`
    /// * `xy` => `Debug`
    /// * `xy::aa` => `Trace`
    /// * `xy::aabb` => `Debug`
    pub fn filter_target(mut self, target: String, filter: LevelFilter) -> Self {
        self.filters.insert(target, filter);
        self
    }

    /// Sets a prefix to prepend to log targets before sending log messages to Python.
    ///
    /// This allows for Python-side arrangements where logging configurations are only
    /// attached to logging names other than the root.
    pub fn set_prefix(mut self, prefix: &str) -> Self {
        self.prefix = Some(prefix.replace("::", "."));
        self
    }

    /// Finds a node in the cache.
    ///
    /// The hierarchy separator is `::`.
    fn lookup(&self, target: &str) -> Option<Arc<CacheNode>> {
        if self.caching == Caching::Nothing {
            return None;
        }

        let root = self.cache.load();
        let mut node: &Arc<CacheNode> = &root;
        for segment in target.split("::") {
            match node.children.get(segment) {
                Some(sub) => node = sub,
                None => return None,
            }
        }

        Some(Arc::clone(node))
    }

    /// Logs stuff
    ///
    /// Returns a logger to be cached, if any. If it already found a cached logger or if caching is
    /// turned off, returns None.
    fn log_inner(
        &self,
        py: Python<'_>,
        record: &Record,
        cache: &Option<Arc<CacheNode>>,
    ) -> PyResult<Option<Py<PyAny>>> {
        let msg = format!("{}", record.args());
        let log_level = map_level(record.level());
        let mut target = record.target().replace("::", ".");
        target = match &self.prefix {
            Some(prefix) => format!("{}.{}", prefix, target),
            None => target,
        };
        let cached_logger = cache
            .as_ref()
            .and_then(|node| node.local.as_ref())
            .map(|local| &local.logger);
        let (logger, cached) = match cached_logger {
            Some(cached) => (cached.bind(py).clone(), true),
            None => (
                self.logging
                    .bind(py)
                    .getattr("getLogger")?
                    .call1((&target,))?,
                false,
            ),
        };
        // We need to check for this ourselves. For some reason, the logger.handle does not check
        // it. And besides, we can save ourselves few python calls if it's turned off.
        if is_enabled_for(&logger, record.level())? {
            let none = py.None();

            #[allow(unused_mut)]
            let mut extra = py.None().into_bound(py);

            #[cfg(feature = "kv")]
            if record.key_values().count() > 0 {
                // write structured data to 'extra', serializing the values
                use log::kv::{Key, Value, VisitSource};
                use pyo3::types::{PyDict, PyString};

                struct PyDictVisitor<'p> {
                    dict: Bound<'p, PyDict>,
                }

                impl<'kvs, 'p> VisitSource<'kvs> for PyDictVisitor<'p> {
                    fn visit_pair(
                        &mut self,
                        key: Key<'kvs>,
                        value: Value<'kvs>,
                    ) -> Result<(), log::kv::Error> {
                        let py_key = PyString::new(self.dict.py(), key.as_str());
                        let py_value = PyString::new(self.dict.py(), &value.to_string());

                        let _ = self.dict.set_item(py_key, py_value);
                        Ok(())
                    }
                }

                let mut visitor = PyDictVisitor {
                    dict: PyDict::new(py),
                };
                let _ = record.key_values().visit(&mut visitor);

                extra = visitor.dict.into_any();
            }

            let record = logger.call_method1(
                "makeRecord",
                (
                    target,
                    log_level,
                    record.file(),
                    record.line().unwrap_or_default(),
                    msg,
                    PyTuple::empty(py), // args
                    &none,              // exc_info
                    &none,              // func
                    extra,              // extra
                ),
            )?;

            logger.call_method1("handle", (record,))?;
        }

        let cache_logger = if !cached && self.caching != Caching::Nothing {
            Some(logger.into())
        } else {
            None
        };

        Ok(cache_logger)
    }

    fn filter_for(&self, target: &str) -> LevelFilter {
        let mut start = 0;
        let mut filter = self.top_filter;
        while let Some(end) = target[start..].find("::") {
            if let Some(f) = self.filters.get(&target[..start + end]) {
                filter = *f;
            }
            start += end + 2;
        }
        if let Some(f) = self.filters.get(target) {
            filter = *f;
        }

        filter
    }

    fn enabled_inner(&self, metadata: &Metadata, cache: &Option<Arc<CacheNode>>) -> bool {
        let cache_filter = cache
            .as_ref()
            .and_then(|node| node.local.as_ref())
            .map(|local| local.filter)
            .unwrap_or_else(LevelFilter::max);

        metadata.level() <= cache_filter && metadata.level() <= self.filter_for(metadata.target())
    }

    fn store_to_cache(&self, py: Python<'_>, target: &str, entry: CacheEntry) {
        let path = target.split("::");

        let orig = self.cache.load();
        // Construct a new cache structure and insert the new root.
        let new = orig.store_to_cache_recursive(py, path, entry);
        // Note: In case of collision, the cache update is lost. This is fine, as we simply lose a
        // tiny bit of performance and will cache the thing next time.
        //
        // We err on the side of losing it here (instead of overwriting), because if the cache is
        // reset, we don't want to re-insert the old value we have.
        self.cache.compare_and_swap(orig, new);
    }
}

impl Default for Logger {
    fn default() -> Self {
        Python::attach(|py| {
            Self::new(py, Caching::LoggersAndLevels).expect("Failed to initialize python logging")
        })
    }
}

impl Log for Logger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let cache = self.lookup(metadata.target());

        self.enabled_inner(metadata, &cache)
    }

    fn log(&self, record: &Record) {
        let cache = self.lookup(record.target());

        // Before calling into Python, make sure the interpreter is actually usable. If it isn't
        // initialized yet or has started finalizing, calling into it would crash or hang, so we
        // silently drop the message instead. (Only checked with the `dangerous-shutdown-guard` feature.)
        #[cfg(feature = "dangerous-shutdown-guard")]
        let usable = interpreter_usable();
        #[cfg(not(feature = "dangerous-shutdown-guard"))]
        let usable = true;

        if self.enabled_inner(record.metadata(), &cache) && usable {
            Python::attach(|py| {
                // If an exception were triggered before this attempt to log,
                // store it to the side for now and restore it afterwards.
                let maybe_existing_exception = PyErr::take(py);
                match self.log_inner(py, record, &cache) {
                    Ok(Some(logger)) => {
                        let filter = match self.caching {
                            Caching::Nothing => unreachable!(),
                            Caching::Loggers => LevelFilter::max(),
                            Caching::LoggersAndLevels => extract_max_level(logger.bind(py))
                                .unwrap_or_else(|e| {
                                    // See detailed NOTE below
                                    e.restore(py);
                                    LevelFilter::max()
                                }),
                        };

                        let entry = CacheEntry { filter, logger };
                        self.store_to_cache(py, record.target(), entry);
                    }
                    Ok(None) => (),
                    Err(e) => {
                        // NOTE: If an exception was triggered _during_ logging, restore it as current Python exception.
                        // We have to use PyErr::restore because we cannot return a PyResult from the Log trait's log method.
                        e.restore(py);
                    }
                };

                // If there was a prior exception, restore it now
                // This ensures that the earliest thrown exception will be the one that's visible to the caller.
                if let Some(e) = maybe_existing_exception {
                    e.restore(py);
                }
            })
        }
    }

    fn flush(&self) {}
}

fn map_level(level: Level) -> usize {
    match level {
        Level::Error => 40,
        Level::Warn => 30,
        Level::Info => 20,
        Level::Debug => 10,
        Level::Trace => 5,
    }
}

fn is_enabled_for(logger: &Bound<'_, PyAny>, level: Level) -> PyResult<bool> {
    let level = map_level(level);
    logger.call_method1("isEnabledFor", (level,))?.is_truthy()
}

fn extract_max_level(logger: &Bound<'_, PyAny>) -> PyResult<LevelFilter> {
    use Level::*;
    for l in &[Trace, Debug, Info, Warn, Error] {
        if is_enabled_for(logger, *l)? {
            return Ok(l.to_level_filter());
        }
    }

    Ok(LevelFilter::Off)
}

/// Installs a default instance of the logger.
///
/// In case a logger is already installed, an error is returned. On success, a handle to reset the
/// internal caches is returned.
///
/// The default logger has a filter set to [`Debug`][LevelFilter::Debug] and caching enabled to
/// [`LoggersAndLevels`][Caching::LoggersAndLevels].
pub fn try_init() -> Result<ResetHandle, SetLoggerError> {
    Logger::default().install()
}

/// Similar to [`try_init`], but panics if there's a previous logger already installed.
pub fn init() -> ResetHandle {
    try_init().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_filter() {
        let logger = Logger::default();
        assert_eq!(logger.filter_for("hello_world"), LevelFilter::Debug);
        assert_eq!(logger.filter_for("hello_world::sub"), LevelFilter::Debug);
    }

    #[test]
    fn set_filter() {
        let logger = Logger::default().filter(LevelFilter::Info);
        assert_eq!(logger.filter_for("hello_world"), LevelFilter::Info);
        assert_eq!(logger.filter_for("hello_world::sub"), LevelFilter::Info);
    }

    #[test]
    fn filter_specific() {
        let logger = Logger::default()
            .filter(LevelFilter::Warn)
            .filter_target("hello_world".to_owned(), LevelFilter::Debug)
            .filter_target("hello_world::sub".to_owned(), LevelFilter::Trace);
        assert_eq!(logger.filter_for("hello_world"), LevelFilter::Debug);
        assert_eq!(logger.filter_for("hello_world::sub"), LevelFilter::Trace);
        assert_eq!(
            logger.filter_for("hello_world::sub::multi::level"),
            LevelFilter::Trace
        );
        assert_eq!(
            logger.filter_for("hello_world::another"),
            LevelFilter::Debug
        );
        assert_eq!(
            logger.filter_for("hello_world::another::level"),
            LevelFilter::Debug
        );
        assert_eq!(logger.filter_for("other"), LevelFilter::Warn);
    }
}
