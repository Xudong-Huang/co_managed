//! create managed sub coroutines. managed sub coroutines will be cancelled when the parent exit
//! this is some like the scoped coroutine creation, the difference is that we manage the sub
//! coroutines in a hash map, so that when sub coroutine exit the entry will be removed dynamically
//! and parent doesn't wait it's children exit
#[macro_use]
extern crate may;
use may::coroutine;
use parking_lot::Mutex;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

// CAUTION: we can't use coroutine mutex here because in the cancelled drop
// lock the mutex would trigger another Cancel panic
// a better solution would be use a lock free hashmap
type CoMap = Arc<Mutex<BTreeMap<usize, coroutine::JoinHandle<()>>>>;

#[derive(Debug, Default)]
pub struct Manager {
    id: AtomicUsize,
    co_map: CoMap,
}

impl Manager {
    pub fn new() -> Self {
        Manager {
            id: AtomicUsize::new(0),
            co_map: Arc::new(Default::default()),
        }
    }

    pub fn add<F>(&self, f: F)
    where
        F: FnOnce(SubCo) + Send + 'static,
    {
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let sub = SubCo {
            id,
            co_map: self.co_map.clone(),
        };

        let co = go!(move || f(sub));

        // it does not matter if the co is already done here
        // this will just leave an entry in the map and eventually
        // will be dropped after all coroutines done
        self.co_map.lock().insert(id, co);
    }

    /// add sub coroutine that not static
    ///
    /// # Safety
    ///
    /// the `SubCo` may not live long enough
    pub unsafe fn add_unsafe<'a, F>(&self, f: F)
    where
        F: FnOnce(SubCo) + Send + 'a,
    {
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let sub = SubCo {
            id,
            co_map: self.co_map.clone(),
        };

        let closure: Box<dyn FnOnce(SubCo) + Send + 'a> = Box::new(f);
        let closure: Box<dyn FnOnce(SubCo) + Send> = std::mem::transmute(closure);

        let co = go!(move || closure(sub));

        // it doesn't' matter if the co is already done here
        // this will just leave any entry in the map and eventually
        // will be dropped after all coroutines done
        self.co_map.lock().insert(id, co);
    }
}

impl Drop for Manager {
    // when parent exit would call this drop
    fn drop(&mut self) {
        println!("drop Manager");
        // cancel all the sub coroutines
        self.co_map.lock().values().for_each(|co| {
            println!("cancel sub co");
            unsafe { co.coroutine().cancel() };
            // we can't wait the sub coroutine here,
            // because we may be in a Cancel panicking context
            // co.wait()
        });

        // the SubCo drop would remove itself from the map
        // we can't remove SubCo here for:
        // 1. reduce contention
        // 2. avoid dashmap dead lock when hold item reference
        //    while other thread is removing the item
        if !std::thread::panicking() {
            while !self.co_map.lock().is_empty() {
                coroutine::yield_now();
            }
        }
    }
}

/// represent a managed sub coroutine
pub struct SubCo {
    id: usize,
    co_map: CoMap,
}

impl Drop for SubCo {
    // when the sub coroutine finished will trigger this drop
    // if this is called due to a panic then it's not safe
    // to call the coroutine mutex lock to trigger another panic
    fn drop(&mut self) {
        if let Some(co) = self.co_map.lock().remove(&self.id) {
            co.join().ok();
        }
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
            manager.add(move |_| {
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
                manager.add(move |_| {
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
