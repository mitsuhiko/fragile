#![allow(clippy::unit_arg)]

use std::cmp;
use std::fmt;
use std::marker::PhantomData;
use std::mem;
use std::num::NonZeroUsize;

use crate::errors::InvalidThreadAccess;
use crate::registry;
use crate::thread_id;

/// A `Sticky<T>` keeps a value T stored in a thread.
///
/// This type works similar in nature to `Fragile<T>` and exposes a
/// similar interface.  The difference is that whereas `Fragile<T>` has
/// its destructor called in the thread where the value was sent, a
/// `Sticky<T>` that is moved to another thread will have the internal
/// destructor called when the originating thread tears down.
///
/// Because `Sticky<T>` allows values to be kept alive for longer than the
/// `Sticky<T>` itself, it requires all its contents to be `'static` for
/// soundness.
///
/// As this uses TLS internally the general rules about the platform limitations
/// of destructors for TLS apply.
pub struct Sticky<T: 'static> {
    item_id: registry::ItemId,
    thread_id: NonZeroUsize,
    _marker: PhantomData<*mut T>,
}

impl<T> Drop for Sticky<T> {
    fn drop(&mut self) {
        if mem::needs_drop::<T>() {
            unsafe {
                if self.is_valid() {
                    self.unsafe_take_value();
                }
            }
        }
    }
}

impl<T> Sticky<T> {
    /// Creates a new `Sticky` wrapping a `value`.
    ///
    /// The value that is moved into the `Sticky` can be non `Send` and
    /// will be anchored to the thread that created the object.  If the
    /// sticky wrapper type ends up being send from thread to thread
    /// only the original thread can interact with the value.
    pub fn new(value: T) -> Self {
        let entry = registry::Entry {
            ptr: Box::into_raw(Box::new(value)).cast(),
            drop: |ptr| {
                let ptr = ptr.cast::<T>();
                // SAFETY: This callback will only be called once, with the
                // above pointer.
                drop(unsafe { Box::from_raw(ptr) });
            },
        };

        let thread_id = thread_id::get();
        let item_id = registry::insert(thread_id, entry);

        Sticky {
            item_id,
            thread_id,
            _marker: PhantomData,
        }
    }

    #[inline(always)]
    fn with_value<F: FnOnce(*mut T) -> R, R>(&self, f: F) -> R {
        self.assert_thread();

        registry::with(self.item_id, self.thread_id, |entry| {
            f(entry.ptr.cast::<T>())
        })
    }

    /// Returns `true` if the access is valid.
    ///
    /// This will be `false` if the value was sent to another thread.
    #[inline(always)]
    pub fn is_valid(&self) -> bool {
        thread_id::get() == self.thread_id
    }

    #[inline(always)]
    fn assert_thread(&self) {
        if !self.is_valid() {
            panic!("trying to access wrapped value in sticky container from incorrect thread.");
        }
    }

    /// Consumes the `Sticky`, returning the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if called from a different thread than the one where the
    /// original value was created.
    pub fn into_inner(mut self) -> T {
        self.assert_thread();
        unsafe {
            let rv = self.unsafe_take_value();
            mem::forget(self);
            rv
        }
    }

    unsafe fn unsafe_take_value(&mut self) -> T {
        let ptr = registry::remove(self.item_id, self.thread_id)
            .ptr
            .cast::<T>();
        *Box::from_raw(ptr)
    }

    /// Consumes the `Sticky`, returning the wrapped value if successful.
    ///
    /// The wrapped value is returned if this is called from the same thread
    /// as the one where the original value was created, otherwise the
    /// `Sticky` is returned as `Err(self)`.
    pub fn try_into_inner(self) -> Result<T, Self> {
        if self.is_valid() {
            Ok(self.into_inner())
        } else {
            Err(self)
        }
    }

    /// Immutably borrows the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_get`](#method.try_get`).
    pub fn get(&self) -> &T {
        self.with_value(|value| unsafe { &*value })
    }

    /// Mutably borrows the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_get_mut`](#method.try_get_mut`).
    pub fn get_mut(&mut self) -> &mut T {
        self.with_value(|value| unsafe { &mut *value })
    }

    /// Tries to immutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get(&self) -> Result<&T, InvalidThreadAccess> {
        if self.is_valid() {
            Ok(self.with_value(|value| unsafe { &*value }))
        } else {
            Err(InvalidThreadAccess)
        }
    }

    /// Tries to mutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get_mut(&mut self) -> Result<&mut T, InvalidThreadAccess> {
        if self.is_valid() {
            Ok(self.with_value(|value| unsafe { &mut *value }))
        } else {
            Err(InvalidThreadAccess)
        }
    }
}

impl<T> From<T> for Sticky<T> {
    #[inline]
    fn from(t: T) -> Sticky<T> {
        Sticky::new(t)
    }
}

impl<T: Clone> Clone for Sticky<T> {
    #[inline]
    fn clone(&self) -> Sticky<T> {
        Sticky::new(self.get().clone())
    }
}

impl<T: Default> Default for Sticky<T> {
    #[inline]
    fn default() -> Sticky<T> {
        Sticky::new(T::default())
    }
}

impl<T: PartialEq> PartialEq for Sticky<T> {
    #[inline]
    fn eq(&self, other: &Sticky<T>) -> bool {
        *self.get() == *other.get()
    }
}

impl<T: Eq> Eq for Sticky<T> {}

impl<T: PartialOrd> PartialOrd for Sticky<T> {
    #[inline]
    fn partial_cmp(&self, other: &Sticky<T>) -> Option<cmp::Ordering> {
        self.get().partial_cmp(&*other.get())
    }

    #[inline]
    fn lt(&self, other: &Sticky<T>) -> bool {
        *self.get() < *other.get()
    }

    #[inline]
    fn le(&self, other: &Sticky<T>) -> bool {
        *self.get() <= *other.get()
    }

    #[inline]
    fn gt(&self, other: &Sticky<T>) -> bool {
        *self.get() > *other.get()
    }

    #[inline]
    fn ge(&self, other: &Sticky<T>) -> bool {
        *self.get() >= *other.get()
    }
}

impl<T: Ord> Ord for Sticky<T> {
    #[inline]
    fn cmp(&self, other: &Sticky<T>) -> cmp::Ordering {
        self.get().cmp(&*other.get())
    }
}

impl<T: fmt::Display> fmt::Display for Sticky<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt::Display::fmt(self.get(), f)
    }
}

impl<T: fmt::Debug> fmt::Debug for Sticky<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self.try_get() {
            Ok(value) => f.debug_struct("Sticky").field("value", value).finish(),
            Err(..) => {
                struct InvalidPlaceholder;
                impl fmt::Debug for InvalidPlaceholder {
                    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str("<invalid thread>")
                    }
                }

                f.debug_struct("Sticky")
                    .field("value", &InvalidPlaceholder)
                    .finish()
            }
        }
    }
}

// similar as for fragile ths type is sync because it only accesses TLS data
// which is thread local.  There is nothing that needs to be synchronized.
unsafe impl<T> Sync for Sticky<T> {}

// The entire point of this type is to be Send
unsafe impl<T> Send for Sticky<T> {}

#[test]
fn test_basic() {
    use std::thread;
    let val = Sticky::new(true);
    assert_eq!(val.to_string(), "true");
    assert_eq!(val.get(), &true);
    assert!(val.try_get().is_ok());
    thread::spawn(move || {
        assert!(val.try_get().is_err());
    })
    .join()
    .unwrap();
}

#[test]
fn test_mut() {
    let mut val = Sticky::new(true);
    *val.get_mut() = false;
    assert_eq!(val.to_string(), "false");
    assert_eq!(val.get(), &false);
}

#[test]
#[should_panic]
fn test_access_other_thread() {
    use std::thread;
    let val = Sticky::new(true);
    thread::spawn(move || {
        val.get();
    })
    .join()
    .unwrap();
}

#[test]
fn test_drop_same_thread() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    let was_called = Arc::new(AtomicBool::new(false));
    struct X(Arc<AtomicBool>);
    impl Drop for X {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let val = Sticky::new(X(was_called.clone()));
    mem::drop(val);
    assert!(was_called.load(Ordering::SeqCst));
}

#[test]
fn test_noop_drop_elsewhere() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;

    let was_called = Arc::new(AtomicBool::new(false));

    {
        let was_called = was_called.clone();
        thread::spawn(move || {
            struct X(Arc<AtomicBool>);
            impl Drop for X {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }

            let val = Sticky::new(X(was_called.clone()));
            assert!(thread::spawn(move || {
                // moves it here but do not deallocate
                val.try_get().ok();
            })
            .join()
            .is_ok());

            assert!(!was_called.load(Ordering::SeqCst));
        })
        .join()
        .unwrap();
    }

    assert!(was_called.load(Ordering::SeqCst));
}

#[test]
fn test_rc_sending() {
    use std::rc::Rc;
    use std::thread;
    let val = Sticky::new(Rc::new(true));
    thread::spawn(move || {
        assert!(val.try_get().is_err());
    })
    .join()
    .unwrap();
}

#[test]
fn test_two_stickies() {
    struct Wat;

    impl Drop for Wat {
        fn drop(&mut self) {
            // do nothing
        }
    }

    let s1 = Sticky::new(Wat);
    let s2 = Sticky::new(Wat);

    // make sure all is well

    drop(s1);
    drop(s2);
}
