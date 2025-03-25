use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crate::Fragile;

impl<F: Future> Future for Fragile<F> {
    type Output = F::Output;

    #[track_caller]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        unsafe {
            self.map_unchecked_mut(|s| match s.try_get_mut() {
                Ok(fut) => fut,
                Err(_) => {
                    panic!(
                        "trying to poll wrapped future in fragile container from incorrect thread."
                    );
                }
            })
        }
        .poll(cx)
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
            unsafe {
                self.map_unchecked_mut(|s| match s.try_get_mut() {
                    Ok(fut) => fut,
                    Err(_) => {
                        panic!(
                        "trying to poll wrapped stream in fragile container from incorrect thread."
                    );
                    }
                })
            }
            .poll_next(cx)
        }

        #[inline]
        fn size_hint(&self) -> (usize, Option<usize>) {
            match self.try_get() {
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
    }

    #[test]
    fn test_stream_panic() {
        use futures_executor as executor;
        use futures_util::{future, stream, StreamExt};

        let mut w = Fragile::new(stream::once(future::ready(42)));
        let t = std::thread::spawn(move || executor::block_on(w.next()));
        assert!(t.join().is_err());
    }
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
}

#[test]
fn test_future_panic() {
    use futures_executor as executor;
    use futures_util::future;
    let w = Fragile::new(future::ready(42));
    let t = std::thread::spawn(move || executor::block_on(w));
    assert!(t.join().is_err());
}
