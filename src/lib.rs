//! create managed sub coroutines. managed sub coroutines will be cancelled when the parent exit
//! this is somelike the scoped coroutine creation, the difference is that we manage the sub
//! coroutines in a hash map, so that when sub coroutine exit the entry will be removed dynamically
//! and parent doesn't wait it's children exit
#[macro_use]
extern crate may;
use may::coroutine;

use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};

// we can't use coroutine mutex here because in the cancelled drop
// lock the mutex would trigger another Cancel panic
// a better solution would be use a lock free hashmap
type CoMap = Arc<Mutex<HashMap<usize, coroutine::JoinHandle<()>>>>;

#[derive(Debug, Default)]
pub struct Manager {
    id: AtomicUsize,
    co_map: CoMap,
}

impl Manager {
    pub fn new() -> Self {
        Manager {
            id: AtomicUsize::new(0),
            co_map: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn add<F>(&self, f: F)
    where
        F: FnOnce(SubCo) + Send + 'static,
    {
        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let sub = SubCo {
            id: id,
            co_map: self.co_map.clone(),
        };

        let co = go!(move || f(sub));

        // it doesnt' matter if the co is already done here
        // this will just leave any entry in the map and eventually
        // will be droped after all coroutines done
        let mut map = self.co_map.lock().unwrap();
        map.insert(id, co);
    }

    pub unsafe fn add_unsafe<'a, F>(&self, f: F)
    where
        F: FnOnce(SubCo) + Send + 'a,
    {
        trait FnBox {
            fn call_box(self: Box<Self>, SubCo);
        }

        impl<F: FnOnce(SubCo)> FnBox for F {
            #[cfg_attr(feature = "cargo-clippy", allow(boxed_local))]
            fn call_box(self: Box<Self>, subco: SubCo) {
                self(subco)
            }
        }

        let id = self.id.fetch_add(1, Ordering::Relaxed);
        let sub = SubCo {
            id: id,
            co_map: self.co_map.clone(),
        };

        let closure: Box<FnBox + Send + 'a> = Box::new(f);
        let closure: Box<FnBox + Send> = ::std::mem::transmute(closure);

        #[allow(unused_unsafe)]
        let co = go!(move || closure.call_box(sub));

        // it doesnt' matter if the co is already done here
        // this will just leave any entry in the map and eventually
        // will be droped after all coroutines done
        let mut map = self.co_map.lock().unwrap();
        map.insert(id, co);
    }
}

impl Drop for Manager {
    // when parent exit would call this drop
    fn drop(&mut self) {
        // cancel all the sub coroutines
        for (_, co) in self.co_map.lock().unwrap().iter() {
            unsafe { co.coroutine().cancel() };
        }

        if ::std::thread::panicking() {
            // if in panick don't join here
            return;
        }

        let mut map = self.co_map.lock().unwrap();
        let co_vec: Vec<_> = map.drain().map(|(_, co)| co).collect();
        drop(map);

        for co in co_vec {
            co.join().ok();
        }
    }
}

pub struct SubCo {
    id: usize,
    co_map: CoMap,
}

unsafe impl Send for SubCo {}

impl Drop for SubCo {
    // when the sub coroutine finished will trigger this drop
    // if this is called due to a panic then it's not safe
    // to call the coroutine mutex lock to trigger another panic
    fn drop(&mut self) {
        if ::std::thread::panicking() {
            // if in panick don't join here
            return;
        }

        let mut map = self.co_map.lock().unwrap();
        if let Some(co) = map.remove(&self.id) {
            drop(map);
            co.join().ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;
    use coroutine;
    use super::*;

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
        coroutine::sleep(Duration::from_millis(1000));
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
