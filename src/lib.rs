//! create managed sub coroutines. managed sub coroutines will be cancelled when the parent exit
//! this is some like the scoped coroutine creation, the difference is that we manage the sub
//! coroutines in a hash map, so that when sub coroutine exit the entry will be removed dynamically
//! and parent doesn't wait it's children exit
#[macro_use]
extern crate may;
use may::coroutine;
use rcu_cell::RcuCell;
use rcu_list::d_list::{Entry, LinkedList};

use std::sync::Arc;

type CoNode = Arc<RcuCell<coroutine::JoinHandle<()>>>;
type CoList = Arc<LinkedList<CoNode>>;

#[derive(Default)]
pub struct Manager {
    co_list: CoList,
}

impl Manager {
    pub fn new() -> Self {
        Manager {
            co_list: Arc::new(Default::default()),
        }
    }

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

    /// add sub coroutine that not static
    ///
    /// # Safety
    ///
    /// the `SubCo` may not live long enough
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

/// represent a managed sub coroutine
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
