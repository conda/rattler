use std::cell::{Cell, UnsafeCell};
use std::cmp;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

const CHUNK_SIZE: usize = 128;

/// An `Arena<TValue>` holds a collection of `TValue`s but allocates persistent `TId`s that are used
/// to refer to an element in the arena. When adding an item to an `Arena` it returns a `TId` that
/// can be used to index into the arena.
///
/// An arena is "frozen". New elements can be added to the arena but existing elements can never be
/// modified. This allows the arena to always return stable references even when the arena is being
/// modified.
///
/// Methods that mutable the arena (like clearing it) still require a mutable reference because they
/// might invalidate existing references.
pub(crate) struct Arena<TId: ArenaId, TValue> {
    chunks: UnsafeCell<Vec<Vec<TValue>>>,
    len: Cell<usize>,
    phantom: PhantomData<TId>,
}

impl<TId: ArenaId, TValue> Default for Arena<TId, TValue> {
    fn default() -> Self {
        Self::new()
    }
}

impl<TId: ArenaId, TValue> Arena<TId, TValue> {
    /// Constructs a new arena.
    pub(crate) fn new() -> Self {
        Arena::with_capacity(1)
    }

    /// Clears all entries from the arena. Although the mutable reference ensures that are no
    /// existing references to internal values the IDs returned from this instance also become
    /// invalid. Accessing this instance with an old ID will result in undefined behavior.
    pub fn clear(&mut self) {
        self.len.set(0);
        for chunk in self.chunks.get_mut().iter_mut() {
            chunk.clear();
        }
    }

    /// Constructs a new arena with a capacity for `n` values pre-allocated.
    pub fn with_capacity(n: usize) -> Self {
        let n = cmp::max(1, n);
        let n_chunks = (n + CHUNK_SIZE - 1) / CHUNK_SIZE;
        let mut chunks = Vec::new();
        chunks.resize_with(n_chunks, || Vec::with_capacity(CHUNK_SIZE));
        Self {
            chunks: UnsafeCell::from(chunks),
            len: Cell::new(0),
            phantom: Default::default(),
        }
    }

    /// Returns the size of the arena
    ///
    /// This is useful for using the size of previous typed arenas to build new typed arenas with
    /// large enough space.
    pub fn len(&self) -> usize {
        self.len.get()
    }

    /// Allocates a new instance of `TValue` and returns an Id that can be used to reference it.
    pub fn alloc(&self, value: TValue) -> TId {
        let id = self.len.get();
        let (chunk_idx, _) = Self::chunk_and_offset(id);
        let chunks = unsafe { &mut *self.chunks.get() };
        if chunk_idx >= chunks.len() {
            chunks.resize_with(chunks.len() + 1, || Vec::with_capacity(CHUNK_SIZE));
        }
        chunks[chunk_idx].push(value);
        self.len.set(id + 1);
        TId::from_usize(id)
    }

    /// Returns an iterator over the elements of the arena.
    pub fn iter(&self) -> ArenaIter<TId, TValue> {
        ArenaIter {
            arena: self,
            index: 0,
        }
    }

    /// Returns an mutable iterator over the elements of the arena.
    pub fn iter_mut(&mut self) -> ArenaIterMut<TId, TValue> {
        ArenaIterMut {
            arena: self,
            index: 0,
        }
    }

    fn chunk_and_offset(index: usize) -> (usize, usize) {
        let offset = index % CHUNK_SIZE;
        let chunk = index / CHUNK_SIZE;
        (chunk, offset)
    }

    /// Returns mutable references to the two values references by the two distinct indices.
    ///
    /// Panics if one of the Ids is invalid or when the two ids are the same.
    pub fn get_two_mut(&mut self, a: TId, b: TId) -> (&mut TValue, &mut TValue) {
        let a_index = a.to_usize();
        let b_index = b.to_usize();
        assert!(a_index < self.len());
        assert!(b_index < self.len());
        assert_ne!(a_index, b_index);
        let (a_chunk, a_offset) = Self::chunk_and_offset(a_index);
        let (b_chunk, b_offset) = Self::chunk_and_offset(b_index);
        // SAFE: because we check that the indices are less than the length and that both indices do
        // not refer to the same item.
        unsafe {
            let chunks = self.chunks.get();
            (
                (*chunks)
                    .get_unchecked_mut(a_chunk)
                    .get_unchecked_mut(a_offset),
                (*chunks)
                    .get_unchecked_mut(b_chunk)
                    .get_unchecked_mut(b_offset),
            )
        }
    }
}

impl<TId: ArenaId, TValue> Index<TId> for Arena<TId, TValue> {
    type Output = TValue;

    fn index(&self, index: TId) -> &Self::Output {
        let index = index.to_usize();
        assert!(index < self.len());
        let (chunk, offset) = Self::chunk_and_offset(index);
        unsafe {
            let vec = self.chunks.get();
            (*vec).get_unchecked(chunk).get_unchecked(offset)
        }
    }
}

impl<TId: ArenaId, TValue> IndexMut<TId> for Arena<TId, TValue> {
    fn index_mut(&mut self, index: TId) -> &mut Self::Output {
        let index = index.to_usize();
        assert!(index < self.len());
        let (chunk, offset) = Self::chunk_and_offset(index);
        // SAFE: because we check that the index is less than the length
        unsafe {
            self.chunks
                .get_mut()
                .get_unchecked_mut(chunk)
                .get_unchecked_mut(offset)
        }
    }
}

/// A trait indicating that the type can be transformed to `usize` and back
pub trait ArenaId {
    fn from_usize(x: usize) -> Self;
    fn to_usize(self) -> usize;
}

/// An iterator over the elements of an [`Arena`].
pub struct ArenaIter<'a, TId: ArenaId, TValue> {
    arena: &'a Arena<TId, TValue>,
    index: usize,
}

impl<'a, TId: ArenaId, TValue> Iterator for ArenaIter<'a, TId, TValue> {
    type Item = (TId, &'a TValue);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.arena.len.get() {
            let (chunk, offset) = Arena::<TId, TValue>::chunk_and_offset(self.index);
            let element = unsafe {
                let vec = self.arena.chunks.get();
                Some((
                    TId::from_usize(self.index),
                    (*vec).get_unchecked(chunk).get_unchecked(offset),
                ))
            };

            self.index += 1;
            element
        } else {
            None
        }
    }
}

/// An mutable iterator over the elements of an [`Arena`].
pub struct ArenaIterMut<'a, TId: ArenaId, TValue> {
    arena: &'a mut Arena<TId, TValue>,
    index: usize,
}

impl<'a, TId: ArenaId, TValue> Iterator for ArenaIterMut<'a, TId, TValue> {
    type Item = (TId, &'a mut TValue);

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.arena.len.get() {
            let (chunk, offset) = Arena::<TId, TValue>::chunk_and_offset(self.index);
            let element = unsafe {
                let vec = self.arena.chunks.get();
                Some((
                    TId::from_usize(self.index),
                    (*vec).get_unchecked_mut(chunk).get_unchecked_mut(offset),
                ))
            };
            self.index += 1;
            element
        } else {
            None
        }
    }
}
