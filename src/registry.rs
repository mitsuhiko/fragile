pub struct Entry {
    /// The pointer to the object stored in the registry. This is a type-erased
    /// `Box<T>`.
    pub ptr: *mut (),
    /// The function that can be called on the above pointer to drop the object
    /// and free its allocation.
    pub drop: unsafe fn(*mut ()),
}

#[cfg(feature = "slab")]
mod slab_impl {
    use std::cell::UnsafeCell;
    use std::num::NonZeroUsize;

    use super::Entry;

    struct Registry(slab::Slab<Entry>);

    impl Drop for Registry {
        fn drop(&mut self) {
            for (_, value) in self.0.iter() {
                // SAFETY: This function is only called once, and is called with the
                // pointer it was created with.
                unsafe { (value.drop)(value.ptr) };
            }
        }
    }

    thread_local!(static REGISTRY: UnsafeCell<Registry> = UnsafeCell::new(Registry(slab::Slab::new())));

    pub use usize as ItemId;

    pub fn insert(thread_id: NonZeroUsize, entry: Entry) -> ItemId {
        let _ = thread_id;

        // SAFETY: The `REGISTRY` is not accessed recursively in this function.
        REGISTRY.with(|registry| unsafe { (*registry.get()).0.insert(entry) })
    }

    pub fn with<R, F: FnOnce(&Entry) -> R>(item_id: ItemId, thread_id: NonZeroUsize, f: F) -> R {
        let _ = thread_id;
        REGISTRY.with(|registry| f(unsafe { &*registry.get() }.0.get(item_id).unwrap()))
    }

    pub fn remove(item_id: ItemId, thread_id: NonZeroUsize) -> Entry {
        let _ = thread_id;
        REGISTRY.with(|registry| unsafe { (*registry.get()).0.remove(item_id) })
    }
}

#[cfg(not(feature = "slab"))]
mod map_impl {
    use std::cell::UnsafeCell;
    use std::num::NonZeroUsize;

    use super::Entry;

    struct Registry(std::collections::HashMap<NonZeroUsize, Entry>);

    impl Drop for Registry {
        fn drop(&mut self) {
            for (_, value) in self.0.iter() {
                // SAFETY: This function is only called once, and is called with the
                // pointer it was created with.
                unsafe { (value.drop)(value.ptr) };
            }
        }
    }

    thread_local!(static REGISTRY: UnsafeCell<Registry> = UnsafeCell::new(Registry(Default::default())));

    pub type ItemId = ();

    pub fn insert(thread_id: NonZeroUsize, entry: Entry) -> ItemId {
        // SAFETY: The `REGISTRY` is not accessed recursively in this function.
        REGISTRY.with(|registry| unsafe { (*registry.get()).0.insert(thread_id, entry) });
    }

    pub fn with<R, F: FnOnce(&Entry) -> R>(item_id: ItemId, thread_id: NonZeroUsize, f: F) -> R {
        let _ = item_id;
        REGISTRY.with(|registry| f(unsafe { &*registry.get() }.0.get(&thread_id).unwrap()))
    }

    pub fn remove(item_id: ItemId, thread_id: NonZeroUsize) -> Entry {
        let _ = item_id;
        REGISTRY.with(|registry| unsafe { (*registry.get()).0.remove(&thread_id).unwrap() })
    }
}

#[cfg(feature = "slab")]
pub use self::slab_impl::*;

#[cfg(not(feature = "slab"))]
pub use self::map_impl::*;
