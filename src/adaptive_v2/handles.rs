use std::fmt;

pub(crate) const NATIVE_HANDLE_CAPACITY: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct RuntimeId(u64);

impl RuntimeId {
    pub(crate) const fn new(value: u64) -> Self {
        Self(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct StableHandle {
    runtime: RuntimeId,
    slot: u32,
    generation: u16,
}

impl StableHandle {
    pub(crate) const fn slot(self) -> u32 {
        self.slot
    }

    pub(crate) const fn generation(self) -> u16 {
        self.generation
    }

    pub(crate) fn packed_local(self) -> u64 {
        u64::from(self.slot) | ((self.generation as u64) << 32)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandleError {
    Capacity,
    WrongRuntime,
    Stale,
}

impl fmt::Display for HandleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Capacity => formatter.write_str("stable handle table capacity exhausted"),
            Self::WrongRuntime => formatter.write_str("stable handle belongs to another runtime"),
            Self::Stale => formatter.write_str("stable handle is stale"),
        }
    }
}

impl std::error::Error for HandleError {}

#[derive(Debug)]
enum Entry<T> {
    Vacant { generation: u16 },
    Live { generation: u16, value: T },
    Retired,
}

#[derive(Debug)]
pub(crate) struct StableHandleTable<T> {
    runtime: RuntimeId,
    capacity: usize,
    entries: Vec<Entry<T>>,
    free: Vec<u32>,
    retired: usize,
}

impl<T> StableHandleTable<T> {
    pub(crate) const fn new(runtime: RuntimeId, capacity: usize) -> Self {
        Self {
            runtime,
            capacity,
            entries: Vec::new(),
            free: Vec::new(),
            retired: 0,
        }
    }

    pub(crate) fn allocate(&mut self, value: T) -> Result<StableHandle, HandleError> {
        let slot = match self.free.pop() {
            Some(slot) => slot,
            None if self.entries.len() < self.capacity => {
                let slot = u32::try_from(self.entries.len()).map_err(|_| HandleError::Capacity)?;
                self.entries.push(Entry::Vacant { generation: 1 });
                slot
            }
            None => return Err(HandleError::Capacity),
        };
        let index = usize::try_from(slot).map_err(|_| HandleError::Stale)?;
        let entry = self.entries.get_mut(index).ok_or(HandleError::Stale)?;
        let generation = match entry {
            Entry::Vacant { generation } => *generation,
            Entry::Live { .. } | Entry::Retired => return Err(HandleError::Stale),
        };
        *entry = Entry::Live { generation, value };
        Ok(StableHandle {
            runtime: self.runtime,
            slot,
            generation,
        })
    }

    pub(crate) fn allocate_with_generation(
        &mut self,
        value: T,
        generation: u16,
    ) -> Result<StableHandle, HandleError> {
        let handle = self.allocate(value)?;
        let index = usize::try_from(handle.slot).map_err(|_| HandleError::Stale)?;
        let entry = self.entries.get_mut(index).ok_or(HandleError::Stale)?;
        match entry {
            Entry::Live {
                generation: live, ..
            } => *live = generation,
            Entry::Vacant { .. } | Entry::Retired => return Err(HandleError::Stale),
        }
        Ok(StableHandle {
            generation,
            ..handle
        })
    }

    pub(crate) fn local_handle(&self, packed: u64) -> StableHandle {
        StableHandle {
            runtime: self.runtime,
            slot: packed as u32,
            generation: (packed >> 32) as u16,
        }
    }

    pub(crate) fn resolve(&self, handle: StableHandle) -> Result<&T, HandleError> {
        self.validate(handle)
            .and_then(|index| match &self.entries[index] {
                Entry::Live { value, .. } => Ok(value),
                Entry::Vacant { .. } | Entry::Retired => Err(HandleError::Stale),
            })
    }

    pub(crate) fn resolve_mut(&mut self, handle: StableHandle) -> Result<&mut T, HandleError> {
        let index = self.validate(handle)?;
        match &mut self.entries[index] {
            Entry::Live { value, .. } => Ok(value),
            Entry::Vacant { .. } | Entry::Retired => Err(HandleError::Stale),
        }
    }

    pub(crate) fn release(&mut self, handle: StableHandle) -> Result<T, HandleError> {
        let index = self.validate(handle)?;
        let old = std::mem::replace(&mut self.entries[index], Entry::Retired);
        match old {
            Entry::Live { generation, value } if generation == u16::MAX => {
                self.retired += 1;
                Ok(value)
            }
            Entry::Live { generation, value } => {
                self.entries[index] = Entry::Vacant {
                    generation: generation + 1,
                };
                self.free.push(handle.slot);
                Ok(value)
            }
            Entry::Vacant { .. } | Entry::Retired => Err(HandleError::Stale),
        }
    }

    pub(crate) const fn retired_slots(&self) -> usize {
        self.retired
    }

    pub(crate) fn remaining_capacity(&self) -> usize {
        self.free.len() + self.capacity.saturating_sub(self.entries.len())
    }

    pub(crate) fn live_handles(&self) -> impl Iterator<Item = StableHandle> + '_ {
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(slot, entry)| match entry {
                Entry::Live { generation, .. } => {
                    u32::try_from(slot).ok().map(|slot| StableHandle {
                        runtime: self.runtime,
                        slot,
                        generation: *generation,
                    })
                }
                Entry::Vacant { .. } | Entry::Retired => None,
            })
    }

    fn validate(&self, handle: StableHandle) -> Result<usize, HandleError> {
        if handle.runtime != self.runtime {
            return Err(HandleError::WrongRuntime);
        }
        let index = usize::try_from(handle.slot).map_err(|_| HandleError::Stale)?;
        match self.entries.get(index) {
            Some(Entry::Live { generation, .. }) if *generation == handle.generation => Ok(index),
            Some(Entry::Vacant { .. } | Entry::Retired | Entry::Live { .. }) | None => {
                Err(HandleError::Stale)
            }
        }
    }
}
