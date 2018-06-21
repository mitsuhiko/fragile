use std::cell::UnsafeCell;
use std::cmp;
use std::fmt;
use std::mem;
use std::mem::ManuallyDrop;
use std::sync::atomic::{AtomicUsize, Ordering, ATOMIC_USIZE_INIT};

use errors::InvalidThreadAccess;

fn next_thread_id() -> usize {
    static mut COUNTER: AtomicUsize = ATOMIC_USIZE_INIT;
    unsafe { COUNTER.fetch_add(1, Ordering::SeqCst) }
}

pub(crate) fn get_thread_id() -> usize {
    thread_local!(static THREAD_ID: usize = next_thread_id());
    THREAD_ID.with(|&x| x)
}

/// A `Fragile<T>` wraps a non sendable `T` to be safely send to other threads.
///
/// Once the value has been wrapped it can be sent to other threads but access
/// to the value on those threads will fail.
///
/// If the value needs destruction and the fragile wrapper is on another thread
/// the destructor will panic.  Alternatively you can use `Sticky<T>` which is
/// not going to panic but might temporarily leak the value.
pub struct Fragile<T> {
    value: ManuallyDrop<UnsafeCell<Box<T>>>,
    thread_id: usize,
}

unsafe impl<T> Sync for Fragile<T> {}

impl<T> Fragile<T> {
    /// Creates a new `Fragile` wrapping a `value`.
    ///
    /// The value that is moved into the `Fragile` can be non `Send` and
    /// will be anchored to the thread that created the object.  If the
    /// fragile wrapper type ends up being send from thread to thread
    /// only the original thread can interact with the value.
    pub fn new(value: T) -> Self {
        Fragile {
            value: ManuallyDrop::new(UnsafeCell::new(Box::new(value))),
            thread_id: get_thread_id(),
        }
    }

    pub(crate) fn is_same_thread(&self) -> bool {
        get_thread_id() == self.thread_id
    }

    #[inline(always)]
    fn assert_thread(&self) {
        if !self.is_same_thread() {
            panic!("trying to access wrapped value in fragile container from incorrect thread.");
        }
    }

    /// Consumes the `Fragile`, returning the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if called from a different thread than the one where the
    /// original value was created.
    pub fn into_inner(mut self) -> T {
        self.assert_thread();
        unsafe {
            let value = mem::replace(&mut self.value, mem::uninitialized());
            mem::forget(self);
            *ManuallyDrop::into_inner(value).into_inner()
        }
    }

    /// Consumes the `Fragile`, returning the wrapped value if successful.
    ///
    /// The wrapped value is returned if this is called from the same thread
    /// as the one where the original value was created, otherwise the
    /// `Fragile` is returned as `Err(self)`.
    pub fn try_into_inner(self) -> Result<T, Self> {
        if get_thread_id() == self.thread_id {
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
        self.assert_thread();
        unsafe { &*self.value.get() }
    }

    /// Mutably borrows the wrapped value.
    ///
    /// # Panics
    ///
    /// Panics if the calling thread is not the one that wrapped the value.
    /// For a non-panicking variant, use [`try_get_mut`](#method.try_get_mut`).
    pub fn get_mut(&mut self) -> &mut T {
        self.assert_thread();
        unsafe { &mut *self.value.get() }
    }

    /// Tries to immutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get(&self) -> Result<&T, InvalidThreadAccess> {
        if get_thread_id() == self.thread_id {
            unsafe { Ok(&*self.value.get()) }
        } else {
            Err(InvalidThreadAccess)
        }
    }

    /// Tries to mutably borrow the wrapped value.
    ///
    /// Returns `None` if the calling thread is not the one that wrapped the value.
    pub fn try_get_mut(&mut self) -> Result<&mut T, InvalidThreadAccess> {
        if get_thread_id() == self.thread_id {
            unsafe { Ok(&mut *self.value.get()) }
        } else {
            Err(InvalidThreadAccess)
        }
    }
}

impl<T> Drop for Fragile<T> {
    fn drop(&mut self) {
        if mem::needs_drop::<T>() {
            if get_thread_id() == self.thread_id {
                unsafe { ManuallyDrop::drop(&mut self.value) }
            } else {
                panic!("destructor of fragile object ran on wrong thread");
            }
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
    fn eq(&self, other: &Fragile<T>) -> bool {
        *self.get() == *other.get()
    }
}

impl<T: Eq> Eq for Fragile<T> {}

impl<T: PartialOrd> PartialOrd for Fragile<T> {
    #[inline]
    fn partial_cmp(&self, other: &Fragile<T>) -> Option<cmp::Ordering> {
        self.get().partial_cmp(&*other.get())
    }

    #[inline]
    fn lt(&self, other: &Fragile<T>) -> bool {
        *self.get() < *other.get()
    }

    #[inline]
    fn le(&self, other: &Fragile<T>) -> bool {
        *self.get() <= *other.get()
    }

    #[inline]
    fn gt(&self, other: &Fragile<T>) -> bool {
        *self.get() > *other.get()
    }

    #[inline]
    fn ge(&self, other: &Fragile<T>) -> bool {
        *self.get() >= *other.get()
    }
}

impl<T: Ord> Ord for Fragile<T> {
    #[inline]
    fn cmp(&self, other: &Fragile<T>) -> cmp::Ordering {
        self.get().cmp(&*other.get())
    }
}

impl<T: fmt::Display> fmt::Display for Fragile<T> {
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
    }).join()
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
    }).join()
        .unwrap();
}

#[test]
fn test_noop_drop_elsewhere() {
    use std::thread;
    let val = Fragile::new(true);
    thread::spawn(move || {
        // force the move
        val.try_get().ok();
    }).join()
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
    assert!(
        thread::spawn(move || {
            val.try_get().ok();
        }).join()
            .is_err()
    );
    assert_eq!(was_called.load(Ordering::SeqCst), false);
}