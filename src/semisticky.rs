use std::cmp;
use std::fmt;
use std::mem;

use crate::errors::InvalidThreadAccess;
use crate::fragile::Fragile;
use crate::sticky::Sticky;

enum SemiStickyImpl<T: 'static> {
    Fragile(Box<Fragile<T>>),
    Sticky(Sticky<T>),
}

/// A [`SemiSticky<T>`] keeps a value T stored in a thread if it has a drop.
///
/// This is a combined version of [`Fragile`] and [`Sticky`].  If the type
/// does not have a drop it will effectively be a [`Fragile`], otherwise it
/// will be internally behave like a [`Sticky`].
///
/// This type requires `T: 'static` for the same reasons as [`Sticky`] and
/// exposes the value only through scoped callbacks. Like [`Sticky`], it is
/// [`Unpin`] only if `T` is `Unpin`.
pub struct SemiSticky<T: 'static> {
    inner: SemiStickyImpl<T>,
}

impl<T> SemiSticky<T> {
    /// Creates a new [`SemiSticky`] wrapping a `value`.
    ///
    /// The value that is moved into the `SemiSticky` can be non `Send` and
    /// will be anchored to the thread that created the object.  If the
    /// sticky wrapper type ends up being send from thread to thread
    /// only the original thread can interact with the value.  In case the
    /// value does not have `Drop` it will be stored in the [`Fragile`]
    /// instead.
    pub fn new(value: T) -> Self {
        SemiSticky {
            inner: if mem::needs_drop::<T>() {
                SemiStickyImpl::Sticky(Sticky::new(value))
            } else {
                SemiStickyImpl::Fragile(Box::new(Fragile::new(value)))
            },
        }
    }

    /// Returns `true` if the access is valid.
    ///
    /// This will be `false` if the value was sent to another thread.
    pub fn is_valid(&self) -> bool {
        match self.inner {
            SemiStickyImpl::Fragile(ref inner) => inner.is_valid(),
            SemiStickyImpl::Sticky(ref inner) => inner.is_valid(),
        }
    }

    /// Consumes the [`SemiSticky`], returning the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if called from a different thread than the one where the
    /// original value was created.
    #[track_caller]
    pub fn into_inner(self) -> T {
        match self.inner {
            SemiStickyImpl::Fragile(inner) => inner.into_inner(),
            SemiStickyImpl::Sticky(inner) => inner.into_inner(),
        }
    }

    /// Consumes the [`SemiSticky`], returning the wrapped value if successful.
    ///
    /// The wrapped value is returned if this is called from the same thread
    /// as the one where the original value was created, otherwise the
    /// [`SemiSticky`] is returned as `Err(self)`.
    pub fn try_into_inner(self) -> Result<T, Self> {
        match self.inner {
            SemiStickyImpl::Fragile(inner) => inner.try_into_inner().map_err(|inner| SemiSticky {
                inner: SemiStickyImpl::Fragile(Box::new(inner)),
            }),
            SemiStickyImpl::Sticky(inner) => inner.try_into_inner().map_err(|inner| SemiSticky {
                inner: SemiStickyImpl::Sticky(inner),
            }),
        }
    }

    /// Invokes a callback with a shared reference to the wrapped value.
    ///
    /// The callback may return owned data, but it cannot return a reference
    /// derived from the wrapped value.
    ///
    /// ```
    /// use fragile::SemiSticky;
    ///
    /// let value = SemiSticky::new(String::from("hello"));
    /// let length = value.with(String::len);
    /// assert_eq!(length, 5);
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_with`](Self::try_with).
    #[track_caller]
    pub fn with<R, F>(&self, f: F) -> R
    where
        F: for<'a> FnOnce(&'a T) -> R,
    {
        match self.inner {
            SemiStickyImpl::Fragile(ref inner) => f(inner.get()),
            SemiStickyImpl::Sticky(ref inner) => inner.with(f),
        }
    }

    /// Invokes a callback with an exclusive reference to the wrapped value.
    ///
    /// The callback may return owned data, but it cannot return a reference
    /// derived from the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_with_mut`](Self::try_with_mut).
    #[track_caller]
    pub fn with_mut<R, F>(&mut self, f: F) -> R
    where
        F: for<'a> FnOnce(&'a mut T) -> R,
    {
        match self.inner {
            SemiStickyImpl::Fragile(ref mut inner) => f(inner.get_mut()),
            SemiStickyImpl::Sticky(ref mut inner) => inner.with_mut(f),
        }
    }

    /// Invokes a callback with a shared reference to the wrapped value.
    ///
    /// As with [`with`](Self::with), the callback cannot return a reference
    /// derived from the wrapped value.
    ///
    /// Returns [`InvalidThreadAccess`] without invoking the callback if called
    /// from a thread other than the one that wrapped the value.
    pub fn try_with<R, F>(&self, f: F) -> Result<R, InvalidThreadAccess>
    where
        F: for<'a> FnOnce(&'a T) -> R,
    {
        if self.is_valid() {
            Ok(self.with(f))
        } else {
            Err(InvalidThreadAccess)
        }
    }

    /// Invokes a callback with an exclusive reference to the wrapped value.
    ///
    /// As with [`with_mut`](Self::with_mut), the callback cannot return a
    /// reference derived from the wrapped value.
    ///
    /// Returns [`InvalidThreadAccess`] without invoking the callback if called
    /// from a thread other than the one that wrapped the value.
    pub fn try_with_mut<R, F>(&mut self, f: F) -> Result<R, InvalidThreadAccess>
    where
        F: for<'a> FnOnce(&'a mut T) -> R,
    {
        if self.is_valid() {
            Ok(self.with_mut(f))
        } else {
            Err(InvalidThreadAccess)
        }
    }
}

impl<T> From<T> for SemiSticky<T> {
    #[inline]
    fn from(t: T) -> SemiSticky<T> {
        SemiSticky::new(t)
    }
}

impl<T: Clone> Clone for SemiSticky<T> {
    #[inline]
    #[track_caller]
    fn clone(&self) -> SemiSticky<T> {
        SemiSticky::new(self.with(Clone::clone))
    }
}

impl<T: Default> Default for SemiSticky<T> {
    #[inline]
    fn default() -> SemiSticky<T> {
        SemiSticky::new(T::default())
    }
}

impl<T: PartialEq> PartialEq for SemiSticky<T> {
    #[inline]
    #[track_caller]
    fn eq(&self, other: &SemiSticky<T>) -> bool {
        self.with(|this| other.with(|other| this == other))
    }
}

impl<T: Eq> Eq for SemiSticky<T> {}

impl<T: PartialOrd> PartialOrd for SemiSticky<T> {
    #[inline]
    #[track_caller]
    fn partial_cmp(&self, other: &SemiSticky<T>) -> Option<cmp::Ordering> {
        self.with(|this| other.with(|other| this.partial_cmp(other)))
    }

    #[inline]
    #[track_caller]
    fn lt(&self, other: &SemiSticky<T>) -> bool {
        self.with(|this| other.with(|other| this < other))
    }

    #[inline]
    #[track_caller]
    fn le(&self, other: &SemiSticky<T>) -> bool {
        self.with(|this| other.with(|other| this <= other))
    }

    #[inline]
    #[track_caller]
    fn gt(&self, other: &SemiSticky<T>) -> bool {
        self.with(|this| other.with(|other| this > other))
    }

    #[inline]
    #[track_caller]
    fn ge(&self, other: &SemiSticky<T>) -> bool {
        self.with(|this| other.with(|other| this >= other))
    }
}

impl<T: Ord> Ord for SemiSticky<T> {
    #[inline]
    #[track_caller]
    fn cmp(&self, other: &SemiSticky<T>) -> cmp::Ordering {
        self.with(|this| other.with(|other| this.cmp(other)))
    }
}

impl<T: fmt::Display> fmt::Display for SemiSticky<T> {
    #[track_caller]
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        self.with(|value| fmt::Display::fmt(value, f))
    }
}

impl<T: fmt::Debug> fmt::Debug for SemiSticky<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_valid() {
            self.with(|value| f.debug_struct("SemiSticky").field("value", value).finish())
        } else {
            struct InvalidPlaceholder;
            impl fmt::Debug for InvalidPlaceholder {
                fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                    f.write_str("<invalid thread>")
                }
            }

            f.debug_struct("SemiSticky")
                .field("value", &InvalidPlaceholder)
                .finish()
        }
    }
}

#[test]
fn test_basic() {
    use std::thread;
    let val = SemiSticky::new(true);
    assert_eq!(val.to_string(), "true");
    assert!(val.with(|value| *value));
    assert!(val.try_with(|value| *value).unwrap());
    thread::spawn(move || {
        assert!(val
            .try_with(|_| panic!("callback ran on the wrong thread"))
            .is_err());
    })
    .join()
    .unwrap();
}

#[test]
fn test_mut() {
    let mut val = SemiSticky::new(true);
    val.with_mut(|value| *value = false);
    assert_eq!(val.to_string(), "false");
    assert!(!val.with(|value| *value));
    assert!(val
        .try_with_mut(|value| {
            *value = true;
            *value
        })
        .unwrap());
}

#[test]
#[should_panic]
fn test_access_other_thread() {
    use std::thread;
    let mut val = SemiSticky::new(true);
    thread::spawn(move || {
        assert!(val
            .try_with_mut(|_| panic!("mutable callback ran on the wrong thread"))
            .is_err());
        val.with(|_| ());
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
    let val = SemiSticky::new(X(was_called.clone()));
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

            let val = SemiSticky::new(X(was_called.clone()));
            val.with(|_| ());
            assert!(thread::spawn(move || {
                // moves it here but do not deallocate
                val.try_with(|_| ()).ok();
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
    let val = SemiSticky::new(Rc::new(true));
    thread::spawn(move || {
        assert!(val
            .try_with(|_| panic!("callback ran on the wrong thread"))
            .is_err());
    })
    .join()
    .unwrap();
}
