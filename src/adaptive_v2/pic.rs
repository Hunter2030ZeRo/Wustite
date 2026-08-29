#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PicState {
    Specialized { cases: u8 },
    Generic,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct PicCounters {
    pub(crate) hits: u64,
    pub(crate) misses: u64,
    pub(crate) generic_fallbacks: u64,
}

#[derive(Debug)]
pub(crate) struct Pic<K, V> {
    entries: Vec<(K, V)>,
    generic: bool,
    counters: PicCounters,
}

impl<K: Copy + Eq, V: Copy> Pic<K, V> {
    pub(crate) const fn new() -> Self {
        Self {
            entries: Vec::new(),
            generic: false,
            counters: PicCounters {
                hits: 0,
                misses: 0,
                generic_fallbacks: 0,
            },
        }
    }

    pub(crate) fn observe(&mut self, key: K, value: V) {
        if self.generic {
            return;
        }
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.0 == key) {
            entry.1 = value;
            return;
        }
        if self.entries.len() == 4 {
            self.entries.clear();
            self.generic = true;
            return;
        }
        self.entries.push((key, value));
    }

    pub(crate) fn resolve_or(&mut self, key: K, fallback: impl FnOnce() -> V) -> V {
        if self.generic {
            self.counters.generic_fallbacks += 1;
            return fallback();
        }
        match self.entries.iter().find(|entry| entry.0 == key) {
            Some(entry) => {
                self.counters.hits += 1;
                entry.1
            }
            None => {
                self.counters.misses += 1;
                fallback()
            }
        }
    }

    pub(crate) fn state(&self) -> PicState {
        if self.generic {
            PicState::Generic
        } else {
            PicState::Specialized {
                cases: u8::try_from(self.entries.len()).unwrap_or(4),
            }
        }
    }

    pub(crate) const fn counters(&self) -> PicCounters {
        self.counters
    }
}

macro_rules! cache_key {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) struct $name {
            identity: u64,
            dependency_epoch: u64,
            executable_epoch: u64,
        }

        impl $name {
            pub(crate) const fn new(
                identity: u64,
                dependency_epoch: u64,
                executable_epoch: u64,
            ) -> Self {
                Self {
                    identity,
                    dependency_epoch,
                    executable_epoch,
                }
            }

            #[cfg(test)]
            pub(crate) const fn test(identity: u32) -> Self {
                Self::new(identity as u64, 0, 0)
            }
        }
    };
}

cache_key!(ObjectGetKey);
cache_key!(ObjectSetKey);
cache_key!(CallKey);
cache_key!(ListKey);
