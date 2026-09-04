use fragile::{Fragile, SemiSticky, Sticky};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct CountDrops(Arc<AtomicUsize>);

impl Drop for CountDrops {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

#[test]
fn wrapper_traits() {
    fn send_sync<T: Send + Sync>() {}
    fn unpin<T: Unpin>() {}
    send_sync::<Fragile<std::rc::Rc<()>>>();
    send_sync::<Sticky<std::rc::Rc<()>>>();
    send_sync::<SemiSticky<std::rc::Rc<()>>>();
    unpin::<Fragile<String>>();
    unpin::<Sticky<String>>();
    unpin::<SemiSticky<String>>();
}

macro_rules! scoped_access_tests {
    ($module:ident, $wrapper:ident) => {
        mod $module {
            use super::*;

            #[test]
            fn registry_growth_during_mutable_callback() {
                let mut value = $wrapper::new(String::from("original"));
                value.with_mut(|value| {
                    let others: Vec<_> = (0..128).map(|i| $wrapper::new(i.to_string())).collect();
                    value.push_str(" modified");
                    drop(others);
                    assert_eq!(value, "original modified");
                });
                value.with(|a| value.with(|b| assert_eq!(a, b)));
            }

            #[test]
            fn callback_unwind_releases_borrow() {
                let mut value = $wrapper::new(String::from("original"));
                assert!(catch_unwind(AssertUnwindSafe(|| {
                    value.with_mut(|value| {
                        value.push_str(" modified");
                        panic!("callback panic");
                    });
                }))
                .is_err());
                value.with_mut(|value| assert_eq!(value, "original modified"));
                assert_eq!(value.into_inner(), "original modified");
            }

            #[test]
            fn reentrant_destructor() {
                struct Reentrant($wrapper<String>);
                impl Drop for Reentrant {
                    fn drop(&mut self) {
                        self.0.with_mut(|value| value.push('!'));
                        let temporary = $wrapper::new(String::from("temporary"));
                        assert_eq!(temporary.into_inner(), "temporary");
                    }
                }
                drop($wrapper::new(Reentrant($wrapper::new(String::from(
                    "sibling",
                )))));
            }

            #[test]
            fn destructor_panic_does_not_leave_an_owned_entry() {
                struct Panics {
                    _value: CountDrops,
                }
                impl Drop for Panics {
                    fn drop(&mut self) {
                        panic!("destructor panic");
                    }
                }
                let drops = Arc::new(AtomicUsize::new(0));
                let value = $wrapper::new(Panics {
                    _value: CountDrops(drops.clone()),
                });
                assert!(catch_unwind(AssertUnwindSafe(|| drop(value))).is_err());
                assert_eq!(drops.load(Ordering::SeqCst), 1);
                assert_eq!($wrapper::new(42u32).into_inner(), 42);
                // TLS cleanup must not attempt to drop the panicking value again.
            }

            #[test]
            fn into_inner_transfers_ownership_once() {
                let drops = Arc::new(AtomicUsize::new(0));
                let value = $wrapper::new(CountDrops(drops.clone()));
                let inner = value.try_into_inner().ok().unwrap();
                assert_eq!(drops.load(Ordering::SeqCst), 0);
                drop(inner);
                assert_eq!(drops.load(Ordering::SeqCst), 1);
            }
        }
    };
}

scoped_access_tests!(sticky, Sticky);
scoped_access_tests!(semisticky, SemiSticky);
