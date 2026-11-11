//! Test utilities shared across modules. Compiled only for tests.

/// A scripted Harness for tests.
pub mod scripted;
/// Temporary directory trees for tests.
pub mod temp_tree;

use core::fmt::Debug;

/// Asserts that `items` is empty, and shows what was in it when it is not.
///
/// # Panics
///
/// When `items` is not empty.
#[track_caller]
#[inline]
pub fn assert_empty<Item, Items>(items: Items)
where
    Item: Debug,
    Items: AsRef<[Item]>,
{
    let listed = items.as_ref();
    assert_eq!(listed.len(), 0, "expected nothing, got {listed:?}");
}

/// Asserts that `items` is not empty.
///
/// # Panics
///
/// When `items` is empty.
#[track_caller]
#[inline]
pub fn assert_not_empty<Item, Items>(items: Items)
where
    Item: Debug,
    Items: AsRef<[Item]>,
{
    let listed = items.as_ref();
    assert_ne!(listed.len(), 0, "expected something, got {listed:?}");
}
