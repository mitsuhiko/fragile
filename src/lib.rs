//! This library provides wrapper types that permit sending non `Send` types to
//! other threads and use runtime checks to ensure safety.
//!
//! The types provided by this crate wrap a value and provide a `Send` bound.  None of
//! the types permit access to the enclosed value unless the thread that wrapped the
//! value is attempting to access it.  In the 1.2 release family only [`Fragile`] is
//! sound to use.
//!
//! A [`Fragile`] will actually send the `T` from thread to thread but will only
//! permit the original thread to invoke the destructor.  If the value gets dropped
//! in a different thread, the destructor will panic.
//!
//! # Example
//!
//! ```
//! use std::thread;
//! use fragile::Fragile;
//!
//! // creating and using a fragile object in the same thread works
//! let val = Fragile::new(true);
//! assert_eq!(*val.get(), true);
//! assert!(val.try_get().is_ok());
//!
//! // once send to another thread it stops working
//! thread::spawn(move || {
//!     assert!(val.try_get().is_err());
//! }).join()
//!     .unwrap();
//! ```
//!
//! # Why?
//!
//! Most of the time trying to use this crate is going to indicate some code smell.  But
//! there are situations where this is useful.  For instance you might have a bunch of
//! non `Send` types but want to work with a `Send` error type.  In that case the non
//! sendable extra information can be contained within the error and in cases where the
//! error did not cross a thread boundary yet extra information can be obtained.
//!
//! # Drop / Cleanup Behavior
//!
//! All types will try to eagerly drop a value if they are dropped on the right thread.
//! [`Sticky`] and [`SemiSticky`] will however permanently leak memory until the program
//! shuts down if the value is dropped on the wrong thread.  This limitation does not
//! exist on the 2.x release family of fragile.
//!
//! # Features
//!
//! By default the crate has no dependencies.  Optionally the `slab` feature can
//! be enabled which optimizes the internal storage of the [`Sticky`] type to
//! make it use a [`slab`](https://docs.rs/slab/latest/slab/) instead.
use std::cmp;
use std::fmt;
use std::mem;

pub use new_fragile::{Fragile, InvalidThreadAccess};

/// A memory leaking semi sticky type.
///
/// This this does not work correctly in the 1.2 release family due to a soundness
/// issue.  Do not use it, upgrade to fragile 2.x.  In the 1.2 release family this
/// type is now emulating the behavior but inherently leaks memory if the drop happens
/// on a different thread.
#[deprecated = "this type is broken on fragile 1.2.  Please upgrade to 2.x"]
pub type SemiSticky<T> = Sticky<T>;

/// A memory leaking sticky type.
///
/// This this does not work correctly in the 1.2 release family due to a soundness
/// issue.  Do not use it, upgrade to fragile 2.x.  In the 1.2 release family this
/// type is now emulating the behavior but inherently leaks memory if the drop happens
/// on a different thread.
#[deprecated = "this type is broken on fragile 1.2.  Please upgrade to 2.x"]
pub struct Sticky<T: 'static> {
    inner: Option<Fragile<T>>,
}

impl<T> Drop for Sticky<T> {
    fn drop(&mut self) {
        // if the type needs dropping we can only do so on the
        // right thread.  worst case we leak the value until the
        // thread dies.
        if mem::needs_drop::<T>() && !self.is_valid() {
            if let Some(val) = self.inner.take() {
                std::mem::forget(val);
            }
        }
    }
}

impl<T> Sticky<T> {
    /// Creates a new [`Sticky`] wrapping a `value`.
    ///
    /// The value that is moved into the [`Sticky`] can be non `Send` and
    /// will be anchored to the thread that created the object.  If the
    /// sticky wrapper type ends up being send from thread to thread
    /// only the original thread can interact with the value.
    pub fn new(value: T) -> Self {
        Sticky {
            inner: Some(Fragile::new(value)),
        }
    }

    /// Returns `true` if the access is valid.
    ///
    /// This will be `false` if the value was sent to another thread.
    #[inline(always)]
    pub fn is_valid(&self) -> bool {
        self.inner.as_ref().unwrap().is_valid()
    }

    /// Consumes the `Sticky`, returning the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if called from a different thread than the one where the
    /// original value was created.
    pub fn into_inner(mut self) -> T {
        self.inner.take().unwrap().into_inner()
    }

    /// Consumes the `Sticky`, returning the wrapped value if successful.
    ///
    /// The wrapped value is returned if this is called from the same thread
    /// as the one where the original value was created, otherwise the
    /// `Sticky` is returned as `Err(self)`.
    pub fn try_into_inner(mut self) -> Result<T, Self> {
        self.inner
            .take()
            .unwrap()
            .try_into_inner()
            .map_err(|inner| Sticky { inner: Some(inner) })
    }

    /// Immutably borrows the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_get`](#method.try_get`).
    pub fn get(&self) -> &T {
        self.inner.as_ref().unwrap().get()
    }

    /// Mutably borrows the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_get_mut`](#method.try_get_mut`).
    pub fn get_mut(&mut self) -> &mut T {
        self.inner.as_mut().unwrap().get_mut()
    }

    /// Tries to immutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get(&self) -> Result<&T, InvalidThreadAccess> {
        self.inner.as_ref().unwrap().try_get()
    }

    /// Tries to mutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get_mut(&mut self) -> Result<&mut T, InvalidThreadAccess> {
        self.inner.as_mut().unwrap().try_get_mut()
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
        self.get().partial_cmp(other.get())
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
        self.get().cmp(other.get())
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
#[should_panic = "assertion failed: was_called.load"]
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
