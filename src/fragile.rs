use std::cmp;
use std::fmt;
use std::mem;
use std::mem::ManuallyDrop;
#[cfg(feature = "future")]
use std::pin::Pin;

use crate::errors::InvalidThreadAccess;
use crate::thread_id::{self, ThreadId};

/// The value starts inline and is moved to stable storage before the first
/// pinned projection.
enum FragileValue<T> {
    Inline(T),
    #[cfg(feature = "future")]
    Pinned(Pin<Box<T>>),
    #[cfg(feature = "future")]
    Taken,
}

/// A [`Fragile<T>`] wraps a non sendable `T` to be safely send to other threads.
///
/// Once the value has been wrapped it can be sent to other threads but access
/// to the value on those threads will fail.
///
/// If the value needs destruction and the fragile wrapper is on another thread
/// the destructor will panic.  Alternatively you can use
/// [`Sticky`](crate::Sticky), which does not panic and instead retains the
/// value until the originating thread exits.
///
/// Polling a `Fragile` as a `Future` or `Stream` moves its value to stable heap
/// storage before the first poll. If a polled `Fragile` with drop glue is dropped
/// on another thread, that storage is leaked to preserve the value's pinning
/// invariant. A `Fragile<T>` is `Unpin` only if `T` is `Unpin`.
pub struct Fragile<T> {
    // ManuallyDrop is necessary because we need to move out of here without running the
    // Drop code in functions like `into_inner`, and to leak pinned storage when dropped
    // on the wrong thread.
    value: ManuallyDrop<FragileValue<T>>,
    // Thread IDs are unique for the duration of the process and stay readable
    // during thread local storage teardown.
    thread_id: ThreadId,
}

impl<T> Fragile<T> {
    /// Creates a new [`Fragile`] wrapping a `value`.
    ///
    /// The value that is moved into the [`Fragile`] can be non `Send` and
    /// will be anchored to the thread that created the object.  If the
    /// fragile wrapper type ends up being send from thread to thread
    /// only the original thread can interact with the value.
    pub fn new(value: T) -> Self {
        Fragile {
            value: ManuallyDrop::new(FragileValue::Inline(value)),
            thread_id: thread_id::current(),
        }
    }

    /// Returns `true` if the access is valid.
    ///
    /// This will be `false` if the value was sent to another thread.
    pub fn is_valid(&self) -> bool {
        thread_id::current() == self.thread_id
    }

    #[inline(always)]
    #[track_caller]
    fn assert_thread(&self) {
        if !self.is_valid() {
            panic!("trying to access wrapped value in fragile container from incorrect thread.");
        }
    }

    fn value(&self) -> &T {
        match &*self.value {
            FragileValue::Inline(value) => value,
            #[cfg(feature = "future")]
            FragileValue::Pinned(value) => value.as_ref().get_ref(),
            #[cfg(feature = "future")]
            FragileValue::Taken => unreachable!("fragile value is missing"),
        }
    }

    fn value_mut(&mut self) -> &mut T {
        match &mut *self.value {
            FragileValue::Inline(value) => value,
            // SAFETY: Safe code cannot obtain `&mut Fragile<T>` after the
            // wrapper has been pinned unless `T` is `Unpin`.
            #[cfg(feature = "future")]
            FragileValue::Pinned(value) => unsafe { value.as_mut().get_unchecked_mut() },
            #[cfg(feature = "future")]
            FragileValue::Taken => unreachable!("fragile value is missing"),
        }
    }

    #[cfg(feature = "future")]
    #[track_caller]
    pub(crate) fn get_pin_mut(self: Pin<&mut Self>) -> Pin<&mut T> {
        // SAFETY: We do not move the wrapper. The inline value has never been
        // exposed through a pinned projection and may be moved to stable
        // storage; an already-pinned value is never moved.
        let this = unsafe { self.get_unchecked_mut() };
        this.assert_thread();

        if matches!(&*this.value, FragileValue::Inline(_)) {
            // Move the value to stable storage before projecting it as pinned.
            let value = match mem::replace(&mut *this.value, FragileValue::Taken) {
                FragileValue::Inline(value) => value,
                _ => unreachable!(),
            };
            *this.value = FragileValue::Pinned(Box::pin(value));
        }

        match &mut *this.value {
            FragileValue::Pinned(value) => value.as_mut(),
            _ => unreachable!(),
        }
    }

    /// Consumes the `Fragile`, returning the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if called from a different thread than the one where the
    /// original value was created.
    #[track_caller]
    pub fn into_inner(self) -> T {
        self.assert_thread();

        let mut this = ManuallyDrop::new(self);

        // SAFETY: `this` is not accessed beyond this point, and because it's in a ManuallyDrop its
        // destructor is not run.
        match unsafe { ManuallyDrop::take(&mut this.value) } {
            FragileValue::Inline(value) => value,
            #[cfg(feature = "future")]
            FragileValue::Pinned(value) => {
                // SAFETY: Safe code can only regain ownership of a previously
                // pinned wrapper if `T` is `Unpin`.
                *unsafe { Pin::into_inner_unchecked(value) }
            }
            #[cfg(feature = "future")]
            FragileValue::Taken => unreachable!("fragile value is missing"),
        }
    }

    /// Consumes the `Fragile`, returning the wrapped value if successful.
    ///
    /// The wrapped value is returned if this is called from the same thread
    /// as the one where the original value was created, otherwise the
    /// [`Fragile`] is returned as `Err(self)`.
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
    /// For a non-panicking variant, use [`try_get`](Self::try_get).
    #[track_caller]
    pub fn get(&self) -> &T {
        self.assert_thread();
        self.value()
    }

    /// Mutably borrows the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_get_mut`](Self::try_get_mut).
    #[track_caller]
    pub fn get_mut(&mut self) -> &mut T {
        self.assert_thread();
        self.value_mut()
    }

    /// Tries to immutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get(&self) -> Result<&T, InvalidThreadAccess> {
        if self.is_valid() {
            Ok(self.value())
        } else {
            Err(InvalidThreadAccess)
        }
    }

    /// Tries to mutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get_mut(&mut self) -> Result<&mut T, InvalidThreadAccess> {
        if self.is_valid() {
            Ok(self.value_mut())
        } else {
            Err(InvalidThreadAccess)
        }
    }
}

impl<T> Drop for Fragile<T> {
    #[track_caller]
    fn drop(&mut self) {
        // Values without drop glue can be released on any thread, so the
        // thread check is skipped for them.
        if !mem::needs_drop::<T>() || self.is_valid() {
            // SAFETY: `ManuallyDrop::drop` cannot be called after this point.
            unsafe { ManuallyDrop::drop(&mut self.value) };
        } else {
            // If the value was pinned, leaving the `Pin<Box<T>>` in
            // `ManuallyDrop` also keeps its storage alive. This is necessary
            // because the destructor cannot run on this thread.
            panic!("destructor of fragile object ran on wrong thread");
        }
    }
}

impl<T> From<T> for Fragile<T> {
    #[inline]
    fn from(t: T) -> Fragile<T> {
        Fragile::new(t)
    }
}

impl<T: Clone> Clone for Fragile<T> {
    #[inline]
    #[track_caller]
    fn clone(&self) -> Fragile<T> {
        Fragile::new(self.get().clone())
    }
}

impl<T: Default> Default for Fragile<T> {
    #[inline]
    fn default() -> Fragile<T> {
        Fragile::new(T::default())
    }
}

impl<T: PartialEq> PartialEq for Fragile<T> {
    #[inline]
    #[track_caller]
    fn eq(&self, other: &Fragile<T>) -> bool {
        *self.get() == *other.get()
    }
}

impl<T: Eq> Eq for Fragile<T> {}

impl<T: PartialOrd> PartialOrd for Fragile<T> {
    #[inline]
    #[track_caller]
    fn partial_cmp(&self, other: &Fragile<T>) -> Option<cmp::Ordering> {
        self.get().partial_cmp(other.get())
    }

    #[inline]
    #[track_caller]
    fn lt(&self, other: &Fragile<T>) -> bool {
        *self.get() < *other.get()
    }

    #[inline]
    #[track_caller]
    fn le(&self, other: &Fragile<T>) -> bool {
        *self.get() <= *other.get()
    }

    #[inline]
    #[track_caller]
    fn gt(&self, other: &Fragile<T>) -> bool {
        *self.get() > *other.get()
    }

    #[inline]
    #[track_caller]
    fn ge(&self, other: &Fragile<T>) -> bool {
        *self.get() >= *other.get()
    }
}

impl<T: Ord> Ord for Fragile<T> {
    #[inline]
    #[track_caller]
    fn cmp(&self, other: &Fragile<T>) -> cmp::Ordering {
        self.get().cmp(other.get())
    }
}

impl<T: fmt::Display> fmt::Display for Fragile<T> {
    #[track_caller]
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        fmt::Display::fmt(self.get(), f)
    }
}

impl<T: fmt::Debug> fmt::Debug for Fragile<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self.try_get() {
            Ok(value) => f.debug_struct("Fragile").field("value", value).finish(),
            Err(..) => {
                struct InvalidPlaceholder;
                impl fmt::Debug for InvalidPlaceholder {
                    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                        f.write_str("<invalid thread>")
                    }
                }

                f.debug_struct("Fragile")
                    .field("value", &InvalidPlaceholder)
                    .finish()
            }
        }
    }
}

// SAFETY: The inner value can only be accessed on its originating thread.
// Shared operations on every other thread inspect only the thread ID and fail.
unsafe impl<T> Sync for Fragile<T> {}

// SAFETY: Moving the wrapper never accesses the inner value. Wrong-thread
// destruction does not run `T`'s destructor and preserves pinned storage.
#[allow(clippy::non_send_fields_in_send_ty)]
unsafe impl<T> Send for Fragile<T> {}

#[test]
fn test_basic() {
    use std::thread;
    let val = Fragile::new(true);
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
    let mut val = Fragile::new(true);
    *val.get_mut() = false;
    assert_eq!(val.to_string(), "false");
    assert_eq!(val.get(), &false);
}

#[test]
#[should_panic]
fn test_access_other_thread() {
    use std::thread;
    let val = Fragile::new(true);
    thread::spawn(move || {
        val.get();
    })
    .join()
    .unwrap();
}

#[test]
fn test_noop_drop_elsewhere() {
    use std::thread;
    let val = Fragile::new(true);
    thread::spawn(move || {
        // force the move
        val.try_get().ok();
    })
    .join()
    .unwrap();
}

#[test]
fn test_panic_on_drop_elsewhere() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    let was_called = Arc::new(AtomicBool::new(false));
    struct X(Arc<AtomicBool>);
    impl Drop for X {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let val = Fragile::new(X(was_called.clone()));
    assert!(thread::spawn(move || {
        val.try_get().ok();
    })
    .join()
    .is_err());
    assert!(!was_called.load(Ordering::SeqCst));
}

#[test]
fn test_drop_in_tls_destructor() {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    struct X(Arc<AtomicUsize>);

    impl Drop for X {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    thread_local!(static SLOT: RefCell<Option<Fragile<X>>> = RefCell::new(None));

    let drop_count = Arc::new(AtomicUsize::new(0));
    let thread_drop_count = drop_count.clone();

    // The thread check in the destructor must keep working while the thread
    // is destroying its thread local storage.
    thread::spawn(move || {
        SLOT.with(|slot| *slot.borrow_mut() = Some(Fragile::new(X(thread_drop_count))));
    })
    .join()
    .unwrap();

    assert_eq!(drop_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_rc_sending() {
    use std::rc::Rc;
    use std::sync::mpsc::channel;
    use std::thread;

    let val = Fragile::new(Rc::new(true));
    let (tx, rx) = channel();

    let thread = thread::spawn(move || {
        assert!(val.try_get().is_err());
        let here = val;
        tx.send(here).unwrap();
    });

    let rv = rx.recv().unwrap();
    assert!(**rv.get());

    thread.join().unwrap();
}
