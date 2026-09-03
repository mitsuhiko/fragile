use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::{stack_token, Fragile, SemiSticky, Sticky};

impl<F: Future> Future for Fragile<F> {
    type Output = F::Output;

    #[track_caller]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.get_pin_mut().poll(cx)
    }
}

impl<F: Future> Future for Sticky<F> {
    type Output = F::Output;

    #[track_caller]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        stack_token!(tok);
        // SAFETY: We do not move the wrapper through this mutable reference.
        let this = unsafe { self.as_mut().get_unchecked_mut() };
        let inner = Sticky::get_mut(this, tok);
        // SAFETY: `Sticky<F>` is `Unpin` only when `F` is `Unpin`, and the
        // out-of-line value is dropped in place. Pinning the wrapper therefore
        // pins the value returned by `get_mut` for the rest of its lifetime.
        unsafe { Pin::new_unchecked(inner) }.poll(cx)
    }
}

impl<F: Future> Future for SemiSticky<F> {
    type Output = F::Output;

    #[track_caller]
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        stack_token!(tok);
        // SAFETY: We do not move the wrapper through this mutable reference.
        let this = unsafe { self.as_mut().get_unchecked_mut() };
        let inner = SemiSticky::get_mut(this, tok);
        // SAFETY: `SemiSticky<F>` is `Unpin` only when `F` is `Unpin`, and its
        // storage does not move the value while the wrapper is pinned.
        unsafe { Pin::new_unchecked(inner) }.poll(cx)
    }
}

#[cfg(feature = "stream")]
mod stream {
    use super::*;
    use futures_core::Stream;

    impl<S: Stream> Stream for Fragile<S> {
        type Item = S::Item;

        #[track_caller]
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            self.get_pin_mut().poll_next(cx)
        }

        #[inline]
        fn size_hint(&self) -> (usize, Option<usize>) {
            match self.try_get() {
                Ok(x) => x.size_hint(),
                Err(_) => (0, None),
            }
        }
    }

    impl<S: Stream> Stream for Sticky<S> {
        type Item = S::Item;

        #[track_caller]
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            stack_token!(tok);
            // SAFETY: See the corresponding `Future` implementation.
            let this = unsafe { self.as_mut().get_unchecked_mut() };
            let inner = Sticky::get_mut(this, tok);
            // SAFETY: See the corresponding `Future` implementation.
            unsafe { Pin::new_unchecked(inner) }.poll_next(cx)
        }

        #[inline]
        fn size_hint(&self) -> (usize, Option<usize>) {
            stack_token!(tok);
            match Sticky::try_get(self, tok) {
                Ok(x) => x.size_hint(),
                Err(_) => (0, None),
            }
        }
    }

    impl<S: Stream> Stream for SemiSticky<S> {
        type Item = S::Item;

        #[track_caller]
        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            stack_token!(tok);
            // SAFETY: See the corresponding `Future` implementation.
            let this = unsafe { self.as_mut().get_unchecked_mut() };
            let inner = SemiSticky::get_mut(this, tok);
            // SAFETY: See the corresponding `Future` implementation.
            unsafe { Pin::new_unchecked(inner) }.poll_next(cx)
        }

        #[inline]
        fn size_hint(&self) -> (usize, Option<usize>) {
            stack_token!(tok);
            match SemiSticky::try_get(self, tok) {
                Ok(x) => x.size_hint(),
                Err(_) => (0, None),
            }
        }
    }

    #[test]
    fn test_stream() {
        use futures_executor as executor;
        use futures_util::{future, stream, StreamExt};
        let mut w1 = Fragile::new(stream::once(future::ready(42)));
        let mut w2 = Fragile::new(stream::once(future::ready(42)));
        assert_eq!(
            format!("{:?}", executor::block_on(w1.next())),
            format!("{:?}", executor::block_on(w2.next())),
        );

        let mut w1 = Sticky::new(stream::once(future::ready(42)));
        let mut w2 = Sticky::new(stream::once(future::ready(42)));
        assert_eq!(
            format!("{:?}", executor::block_on(w1.next())),
            format!("{:?}", executor::block_on(w2.next())),
        );

        let mut w1 = SemiSticky::new(stream::once(future::ready(42)));
        let mut w2 = SemiSticky::new(stream::once(future::ready(42)));
        assert_eq!(
            format!("{:?}", executor::block_on(w1.next())),
            format!("{:?}", executor::block_on(w2.next())),
        );
    }

    #[test]
    fn test_stream_panic() {
        use futures_executor as executor;
        use futures_util::{future, stream, StreamExt};

        let mut w = Fragile::new(stream::once(future::ready(42)));
        let t = std::thread::spawn(move || executor::block_on(w.next()));
        assert!(t.join().is_err());

        let mut w = Sticky::new(stream::once(future::ready(42)));
        let t = std::thread::spawn(move || executor::block_on(w.next()));
        assert!(t.join().is_err());

        let mut w = SemiSticky::new(stream::once(future::ready(42)));
        let t = std::thread::spawn(move || executor::block_on(w.next()));
        assert!(t.join().is_err());
    }
}

#[cfg(test)]
struct PendingUntilDrop {
    address: std::cell::Cell<*const PendingUntilDrop>,
    registration: Option<std::sync::Arc<std::sync::atomic::AtomicPtr<PendingUntilDrop>>>,
    _pin: std::marker::PhantomPinned,
}

#[cfg(test)]
impl PendingUntilDrop {
    fn new() -> Self {
        PendingUntilDrop {
            address: std::cell::Cell::new(std::ptr::null()),
            registration: None,
            _pin: std::marker::PhantomPinned,
        }
    }

    fn with_registration(
        registration: std::sync::Arc<std::sync::atomic::AtomicPtr<PendingUntilDrop>>,
    ) -> Self {
        PendingUntilDrop {
            address: std::cell::Cell::new(std::ptr::null()),
            registration: Some(registration),
            _pin: std::marker::PhantomPinned,
        }
    }
}

#[cfg(test)]
impl Future for PendingUntilDrop {
    type Output = ();

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        let current = &*self as *const PendingUntilDrop;
        if self.address.get().is_null() {
            self.address.set(current);
        } else {
            assert_eq!(self.address.get(), current);
        }
        if let Some(registration) = &self.registration {
            registration.store(
                current as *mut PendingUntilDrop,
                std::sync::atomic::Ordering::SeqCst,
            );
        }
        Poll::Pending
    }
}

#[cfg(all(test, feature = "stream"))]
impl futures_core::Stream for PendingUntilDrop {
    type Item = ();

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Future::poll(self, cx).map(Some)
    }
}

#[cfg(test)]
impl Drop for PendingUntilDrop {
    fn drop(&mut self) {
        if !self.address.get().is_null() {
            assert_eq!(self.address.get(), self as *const PendingUntilDrop);
        }
        if let Some(registration) = &self.registration {
            registration.store(std::ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
        }
    }
}

#[test]
fn test_pinned_future_dropped_in_place() {
    let waker = futures_util::task::noop_waker();
    let mut context = Context::from_waker(&waker);

    let mut fragile = Box::pin(Fragile::new(PendingUntilDrop::new()));
    assert!(Future::poll(fragile.as_mut(), &mut context).is_pending());
    drop(fragile);

    let mut sticky = Box::pin(Sticky::new(PendingUntilDrop::new()));
    assert!(Future::poll(sticky.as_mut(), &mut context).is_pending());
    drop(sticky);

    let mut semi_sticky = Box::pin(SemiSticky::new(PendingUntilDrop::new()));
    assert!(Future::poll(semi_sticky.as_mut(), &mut context).is_pending());
    drop(semi_sticky);
}

#[cfg(feature = "stream")]
#[test]
fn test_pinned_stream_dropped_in_place() {
    use futures_core::Stream;

    let waker = futures_util::task::noop_waker();
    let mut context = Context::from_waker(&waker);

    let mut fragile = Box::pin(Fragile::new(PendingUntilDrop::new()));
    assert!(Stream::poll_next(fragile.as_mut(), &mut context).is_pending());
    drop(fragile);

    let mut sticky = Box::pin(Sticky::new(PendingUntilDrop::new()));
    assert!(Stream::poll_next(sticky.as_mut(), &mut context).is_pending());
    drop(sticky);

    let mut semi_sticky = Box::pin(SemiSticky::new(PendingUntilDrop::new()));
    assert!(Stream::poll_next(semi_sticky.as_mut(), &mut context).is_pending());
    drop(semi_sticky);
}

#[test]
fn test_pinned_fragile_drop_on_wrong_thread_keeps_storage_alive() {
    use std::sync::atomic::{AtomicPtr, Ordering};
    use std::sync::Arc;

    let registration = Arc::new(AtomicPtr::new(std::ptr::null_mut()));
    let future = PendingUntilDrop::with_registration(registration.clone());
    let mut fragile = Box::pin(Fragile::new(future));

    let waker = futures_util::task::noop_waker();
    let mut context = Context::from_waker(&waker);
    assert!(Future::poll(fragile.as_mut(), &mut context).is_pending());

    assert!(std::thread::spawn(move || drop(fragile)).join().is_err());

    let registered = registration.load(Ordering::SeqCst);
    assert!(!registered.is_null());
    // SAFETY: Polling registered this pointer only after the future was pinned,
    // and its destructor clears the registration before invalidating storage.
    // A wrong-thread `Fragile` drop must therefore keep that storage alive.
    assert_eq!(unsafe { (*registered).address.get() }, registered);
}

#[test]
fn test_future() {
    use futures_executor as executor;
    use futures_util::future;
    let w1 = Fragile::new(future::ready(42));
    let w2 = w1.clone();
    assert_eq!(
        format!("{:?}", executor::block_on(w1)),
        format!("{:?}", executor::block_on(w2)),
    );

    let w1 = Sticky::new(future::ready(42));
    let w2 = w1.clone();
    assert_eq!(
        format!("{:?}", executor::block_on(w1)),
        format!("{:?}", executor::block_on(w2)),
    );

    let w1 = SemiSticky::new(future::ready(42));
    let w2 = w1.clone();
    assert_eq!(
        format!("{:?}", executor::block_on(w1)),
        format!("{:?}", executor::block_on(w2)),
    );
}

#[test]
fn test_future_panic() {
    use futures_executor as executor;
    use futures_util::future;
    let w = Fragile::new(future::ready(42));
    let t = std::thread::spawn(move || executor::block_on(w));
    assert!(t.join().is_err());

    let w = Sticky::new(future::ready(42));
    let t = std::thread::spawn(move || executor::block_on(w));
    assert!(t.join().is_err());

    let w = SemiSticky::new(future::ready(42));
    let t = std::thread::spawn(move || executor::block_on(w));
    assert!(t.join().is_err());
}
