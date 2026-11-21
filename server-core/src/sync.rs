use std::sync::{Mutex, MutexGuard, PoisonError};

// INVARIANT: A lock whose holder panicked still guards consistent state here: every
// critical section leaves it valid between statements, and the loop must keep
// serving the arm rather than panic in turn.
pub(crate) trait LockUnpoisoned<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T>;
}

impl<T> LockUnpoisoned<T> for Mutex<T> {
    fn lock_unpoisoned(&self) -> MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
