# co_managed

This lib could create managed sub coroutines.

Managed sub coroutines will be cancelled when their parent exit.  This is something like the scoped coroutine creation, the difference is that we manage the sub coroutines in a hash map, so that when sub coroutine exit the entry will be removed dynamically and parent doesn't wait it's children exit.

[![Build Status](https://github.com/Xudong-Huang/co_managed/workflows/CI/badge.svg)](https://github.com/Xudong-Huang/co_managed/actions?query=workflow%3ACI)
[![Current Crates.io Version](https://img.shields.io/crates/v/co_managed.svg)](https://crates.io/crates/co_managed)
[![Document](https://img.shields.io/badge/doc-co_managed-green.svg)](https://docs.rs/co_managed)

## Usage

First, add this to your `Cargo.toml`:

```toml
[dependencies]
co_managed = "0.1"
```

Then just simply implement your http service

```rust,no_run
use may::{go, coroutine};
use co_managed::Manager;
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
```

## License

This project is licensed under either of the following, at your option:

 * Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT License ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)