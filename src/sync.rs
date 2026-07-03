//! Small concurrency helpers shared across modules.

use std::sync::{Mutex, MutexGuard};

/// Locking that recovers from poisoning instead of propagating it, so a single
/// panicked task can't wedge every other holder of the lock.
pub(crate) trait MutexExt<T> {
    /// Locks `self`, taking the guard even if the mutex was poisoned.
    fn lock_recover(&self) -> MutexGuard<'_, T>;
}

impl<T> MutexExt<T> for Mutex<T> {
    fn lock_recover(&self) -> MutexGuard<'_, T> {
        self.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
