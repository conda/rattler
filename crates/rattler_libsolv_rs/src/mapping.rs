use crate::arena::ArenaId;
use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

/// A `Mapping<TValue>` holds a collection of `TValue`s that can be addressed by `TId`s. You can
/// think of it as a HashMap<TId, TValue>, optimized for the case in which we know the `TId`s are
/// contiguous.
#[derive(Default)]
pub struct Mapping<TId: ArenaId, TValue> {
    data: Vec<TValue>,
    phantom: PhantomData<TId>,
}

impl<TId: ArenaId, TValue> Mapping<TId, TValue> {
    pub(crate) fn empty() -> Self {
        Self::new(Vec::new())
    }

    pub(crate) fn new(data: Vec<TValue>) -> Self {
        Self {
            data,
            phantom: PhantomData::default(),
        }
    }

    pub(crate) fn get(&self, id: TId) -> Option<&TValue> {
        self.data.get(id.to_usize())
    }

    pub(crate) fn extend(&mut self, value: TValue) {
        self.data.push(value);
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &TValue> {
        self.data.iter()
    }

    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }
}

impl<TId: ArenaId, TValue> Index<TId> for Mapping<TId, TValue> {
    type Output = TValue;

    fn index(&self, index: TId) -> &Self::Output {
        &self.data[index.to_usize()]
    }
}

impl<TId: ArenaId, TValue> IndexMut<TId> for Mapping<TId, TValue> {
    fn index_mut(&mut self, index: TId) -> &mut Self::Output {
        &mut self.data[index.to_usize()]
    }
}
