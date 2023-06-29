use crate::arena::ArenaId;

#[repr(transparent)]
#[derive(Clone, Copy, Eq, PartialEq, Hash)]
pub struct RepoId(u32);

impl RepoId {
    pub(crate) fn new(id: u32) -> Self {
        Self(id)
    }
}

#[repr(transparent)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct NameId(u32);

impl NameId {
    pub(crate) fn new(index: usize) -> Self {
        Self(index as u32)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

impl ArenaId for NameId {
    fn from_usize(x: usize) -> Self {
        NameId::new(x)
    }

    fn to_usize(self) -> usize {
        self.index()
    }
}

#[repr(transparent)]
#[derive(Clone, Copy, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
pub struct MatchSpecId(u32);

impl MatchSpecId {
    pub(crate) fn new(index: usize) -> Self {
        Self(index as u32)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

impl ArenaId for MatchSpecId {
    fn from_usize(x: usize) -> Self {
        MatchSpecId::new(x)
    }

    fn to_usize(self) -> usize {
        self.index()
    }
}

#[repr(transparent)]
#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug, Ord, PartialOrd)]
pub struct SolvableId(u32);

impl SolvableId {
    pub(crate) fn new(index: usize) -> Self {
        debug_assert_ne!(index, u32::MAX as usize);
        Self(index as u32)
    }

    pub(crate) fn root() -> Self {
        Self(0)
    }

    pub(crate) fn is_root(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn null() -> Self {
        Self(u32::MAX)
    }

    pub(crate) fn is_null(self) -> bool {
        self.0 == u32::MAX
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }
}

impl ArenaId for SolvableId {
    fn from_usize(x: usize) -> Self {
        SolvableId::new(x)
    }

    fn to_usize(self) -> usize {
        self.index()
    }
}

#[repr(transparent)]
#[derive(Copy, Clone, PartialOrd, Ord, Eq, PartialEq, Debug, Hash)]
pub(crate) struct RuleId(u32);

impl RuleId {
    pub(crate) fn new(index: usize) -> Self {
        debug_assert_ne!(index, 0);
        debug_assert_ne!(index, u32::MAX as usize);

        Self(index as u32)
    }

    pub(crate) fn install_root() -> Self {
        Self(0)
    }

    pub(crate) fn index(self) -> usize {
        self.0 as usize
    }

    pub(crate) fn is_null(self) -> bool {
        self.0 == u32::MAX
    }

    pub(crate) fn null() -> RuleId {
        RuleId(u32::MAX)
    }
}
