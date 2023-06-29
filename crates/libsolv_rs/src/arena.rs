use std::marker::PhantomData;
use std::ops::{Index, IndexMut};

pub(crate) trait ArenaId {
    fn from_usize(x: usize) -> Self;
    fn to_usize(self) -> usize;
}

pub(crate) struct Arena<TId: ArenaId, TValue> {
    data: Vec<TValue>,
    phantom: PhantomData<TId>,
}

impl<TId: ArenaId, TValue> Arena<TId, TValue> {
    pub(crate) fn new() -> Self {
        Self {
            data: Vec::new(),
            phantom: PhantomData::default(),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.data.clear();
    }

    pub(crate) fn alloc(&mut self, value: TValue) -> TId {
        let id = TId::from_usize(self.data.len());
        self.data.push(value);
        id
    }

    // TODO: all places where we are using len, are places where we should introduce mappings
    pub(crate) fn len(&self) -> usize {
        self.data.len()
    }

    #[cfg(test)]
    pub(crate) fn as_slice(&self) -> &[TValue] {
        &self.data
    }
}

impl<TId: ArenaId, TValue> Index<TId> for Arena<TId, TValue> {
    type Output = TValue;

    fn index(&self, index: TId) -> &Self::Output {
        &self.data[index.to_usize()]
    }
}

impl<TId: ArenaId, TValue> IndexMut<TId> for Arena<TId, TValue> {
    fn index_mut(&mut self, index: TId) -> &mut Self::Output {
        &mut self.data[index.to_usize()]
    }
}
