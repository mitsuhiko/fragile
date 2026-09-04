pub struct Entry {
    /// The pointer to the object stored in the registry. This is a type-erased
    /// `Box<T>`.
    pub ptr: *mut (),
    /// The function that can be called on the above pointer to drop the object
    /// and free its allocation.
    pub drop: unsafe fn(*mut ()),
}

// Used for map entries and slab registry generations. Exhaustion must never
// wrap: identifiers can remain in wrappers after their registry is destroyed.
fn next_id() -> std::num::NonZeroUsize {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(1);
    let mut id = COUNTER.load(Ordering::Relaxed);
    loop {
        let next = id
            .checked_add(1)
            .expect("more than usize::MAX registry IDs");
        match COUNTER.compare_exchange_weak(id, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return std::num::NonZeroUsize::new(id).unwrap(),
            Err(actual) => id = actual,
        }
    }
}

#[cfg(feature = "slab")]
mod slab_impl {
    use std::cell::RefCell;
    use std::num::NonZeroUsize;

    use super::Entry;

    pub struct Registry {
        pub entries: slab::Slab<Entry>,
        generation: NonZeroUsize,
    }

    impl Registry {
        fn new() -> Self {
            Registry {
                entries: slab::Slab::new(),
                generation: super::next_id(),
            }
        }

        fn insert(&mut self, entry: Entry) -> ItemId {
            ItemId {
                index: self.entries.insert(entry),
                generation: self.generation,
            }
        }

        fn get(&self, item_id: ItemId) -> Option<*mut ()> {
            if item_id.generation != self.generation {
                return None;
            }
            self.entries.get(item_id.index).map(|entry| entry.ptr)
        }

        fn remove(&mut self, item_id: ItemId) -> Option<Entry> {
            if item_id.generation != self.generation {
                return None;
            }
            self.entries.try_remove(item_id.index)
        }
    }

    thread_local!(static REGISTRY: RefCell<Registry> = RefCell::new(Registry::new()));

    // OS-key TLS can be reinitialized after its destructor returns. A slot
    // index alone could then select a different allocation (and a different T)
    // in a new registry on the same thread. Thread identity is not sufficient.
    #[derive(Copy, Clone)]
    pub struct ItemId {
        index: usize,
        generation: NonZeroUsize,
    }

    pub fn insert(entry: Entry) -> ItemId {
        REGISTRY
            .try_with(|registry| registry.borrow_mut().insert(entry))
            .unwrap_or_else(|_| super::unavailable())
    }

    pub fn try_get(item_id: ItemId) -> Option<*mut ()> {
        REGISTRY
            .try_with(|registry| registry.borrow().get(item_id))
            .ok()
            .flatten()
    }

    pub fn try_remove(item_id: ItemId) -> Option<Entry> {
        REGISTRY
            .try_with(|registry| registry.borrow_mut().remove(item_id))
            .ok()
            .flatten()
    }

    #[cfg(test)]
    pub fn len() -> usize {
        REGISTRY.with(|registry| registry.borrow().entries.len())
    }

    #[test]
    fn test_registry_generation_rejects_stale_ids() {
        fn entry() -> Entry {
            Entry {
                ptr: Box::into_raw(Box::new(42u32)).cast(),
                drop: |ptr| {
                    // SAFETY: Each entry owns exactly this allocation.
                    drop(unsafe { Box::from_raw(ptr.cast::<u32>()) });
                },
            }
        }

        let mut old_registry = Registry::new();
        let old_id = old_registry.insert(entry());
        drop(old_registry);

        let mut new_registry = Registry::new();
        let new_id = new_registry.insert(entry());
        assert_eq!(old_id.index, new_id.index);
        assert_ne!(old_id.generation, new_id.generation);
        assert!(new_registry.get(old_id).is_none());
        assert!(new_registry.remove(old_id).is_none());
        assert!(new_registry.get(new_id).is_some());
        // The new entry is still owned and dropped by the new registry.
    }
}

#[cfg(not(feature = "slab"))]
mod map_impl {
    use std::cell::RefCell;
    use std::num::NonZeroUsize;

    use super::Entry;

    pub struct Registry {
        pub entries: std::collections::HashMap<NonZeroUsize, Entry>,
    }

    thread_local!(static REGISTRY: RefCell<Registry> = RefCell::new(Registry {
        entries: Default::default(),
    }));

    pub type ItemId = NonZeroUsize;

    pub fn insert(entry: Entry) -> ItemId {
        let item_id = super::next_id();
        REGISTRY
            .try_with(|registry| registry.borrow_mut().entries.insert(item_id, entry))
            .unwrap_or_else(|_| super::unavailable());
        item_id
    }

    pub fn try_get(item_id: ItemId) -> Option<*mut ()> {
        REGISTRY
            .try_with(|registry| {
                registry
                    .borrow()
                    .entries
                    .get(&item_id)
                    .map(|entry| entry.ptr)
            })
            .ok()
            .flatten()
    }

    pub fn try_remove(item_id: ItemId) -> Option<Entry> {
        REGISTRY
            .try_with(|registry| registry.borrow_mut().entries.remove(&item_id))
            .ok()
            .flatten()
    }

    #[cfg(test)]
    pub fn len() -> usize {
        REGISTRY.with(|registry| registry.borrow().entries.len())
    }
}

#[cfg(feature = "slab")]
pub use self::slab_impl::*;

#[cfg(not(feature = "slab"))]
pub use self::map_impl::*;

#[cold]
#[track_caller]
fn unavailable() -> ! {
    panic!("cannot create sticky container while the thread's local storage is being destroyed.");
}

impl Drop for Registry {
    fn drop(&mut self) {
        // Detach all entries before running user destructors so reentrant code
        // cannot alias the collection being iterated.
        let entries = std::mem::take(&mut self.entries);
        for (_, value) in entries.iter() {
            // SAFETY: This function is only called once, and is called with the
            // pointer it was created with. If a callback panics, the remaining
            // raw entries are leaked rather than deallocated without being dropped.
            unsafe { (value.drop)(value.ptr) };
        }
    }
}
