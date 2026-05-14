// Licensed under the Apache-2.0 license

#![cfg_attr(target_arch = "riscv32", no_std)]

extern crate alloc;

pub mod blocking;
mod future;
pub use future::TockSubscribe;
#[cfg(feature = "executor")]
mod tock_executor;
#[cfg(feature = "executor")]
pub use tock_executor::TockExecutor;

use critical_section::RawRestoreState;

// copied from libtock-rs/demos/st7789-slint/src/main.rs
struct NullCriticalSection;
critical_section::set_impl!(NullCriticalSection);

// Safety: there is no code here.
unsafe impl critical_section::Impl for NullCriticalSection {
    unsafe fn acquire() -> RawRestoreState {
        // Tock is single threaded, so this can only be preempted by interrupts
        // The kernel won't schedule anything from our app unless we yield
        // so as long as we don't yield we won't concurrently run with
        // other critical sections from our app.
        // The kernel might schedule itself or other applications, but there
        // is nothing we can do about that.
    }
    unsafe fn release(_token: RawRestoreState) {}
}

#[cfg(feature = "executor")]
pub fn init<S>(spawner: embassy_executor::Spawner, main: embassy_executor::SpawnToken<S>) {
    spawner.spawn(main).unwrap();
}

#[cfg(feature = "executor")]
pub fn start_async<S>(main: embassy_executor::SpawnToken<S>) -> ! {
    let mut executor = TockExecutor::new();
    let executor: &'static mut TockExecutor = unsafe { core::mem::transmute(&mut executor) };
    executor.run(|spawner: embassy_executor::Spawner| init(spawner, main));
}
