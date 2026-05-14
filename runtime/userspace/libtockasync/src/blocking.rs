// Licensed under the Apache-2.0 license

//! Blocking (synchronous) wrappers for Tock subscribe/command operations.
//!
//! These functions perform the same operations as [`TockSubscribe`](crate::TockSubscribe)
//! but block the calling thread (via `yield_wait`) instead of returning a `Future`.
//! This eliminates async state machines, Box allocations, and waker overhead.

use caliptra_mcu_libtock_platform::exit_on_drop::ExitOnDrop;
use caliptra_mcu_libtock_platform::*;
use core::cell::Cell;

/// Stack-allocated result container for blocking Tock operations.
///
/// Placed on the stack and its address passed to the kernel as upcall data.
/// The kernel upcall sets the result, and the blocking wait loop reads it.
struct BlockingResult {
    result: Cell<Option<(u32, u32, u32)>>,
}

/// Kernel upcall handler for blocking operations.
///
/// Sets the result on the `BlockingResult` and returns. No waker is needed
/// because the caller is blocked in a `yield_wait` loop and will check the
/// result after each yield.
extern "C" fn blocking_upcall<S: Syscalls>(arg0: u32, arg1: u32, arg2: u32, data: Register) {
    let exit: ExitOnDrop<S> = Default::default();
    let sub_ptr: usize = data.into();
    let sub = sub_ptr as *mut BlockingResult;
    // Safety: `sub` points to a valid `BlockingResult` on the caller's stack frame.
    // The caller is blocked in `wait_for_result` so the stack frame is alive.
    unsafe { (*sub).result.set(Some((arg0, arg1, arg2))) };
    core::mem::forget(exit);
}

/// Block until the result is available, yielding to the Tock kernel.
fn wait_for_result<S: Syscalls>(result: &BlockingResult) -> (u32, u32, u32) {
    loop {
        if let Some(r) = result.result.get() {
            return r;
        }
        S::yield_wait();
    }
}

/// Register a SUBSCRIBE syscall, pointing the upcall to a `BlockingResult`.
fn do_subscribe<S: Syscalls>(
    driver_num: u32,
    subscribe_num: u32,
    result: &BlockingResult,
) -> Result<(), ErrorCode> {
    let upcall_fcn = (blocking_upcall::<S> as *const ()) as usize;
    let upcall_data = (result as *const BlockingResult) as usize;

    // Safety: We pass a valid function pointer and a pointer to a stack-local
    // BlockingResult. The caller blocks until the upcall fires, keeping the
    // stack frame alive.
    let [r0, r1, _, _] = unsafe {
        S::syscall4::<{ syscall_class::SUBSCRIBE }>([
            driver_num.into(),
            subscribe_num.into(),
            upcall_fcn.into(),
            upcall_data.into(),
        ])
    };
    let return_variant: ReturnVariant = r0.as_u32().into();
    match return_variant {
        return_variant::SUCCESS_2_U32 => Ok(()),
        return_variant::FAILURE_2_U32 => {
            Err(r1.as_u32().try_into().unwrap_or(ErrorCode::Fail))
        }
        _ => Err(ErrorCode::Fail),
    }
}

/// Issue an ALLOW_RW syscall (share a mutable buffer with the kernel).
fn do_allow_rw<S: Syscalls>(
    driver_num: u32,
    buffer_num: u32,
    buffer: &mut [u8],
) -> Result<(), ErrorCode> {
    let [r0, r1, _, _] = unsafe {
        S::syscall4::<{ syscall_class::ALLOW_RW }>([
            driver_num.into(),
            buffer_num.into(),
            buffer.as_mut_ptr().into(),
            buffer.len().into(),
        ])
    };
    let return_variant: ReturnVariant = r0.as_u32().into();
    match return_variant {
        return_variant::SUCCESS_2_U32 => Ok(()),
        return_variant::FAILURE_2_U32 => {
            Err(r1.as_u32().try_into().unwrap_or(ErrorCode::Fail))
        }
        _ => Err(ErrorCode::Fail),
    }
}

/// Issue an ALLOW_RO syscall (share an immutable buffer with the kernel).
fn do_allow_ro<S: Syscalls>(
    driver_num: u32,
    buffer_num: u32,
    buffer: &[u8],
) -> Result<(), ErrorCode> {
    let [r0, r1, _, _] = unsafe {
        S::syscall4::<{ syscall_class::ALLOW_RO }>([
            driver_num.into(),
            buffer_num.into(),
            buffer.as_ptr().into(),
            buffer.len().into(),
        ])
    };
    let return_variant: ReturnVariant = r0.as_u32().into();
    match return_variant {
        return_variant::SUCCESS_2_U32 => Ok(()),
        return_variant::FAILURE_2_U32 => {
            Err(r1.as_u32().try_into().unwrap_or(ErrorCode::Fail))
        }
        _ => Err(ErrorCode::Fail),
    }
}

/// ALLOW_RW + SUBSCRIBE + COMMAND + blocking wait.
///
/// Shares `buffer` (read-write) with the kernel, subscribes for a completion
/// upcall, issues the command, then blocks (via `yield_wait`) until the
/// upcall fires.
///
/// Returns the three upcall arguments `(u32, u32, u32)` on success.
pub fn subscribe_allow_rw_and_wait<S: Syscalls>(
    driver_num: u32,
    subscribe_num: u32,
    buffer_num: u32,
    buffer: &mut [u8],
    cmd: u32,
    arg1: u32,
    arg2: u32,
) -> Result<(u32, u32, u32), ErrorCode> {
    let result = BlockingResult {
        result: Cell::new(None),
    };

    do_allow_rw::<S>(driver_num, buffer_num, buffer)?;
    do_subscribe::<S>(driver_num, subscribe_num, &result)?;

    S::command(driver_num, cmd, arg1, arg2)
        .to_result::<(), ErrorCode>()?;

    Ok(wait_for_result::<S>(&result))
}

/// ALLOW_RO + SUBSCRIBE + COMMAND + blocking wait.
///
/// Shares `buffer` (read-only) with the kernel, subscribes for a completion
/// upcall, issues the command, then blocks until the upcall fires.
pub fn subscribe_allow_ro_and_wait<S: Syscalls>(
    driver_num: u32,
    subscribe_num: u32,
    buffer_num: u32,
    buffer: &[u8],
    cmd: u32,
    arg1: u32,
    arg2: u32,
) -> Result<(u32, u32, u32), ErrorCode> {
    let result = BlockingResult {
        result: Cell::new(None),
    };

    do_allow_ro::<S>(driver_num, buffer_num, buffer)?;
    do_subscribe::<S>(driver_num, subscribe_num, &result)?;

    S::command(driver_num, cmd, arg1, arg2)
        .to_result::<(), ErrorCode>()?;

    Ok(wait_for_result::<S>(&result))
}

/// ALLOW_RO + ALLOW_RW + SUBSCRIBE + COMMAND + blocking wait.
///
/// Shares both a read-only and a read-write buffer with the kernel,
/// subscribes for a completion upcall, issues the command, then blocks
/// until the upcall fires.
pub fn subscribe_allow_ro_rw_and_wait<S: Syscalls>(
    driver_num: u32,
    subscribe_num: u32,
    ro_buffer_num: u32,
    ro_buffer: &[u8],
    rw_buffer_num: u32,
    rw_buffer: &mut [u8],
    cmd: u32,
    arg1: u32,
    arg2: u32,
) -> Result<(u32, u32, u32), ErrorCode> {
    let result = BlockingResult {
        result: Cell::new(None),
    };

    do_allow_ro::<S>(driver_num, ro_buffer_num, ro_buffer)?;
    do_allow_rw::<S>(driver_num, rw_buffer_num, rw_buffer)?;
    do_subscribe::<S>(driver_num, subscribe_num, &result)?;

    S::command(driver_num, cmd, arg1, arg2)
        .to_result::<(), ErrorCode>()?;

    Ok(wait_for_result::<S>(&result))
}

/// SUBSCRIBE + COMMAND + blocking wait (no buffer sharing).
///
/// Subscribes for a completion upcall, issues the command, then blocks
/// until the upcall fires. Use this when no buffers need to be shared
/// with the kernel (e.g., DMA from AXI addresses, erase operations).
pub fn subscribe_and_wait<S: Syscalls>(
    driver_num: u32,
    subscribe_num: u32,
    cmd: u32,
    arg1: u32,
    arg2: u32,
) -> Result<(u32, u32, u32), ErrorCode> {
    let result = BlockingResult {
        result: Cell::new(None),
    };

    do_subscribe::<S>(driver_num, subscribe_num, &result)?;

    S::command(driver_num, cmd, arg1, arg2)
        .to_result::<(), ErrorCode>()?;

    Ok(wait_for_result::<S>(&result))
}

/// Single-threaded synchronous mutex for Tock userspace.
///
/// Provides a `.lock()` API that returns `&mut T` directly, matching the
/// ergonomics of `embassy_sync::mutex::Mutex::lock().await` without requiring
/// async or allocations. Safe to use in statics (implements `Sync` + `Send`).
///
/// # Safety
/// Only correct in a single-threaded environment (no preemptive concurrency).
pub struct SyncMutex<T> {
    inner: core::cell::UnsafeCell<T>,
}

// Safety: single-threaded Tock userspace — no concurrent access possible
unsafe impl<T> Sync for SyncMutex<T> {}
unsafe impl<T> Send for SyncMutex<T> {}

impl<T> SyncMutex<T> {
    pub const fn new(val: T) -> Self {
        Self {
            inner: core::cell::UnsafeCell::new(val),
        }
    }

    /// Acquire exclusive access. Returns `&mut T` directly (no guard needed).
    pub fn lock(&self) -> &mut T {
        // Safety: single-threaded environment — no other code can access
        // this cell concurrently. Tock upcalls don't preempt user code.
        unsafe { &mut *self.inner.get() }
    }
}

/// Drop-in replacement for `embassy_sync::blocking_mutex::Mutex<CriticalSectionRawMutex, T>`.
///
/// Provides the same `.lock(|&T| ...)` closure API used throughout the codebase.
/// Safe in single-threaded Tock userspace — the closure runs inline without
/// actual locking since preemption is impossible.
pub struct CsMutex<T> {
    inner: core::cell::UnsafeCell<T>,
}

// SAFETY: single-threaded Tock userspace — no concurrent access possible.
unsafe impl<T> Sync for CsMutex<T> {}
unsafe impl<T> Send for CsMutex<T> {}

impl<T> CsMutex<T> {
    pub const fn new(val: T) -> Self {
        Self {
            inner: core::cell::UnsafeCell::new(val),
        }
    }

    /// Execute a closure with access to the inner value.
    ///
    /// Matches embassy's `blocking_mutex::Mutex::lock()` API.
    pub fn lock<R>(&self, f: impl FnOnce(&T) -> R) -> R {
        // Safety: single-threaded, non-preemptive Tock userspace.
        let inner = unsafe { &*self.inner.get() };
        f(inner)
    }
}

/// A lazily-initialized static value.
///
/// Replacement for `embassy_sync::lazy_lock::LazyLock`. Safe for single-threaded
/// Tock userspace: initialization runs at most once (on first `.get()` call).
///
/// # Safety
/// Uses `UnsafeCell` internally. Sound only in single-threaded, non-preemptive
/// environments (Tock userspace). Do not use in multi-threaded contexts.
pub struct SyncLazy<T, F = fn() -> T> {
    init: F,
    value: core::cell::UnsafeCell<Option<T>>,
}

// SAFETY: single-threaded Tock userspace — no concurrent access possible.
unsafe impl<T, F> Sync for SyncLazy<T, F> {}

impl<T, F: Fn() -> T> SyncLazy<T, F> {
    pub const fn new(init: F) -> Self {
        Self {
            init,
            value: core::cell::UnsafeCell::new(None),
        }
    }

    pub fn get(&self) -> &T {
        // SAFETY: single-threaded, non-preemptive environment.
        let slot = unsafe { &mut *self.value.get() };
        if slot.is_none() {
            *slot = Some((self.init)());
        }
        slot.as_ref().unwrap()
    }
}
