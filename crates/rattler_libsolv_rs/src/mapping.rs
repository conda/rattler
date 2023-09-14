use crate::arena::ArenaId;
use std::cmp;
use std::iter::FusedIterator;
use std::marker::PhantomData;

const VALUES_PER_CHUNK: usize = 128;

/// A `Mapping<TValue>` holds a collection of `TValue`s that can be addressed by `TId`s. You can
/// think of it as a HashMap<TId, TValue>, optimized for the case in which we know the `TId`s are
/// contiguous.
pub struct Mapping<TId, TValue> {
    chunks: Vec<[Option<TValue>; VALUES_PER_CHUNK]>,
    len: usize,
    _phantom: PhantomData<TId>,
}

impl<TId: ArenaId, TValue: Clone> Default for Mapping<TId, TValue> {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(unused)]
impl<TId: ArenaId, TValue> Mapping<TId, TValue> {
    pub(crate) fn new() -> Self {
        Self::with_capacity(1)
    }

    /// Constructs a new arena with a capacity for `n` values pre-allocated.
    pub fn with_capacity(n: usize) -> Self {
        let n = cmp::max(1, n);
        let n_chunks = (n - 1) / VALUES_PER_CHUNK + 1;
        let mut chunks = Vec::new();
        chunks.resize_with(n_chunks, || std::array::from_fn(|_| None));
        Self {
            chunks,
            len: 0,
            _phantom: Default::default(),
        }
    }

    /// Get chunk and offset for specific id
    #[inline]
    pub const fn chunk_and_offset(id: usize) -> (usize, usize) {
        let chunk = id / VALUES_PER_CHUNK;
        let offset = id % VALUES_PER_CHUNK;
        (chunk, offset)
    }

    /// Insert into the mapping with the specific value
    pub fn insert(&mut self, id: TId, value: TValue) {
        let (chunk, offset) = Self::chunk_and_offset(id.to_usize());

        // Resize to fit if needed
        if chunk >= self.chunks.len() {
            self.chunks
                .resize_with(chunk + 1, || std::array::from_fn(|_| None));
        }
        self.chunks[chunk][offset] = Some(value);
        self.len += 1;
    }

    /// Get a specific value in the mapping with bound checks
    pub fn get(&self, id: TId) -> Option<&TValue> {
        let (chunk, offset) = Self::chunk_and_offset(id.to_usize());
        if chunk >= self.chunks.len() {
            return None;
        }

        // Safety: we know that the chunk and offset are valid
        unsafe {
            self.chunks
                .get_unchecked(chunk)
                .get_unchecked(offset)
                .as_ref()
        }
    }

    /// Get a mutable specific value in the mapping with bound checks
    pub fn get_mut(&mut self, id: TId) -> Option<&mut TValue> {
        let (chunk, offset) = Self::chunk_and_offset(id.to_usize());
        if chunk >= self.chunks.len() {
            return None;
        }

        // Safety: we know that the chunk and offset are valid
        unsafe {
            self.chunks
                .get_unchecked_mut(chunk)
                .get_unchecked_mut(offset)
                .as_mut()
        }
    }

    /// Get a specific value in the mapping without bound checks
    ///
    /// # Safety
    /// The caller must uphold most of the safety requirements for `get_unchecked`. i.e. the id having been inserted into the Mapping before.
    pub unsafe fn get_unchecked(&self, id: TId) -> &TValue {
        let (chunk, offset) = Self::chunk_and_offset(id.to_usize());
        self.chunks
            .get_unchecked(chunk)
            .get_unchecked(offset)
            .as_ref()
            .unwrap()
    }

    /// Get a specific value in the mapping without bound checks
    ///
    /// # Safety
    /// The caller must uphold most of the safety requirements for `get_unchecked_mut`. i.e. the id having been inserted into the Mapping before.
    pub unsafe fn get_unchecked_mut(&mut self, id: TId) -> &mut TValue {
        let (chunk, offset) = Self::chunk_and_offset(id.to_usize());
        self.chunks
            .get_unchecked_mut(chunk)
            .get_unchecked_mut(offset)
            .as_mut()
            .unwrap()
    }

    /// Returns the number of mapped items
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the Mapping is empty
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Defines the number of slots that can be used
    /// theses slots are not initialized
    pub fn slots(&self) -> usize {
        self.chunks.len() * VALUES_PER_CHUNK
    }

    /// Returns an iterator over all the existing key value pairs.
    pub fn iter(&self) -> MappingIter<TId, TValue> {
        MappingIter {
            mapping: self,
            offset: 0,
        }
    }
}

pub struct MappingIter<'a, TId, TValue> {
    mapping: &'a Mapping<TId, TValue>,
    offset: usize,
}

impl<'a, TId: ArenaId, TValue> Iterator for MappingIter<'a, TId, TValue> {
    type Item = (TId, &'a TValue);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.offset >= self.mapping.len {
                return None;
            }

            let (chunk, offset) = Mapping::<TId, TValue>::chunk_and_offset(self.offset);
            let id = TId::from_usize(self.offset);
            self.offset += 1;

            unsafe {
                if let Some(value) = &self
                    .mapping
                    .chunks
                    .get_unchecked(chunk)
                    .get_unchecked(offset)
                {
                    break Some((id, value));
                }
            }
        }
    }
}

impl<'a, TId: ArenaId, TValue> FusedIterator for MappingIter<'a, TId, TValue> {}

#[cfg(test)]
mod tests {
    use crate::arena::ArenaId;

    struct Id {
        id: usize,
    }

    impl ArenaId for Id {
        fn from_usize(x: usize) -> Self {
            Id { id: x }
        }

        fn to_usize(self) -> usize {
            self.id
        }
    }

    #[test]
    pub fn test_mapping() {
        // New mapping should have 128 slots per default
        let mut mapping = super::Mapping::<Id, usize>::new();
        assert_eq!(mapping.len(), 0);
        assert_eq!(mapping.slots(), super::VALUES_PER_CHUNK);

        // Inserting a value should increase the length
        // and the number of slots should stay the same
        mapping.insert(Id::from_usize(0), 10usize);
        assert_eq!(mapping.len(), 1);

        // Should be able to get it
        assert_eq!(*mapping.get(Id::from_usize(0)).unwrap(), 10usize);
        assert_eq!(mapping.slots(), super::VALUES_PER_CHUNK);

        // Inserting higher than the slot size should trigger a resize
        mapping.insert(Id::from_usize(super::VALUES_PER_CHUNK), 20usize);
        assert_eq!(
            *mapping
                .get(Id::from_usize(super::VALUES_PER_CHUNK))
                .unwrap(),
            20usize
        );

        // Now contains 2 elements
        assert_eq!(mapping.len(), 2);
        // And double number of slots due to resize
        assert_eq!(mapping.slots(), super::VALUES_PER_CHUNK * 2);
    }
}
