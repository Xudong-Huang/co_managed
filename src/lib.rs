//! create managed sub coroutines. managed sub coroutines will be cancelled when the parent exit
//! this is some like the scoped coroutine creation, the difference is that we manage the sub
//! coroutines in a hash map, so that when sub coroutine exit the entry will be removed dynamically
//! and parent doesn't wait it's children exit
#[macro_use]
extern crate may;
use congee::Congee;
use may::coroutine::{self, JoinHandle};

use core::fmt;
use std::mem::forget;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone)]
struct CoHandle {
    co: Arc<JoinHandle<()>>,
}

impl From<usize> for CoHandle {
    /// SAFETY
    fn from(co: usize) -> Self {
        CoHandle {
            co: unsafe { Arc::from_raw(co as *mut _) },
        }
    }
}

impl From<CoHandle> for usize {
    fn from(co: CoHandle) -> Self {
        Arc::into_raw(co.co) as usize
    }
}

impl CoHandle {
    fn new(co: JoinHandle<()>) -> Self {
        CoHandle { co: Arc::new(co) }
    }

    fn join(self) {
        let co = Arc::into_inner(self.co).expect("can't get co from Arc");
        co.join().ok();
    }
}

impl CoHandle {}
/// concurrent map for coroutine
struct CoMap {
    map: Congee<usize, CoHandle>,
}

impl fmt::Debug for CoMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CoMap")
    }
}

impl Default for CoMap {
    fn default() -> Self {
        Self::new()
    }
}

impl CoMap {
    fn new() -> Self {
        // let allocator = congee::DefaultAllocator {};
        // let drainer = |id, co: CoHandle| {
        //     println!("clear co handle, id={}", id);
        //     unsafe { co.co.coroutine().cancel() };
        //     // we can't wait the sub coroutine here,
        //     // because we may be in a Cancel panicking context
        //     // co.join()
        // };
        CoMap {
            // map: Congee::new_with_drainer(allocator, drainer),
            map: Congee::default(),
        }
    }

    fn insert(&self, id: usize, co: JoinHandle<()>) {
        let value = CoHandle::new(co);
        let guard = self.map.pin();
        self.map.insert(id, value, &guard).unwrap();
    }

    fn remove(&self, id: &usize) -> Option<CoHandle> {
        let guard = self.map.pin();
        self.map.remove(id, &guard)
    }

    fn is_empty(&self) -> bool {
        let guard = self.map.pin();
        self.map.is_empty(&guard)
    }

    fn cancel_all(&self, max: usize) {
        const MAX_LEN: usize = 64;
        let mut ids = [(0, 0); MAX_LEN];
        let mut end = max;
        let mut start = end.max(MAX_LEN) - MAX_LEN;

        // we are using a stupid way to iter the map
        while end > 0 {
            let guard = self.map.pin();
            if self.map.is_empty(&guard) {
                return;
            }

            let len = self.map.range(&start, &end, &mut ids, &guard);
            println!("clear range, start={}, end={}, len={}", start, end, len);

            for id in ids.iter().take(len) {
                println!("cancel id={}", id.0);
                unsafe {
                    // the co could be deleted, but we can sure that
                    // the co would not be dropped since we hold guard
                    let co = CoHandle::from(id.1);
                    co.co.coroutine().cancel();
                    forget(co);
                }
            }

            end -= end.min(MAX_LEN);
            start = end.max(MAX_LEN) - MAX_LEN;
        }
    }
}

#[derive(Debug, Default)]
pub struct Manager {
    id: AtomicUsize,
    co_map: Arc<CoMap>,
}

impl Manager {
    pub fn new() -> Self {
        Manager {
            id: AtomicUsize::new(0),
            co_map: Arc::new(CoMap::new()),
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
        self.co_map.insert(id, co);
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
        self.co_map.insert(id, co);
    }
}

impl Drop for Manager {
    // when parent exit would call this drop
    fn drop(&mut self) {
        println!("Drop manager");
        // the SubCo drop would remove itself from the map
        // we can't remove SubCo here for:
        self.co_map.cancel_all(self.id.load(Ordering::Relaxed));

        if !std::thread::panicking() {
            while !self.co_map.is_empty() {
                coroutine::yield_now();
            }
        }
    }
}

/// represent a managed sub coroutine
pub struct SubCo {
    id: usize,
    co_map: Arc<CoMap>,
}

impl Drop for SubCo {
    // when the sub coroutine finished will trigger this drop
    // if this is called due to a panic then it's not safe
    // to call the coroutine mutex lock to trigger another panic
    fn drop(&mut self) {
        if let Some(co) = self.co_map.remove(&self.id) {
            co.join();
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
