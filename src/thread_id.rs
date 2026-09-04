//! Process-unique thread identifiers that remain accessible during thread
//! local storage teardown.
//!
//! # Why not `std::thread::current().id()`?
//!
//! Version 2.1.0 switched from a counter like this one to the standard
//! library's [`ThreadId`](std::thread::ThreadId).  That turned out to be a
//! mistake: the only stable way to obtain it is `thread::current().id()`, and
//! `thread::current()` panics once the thread's own handle has been torn down.
//! Before Rust 1.84 the handle lived in a regular thread local with a
//! destructor, so it could be gone while other thread local destructors were
//! still running.  Since 1.84 it is destroyed after all other thread locals,
//! but it still panics after that point.
//!
//! The wrapper types call the thread check from their `Drop` impls, and those
//! regularly run from thread local destructors: a `Fragile` or `Sticky` stored
//! in a `thread_local!`, or a value released by the sticky registry itself.  A
//! panic there is a panic inside a TLS destructor, which aborts the process.
//! `thread::current_id()` would avoid the handle, but it is unstable.
//!
//! So every thread lazily draws an identifier from a global counter and caches
//! it in a thread local *without* a destructor.  The standard library never
//! tears such a value down, so the check keeps working for the entire lifetime
//! of the thread.  This is also cheaper than `thread::current()`, which clones
//! and drops an `Arc` on every call.
//!
//! Uniqueness is the same as for `ThreadId`: the counter is monotonic and
//! never reused, and exhausting it panics rather than wrapping.  It is `usize`
//! wide because `AtomicU64` is unavailable on some 32-bit targets and
//! `cfg(target_has_atomic)` requires Rust 1.60.
//!
//! One caveat: on targets without native `#[thread_local]` support the
//! standard library backs `thread_local!` with OS keys, which can be destroyed
//! and re-initialized during teardown.  A thread may then observe a fresh
//! identifier late in its shutdown, which merely makes its own values look like
//! they belong to another thread.  Two threads can never share an identifier.
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A process-unique identifier for a thread.
///
/// Identifiers are never reused, even after a thread has terminated.  The
/// counter is `usize` wide so that it can be updated atomically on every
/// target; exhausting it panics instead of wrapping around.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) struct ThreadId(NonZeroUsize);

#[cold]
fn next() -> ThreadId {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    let mut last = COUNTER.load(Ordering::Relaxed);
    loop {
        let id = match last.checked_add(1) {
            Some(id) => id,
            None => panic!("failed to generate unique thread ID: bitspace exhausted"),
        };
        match COUNTER.compare_exchange_weak(last, id, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return ThreadId(NonZeroUsize::new(id).unwrap()),
            Err(actual) => last = actual,
        }
    }
}

/// Returns the identifier of the calling thread.
#[inline]
pub(crate) fn current() -> ThreadId {
    // The stored value has no drop glue, so no destructor is registered and
    // the value stays readable for the entire lifetime of the thread.
    thread_local!(static THREAD_ID: ThreadId = next());
    THREAD_ID.with(|&id| id)
}

#[test]
fn test_unique_per_thread() {
    use std::thread;

    let main = current();
    assert_eq!(main, current());

    let other = thread::spawn(current).join().unwrap();
    assert_ne!(main, other);

    // Identifiers are never reused after a thread has terminated.
    let another = thread::spawn(current).join().unwrap();
    assert_ne!(other, another);
    assert_eq!(main, current());
}
