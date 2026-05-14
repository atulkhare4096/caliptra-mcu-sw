# Async Code Size Experiment Results

## Objective

Determine whether async Rust code size can be made competitive with sync through
structural optimizations, without converting handlers from async to sync.

## Baseline Measurements

| Configuration | .text | .rodata | .bss | Flash (.text+.rodata+.data) |
|---|---|---|---|---|
| **Sync branch** (dev/atul/sync_conversion) | 83,040 | 10,884 | 31,960 | 93,956 |
| **Async optimized** (dev/atul/async_opt) | 132,858 | 18,040 | 48,944 | 150,958 |
| **Gap** | +49,818 | +7,156 | +16,984 | +57,002 |

## Key Finding: The `dispatch_request` State Machine

The single largest symbol in the async binary:

```
dispatch_request poll fn: 49,144 bytes (.text)
```

This ONE function is 37% of total .text and accounts for virtually the entire
sync-vs-async .text gap (49,818 bytes). It contains all 8 handler poll functions
inlined by LTO.

## Experiments Tried

### 1. `#[inline(always)]` on dispatch_request
- **Result**: No change (LTO already inlines)

### 2. `#[inline(never)]` on dispatch_request
- **Result**: +84 bytes (LTO ignores it for async poll fns)

### 3. Remove `Box::pin` (inline futures directly)
- **Result**: .text +2,046, .bss +11,096
- Box::pin was actually helping reduce BSS (task POOL size)
- Confirms Box::pin is the right choice for the current async architecture

### 4. `dyn Future` (dynamic dispatch to prevent inlining)
- **Result**: .text +518, .rodata +92
- Handler code still exists as separate functions, total size unchanged
- vtable overhead slightly increases size

### 5. Remove measurements_rsp + key_exchange_rsp handlers (2 largest, 20+ await points each)
- **Result**: .text -28,008 (104,850), dispatch poll fn: 33,534 (from 49,144)
- Two handlers alone contribute 28KB

### 6. Remove ALL 8 async handlers (stubs only)
- **Result**: .text 44,642 (-88,216), .rodata 9,412, .bss 45,768
- dispatch_request poll fn vanishes entirely (no await points = no state machine)

## Analysis

```mermaid
%%{init: {'theme': 'base', 'themeVariables': {'fontSize': '14px'}}}%%
pie title .text Breakdown — Async Architecture (132,858 bytes)
    "dispatch_request poll fn" : 49144
    "Other handler code" : 39072
    "Non-handler code (transport, runtime, etc.)" : 44642
```

```mermaid
%%{init: {'theme': 'base'}}%%
xychart-beta
    title ".text Size Comparison"
    x-axis ["Sync handlers", "Async overhead", "Base code"]
    y-axis "Bytes" 0 --> 50000
    bar [39000, 49000, 44642]
```

The async state machine overhead is:
- **49KB of .text** — entirely from state machine poll code
- **~3KB of .bss** — task POOL size increase
- **~7KB of .rodata** — state transition tables, vtables

### Why structural optimizations can't fix this:

1. **LTO inlines everything**: Regardless of Box::pin, inline attributes, or dyn dispatch,
   the state machine code exists somewhere. LTO either inlines it (49KB monolith) or
   keeps it separate (same total code, slightly more overhead from vtables).

2. **State machines are inherently larger**: Each await point creates enum variants,
   save/restore code for all live locals, and poll/match logic. This 2-3x size
   multiplier is fundamental to how `async fn` compiles.

3. **No middle ground**: You either have the state machine (full async overhead) or
   you don't (sync code). There's no way to get "partial" savings without actually
   removing await points.

## Architectural Insight: Async Belongs at the Task Boundary

The experiment reveals a clear principle: **async is the right model for transport
(waiting for network packets), but the wrong model for handler internals (sequential
mailbox round-trips).**

### Two categories of I/O in this system

| Category | Examples | Latency | Concurrent work? | Right model |
|---|---|---|---|---|
| **Transport** | MCTP receive/send | ms–seconds | Yes (other tasks) | **Async** |
| **Crypto/mailbox** | hash, sign, DPE | microseconds | No | **Sync (blocking)** |

Transport operations (MCTP packet receive/send) are the *only* points where the MCU
legitimately has nothing to do but wait for an external event. While waiting, other
embassy tasks (PLDM, MCTP-VDM) can productively run. Async is correct here.

Every await point inside the handlers is a Caliptra mailbox round-trip — these complete
in microseconds, there's no useful work to interleave, and the handler cannot make
progress until the result returns. These are **blocking operations dressed up as async**.
The `async` keyword buys nothing except 49KB of state machine overhead.

### Before: Fully Async Architecture

```mermaid
graph TD
    subgraph executor["Embassy Executor"]
        subgraph spdm_task["spdm_task (async fn)"]
            recv["transport.receive().await"] --> dispatch
            dispatch["dispatch_request().await"] --> send
            send["transport.send().await"] --> recv

            subgraph dispatch_sm["dispatch_request poll fn — 49KB state machine"]
                d1["Box::pin digests_rsp.await
12 await points"]
                d2["Box::pin challenge_rsp.await
16 await points"]
                d3["Box::pin measurements_rsp.await
20 await points"]
                d4["Box::pin key_exchange_rsp.await
21 await points"]
                d5["... 4 more async handlers"]
            end

            subgraph handler_internals["Each handler awaits mailbox"]
                h1["hash.init().await"] --> mb1["mailbox.execute().await"]
                h2["hash.update().await"] --> mb2["mailbox.execute().await"]
                h3["sign_hash().await"] --> mb3["mailbox.execute().await"]
            end

            subgraph tock_sub["TockSubscribe (heap-allocated)"]
                box_alloc["Box::new + Pin"] --> waker["Waker"] --> future_poll["Future::poll"]
            end
        end

        pldm["pldm_task (async)"]
        vdm["mctp_vdm_task (async)"]
    end

    style dispatch_sm fill:#ff6b6b,color:#fff
    style handler_internals fill:#ffa07a
    style tock_sub fill:#ffa07a
    style recv fill:#4ecdc4
    style send fill:#4ecdc4
    style dispatch fill:#ff6b6b,color:#fff
```

### After: Hybrid Architecture (async transport + sync handlers)

```mermaid
graph TD
    subgraph executor["Embassy Executor"]
        subgraph spdm_task["spdm_task (async fn — 2 await points only)"]
            recv["transport.receive().await"] --> dispatch
            dispatch["dispatch_request(&msg)"] --> send
            send["transport.send().await"] --> recv

            subgraph dispatch_sync["dispatch_request — plain match (no state machine)"]
                d1["handle_get_digests()
plain fn"]
                d2["handle_challenge()
plain fn"]
                d3["handle_measurements()
plain fn"]
                d4["handle_key_exchange()
plain fn"]
                d5["... 4 more sync handlers"]
            end

            subgraph handler_internals["Each handler calls blocking mailbox"]
                h1["hash.init()"] --> mb1["mailbox.execute_blocking()"]
                h2["hash.update()"] --> mb2["mailbox.execute_blocking()"]
                h3["sign_hash()"] --> mb3["mailbox.execute_blocking()"]
            end

            subgraph blocking["BlockingResult (stack-allocated)"]
                cell["Cell on stack"] --> yield_w["yield_wait loop"] --> upcall["kernel upcall sets Cell"]
            end
        end

        pldm["pldm_task (async)"]
        vdm["mctp_vdm_task (async)"]
    end

    style dispatch_sync fill:#4ecdc4
    style handler_internals fill:#b8e6b8
    style blocking fill:#b8e6b8
    style recv fill:#4ecdc4
    style send fill:#4ecdc4
    style dispatch fill:#4ecdc4
```

The blocking mailbox uses a stack-allocated `Cell` + `yield_wait` loop instead of
`TockSubscribe` (which requires `Box::new`, `Pin`, `Waker`, `Future` trait impl).
This is safe because Tock userspace is single-threaded and cooperative — `yield_wait`
returns to the kernel, which fires the upcall setting the Cell, and the loop reads it.

### Why this eliminates 49KB

```mermaid
graph LR
    subgraph before["BEFORE: async handler compile output"]
        direction TB
        enum["enum DispatchState<br/>Variant0_PreDigests<br/>Variant1_AwaitHash1<br/>Variant2_AwaitHash2<br/>...<br/>Variant108_Final"] --> poll_fn["fn poll()<br/>match state: save/restore<br/>locals for every await point"]
        poll_fn --> size1["49,144 bytes"]
    end

    subgraph after["AFTER: sync handler compile output"]
        direction TB
        match_stmt["match req_code<br/>GetDigests => fn()<br/>Challenge => fn()<br/>..."] --> stack["Normal stack frames<br/>No save/restore<br/>No enum variants"]
        stack --> size2["~0 bytes overhead"]
    end

    style before fill:#ff6b6b,color:#fff
    style after fill:#4ecdc4
    style size1 fill:#ff6b6b,color:#fff
    style size2 fill:#4ecdc4
```

The 49KB `dispatch_request` poll function exists because each of the 8 handlers is an
`async fn` with 12–21 await points. The compiler generates an enum variant for every
suspend point, with save/restore code for all live locals. With sync handlers, dispatch
is a plain `match` calling regular functions — zero state machines, zero save/restore,
zero poll/match logic. The compiler uses normal stack frames instead.

### Trade-off: None

There is no behavioral difference. In the async model, `mailbox.execute().await` calls
`yield_wait` internally via the Tock executor's poll loop — the single-threaded executor
cannot run other embassy tasks while the current task's future is being polled. Other
tasks only get a chance to run when the *outermost* `.await` (transport receive/send)
yields back to the executor.

Since mailbox operations complete in microseconds and the executor is single-threaded,
both models block identically on crypto/mailbox I/O. The only difference is whether the
compiler generates a 49KB state machine around those blocking points.

## Conclusion

**Async code size cannot be made competitive with sync through structural optimizations.**

The only way to eliminate the ~49KB overhead is to convert handlers from `async fn` to
regular `fn` while keeping transport async — the hybrid architecture.

### Estimated sizes with hybrid (async transport + sync handlers):
- .text: ~84,000 (44,642 base + ~39,000 sync handler code)
- This matches the sync branch's 83,040 almost exactly
- The async transport shell (embassy executor, TockSubscribe) adds minimal overhead (~1–2KB)

---

## Implementation Plan

```mermaid
graph BT
    subgraph L0["Phase 1 — Blocking Infrastructure"]
        blocking["blocking.rs<br/>BlockingResult + yield_wait"]
    end

    subgraph L1["Phase 2 — Syscall Drivers"]
        mailbox["mailbox.rs<br/>execute_blocking()"]
        flash["flash.rs"]
        dma["dma.rs"]
        doe["doe.rs"]
        mctp_sys["mctp.rs<br/>⚡ KEEP ASYNC"]
    end

    subgraph L2["Phase 3 — Caliptra API"]
        mbox_api["mailbox_api.rs"]
        crypto["crypto/<br/>hash, asym, aes_gcm"]
        cert["certificate.rs"]
        fw_update["firmware_update/"]
        img_load["image_loading/"]
    end

    subgraph L3["Phase 4 — SPDM Handlers ⭐ 49KB savings"]
        handlers["digests, certificate,<br/>challenge, measurements,<br/>key_exchange, finish,<br/>chunk_get, vendor_defined"]
        ctx["context.rs<br/>dispatch_request"]
        spdm_support["cert_store, transcript,<br/>signature, session"]
    end

    subgraph L4["Phase 5 — Library Daemons"]
        pldm["pldm-lib"]
        vdm_lib["mctp-vdm-lib"]
        mbox_lib["mcu-mbox-lib"]
    end

    subgraph L5["Phase 6 — Platform App"]
        app["user app<br/>spdm/mod.rs, riscv.rs"]
    end

    L1 --> L0
    L2 --> L1
    L3 --> L2
    L4 --> L3
    L5 --> L4

    style L0 fill:#e8e8e8
    style L1 fill:#d4e6f1
    style L2 fill:#d5f5e3
    style L3 fill:#fcf3cf
    style L4 fill:#fadbd8
    style L5 fill:#e8daef
    style mctp_sys fill:#4ecdc4,color:#fff
```

### Phase 1: Blocking Infrastructure (Layer 0)

Add the blocking syscall primitives that replace `TockSubscribe` for handler-internal I/O.

**Files to create:**
- `runtime/userspace/libtockasync/src/blocking.rs` — `BlockingResult` struct (stack Cell),
  `blocking_upcall` (extern "C"), `wait_for_result` (yield_wait loop),
  `subscribe_allow_ro_rw_and_wait`, `subscribe_allow_rw_and_wait`,
  `subscribe_allow_ro_and_wait`, `subscribe_and_wait`

**Files to modify:**
- `runtime/userspace/libtockasync/src/lib.rs` — export `pub mod blocking`
- `runtime/userspace/libtockasync/Cargo.toml` — remove `embassy-sync` dependency
  (no longer needed once Mutex is replaced)

### Phase 2: Syscall Drivers (Layer 1)

Convert all kernel-facing drivers from async to sync using the blocking primitives.

**Files to modify:**
| File | Change |
|---|---|
| `runtime/userspace/syscall/src/mailbox.rs` | `execute()` / `execute_with_payload_stream()`: async→sync, replace TockSubscribe with `blocking::subscribe_allow_ro_rw_and_wait`, remove `MAILBOX_MUTEX` (single-threaded, no contention without async) |
| `runtime/userspace/syscall/src/flash.rs` | `write()`/`read()`/`erase()`: async→sync |
| `runtime/userspace/syscall/src/dma.rs` | DMA ops: async→sync |
| `runtime/userspace/syscall/src/doe.rs` | DOE ops: async→sync |
| `runtime/userspace/syscall/src/mctp.rs` | **Keep async** — this is transport (genuine wait-for-packet) |
| `runtime/userspace/syscall/src/mcu_mbox.rs` | async→sync |
| `runtime/userspace/syscall/src/mbox_sram.rs` | async→sync |
| `runtime/userspace/syscall/src/logging.rs` | async→sync |
| `runtime/userspace/syscall/Cargo.toml` | Remove `embassy-sync`, `async-trait` deps if no longer needed |

### Phase 3: Caliptra API Layer (Layer 2)

Convert the middleware that wraps mailbox commands into typed crypto/cert operations.

**Files to modify:**
| File | Change |
|---|---|
| `runtime/userspace/api/caliptra-api/src/mailbox_api.rs` | `execute_mailbox_cmd()`: async→sync |
| `runtime/userspace/api/caliptra-api/src/crypto/hash.rs` | `init()`/`update()`/`finalize()`: async→sync |
| `runtime/userspace/api/caliptra-api/src/crypto/asym.rs` | Signing ops: async→sync |
| `runtime/userspace/api/caliptra-api/src/crypto/aes_gcm.rs` | AES-GCM ops: async→sync |
| `runtime/userspace/api/caliptra-api/src/certificate.rs` | DPE cert ops: async→sync |
| `runtime/userspace/api/caliptra-api/src/error.rs` | Remove async-related error variants if any |
| `runtime/userspace/api/caliptra-api/src/firmware_update/*.rs` | async→sync (4 files) |
| `runtime/userspace/api/caliptra-api/src/image_loading/*.rs` | async→sync (4 files) |
| `runtime/userspace/api/caliptra-api/Cargo.toml` | Remove `async-trait` dep |

### Phase 4: SPDM Handlers (Layer 3) — The Big Win

Convert all 8 async command handlers to plain `fn`. This is where the 49KB savings come from.

**Files to modify (in `runtime/userspace/api/spdm-lib/src/`):**
| File | Await points removed |
|---|---|
| `commands/digests_rsp.rs` | 12 |
| `commands/certificate_rsp.rs` | 13 |
| `commands/challenge_auth_rsp.rs` | 16 |
| `commands/finish_rsp.rs` | 14 |
| `commands/key_exchange_rsp.rs` | 21 |
| `commands/measurements_rsp.rs` | 20 |
| `commands/chunk_get_rsp.rs` | ~8 |
| `commands/vendor_defined_rsp.rs` | ~5 |
| `context.rs` | `dispatch_request`: async→sync (remove all Box::pin), `send_response`/`handle_request`: keep async (transport) |
| `cert_store.rs` | Hash operations: async→sync |
| `protocol/signature.rs` | Sign/verify: async→sync |
| `session/*.rs` | Session crypto (HMAC, AEAD): async→sync |
| `transcript.rs` | Transcript hashing: async→sync |
| `measurements.rs` | Measurement fetching: async→sync |
| `transport/mctp.rs` | **Keep async** — transport layer |
| `vdm_handler/**/*.rs` | Remove `#[async_trait]`, convert to sync (~6 files) |
| `Cargo.toml` | Remove `async-trait` dep |

### Phase 5: Library Daemons & Transport (Layer 4)

Convert library-level daemons while keeping transport async.

**Files to modify:**
| File | Change |
|---|---|
| `runtime/userspace/api/pldm-lib/src/*.rs` | `cmd_interface`, `transport`, `timer`, `fd_ops`: async→sync for handler logic, keep transport async |
| `runtime/userspace/api/mctp-vdm-lib/src/*.rs` | Same pattern |
| `runtime/userspace/api/mcu-mbox-lib/src/*.rs` | Same pattern |
| `runtime/userspace/api/caliptra-common-commands/src/lib.rs` | Remove `#[async_trait]` from 2 trait defs |

### Phase 6: Platform App Integration (Layer 5)

Wire up the emulator app to use sync handlers with async task shell.

**Files to modify:**
| File | Change |
|---|---|
| `platforms/emulator/.../apps/user/src/spdm/mod.rs` | Task stays async, handler calls become sync |
| `platforms/emulator/.../apps/user/src/spdm/device_cert_store.rs` | Cert store impl: async→sync |
| `platforms/emulator/.../apps/user/src/spdm/cert_store/cert_chain/leaf.rs` | Leaf cert: async→sync |
| `platforms/emulator/.../apps/user/src/spdm/shared_large_msg_buf.rs` | Shared buf: async→sync |
| `platforms/emulator/.../apps/user/src/vdm/mod.rs` | VDM handler: async→sync |
| `platforms/emulator/.../apps/user/src/firmware_update/mod.rs` | FW update: async→sync |
| `platforms/emulator/.../apps/user/src/image_loader/*.rs` | Image loader: async→sync |
| `platforms/emulator/.../apps/user/src/riscv.rs` | Main task wiring |
| Various `Cargo.toml` files | Remove `async-trait`, `embassy-sync` deps |

### Phase 7: Validation

1. Build for riscv32imc-unknown-none-elf and verify flash size ≤ 98,304 bytes
2. Run integration tests (`tests/integration/`)
3. Verify SPDM responder functional correctness
4. Compare section sizes against sync branch expectations

### Conversion Pattern (mechanical per-function)

```mermaid
graph LR
    subgraph before["BEFORE"]
        direction TB
        sig_b["pub async fn execute_mailbox_cmd(...)"]
        body_b[".await on every call"]
        trait_b["#[async_trait(?Send)]"]
        dep_b["extern crate alloc<br/>use Box"]
    end

    subgraph after["AFTER"]
        direction TB
        sig_a["pub fn execute_mailbox_cmd(...)"]
        body_a["direct synchronous calls"]
        trait_a["plain trait (no macro)"]
        dep_a["no alloc needed"]
    end

    sig_b -- "remove async" --> sig_a
    body_b -- "remove .await" --> body_a
    trait_b -- "remove attribute" --> trait_a
    dep_b -- "remove imports" --> dep_a

    style before fill:#ff6b6b,color:#fff
    style after fill:#4ecdc4
```

```rust
// BEFORE (async)
pub async fn execute_mailbox_cmd(
    mailbox: &Mailbox, cmd: u32, req: &mut [u8], resp: &mut [u8],
) -> Result<usize> {
    mailbox.execute(cmd, req, resp).await
}

// AFTER (sync)
pub fn execute_mailbox_cmd(
    mailbox: &Mailbox, cmd: u32, req: &mut [u8], resp: &mut [u8],
) -> Result<usize> {
    mailbox.execute(cmd, req, resp)
}
```

For traits: remove `#[async_trait(?Send)]`, `async` from methods, `.await` from calls,
and `extern crate alloc` / `Box` imports that async_trait required.

### Estimated effort

~50 files, ~200 mechanical edits (remove `async`, `.await`, `#[async_trait]`).
The changes are **bottom-up**: each layer can be converted and tested independently.
Phase 1–2 can be validated by building just the syscall crate. Phase 3–4 is the
bulk of the work and the source of nearly all size savings.
