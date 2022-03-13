use std::sync::atomic::{AtomicUsize, Ordering};

fn next() -> usize {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

pub(crate) fn get() -> usize {
    thread_local!(static THREAD_ID: usize = next());
    THREAD_ID.with(|&x| x)
}
