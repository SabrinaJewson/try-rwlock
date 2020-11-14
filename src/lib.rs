//! `TryRwLock` is a lightweight readers-writer lock implemented with atomics that does not support
//! blocking.
//!
//! A readers-writer lock allows multiple readers or one writer to access it at a time.
//!
//! # See Also
//!
//! [`try-lock`](https://crates.io/crates/try-lock) and
//! [`try-mutex`](https://crates.io/crates/try-mutex) provide a similar function to this, but
//! implement mutexes not readers-writer locks.
#![warn(
    clippy::pedantic,
    rust_2018_idioms,
    missing_docs,
    unused_qualifications
)]
#![cfg_attr(not(test), no_std)]

use core::cell::UnsafeCell;
use core::fmt::{self, Debug, Display, Formatter};
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{self, AtomicUsize};

/// A readers-writer lock.
#[derive(Default)]
pub struct TryRwLock<T> {
    /// The number of readers currently holding the lock. 0 means the lock is free, usize::MAX
    /// means there are usize::MAX readers or it is being written.
    readers: AtomicUsize,
    /// The internal value.
    data: UnsafeCell<T>,
}

impl<T> TryRwLock<T> {
    /// Create a new unlocked `TryRwLock<T>`.
    #[must_use]
    pub const fn new(data: T) -> Self {
        Self {
            readers: AtomicUsize::new(0),
            data: UnsafeCell::new(data),
        }
    }

    /// Attempt to lock this `TryRwLock` with shared read access.
    ///
    /// If the lock is currently being written to or there are `usize::MAX` existing readers, this
    /// function will return `None`.
    pub fn try_read(&self) -> Option<ReadGuard<'_, T>> {
        let mut readers = self.readers.load(atomic::Ordering::Acquire);

        loop {
            if readers == usize::MAX {
                return None;
            }
            let new_readers =
                self.readers
                    .compare_and_swap(readers, readers + 1, atomic::Ordering::AcqRel);
            if new_readers == readers {
                return Some(ReadGuard {
                    lock: self,
                    not_send: PhantomData,
                });
            }
            readers = new_readers;
        }
    }

    /// Attempt to lock this `TryRwLock` with unique write access.
    ///
    /// If the lock is currently being written to or read from, this function will return `None`.
    pub fn try_write(&self) -> Option<WriteGuard<'_, T>> {
        if self
            .readers
            .compare_and_swap(0, usize::MAX, atomic::Ordering::AcqRel)
            == 0
        {
            Some(WriteGuard {
                lock: self,
                not_send: PhantomData,
            })
        } else {
            None
        }
    }

    /// Get the underlying data of the lock.
    #[must_use]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }

    /// Get a mutable reference to the underlying data.
    ///
    /// As this method takes a mutable reference, no locking needs to take place.
    #[must_use]
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: We have a mutable reference to `self`.
        unsafe { &mut *self.data.get() }
    }
}

impl<T: Debug> Debug for TryRwLock<T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        #[allow(clippy::option_if_let_else)]
        if let Some(guard) = self.try_read() {
            f.debug_struct("TryRwLock").field("data", &*guard).finish()
        } else {
            struct LockedPlaceholder;
            impl Debug for LockedPlaceholder {
                fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                    f.write_str("<locked>")
                }
            }

            f.debug_struct("TryRwLock")
                .field("data", &LockedPlaceholder)
                .finish()
        }
    }
}

impl<T> From<T> for TryRwLock<T> {
    fn from(data: T) -> Self {
        Self::new(data)
    }
}

unsafe impl<T: Send> Send for TryRwLock<T> {}
unsafe impl<T: Send + Sync> Sync for TryRwLock<T> {}

/// A RAII guard that guarantees shared read access to a `TryRwLock`.
#[must_use = "if unused the TryRwLock will immediately unlock"]
pub struct ReadGuard<'a, T> {
    lock: &'a TryRwLock<T>,
    not_send: PhantomData<*mut ()>,
}

impl<'a, T> ReadGuard<'a, T> {
    /// Attempt to upgrade the `ReadGuard` to a `WriteGuard`.
    ///
    /// # Errors
    ///
    /// Fails if there is more than one reader currently using the lock.
    pub fn try_upgrade(guard: Self) -> Result<WriteGuard<'a, T>, Self> {
        if guard
            .lock
            .readers
            .compare_and_swap(1, usize::MAX, atomic::Ordering::AcqRel)
            == 1
        {
            let lock = guard.lock;
            core::mem::forget(guard);
            Ok(WriteGuard {
                lock,
                not_send: PhantomData,
            })
        } else {
            Err(guard)
        }
    }
}

impl<T> Deref for ReadGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> Drop for ReadGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.readers.fetch_sub(1, atomic::Ordering::Release);
    }
}

unsafe impl<T: Sync> Sync for ReadGuard<'_, T> {}

impl<T: Debug> Debug for ReadGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TryRwLockReadGuard")
            .field("data", &**self)
            .finish()
    }
}
impl<T: Display> Display for ReadGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

/// A RAII guard that guarantees unique write access to a `TryRwLock`.
#[must_use = "if unused the TryRwLock will immediately unlock"]
pub struct WriteGuard<'a, T> {
    lock: &'a TryRwLock<T>,
    not_send: PhantomData<*mut ()>,
}

impl<'a, T> WriteGuard<'a, T> {
    /// Downgrade the `WriteGuard` to a `ReadGuard`.
    pub fn downgrade(guard: Self) -> ReadGuard<'a, T> {
        let lock = guard.lock;
        core::mem::forget(guard);
        lock.readers.store(1, atomic::Ordering::Release);
        ReadGuard {
            lock,
            not_send: PhantomData,
        }
    }
}

impl<T> Deref for WriteGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}
impl<T> DerefMut for WriteGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for WriteGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.readers.store(0, atomic::Ordering::Release);
    }
}

unsafe impl<T: Sync> Sync for WriteGuard<'_, T> {}

impl<T: Debug> Debug for WriteGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("TryRwLockWriteGuard")
            .field("data", &**self)
            .finish()
    }
}
impl<T: Display> Display for WriteGuard<'_, T> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&**self, f)
    }
}

#[test]
fn test_read() {
    let lock = TryRwLock::new("Hello World!".to_owned());

    let guard_1 = lock.try_read().unwrap();
    let guard_2 = lock.try_read().unwrap();

    assert_eq!(&*guard_1, "Hello World!");
    assert_eq!(&*guard_2, "Hello World!");

    assert!(lock.try_write().is_none());
    let guard_1 = ReadGuard::try_upgrade(guard_1).unwrap_err();
    let guard_2 = ReadGuard::try_upgrade(guard_2).unwrap_err();

    drop(guard_1);

    assert!(lock.try_write().is_none());
    assert!(lock.try_read().is_some());
    let guard_2 = ReadGuard::try_upgrade(guard_2).unwrap();
    assert!(lock.try_read().is_none());
    let guard_2 = WriteGuard::downgrade(guard_2);
    assert!(lock.try_read().is_some());

    drop(guard_2);
}

#[test]
fn test_write() {
    let lock = TryRwLock::new("Hello World!".to_owned());

    let mut guard = lock.try_write().unwrap();

    assert_eq!(&*guard, "Hello World!");
    *guard = "Foo".to_owned();
    assert_eq!(&*guard, "Foo");

    assert!(lock.try_read().is_none());
    assert!(lock.try_write().is_none());

    drop(guard);

    assert_eq!(&*lock.try_read().unwrap(), "Foo");
}
