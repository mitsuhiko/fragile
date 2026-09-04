# Changelog

All notable changes to fragile are documented here.

## Unreleased

* Removed `StackToken` and `stack_token!`, and replaced `Sticky` and
  `SemiSticky`'s `get`, `get_mut`, `try_get`, and `try_get_mut` methods with
  scoped `with`, `with_mut`, `try_with`, and `try_with_mut` callbacks. This
  breaking change prevents thread-local references from escaping and removes
  the need to permanently leak registry values.
* Fixed soundness issues in the `Future` and `Stream` implementations that could
  move `!Unpin` values after pinning or free pinned storage on the wrong thread.
  As a result, `Sticky<T>` and `SemiSticky<T>` now implement `Unpin` only when
  `T: Unpin`.
* Hardened thread-local registry cleanup for nested `Sticky` values and
  reentrant destructors.
* `Sticky` and `SemiSticky`'s `is_valid`, `try_with`, `try_with_mut`,
  `try_into_inner` and `Debug` now report an invalid access instead of
  panicking when used on the originating thread while its thread-local storage
  is being destroyed. Previously this panicked inside a TLS destructor, which
  aborts the process.
* Thread identity checks no longer use `std::thread::current()`, which panics
  during thread-local storage teardown on Rust versions before 1.84. Dropping a
  `Fragile` or `Sticky` from a thread-local destructor on those versions no
  longer aborts the process.

## 2.1.0

* Implement `Future` and `Stream` for `Fragile`, `Sticky` and `SemiSticky`.
  [#38](https://github.com/mitsuhiko/fragile/pull/38), [#40](https://github.com/mitsuhiko/fragile/pull/40)
* Better panic error reporting by adding `#[track_caller]`.
* Fragile now internally uses the stdlib's thread IDs instead of its own counter.  [#39](https://github.com/mitsuhiko/fragile/pull/39)

## 2.0.1

* Fixed a soundness issue with `Sticky` if the `slab` variant was enabled.
  This caused a use after free if the type was freed in the wrong thread.
  [#37](https://github.com/mitsuhiko/fragile/pull/37)

## 2.0.0

* `Fragile` no longer boxes internally.
* `Sticky` and `SemiSticky` now require the use of stack tokens.
  For more information see [#26](https://github.com/mitsuhiko/fragile/issues/26)
* `Sticky` now tries to drop entries from the thread local registry eagerly
  if it's dropped on the right thread.

## 1.2.1

* Fixed non slab versions only allowing a single sticky.

## 1.2.0

Note on safety: the `Sticky` and `SemiSticky` types allow data to live
longer than the wrapper type which is why they are now requiring a `'static`
bound.  Previously it was possible to create a sticky containing a bare
reference which permitted unsafe access.

* `Sticky` now requires `'static`.
* Added the `slab` feature for an internal optimization for `Sticky` to use
  a slab instead of a `HashMap`.

## Older Releases

Older releases were yanked due to the insufficient trait bound on `Sticky`.
