//! Create managed sub coroutines for the [`may`] coroutine runtime.
//!
//! A [`Manager`] owns a dynamic set of child coroutines. Dropping the manager
//! cancels every child that is still running and waits for all of them to
//! finish, while a child that exits on its own removes itself from the manager
//! immediately. Unlike a scoped spawn, the parent does not wait for its
//! children while it is running.
//!
//! The project README, included below, is the crate-level guide with a complete
//! example and notes on cancellation.

#![doc = include_str!("../README.md")]
#![warn(missing_docs)]

#[macro_use]
extern crate may;
use may::coroutine;
use rcu_list::d_list::{Entry, LinkedList};
use rcu_list::rcu_cell::RcuCell;

use std::sync::Arc;

type CoNode = Arc<RcuCell<coroutine::JoinHandle<()>>>;
type CoList = Arc<LinkedList<CoNode>>;

/// A collection of managed sub coroutines.
///
/// Every coroutine spawned with [`add`](Manager::add) or
/// [`add_unsafe`](Manager::add_unsafe) is tracked by the manager until it
/// finishes. Tracking is dynamic: a coroutine that exits removes itself, so a
/// long-lived manager does not accumulate finished coroutines. Sub coroutines
/// are tracked in a lock-free list, so `add` may be called concurrently from
/// several coroutines.
///
/// Dropping the manager cancels every coroutine that is still running and
/// blocks until all of them have finished. This ties the lifetime of the child
/// coroutines to the owner of the manager.
///
/// # Cancellation
///
/// Cancellation is cooperative: a sub coroutine is unwound at its next
/// cancellation point (a `may` yield, sleep, or I/O operation). A sub
/// coroutine that never reaches such a point cannot be cancelled, and dropping
/// the manager blocks until it does.
///
/// # Panics
///
/// A panic in a sub coroutine unwinds only that coroutine and is not
/// propagated through the manager. Its entry is removed just as it is on
/// normal completion.
///
/// # Deadlocks
///
/// Do not drop a manager from inside one of its own sub coroutines: teardown
/// waits for every tracked coroutine, including the caller, and would block
/// forever.
#[derive(Default)]
pub struct Manager {
    co_list: CoList,
}

impl Manager {
    /// Creates a new, empty manager.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use co_managed::Manager;
    ///
    /// let manager = Manager::new();
    /// manager.add(|| {
    ///     // runs as a managed sub coroutine
    /// });
    /// ```
    pub fn new() -> Self {
        Manager {
            co_list: Arc::new(Default::default()),
        }
    }

    /// Spawns `f` as a new managed sub coroutine.
    ///
    /// The closure runs on a new `may` coroutine. It is registered with this
    /// manager before it starts, even if it returns immediately. When `f`
    /// returns, panics, or is cancelled, the sub coroutine removes itself from
    /// the manager.
    ///
    /// This method returns immediately and does not wait for `f`. The sub
    /// coroutine runs until it finishes on its own or until the manager is
    /// dropped, at which point it is cancelled.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use co_managed::Manager;
    /// use may::coroutine;
    /// use std::time::Duration;
    ///
    /// let manager = Manager::new();
    ///
    /// manager.add(|| loop {
    ///     coroutine::sleep(Duration::from_millis(100));
    /// });
    ///
    /// // Dropping the manager cancels and joins the sub coroutine above.
    /// drop(manager);
    /// ```
    pub fn add<F>(&self, f: F)
    where
        F: FnOnce() + Send + 'static,
    {
        let slot = Arc::new(RcuCell::none());
        let slot_dup = slot.clone();

        let co_list = self.co_list.clone();

        let co = go!(move || {
            let entry = co_list.push_front(slot_dup);
            let _sub_co = SubCo { entry };
            f();
        });
        // setup the JoinHandle
        slot.write(co);
    }

    /// Spawns `f` as a managed sub coroutine, allowing it to borrow data that
    /// is not `'static`.
    ///
    /// This is the `unsafe` counterpart of [`add`](Manager::add). The closure
    /// is erased to `'static` and spawned on a coroutine, so the compiler can
    /// no longer check that everything it borrows stays alive for as long as
    /// the coroutine runs.
    ///
    /// Prefer [`add`](Manager::add) whenever the closure can be made
    /// `'static`, for example by moving owned data into it.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that every reference borrowed by `f` stays
    /// valid until the spawned sub coroutine has finished. In practice this
    /// requires that:
    ///
    /// * the manager is dropped before the borrowed data is dropped, and the
    ///   manager is not leaked (for example with [`std::mem::forget`]), since
    ///   dropping the manager is what guarantees that the sub coroutine has
    ///   finished;
    /// * the sub coroutine does not outlive the borrowed data, keeping
    ///   cancellation points in mind: cancellation is cooperative, so a
    ///   coroutine that never yields may keep running for longer than expected
    ///   (see the cancellation notes on [`Manager`]).
    ///
    /// Violating these requirements lets the sub coroutine access dangling
    /// references, which is undefined behavior.
    pub unsafe fn add_unsafe<'a, F>(&self, f: F)
    where
        F: FnOnce() + Send + 'a,
    {
        let slot = Arc::new(RcuCell::none());
        let slot_dup = slot.clone();

        let co_list = self.co_list.clone();

        let closure: Box<dyn FnOnce() + Send + 'a> = Box::new(f);
        let closure: Box<dyn FnOnce() + Send> = std::mem::transmute(closure);

        let co = go!(move || {
            let entry = co_list.push_front(slot_dup);
            let _sub_co = SubCo { entry };
            closure()
        });
        // setup the JoinHandle
        slot.write(co);
    }
}

impl Drop for Manager {
    // when parent exit would call this drop
    fn drop(&mut self) {
        // cancel all the sub coroutines
        self.co_list.iter().for_each(|co| {
            let co = co.read().unwrap();
            unsafe { co.coroutine().cancel() };
            co.wait()
        });

        // the SubCo drop would remove itself from the list
        while !self.co_list.is_empty() {
            coroutine::yield_now();
        }
    }
}

/// A guard that keeps a running sub coroutine tracked by its [`Manager`].
///
/// [`Manager::add`] and [`Manager::add_unsafe`] create one of these inside
/// every spawned sub coroutine. It is dropped when the coroutine finishes,
/// whether normally, by panicking, or by being cancelled. Dropping the guard
/// unlinks the coroutine's entry from the manager, so a later teardown will
/// not visit it.
///
/// Values of this type are created by the crate and cannot be constructed
/// directly.
pub struct SubCo<'a> {
    entry: Entry<'a, CoNode>,
}

impl Drop for SubCo<'_> {
    // when the sub coroutine finished will trigger this drop
    fn drop(&mut self) {
        self.entry.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn thread_exit() {
        let manager = Manager::new();
        struct Dummy(usize);
        impl Drop for Dummy {
            fn drop(&mut self) {
                println!("co dropped, id={}", self.0);
            }
        }
        for i in 0..10 {
            manager.add(move || {
                let d = Dummy(i);
                println!("sub started, id = {}", d.0);
                loop {
                    coroutine::sleep(Duration::from_millis(10));
                }
            });
        }
        coroutine::sleep(Duration::from_millis(100));
        println!("parent started");
        drop(manager);
        println!("parent exit");
    }

    #[test]
    fn coroutine_cancel() {
        let j = go!(|| {
            println!("parent started");
            let manager = Manager::new();
            struct Dummy(usize);
            impl Drop for Dummy {
                fn drop(&mut self) {
                    println!("co dropped, id={}", self.0);
                }
            }
            for i in 0..10 {
                manager.add(move || {
                    let d = Dummy(i);
                    println!("sub started, id = {}", d.0);
                    loop {
                        coroutine::sleep(Duration::from_millis(10));
                    }
                });
            }
            coroutine::park();
        });

        coroutine::sleep(Duration::from_millis(100));
        unsafe { j.coroutine().cancel() };
        j.join().ok();
        println!("parent exit");
        coroutine::sleep(Duration::from_millis(1000));
    }
}
