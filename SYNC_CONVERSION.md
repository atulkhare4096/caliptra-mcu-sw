# Sync Conversion: Removing Async/Await from the MCU User-App

## Summary

This document details the conversion of the `user-app` firmware binary from an
async/await architecture (built on Embassy) to a fully synchronous,
blocking-call architecture. The goal was to reduce binary size by eliminating
the overhead introduced by Rust async state machines, the Embassy executor, and
heap-allocated futures.

**Branch:** `dev/atul/sync_conversion`  
**Base:** `origin/main` (commit `871ab6c9`)  
**Target:** `riscv32imc-unknown-none-elf` (RISC-V 32-bit MCU, Tock OS)  
**Build profile:** `opt-level = "z"`, `lto = true`, `codegen-units = 1`, `panic = "abort"`

---

## Results

| Section | Before | After | Δ Bytes | Δ % |
|---------|-------:|------:|--------:|----:|
| `.text` | 144,346 | 90,576 | −53,770 | −37.3% |
| `.rodata` | 26,876 | 17,404 | −9,472 | −35.2% |
| `.data` | 60 | 32 | −28 | −46.7% |
| `.bss` | 60,056 | 30,920 | −29,136 | −48.5% |
| `.stack` | 44,544 | 44,544 | 0 | 0% |
| **Flash total** | **171,282** | **108,012** | **−63,270** | **−36.9%** |

129 files changed, 2,174 insertions, 2,427 deletions (net −253 lines).

---

## Why Async Was Expensive

The original architecture used Embassy (`embassy-executor`, `embassy-sync`) to
run cooperative async tasks on a single-threaded RISC-V MCU under Tock OS.
While ergonomic, this imposed significant binary costs:

1. **Async state machines** — Every `async fn` compiles to an enum state
   machine. Each `.await` point adds a variant. With dozens of async functions
   chained together, these enums grow large and deeply nested.

2. **Heap-allocated futures** — `TockSubscribe::subscribe()` returned a
   `Pin<Box<TockSubscribe>>`. Every syscall driver operation paid for a heap
   allocation, vtable pointer, and `Drop` glue.

3. **Embassy executor** — `TockExecutor` and the `Spawner` infrastructure
   pulled in task-queue management, waker registration, and polling loops — all
   unnecessary on a single-threaded system that can simply `yield_wait()`.

4. **`async_trait` boxing** — Trait methods marked `#[async_trait]` required
   `Box<dyn Future>` return types, adding allocation and dynamic dispatch.

5. **`embassy_sync::mutex::Mutex`** — Its `.lock()` method returned a Future.
   After removing `.await`, callers got an opaque `impl Future` instead of a
   guard, causing type errors everywhere.

6. **`embassy_sync::signal::Signal`** — Used for inter-task signaling, its
   `.wait()` was async. On a single-threaded system, an `AtomicBool` flag with
   `yield_wait()` polling achieves the same effect at zero cost.

---

## Approach

### Phase 1: Blocking Primitives (`blocking.rs`)

Created `runtime/userspace/libtockasync/src/blocking.rs` with stack-based
synchronous alternatives to `TockSubscribe`:

| Function | Purpose |
|----------|---------|
| `subscribe_and_wait<S>()` | SUBSCRIBE → COMMAND → `yield_wait` loop |
| `subscribe_allow_rw_and_wait<S>()` | ALLOW_RW → SUBSCRIBE → COMMAND → wait |
| `subscribe_allow_ro_and_wait<S>()` | ALLOW_RO → SUBSCRIBE → COMMAND → wait |
| `subscribe_allow_ro_rw_and_wait<S>()` | ALLOW_RO → ALLOW_RW → SUBSCRIBE → COMMAND → wait |

Key design decisions:
- **`BlockingResult` on the stack** — A `Cell<Option<(u32,u32,u32)>>` replaces
  the heap-allocated `TockSubscribe` struct. The kernel upcall writes directly
  to this cell.
- **No waker** — The caller spins in `yield_wait()` checking the cell. No
  waker registration or executor polling needed.
- **Raw syscalls** — `do_subscribe()`, `do_allow_rw()`, `do_allow_ro()` issue
  syscalls directly via `S::syscall4`, bypassing the `share::scope` / `Subscribe`
  builder pattern that existed to support async lifetimes.

Also added `SyncMutex<T>` — an `UnsafeCell`-based mutex safe for
single-threaded Tock userspace, providing `.lock() -> &mut T` without futures.

### Phase 2: Syscall Driver Conversion

Converted all 8 syscall drivers in `runtime/userspace/syscall/src/`:

| Driver | Key Change |
|--------|-----------|
| `console.rs` | `TockSubscribe` → `blocking::subscribe_allow_ro_and_wait` |
| `dma.rs` | Removed `share::scope`, direct `subscribe_and_wait` |
| `doe.rs` | Same pattern |
| `flash.rs` | Same pattern |
| `mailbox.rs` | `PayloadStream::read` changed from `async fn` → `fn` |
| `mbox_sram.rs` | Same pattern |
| `mctp.rs` | Same pattern |
| `mcu_mbox.rs` | Removed `MCU_MBOX_MUTEX` (embassy async Mutex) |

Common transformation:
```rust
// Before (async)
async fn write(buf: &[u8]) -> Result<(), ErrorCode> {
    share::scope::<_, _, S, _>(|_allow_ro| async {
        let sub = TockSubscribe::subscribe_allow_ro::<S>(DRIVER, 0, 0, buf);
        S::command(DRIVER, cmd::WRITE, buf.len() as u32, 0)
            .to_result()?;
        sub.await.map(|_| ())
    }).await
}

// After (sync)
fn write(buf: &[u8]) -> Result<(), ErrorCode> {
    blocking::subscribe_allow_ro_and_wait::<S>(
        DRIVER, 0, 0, buf, cmd::WRITE, buf.len() as u32, 0,
    )?;
    Ok(())
}
```

### Phase 3: Mass Async Removal

Used targeted `sed` passes across ~80 files to:
1. Remove `async fn` → `fn`
2. Remove `.await`
3. Remove `#[async_trait]` and `#[async_trait(?Send)]`
4. Remove `use async_trait::async_trait`
5. Remove `#[allow(async_fn_in_trait)]`

### Phase 4: Structural Fixes

After the mass removal, several categories of breakage required manual fixes:

#### Embassy Mutex → RefCell / SyncMutex

| Location | Replacement |
|----------|------------|
| `pldm-lib/src/firmware_device/fd_internal.rs` | `Mutex<NoopRawMutex, T>` → `RefCell<T>`, `.lock()` → `.borrow()` / `.borrow_mut()` |
| `spdm/cert_store/cert_chain/leaf.rs` | `Mutex<CriticalSectionRawMutex, T>` → `SyncMutex<T>` |
| `spdm/device_cert_store.rs` | Same |

#### Embassy Signal → AtomicBool

| Location | Replacement |
|----------|------------|
| `pldm-lib/src/daemon.rs` | `Signal<..., ()>` → `AtomicU8` flag |
| `mcu-mbox-lib/src/fips_periodic.rs` | `Signal<..., ()>` → `AtomicBool` + `yield_wait()` poll |

#### Spawner Removal

The Embassy `Spawner` was threaded through many structs and functions. All
`spawner.spawn(task_fn())` calls were replaced with direct `task_fn()` calls:

| Component | Change |
|-----------|--------|
| `PldmService` | Removed `spawner` field; `start()` calls `pldm_service_loop()` directly |
| `McuMboxService` | Removed `spawner` field; `start()` calls `mcu_mbox_responder_task()` |
| `spawn_vdm_responder()` | Removed `Spawner` param; calls `vdm_responder_task()` directly |
| `FirmwareUpdater` | Removed `spawner` field |
| `PldmImageLoader` | Removed `spawner` field |
| `initialize_pldm()` | Removed `Spawner` param; calls `pldm_service_task()` directly |

#### PLDM Daemon Restructuring

The original PLDM daemon spawned two tasks:
- `pldm_responder_task` — handled incoming messages
- `pldm_initiator_task` — waited on a `Signal`, then ran firmware download

After conversion, these were merged into `pldm_service_loop()`:
```rust
fn pldm_service_loop(cmd_interface: &'static CmdInterface, running: &'static AtomicBool) {
    while running.load(Ordering::SeqCst) {
        // Handle one responder message
        cmd_interface.handle_responder_msg(&mut transport, &mut msg_buffer);

        // If download state, run initiator inline
        if cmd_interface.should_start_initiator_mode() {
            pldm_initiator_inline(cmd_interface, running, &mut transport, &mut msg_buffer);
        }
    }
}
```

#### PLDM Signal Handshake → `run_until()` (Phase-Driven Service Loop)

The original async design used `embassy_sync::Signal` to coordinate a
producer-consumer handshake between the PLDM service loop (running as an
async task) and the image loading / firmware update caller (running as
another async task). Signals synchronized phase transitions:
- Caller sets download state → signals PLDM task
- PLDM task processes messages → signals caller on completion
- Caller proceeds to next phase

After removing `.await`, `Signal::wait()` created a `Future` that was
immediately dropped — effectively a **no-op**. And `pldm_service_task()`
(which called `PldmService::start()`) blocked forever in the service loop,
so the caller never regained control.

**Solution:** Added `PldmService::run_until<F: Fn() -> bool>()` — a method
that drives the service loop until an arbitrary predicate becomes true,
then returns control to the caller:
```rust
impl PldmService<'_> {
    pub fn run_until<F: Fn() -> bool>(&mut self, done: F) -> Result<(), PldmServiceError> {
        while self.running.load(Ordering::SeqCst) && !done() {
            cmd_interface.handle_responder_msg(...);
            if cmd_interface.should_start_initiator_mode() {
                pldm_initiator_inline(...);
            }
        }
        Ok(())
    }
}
```

Callers now drive the service phase-by-phase:
```rust
// image_loading/pldm_client.rs
pub fn initialize_pldm(...) -> Result<PldmService<'a>, ErrorCode> {
    let mut service = PldmService::init(fd_ops);
    service.run_until(|| get_pldm_state() == State::Initialized)?;
    pldm_download_header(&mut service)?;
    Ok(service)
}

// Caller (load_and_authorize):
let mut service = pldm_client::initialize_pldm(...)?;
let (offset, size) = pldm_client::pldm_download_toc(&mut service, component_id)?;
pldm_client::pldm_download_image(&mut service, load_address, offset, size)?;
```

The fdops callbacks (invoked from within the service loop) simply set state
transitions in `PLDM_STATE`; no signaling needed — the `run_until` predicate
sees the change on the next iteration.

Same pattern for firmware update:
```rust
let mut service = pldm_client::initialize_pldm(...)?;
pldm_client::pldm_wait(&mut service, State::Verifying)?;
// ... verify inline ...
pldm_client::pldm_set_verification_result(VerifyResult::VerifySuccess);
pldm_client::pldm_wait(&mut service, State::Apply)?;
```

#### Timer Conversion

`pldm-lib/src/timer.rs` used `TockSubscribe::subscribe` + `ALARM_MUTEX`. Replaced with:
```rust
fn sleep_ticks(ticks: u32) -> Result<(), ErrorCode> {
    blocking::subscribe_and_wait::<S>(DRIVER_NUM, 0, command::SET_RELATIVE, ticks, 0)?;
    Ok(())
}
```

### Phase 5: Entry Point

The riscv entry point called `caliptra_mcu_libtockasync::start_async(start())`
which expected a `SpawnToken`. Changed to a direct `crate::start()` call.

The main loop changed from:
```rust
// Before: Embassy executor polls async tasks
loop { EXECUTOR.get().poll(); }

// After: Direct synchronous calls (first blocking call runs to completion)
spdm::spdm_task();
image_loader::image_loading_task();
mcu_mbox::mcu_mbox_task();
```

---

## What Remains

The following Embassy dependencies are still referenced but no longer on the
runtime hot path:

- **`embassy_sync::blocking_mutex`** — Used by `caliptra-api` for
  `pldm_context.rs` (`Mutex<CriticalSectionRawMutex, RefCell<T>>` in
  `PLDM_STATE` / `DOWNLOAD_CTX`). These are the *blocking* (non-async) mutex
  from Embassy, not the async one. They could be replaced with bare
  `critical_section` crate usage.

- **`embassy_sync::lazy_lock::LazyLock`** — Used for `DESCRIPTOR`,
  `FIRMWARE_PARAMS`, `PLDM_PROTOCOL_CAPABILITIES` config statics. Could be
  replaced with `core::cell::OnceCell` or a `static mut` + init guard.

- **`embassy-executor`** — Still in workspace `Cargo.toml`, referenced by
  `libtockasync/src/tock_executor.rs` and `lib.rs`. The `TockExecutor` and
  `Spawner` types still exist in the library but are never instantiated by
  the user-app after the EXECUTOR static was removed.

All `embassy_sync::signal::Signal` usage has been removed. The PLDM
handshake was replaced with `PldmService::run_until()` (see above).

Removing the remaining dependencies could yield additional size savings but
is low priority — they are compile-time-only overhead at this point.

---

## Lessons Learned

1. **Async is expensive on embedded** — On a single-threaded MCU with no
   preemption, async/await adds ~63 KB of flash overhead (37%) with zero
   benefit. The cooperative scheduling that Embassy provides is unnecessary
   when there's only one thread.

2. **Stack-based blocking is simpler and smaller** — The `BlockingResult` +
   `yield_wait` loop pattern is trivially correct, needs no heap, and compiles
   to minimal code.

3. **Mass sed + targeted fixes works** — Removing `async`/`.await` with sed
   across 80+ files is fast. The ~20 files that needed manual structural fixes
   (Mutex, Signal, Spawner) were a manageable subset.

4. **BSS savings are significant** — Nearly half the BSS reduction came from
   removing the `TockExecutor` static, Embassy task queues, and
   `Mutex`/`Signal` statics.

5. **Trait objects (`dyn FdOps`, `dyn SpdmTransport`) survive** — The sync
   conversion preserved all trait-object boundaries. The only change was
   removing `async` from trait method signatures.

6. **`run_until()` replaces inter-task signals** — When two async tasks
   coordinated via a Signal handshake, the sync equivalent is a service loop
   with a predicate-based exit condition. The caller drives the loop forward
   phase-by-phase, checking shared state between phases. This preserves the
   same logical flow without requiring a scheduler.

7. **`AtomicBool::swap()` unavailable on riscv32imc** — The target lacks
   atomic RMW instructions. Use `load()` + `store()` instead (safe since
   the system is single-threaded and non-preemptive).

---

## Maintainability Assessment

### What improved

**Readability.** The sync code is substantially easier to follow. Every
function call does what it says and returns when it's done. There are no hidden
state machines, no poll/wake cycles to reason about, and no `Pin<Box<...>>`
indirection. A developer reading `blocking::subscribe_and_wait()` immediately
understands: subscribe, issue command, spin until upcall fires. The async
equivalent required understanding `TockSubscribe` internals, `share::scope`
lifetime gymnastics, and the Embassy executor's polling model.

**Debugging.** Stack traces are now meaningful. In the async version, a panic
inside a deeply-nested `.await` chain produced a trace through
`embassy_executor::raw::TaskStorage::poll` → opaque state machine variant
numbers. Now traces show the actual call chain. GDB breakpoints work on the
real function bodies instead of compiler-generated `poll()` methods.

**Dependency surface.** The runtime no longer depends on Embassy's executor or
its async mutex/signal types at the hot path. Fewer upstream crate updates to
track, fewer potential breaking changes, and fewer features to audit for a
security-sensitive firmware target.

**Onboarding.** A new contributor no longer needs to understand Rust's async
model, Embassy's executor architecture, or the `#[embassy_executor::task]`
macro to work on driver or protocol code. The code reads like straightforward
embedded C ported to Rust.

### What got worse

**Concurrency model is now sequential.** The async version could interleave
multiple tasks (SPDM responder, MCU mailbox, PLDM, VDM) within a single
thread via cooperative scheduling. The sync version calls them sequentially in
`async_main()`:
```rust
spdm::spdm_task();       // blocks forever handling SPDM messages
image_loader::...();      // never reached unless spdm_task returns
mcu_mbox::...();          // never reached
```
Only the first task actually runs. This works for testing (where each test
binary exercises one protocol) but is a regression for a production scenario
that needs multiple services running concurrently. Restoring concurrency
would require either:
- A simple round-robin loop that calls non-blocking "poll one message"
  functions from each service, or
- Re-introducing a lightweight task scheduler (much simpler than Embassy).

**Remaining Embassy dependencies.** The `embassy_sync::blocking_mutex` and
`embassy_sync::lazy_lock::LazyLock` types are still used for PLDM context
statics and configuration. These are functional (non-async) and work
correctly, but represent an avoidable dependency.

**`SyncMutex` is unsound in general.** The `SyncMutex<T>` added in
`blocking.rs` uses `UnsafeCell` and returns `&mut T` from `.lock()` without
any runtime borrow checking. It is safe only because Tock userspace is
single-threaded and non-preemptive — kernel upcalls do not interrupt user
code mid-function. If either assumption changes (e.g., multi-threaded Tock
apps, or signal-like preemption), every `SyncMutex` usage becomes UB. This
should be documented with a clear `// SAFETY:` invariant, and ideally gated
behind a `cfg` so it cannot accidentally be used in a multi-threaded
context.

**PLDM initiator/responder interleaving is sequential.** The original design ran
the responder and initiator as separate Embassy tasks so they could be
polled independently. The merged `pldm_service_loop` (and `run_until`)
handles one responder message, then optionally runs the initiator inline.
This means the initiator's download loop blocks further responder processing
until it yields (via `sleep_ticks`). In practice this is fine because the
download protocol is lock-step (request-response), but it is a subtle
behavioral change that could matter if the UA sends unsolicited messages
during download.

### Net assessment

For a **single-purpose MCU firmware** running one protocol at a time under
test, the sync version is strictly better: simpler, smaller, faster to
build, easier to debug. The 37% flash reduction alone justifies it for
resource-constrained targets.

For a **production multi-service firmware** that needs SPDM + PLDM + MCU
mailbox + VDM running concurrently, the sync conversion is incomplete. The
sequential call model needs to be replaced with a cooperative poll loop.
This is a smaller and simpler piece of infrastructure than Embassy's full
executor, but it does need to be built.

The recommended path forward is:
1. ~~Fix the remaining broken call sites~~ ✅ Done.
2. Convert each service's main loop from "block forever" to "poll one
   message and return," with a `PollResult` enum (`Handled`, `WouldBlock`).
3. Add a top-level round-robin loop in `async_main()` that calls each
   service's poll function and `yield_wait()` when all return `WouldBlock`.
4. Remove the remaining Embassy dependencies entirely (`embassy_sync::blocking_mutex`,
   `embassy_sync::lazy_lock`, `embassy-executor` crate).

