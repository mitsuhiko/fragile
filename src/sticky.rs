use std::cmp;
use std::fmt;
use std::marker::{PhantomData, PhantomPinned};
use std::mem;

use crate::errors::InvalidThreadAccess;
use crate::registry;
use crate::thread_id::{self, ThreadId};

/// A [`Sticky<T>`] keeps a value T stored in a thread.
///
/// This type works similar in nature to [`Fragile`](crate::Fragile) and exposes a
/// similar interface.  The difference is that whereas [`Fragile`](crate::Fragile) has
/// its destructor called in the thread where the value was sent, a
/// [`Sticky`] that is moved to another thread will have the internal
/// destructor called when the originating thread tears down.
///
/// Because [`Sticky`] allows values to be kept alive for longer than the
/// [`Sticky`] itself, it requires all its contents to be `'static` for
/// soundness. Access to the value is limited to scoped callbacks so references
/// cannot outlive the callback or the originating thread.
///
/// As this uses TLS internally the general rules about the platform limitations
/// of destructors for TLS apply.
///
/// A `Sticky<T>` is [`Unpin`] only if `T` is `Unpin`. This ensures that once a
/// wrapped value has been pinned through the `Future` or `Stream`
/// implementation, it cannot later be moved out of the wrapper.
pub struct Sticky<T: 'static> {
    item_id: registry::ItemId,
    thread_id: ThreadId,
    _marker: PhantomData<*mut T>,
    _pin: PhantomPinned,
}

impl<T: Unpin> Unpin for Sticky<T> {}

impl<T> Drop for Sticky<T> {
    #[track_caller]
    fn drop(&mut self) {
        // The value and its allocation can only be released on the originating
        // thread while its registry is alive. Otherwise the registry retains
        // the entry until the thread dies, when it releases it itself.
        if self.is_valid() {
            // SAFETY: This is the originating thread and the wrapper is being
            // consumed, so no safe references to the entry can remain.
            unsafe { self.unsafe_drop_value() };
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
        let entry = registry::Entry {
            ptr: Box::into_raw(Box::new(value)).cast(),
            drop: |ptr| {
                let ptr = ptr.cast::<T>();
                // SAFETY: This callback will only be called once, with the
                // above pointer.
                drop(unsafe { Box::from_raw(ptr) });
            },
        };

        let thread_id = thread_id::current();
        let item_id = registry::insert(entry);

        Sticky {
            item_id,
            thread_id,
            _marker: PhantomData,
            _pin: PhantomPinned,
        }
    }

    #[inline(always)]
    #[track_caller]
    fn value_ptr(&self) -> *mut T {
        self.assert_thread();
        match registry::try_get(self.item_id) {
            Some(ptr) => ptr.cast::<T>(),
            None => registry_destroyed(),
        }
    }

    #[inline(always)]
    fn try_value_ptr(&self) -> Result<*mut T, InvalidThreadAccess> {
        if !self.is_on_thread() {
            return Err(InvalidThreadAccess);
        }
        match registry::try_get(self.item_id) {
            Some(ptr) => Ok(ptr.cast::<T>()),
            None => Err(InvalidThreadAccess),
        }
    }

    #[inline(always)]
    fn is_on_thread(&self) -> bool {
        thread_id::current() == self.thread_id
    }

    /// Returns `true` if the access is valid.
    ///
    /// This will be `false` if the value was sent to another thread.  It is
    /// also `false` on the originating thread once that thread has started
    /// destroying its thread local storage, for instance when a `Sticky` is
    /// accessed from a destructor that runs during thread shutdown.
    #[inline(always)]
    pub fn is_valid(&self) -> bool {
        self.is_on_thread() && registry::is_available()
    }

    #[inline(always)]
    #[track_caller]
    fn assert_thread(&self) {
        if !self.is_on_thread() {
            panic!("trying to access wrapped value in sticky container from incorrect thread.");
        }
    }

    /// Consumes the `Sticky`, returning the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if called from a different thread than the one where the
    /// original value was created, or if that thread is already destroying
    /// its thread local storage.
    #[track_caller]
    pub fn into_inner(mut self) -> T {
        self.assert_thread();
        // SAFETY: Ownership and the successful thread check ensure there are
        // no outstanding safe references when the registry entry is removed.
        unsafe {
            let rv = self.unsafe_take_value();
            mem::forget(self);
            rv
        }
    }

    unsafe fn unsafe_drop_value(&mut self) {
        // The registry is unavailable while its TLS destructor is running. In
        // that case it still owns the entry and will drop the value itself.
        if let Some(entry) = registry::try_remove(self.item_id) {
            // SAFETY: The callback is paired with this entry's pointer and the
            // removal ensures it can only be called once.
            unsafe { (entry.drop)(entry.ptr) };
        }
    }

    #[track_caller]
    unsafe fn unsafe_take_value(&mut self) -> T {
        let ptr = match registry::try_remove(self.item_id) {
            Some(entry) => entry.ptr.cast::<T>(),
            None => registry_destroyed(),
        };
        // SAFETY: This is the pointer created for `T` in `new`, and removing
        // the entry transfers its allocation to this call exactly once.
        unsafe { *Box::from_raw(ptr) }
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

    /// Invokes a callback with a shared reference to the wrapped value.
    ///
    /// The callback may return owned data, but it cannot return a reference
    /// derived from the wrapped value. This keeps every borrow within the
    /// lifetime of the originating thread.
    ///
    /// ```
    /// use fragile::Sticky;
    ///
    /// let value = Sticky::new(String::from("hello"));
    /// let length = value.with(String::len);
    /// assert_eq!(length, 5);
    /// ```
    ///
    /// Borrows also cannot be held across an asynchronous suspension point.
    /// Clone or otherwise create owned data before returning a future:
    ///
    /// ```
    /// use fragile::Sticky;
    ///
    /// let value = Sticky::new(String::from("hello"));
    /// let future = value.with(|value| {
    ///     let value = value.clone();
    ///     async move {
    ///         std::future::ready(()).await;
    ///         assert_eq!(value, "hello");
    ///     }
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value, or
    /// if that thread is already destroying its thread local storage.
    /// For a non-panicking variant, use [`try_with`](Self::try_with).
    #[track_caller]
    pub fn with<R, F>(&self, f: F) -> R
    where
        F: for<'a> FnOnce(&'a T) -> R,
    {
        // SAFETY: The registry entry owns a live `T`. The higher-ranked
        // callback bound prevents this reference from escaping the call.
        f(unsafe { &*self.value_ptr() })
    }

    /// Invokes a callback with an exclusive reference to the wrapped value.
    ///
    /// The callback may return owned data, but it cannot return a reference
    /// derived from the wrapped value.
    ///
    /// ```
    /// use fragile::Sticky;
    ///
    /// let mut value = Sticky::new(String::from("hello"));
    /// value.with_mut(|value| value.push_str(" world"));
    /// assert_eq!(value.with(Clone::clone), "hello world");
    /// ```
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value, or
    /// if that thread is already destroying its thread local storage.
    /// For a non-panicking variant, use [`try_with_mut`](Self::try_with_mut).
    #[track_caller]
    pub fn with_mut<R, F>(&mut self, f: F) -> R
    where
        F: for<'a> FnOnce(&'a mut T) -> R,
    {
        // SAFETY: The registry entry owns a live `T`, `self` is borrowed
        // exclusively, and the callback bound prevents the reference escaping.
        f(unsafe { &mut *self.value_ptr() })
    }

    /// Invokes a callback with a shared reference to the wrapped value.
    ///
    /// As with [`with`](Self::with), the callback cannot return a reference
    /// derived from the wrapped value.
    ///
    /// Returns [`InvalidThreadAccess`] without invoking the callback if
    /// [`is_valid`](Self::is_valid) is `false`, that is when called from a
    /// thread other than the one that wrapped the value or while that thread
    /// is destroying its thread local storage.
    pub fn try_with<R, F>(&self, f: F) -> Result<R, InvalidThreadAccess>
    where
        F: for<'a> FnOnce(&'a T) -> R,
    {
        let ptr = self.try_value_ptr()?;
        // SAFETY: See `with`.
        Ok(f(unsafe { &*ptr }))
    }

    /// Invokes a callback with an exclusive reference to the wrapped value.
    ///
    /// As with [`with_mut`](Self::with_mut), the callback cannot return a
    /// reference derived from the wrapped value.
    ///
    /// Returns [`InvalidThreadAccess`] without invoking the callback if
    /// [`is_valid`](Self::is_valid) is `false`, that is when called from a
    /// thread other than the one that wrapped the value or while that thread
    /// is destroying its thread local storage.
    pub fn try_with_mut<R, F>(&mut self, f: F) -> Result<R, InvalidThreadAccess>
    where
        F: for<'a> FnOnce(&'a mut T) -> R,
    {
        let ptr = self.try_value_ptr()?;
        // SAFETY: See `with_mut`.
        Ok(f(unsafe { &mut *ptr }))
    }
}

#[cold]
#[track_caller]
fn registry_destroyed() -> ! {
    panic!("trying to access wrapped value in sticky container while its thread's local storage is being destroyed.");
}

impl<T> From<T> for Sticky<T> {
    #[inline]
    fn from(t: T) -> Sticky<T> {
        Sticky::new(t)
    }
}

impl<T: Clone> Clone for Sticky<T> {
    #[inline]
    #[track_caller]
    fn clone(&self) -> Sticky<T> {
        Sticky::new(self.with(Clone::clone))
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
    #[track_caller]
    fn eq(&self, other: &Sticky<T>) -> bool {
        self.with(|this| other.with(|other| this == other))
    }
}

impl<T: Eq> Eq for Sticky<T> {}

impl<T: PartialOrd> PartialOrd for Sticky<T> {
    #[inline]
    #[track_caller]
    fn partial_cmp(&self, other: &Sticky<T>) -> Option<cmp::Ordering> {
        self.with(|this| other.with(|other| this.partial_cmp(other)))
    }

    #[inline]
    #[track_caller]
    fn lt(&self, other: &Sticky<T>) -> bool {
        self.with(|this| other.with(|other| this < other))
    }

    #[inline]
    #[track_caller]
    fn le(&self, other: &Sticky<T>) -> bool {
        self.with(|this| other.with(|other| this <= other))
    }

    #[inline]
    #[track_caller]
    fn gt(&self, other: &Sticky<T>) -> bool {
        self.with(|this| other.with(|other| this > other))
    }

    #[inline]
    #[track_caller]
    fn ge(&self, other: &Sticky<T>) -> bool {
        self.with(|this| other.with(|other| this >= other))
    }
}

impl<T: Ord> Ord for Sticky<T> {
    #[inline]
    #[track_caller]
    fn cmp(&self, other: &Sticky<T>) -> cmp::Ordering {
        self.with(|this| other.with(|other| this.cmp(other)))
    }
}

impl<T: fmt::Display> fmt::Display for Sticky<T> {
    #[track_caller]
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        self.with(|value| fmt::Display::fmt(value, f))
    }
}

impl<T: fmt::Debug> fmt::Debug for Sticky<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        if self.is_valid() {
            self.with(|value| f.debug_struct("Sticky").field("value", value).finish())
        } else {
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

// SAFETY: The inner value remains in its originating thread's registry and can
// only be accessed there. Other threads inspect only the stored thread ID.
unsafe impl<T> Sync for Sticky<T> {}

// SAFETY: Moving the wrapper moves only registry metadata. The inner value is
// accessed and destroyed exclusively on its originating thread.
unsafe impl<T> Send for Sticky<T> {}

#[test]
fn test_basic() {
    use std::thread;
    let val = Sticky::new(true);
    assert_eq!(val.to_string(), "true");
    assert!(val.with(|value| *value));
    assert!(val.try_with(|value| *value).unwrap());
    let external = String::from("external");
    assert_eq!(val.with(|_| external.as_str()), "external");
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
    let mut val = Sticky::new(true);
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
    let mut val = Sticky::new(true);
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
    let val = Sticky::new(Rc::new(true));
    thread::spawn(move || {
        assert!(val
            .try_with(|_| panic!("callback ran on the wrong thread"))
            .is_err());
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

#[test]
fn test_registry_mutation_during_callback() {
    let first = Sticky::new(String::from("first"));
    let second = Sticky::new(String::from("second"));

    first.with(|first| {
        drop(second);
        let third = Sticky::new(String::from("third"));
        assert_eq!(third.with(|value| value.clone()), "third");
        assert_eq!(first, "first");
    });
}

#[test]
fn test_thread_spawn() {
    use crate::Sticky;
    use std::{mem::ManuallyDrop, thread};

    let dummy_sticky = thread::spawn(|| Sticky::new(())).join().unwrap();
    let sticky_string = ManuallyDrop::new(Sticky::new(String::from("Hello World")));

    sticky_string.with(|hello| {
        assert_eq!(hello, "Hello World");
        drop(dummy_sticky);
        assert_eq!(hello, "Hello World");
    });
}

#[test]
fn test_no_drop_glue_released_eagerly() {
    assert!(!mem::needs_drop::<u32>());
    let before = registry::len();
    for value in 0..64u32 {
        let sticky = Sticky::new(value);
        assert_eq!(registry::len(), before + 1);
        assert_eq!(sticky.with(|value| *value), value);
    }
    assert_eq!(registry::len(), before);

    let sticky = Sticky::new(1u32);
    assert_eq!(sticky.into_inner(), 1);
    assert_eq!(registry::len(), before);
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

    thread_local!(static SLOT: RefCell<Option<Sticky<X>>> = RefCell::new(None));

    let drop_count = Arc::new(AtomicUsize::new(0));
    let thread_drop_count = drop_count.clone();

    // Dropping the wrapper from a thread local destructor must neither panic
    // nor abort, regardless of the order in which destructors run.
    thread::spawn(move || {
        SLOT.with(|slot| *slot.borrow_mut() = Some(Sticky::new(X(thread_drop_count))));
    })
    .join()
    .unwrap();

    assert_eq!(drop_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_access_during_registry_teardown() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    struct Outcome {
        errors: AtomicUsize,
        dropped: AtomicUsize,
    }

    struct AccessesSibling {
        sibling: Sticky<String>,
        outcome: Arc<Outcome>,
    }

    impl Drop for AccessesSibling {
        fn drop(&mut self) {
            // This runs from the registry destructor during thread shutdown.
            // Non-panicking accessors must report an error instead of
            // panicking, which would abort the process.
            if !self.sibling.is_valid() {
                self.outcome.errors.fetch_add(1, Ordering::SeqCst);
            }
            if self.sibling.try_with(|value| value.len()).is_err() {
                self.outcome.errors.fetch_add(1, Ordering::SeqCst);
            }
            if self.sibling.try_with_mut(|value| value.len()).is_err() {
                self.outcome.errors.fetch_add(1, Ordering::SeqCst);
            }
            if format!("{:?}", self.sibling).contains("<invalid thread>") {
                self.outcome.errors.fetch_add(1, Ordering::SeqCst);
            }
            self.outcome.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    let outcome = Arc::new(Outcome {
        errors: AtomicUsize::new(0),
        dropped: AtomicUsize::new(0),
    });
    let thread_outcome = outcome.clone();

    thread::spawn(move || {
        let value = Sticky::new(AccessesSibling {
            sibling: Sticky::new(String::from("sibling")),
            outcome: thread_outcome,
        });
        // Move the wrapper away so the value is only released by the
        // registry when the originating thread shuts down.
        thread::spawn(move || drop(value)).join().unwrap();
    })
    .join()
    .unwrap();

    assert_eq!(outcome.errors.load(Ordering::SeqCst), 4);
    assert_eq!(outcome.dropped.load(Ordering::SeqCst), 1);
}

#[test]
fn test_nested_sticky_drop_at_thread_exit() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::thread;

    let drop_count = Arc::new(AtomicUsize::new(0));
    let thread_drop_count = drop_count.clone();

    thread::spawn(move || {
        struct X(Arc<AtomicUsize>);

        impl Drop for X {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let outer = Sticky::new(Sticky::new(X(thread_drop_count)));
        thread::spawn(move || drop(outer)).join().unwrap();
    })
    .join()
    .unwrap();

    assert_eq!(drop_count.load(Ordering::SeqCst), 1);
}
