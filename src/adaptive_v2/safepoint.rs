#[cfg(all(test, loom))]
use loom::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
#[cfg(not(all(test, loom)))]
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SafepointError {
    Poisoned,
}

impl fmt::Display for SafepointError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("safepoint coordinator lock was poisoned")
    }
}

impl std::error::Error for SafepointError {}

#[derive(Debug)]
struct Control {
    epoch: u64,
    next_id: u64,
    requested_by: Option<u64>,
    mutators: BTreeMap<u64, bool>,
}

#[derive(Debug)]
struct Shared {
    control: Mutex<Control>,
    changed: Condvar,
}

#[derive(Debug, Clone)]
pub(crate) struct SafepointCoordinator {
    shared: Arc<Shared>,
}

impl SafepointCoordinator {
    pub(crate) fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                control: Mutex::new(Control {
                    epoch: 0,
                    next_id: 0,
                    requested_by: None,
                    mutators: BTreeMap::new(),
                }),
                changed: Condvar::new(),
            }),
        }
    }

    pub(crate) fn register(&self) -> Mutator {
        let mut control = self.lock_recover();
        let id = control.next_id;
        control.next_id = control.next_id.wrapping_add(1);
        let epoch = control.epoch;
        control.mutators.insert(id, false);
        Mutator {
            id,
            epoch,
            shared: Arc::clone(&self.shared),
        }
    }

    pub(crate) fn request_with<R>(
        &self,
        initiator: &Mutator,
        operation: impl FnOnce() -> R,
    ) -> Result<R, SafepointError> {
        let mut control = self
            .shared
            .control
            .lock()
            .map_err(|_| SafepointError::Poisoned)?;
        control.epoch = control.epoch.wrapping_add(1);
        control.requested_by = Some(initiator.id);
        self.shared.changed.notify_all();
        while control
            .mutators
            .iter()
            .any(|(id, parked)| *id != initiator.id && !parked)
        {
            control = self
                .shared
                .changed
                .wait(control)
                .map_err(|_| SafepointError::Poisoned)?;
        }
        drop(control);

        let outcome = catch_unwind(AssertUnwindSafe(operation));
        let mut control = self.lock_recover();
        control.requested_by = None;
        for parked in control.mutators.values_mut() {
            *parked = false;
        }
        self.shared.changed.notify_all();
        drop(control);
        match outcome {
            Ok(value) => Ok(value),
            Err(payload) => resume_unwind(payload),
        }
    }

    fn lock_recover(&self) -> MutexGuard<'_, Control> {
        self.shared
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub(crate) struct Mutator {
    id: u64,
    epoch: u64,
    shared: Arc<Shared>,
}

impl Mutator {
    pub(crate) fn epoch(&mut self) -> u64 {
        let control = self
            .shared
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.epoch = control.epoch;
        self.epoch
    }

    pub(crate) fn poll(&mut self) -> Result<(), SafepointError> {
        let mut control = self
            .shared
            .control
            .lock()
            .map_err(|_| SafepointError::Poisoned)?;
        if control
            .requested_by
            .is_some_and(|requester| requester != self.id)
        {
            control.mutators.insert(self.id, true);
            self.shared.changed.notify_all();
            while control.requested_by.is_some() {
                control = self
                    .shared
                    .changed
                    .wait(control)
                    .map_err(|_| SafepointError::Poisoned)?;
            }
        }
        self.epoch = control.epoch;
        Ok(())
    }

    pub(crate) fn park_for_next_request(&mut self) -> Result<(), SafepointError> {
        let mut control = self
            .shared
            .control
            .lock()
            .map_err(|_| SafepointError::Poisoned)?;
        while control.requested_by.is_none() {
            control = self
                .shared
                .changed
                .wait(control)
                .map_err(|_| SafepointError::Poisoned)?;
        }
        if control
            .requested_by
            .is_some_and(|requester| requester != self.id)
        {
            control.mutators.insert(self.id, true);
            self.shared.changed.notify_all();
            while control.requested_by.is_some() {
                control = self
                    .shared
                    .changed
                    .wait(control)
                    .map_err(|_| SafepointError::Poisoned)?;
            }
        }
        self.epoch = control.epoch;
        Ok(())
    }
}

impl Drop for Mutator {
    fn drop(&mut self) {
        let mut control = self
            .shared
            .control
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        control.mutators.remove(&self.id);
        self.shared.changed.notify_all();
    }
}
