#![deny(unsafe_op_in_unsafe_fn)]

//! This library provides wrapper types that permit sending non `Send` types to
//! other threads and use runtime checks to ensure safety.
//!
//! It provides three types: [`Fragile`] and [`Sticky`] which are similar in nature
//! but have different behaviors with regards to how destructors are executed and
//! the extra [`SemiSticky`] type which uses [`Sticky`] if the value has a
//! destructor and [`Fragile`] if it does not.
//!
//! All three types wrap a value and provide a `Send` bound.  Neither of the types permit
//! access to the enclosed value unless the thread that wrapped the value is attempting
//! to access it.  The difference between the types starts playing a role once
//! destructors are involved.
//!
//! A [`Fragile`] will actually send the `T` from thread to thread but will only
//! permit the original thread to invoke the destructor.  If the value gets dropped
//! in a different thread, the destructor will panic.
//!
//! A [`Sticky`] on the other hand does not actually send the `T` around but keeps
//! it stored in the original thread's thread local storage.  If it gets dropped
//! in the originating thread it gets cleaned up immediately, otherwise it retains
//! the value until the thread shuts down naturally. Access uses scoped callbacks so
//! references to the TLS value cannot outlive the callback or the originating thread.
//!
//! There is a third type called [`SemiSticky`] which shares the API with [`Sticky`]
//! but internally uses a boxed [`Fragile`] if the type does not actually need a dtor
//! in which case [`Fragile`] is preferred.
//!
//! # Fragile Usage
//!
//! [`Fragile`] is the easiest type to use.  It works almost like a cell.
//!
//! ```
//! use std::thread;
//! use fragile::Fragile;
//!
//! // creating and using a fragile object in the same thread works
//! let val = Fragile::new(true);
//! assert_eq!(*val.get(), true);
//! assert!(val.try_get().is_ok());
//!
//! // once send to another thread it stops working
//! thread::spawn(move || {
//!     assert!(val.try_get().is_err());
//! }).join()
//!     .unwrap();
//! ```
//!
//! # Sticky Usage
//!
//! [`Sticky`] is similar to [`Fragile`] but because it places the value in the
//! thread local storage it comes with some extra restrictions to make it sound.
//! The advantage is it can be dropped from any thread but it comes with extra
//! restrictions. In particular, values placed in it must be `'static`, and access
//! is limited to callbacks that cannot return a reference to the stored value.
//!
//! ```
//! use std::thread;
//! use fragile::Sticky;
//!
//! // Creating and using a sticky object in the same thread works.
//! let val = Sticky::new(true);
//! assert!(val.with(|value| *value));
//! assert!(val.try_with(|value| *value).is_ok());
//!
//! // Once sent to another thread, its value cannot be accessed.
//! thread::spawn(move || {
//!     assert!(val.try_with(|_| ()).is_err());
//! }).join()
//!     .unwrap();
//! ```
//!
//! # Why?
//!
//! Most of the time trying to use this crate is going to indicate some code smell.  But
//! there are situations where this is useful.  For instance you might have a bunch of
//! non `Send` types but want to work with a `Send` error type.  In that case the non
//! sendable extra information can be contained within the error and in cases where the
//! error did not cross a thread boundary yet extra information can be obtained.
//!
//! # Drop / Cleanup Behavior
//!
//! All types will try to eagerly drop a value if they are dropped on the right thread.
//! [`Sticky`] and [`SemiSticky`] will however retain memory until a thread shuts down
//! if the value is dropped on the wrong thread. The benefit is that these types do not
//! panic. A [`Fragile`] dropped in the wrong thread will not just panic,
//! it will effectively also tear down the process because panicking in destructors is
//! non recoverable.
//!
//! # Features
//!
//! The default `stream` feature enables `Future` and `Stream` support and uses
//! `futures-core`. Disable default features to use the crate without dependencies.
//! The optional `slab` feature optimizes [`Sticky`]'s internal registry.
//!
//! The `future` feature implements [`Future`](std::future::Future) for the wrapper
//! types. The `stream` feature includes `future` and implements
//! `futures_core::Stream` as well.
mod errors;
mod fragile;
mod registry;
mod semisticky;
mod sticky;

#[cfg(feature = "future")]
mod futures;

pub use crate::errors::InvalidThreadAccess;
pub use crate::fragile::Fragile;
pub use crate::semisticky::SemiSticky;
pub use crate::sticky::Sticky;
