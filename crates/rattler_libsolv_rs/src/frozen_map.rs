use std::borrow::Borrow;
use std::cell::UnsafeCell;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// An insert only map where items can only be returned by cloning the values. This ensures that the
/// map can safely be used in an immutable context.
pub struct FrozenMap<K, V, S = RandomState> {
    map: UnsafeCell<HashMap<K, V, S>>,
}

impl<K: Eq + Hash, V> FrozenMap<K, V> {
    pub fn new() -> Self {
        Self {
            map: UnsafeCell::new(Default::default()),
        }
    }

    pub fn len(&self) -> usize {
        self.in_use.set(true);
        let len = unsafe {
            let map = self.map.get();
            (*map).len()
        };
        len
    }

    /// # Examples
    ///
    /// ```
    /// use elsa::FrozenMap;
    ///
    /// let map = FrozenMap::new();
    /// assert_eq!(map.is_empty(), true);
    /// map.insert(1, Box::new("a"));
    /// assert_eq!(map.is_empty(), false);
    /// ```
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<K: Eq + Hash, V: Clone, S: BuildHasher> FrozenMap<K, V, S> {
    pub fn insert_copy(&self, k: K, v: V) -> Option<V> {
        unsafe {
            let map = self.map.get();
            (*map).insert(k, v)
        };
    }

    pub fn get_copy<Q: ?Sized>(&self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
    {
        unsafe {
            let map = self.map.get();
            (*map).get(k).cloned()
        }
    }
}

impl<K: Eq + Hash, V, S: Default> Default for FrozenMap<K, V, S> {
    fn default() -> Self {
        Self {
            map: UnsafeCell::new(Default::default()),
        }
    }
}
