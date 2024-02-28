use co_managed::Manager;
use may::{coroutine, go};
use std::time::Duration;

fn main() {
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
