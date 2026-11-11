//! Locks that stay usable after a thread panicked while holding them.
//!
//! `std` locks are "poisoned" by such a panic, and every later `lock().unwrap()` panics too,
//! which turns one crashed task into a crashed program. The data these locks guard is replaced
//! or appended to in single steps, so it is still valid after a panic, and the lock is simply
//! opened again.

use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A [`Mutex`] that opens even if it was poisoned.
pub trait MutexExt<Value: ?Sized> {
    /// Locks the mutex, ignoring poisoning.
    fn lock_unpoisoned(&self) -> MutexGuard<'_, Value>;
}

impl<Value: ?Sized> MutexExt<Value> for Mutex<Value> {
    #[inline]
    fn lock_unpoisoned(&self) -> MutexGuard<'_, Value> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// An [`RwLock`] that opens even if it was poisoned.
pub trait RwLockExt<Value: ?Sized> {
    /// Locks for reading, ignoring poisoning.
    fn read_unpoisoned(&self) -> RwLockReadGuard<'_, Value>;
    /// Locks for writing, ignoring poisoning.
    fn write_unpoisoned(&self) -> RwLockWriteGuard<'_, Value>;
}

impl<Value: ?Sized> RwLockExt<Value> for RwLock<Value> {
    #[inline]
    fn read_unpoisoned(&self) -> RwLockReadGuard<'_, Value> {
        self.read().unwrap_or_else(PoisonError::into_inner)
    }

    #[inline]
    fn write_unpoisoned(&self) -> RwLockWriteGuard<'_, Value> {
        self.write().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use super::*;

    #[test]
    fn a_mutex_opens_after_a_panic_while_it_was_held() {
        let mutex = Mutex::new(String::from("kept"));
        thread::scope(|scope| {
            let crashed = scope
                .spawn(|| {
                    let _guard = mutex.lock_unpoisoned();
                    panic!("crash while holding the lock");
                })
                .join();
            assert!(crashed.is_err());
        });
        assert!(mutex.is_poisoned());
        assert_eq!(*mutex.lock_unpoisoned(), "kept");
    }

    #[test]
    fn an_rwlock_opens_after_a_panic_while_it_was_held() {
        let lock = RwLock::new(1i32);
        thread::scope(|scope| {
            let crashed = scope
                .spawn(|| {
                    let _guard = lock.write_unpoisoned();
                    panic!("crash while holding the lock");
                })
                .join();
            assert!(crashed.is_err());
        });
        *lock.write_unpoisoned() = 2i32;
        assert_eq!(*lock.read_unpoisoned(), 2i32);
    }
}
