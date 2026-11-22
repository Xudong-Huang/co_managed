# co_managed

Create managed sub coroutines on the [`may`](https://crates.io/crates/may)
coroutine runtime.

A `Manager` owns a dynamic set of child coroutines:

- **Cancelled on parent exit.** Dropping the manager cancels every child that
  is still running and blocks until all of them have finished.
- **Automatic cleanup.** A child that finishes on its own removes its entry
  from the manager immediately, so finished coroutines do not accumulate.
- **No waiting while running.** Unlike a scoped spawn, the parent does not wait
  for its children; children are only awaited during teardown.

Children are tracked in a lock-free RCU linked list, so `add` can be called
concurrently from several coroutines and finished children unlink themselves
without taking a global lock.

[![Build Status](https://github.com/Xudong-Huang/co_managed/workflows/CI/badge.svg)](https://github.com/Xudong-Huang/co_managed/actions?query=workflow%3ACI)
[![Current Crates.io Version](https://img.shields.io/crates/v/co_managed.svg)](https://crates.io/crates/co_managed)
[![Document](https://img.shields.io/badge/doc-co_managed-green.svg)](https://docs.rs/co_managed)

## Usage

Add the dependency to your `Cargo.toml`:

```toml
[dependencies]
co_managed = "0.2"
```

Then spawn managed sub coroutines from a `may` coroutine:

```rust,no_run
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
            manager.add(move || {
                let d = Dummy(i);
                println!("sub started, id = {}", d.0);
                loop {
                    coroutine::sleep(Duration::from_millis(10));
                }
            });
        }

        // Keep the manager alive until the parent is cancelled. The children
        // are not joined here.
        coroutine::park();
    });

    coroutine::sleep(Duration::from_millis(100));

    // Cancelling the parent unwinds its stack, dropping `manager`, which in
    // turn cancels and joins every sub coroutine.
    unsafe { j.coroutine().cancel() };
    j.join().ok();
    println!("parent exit");

    coroutine::sleep(Duration::from_millis(1000));
}
```

A runnable version lives in [`examples/main.rs`](examples/main.rs):

```sh
cargo run --example main
```

## How it works

`Manager::add` spawns a coroutine with `may::go!`. Before the supplied closure
runs, the coroutine inserts a slot into the manager's RCU linked list and
creates a `SubCo` guard. The guard removes the slot when the closure returns or
unwinds, so the list always contains only the coroutines that are still alive.
Dropping the `Manager` walks the list, cancels each coroutine, waits for it to
finish, and yields until the list is empty.

## Caveats

- **Cooperative cancellation.** `may` coroutines observe cancellation only at
  cancellation points (yield, sleep, or I/O). A coroutine that never reaches
  one cannot be cancelled, and dropping the manager blocks until it does.
- **Do not drop the manager from one of its own children.** Teardown waits for
  every tracked coroutine, including the caller, and would deadlock.
- **Panics are contained.** A panic in a child unwinds only that child; it is
  not propagated through the manager.
- **Borrowed data.** `Manager::add` requires a `'static` closure. Use
  `Manager::add_unsafe` to borrow non-`'static` data, and follow its safety
  contract; violations are undefined behavior.

## Documentation

API documentation is available at [docs.rs/co_managed](https://docs.rs/co_managed).

## License

Licensed under either of

- Apache License, Version 2.0
  ([http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0))
- MIT license
  ([LICENSE](https://github.com/Xudong-Huang/co_managed/blob/master/LICENSE) or
  [http://opensource.org/licenses/MIT](http://opensource.org/licenses/MIT))

at your option.
