use std::borrow::Borrow;
use std::cell::UnsafeCell;
use std::collections::hash_map::RandomState;
use std::collections::HashMap;
use std::hash::{BuildHasher, Hash};

/// An insert only map where items can only be returned by cloning the values. This ensures that the
/// map can safely be used in an immutable context.
pub struct FrozenCopyMap<K, V, S = RandomState> {
    map: UnsafeCell<HashMap<K, V, S>>,
}

impl<K: Eq + Hash, V: Clone, S: BuildHasher> FrozenCopyMap<K, V, S> {
    pub fn insert_copy(&self, k: K, v: V) -> Option<V> {
        unsafe {
            let map = self.map.get();
            (*map).insert(k, v)
        }
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

impl<K: Eq + Hash, V, S: Default> Default for FrozenCopyMap<K, V, S> {
    fn default() -> Self {
        Self {
            map: UnsafeCell::new(Default::default()),
        }
    }
}
