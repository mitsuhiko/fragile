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
    use std::cell::RefCell;

    use super::Entry;

    pub struct Registry {
        pub entries: slab::Slab<Entry>,
    }

    thread_local!(static REGISTRY: RefCell<Registry> = RefCell::new(Registry {
        entries: slab::Slab::new(),
    }));

    pub use usize as ItemId;

    pub fn insert(entry: Entry) -> ItemId {
        REGISTRY.with(|registry| registry.borrow_mut().entries.insert(entry))
    }

    pub fn is_available() -> bool {
        REGISTRY.try_with(|_| ()).is_ok()
    }

    pub fn get(item_id: ItemId) -> *mut () {
        REGISTRY.with(|registry| registry.borrow().entries.get(item_id).unwrap().ptr)
    }

    pub fn try_remove(item_id: ItemId) -> Option<Entry> {
        REGISTRY
            .try_with(|registry| registry.borrow_mut().entries.try_remove(item_id))
            .ok()
            .flatten()
    }
}

#[cfg(not(feature = "slab"))]
mod map_impl {
    use std::cell::RefCell;
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::Entry;

    pub struct Registry {
        pub entries: std::collections::HashMap<NonZeroUsize, Entry>,
    }

    thread_local!(static REGISTRY: RefCell<Registry> = RefCell::new(Registry {
        entries: Default::default(),
    }));

    pub type ItemId = NonZeroUsize;

    fn next_item_id() -> NonZeroUsize {
        static COUNTER: AtomicUsize = AtomicUsize::new(1);
        let mut item_id = COUNTER.load(Ordering::Relaxed);
        loop {
            let next = item_id.checked_add(1).expect("more than usize::MAX items");
            match COUNTER.compare_exchange_weak(item_id, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => return NonZeroUsize::new(item_id).unwrap(),
                Err(actual) => item_id = actual,
            }
        }
    }

    pub fn insert(entry: Entry) -> ItemId {
        let item_id = next_item_id();
        REGISTRY.with(|registry| registry.borrow_mut().entries.insert(item_id, entry));
        item_id
    }

    pub fn is_available() -> bool {
        REGISTRY.try_with(|_| ()).is_ok()
    }

    pub fn get(item_id: ItemId) -> *mut () {
        REGISTRY.with(|registry| registry.borrow().entries.get(&item_id).unwrap().ptr)
    }

    pub fn try_remove(item_id: ItemId) -> Option<Entry> {
        REGISTRY
            .try_with(|registry| registry.borrow_mut().entries.remove(&item_id))
            .ok()
            .flatten()
    }
}

#[cfg(feature = "slab")]
pub use self::slab_impl::*;

#[cfg(not(feature = "slab"))]
pub use self::map_impl::*;

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
