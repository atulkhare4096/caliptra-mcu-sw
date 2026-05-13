# User-App Binary Size Optimization Report

**Binary:** `target/riscv32imc-unknown-none-elf/release/user-app`
**Target:** riscv32imc-unknown-none-elf (RISC-V 32-bit MCU, no OS)
**Build profile:** `opt-level = "z"`, `lto = true`, `codegen-units = 1`
**Toolchain:** Rust 1.85
**Linker:** rust-lld with `-nmagic`, `-icf=all`

---

## 1. Results

| Section | Original | Final | Delta |
|:--|--:|--:|:--|
| .text | 144,346 | 132,858 | **−11,488 (−8.0%)** |
| .rodata | 26,876 | 18,040 | **−8,836 (−32.9%)** |
| .data | 60 | 60 | 0 |
| .bss | 60,056 | 48,944 | **−11,112 (−18.5%)** |
| .stack | 44,544 | 44,544 | 0 |
| **Flash (.text+.rodata+.data)** | **171,282** | **150,958** | **−20,324 (−11.9%)** |
| **Total RAM (.bss+.stack+.data)** | **104,660** | **93,548** | **−11,112 (−10.6%)** |

---

## 2. Changes Made

All changes preserve functional correctness. Tests: 15/15 pass, 0 warnings, 0 errors.

| # | Change | .text Δ | .rodata Δ | .bss Δ | Flash Δ |
|:--|:--|--:|--:|--:|--:|
| 1 | Replace `async_trait` with native AFIT + generics on 5 traits (25 methods); convert 4 zero-await handlers to sync | −1,698 | 0 | +8 | −1,698 |
| 2 | `--remap-path-prefix` to strip build paths from panic strings | 0 | −4,134 | 0 | −4,134 |
| 3 | Replace `#[derive(Debug)]` with `impl Debug { f.write_str("TypeName") }` on 16 error enums | −10,198 | −4,130 | 0 | −14,328 |
| 4 | `Box::pin()` 8 async handler calls in `dispatch_request` match arms | −3,062 | 0 | −11,120 | −3,062 |
| 5 | `Box::pin()` `send_response` calls; static `CLAIMS_BUF` | −294 | minor | 0 | −294 |
| 6 | Split `generate_claims` into async gather + `#[inline(never)]` sync builder | −254 | −144 | 0 | −398 |

### Detailed descriptions

**#1 — Async trait optimization.** Replaced `async_trait` crate (which boxes every call as `Box<dyn Future + Send>`) with native async fn in traits (AFIT, stable since Rust 1.75) + generic parameters. Introduced `SpdmProvider` trait to bundle `Transport`, `CertStore`, `Measurements` as associated types. `VdmResponder` retains `async_trait(?Send)` — it has genuine heterogeneous dispatch via `&mut [&mut dyn VdmHandler]`. Also extracted non-generic Codec helpers to deduplicate 50+ monomorphized copies.

**#2 — Path prefix stripping.** Added `--remap-path-prefix` flags to `.cargo/config.toml` to replace absolute build paths in panic/debug strings with short prefixes. Zero behavioral change.

**#3 — Minimal Debug impls.** The single largest win. `#[derive(Debug)]` on error enums with multiple variants pulls in per-variant string literals, match dispatch, nested `fmt` calls, and shared `core::fmt` infrastructure (`pad`, `write_str`, `debug_tuple_field1_finish`). Replacing with `f.write_str("TypeName")` satisfies the `Debug` trait bound (needed for `Result` propagation) at near-zero cost.

**#4 — Box::pin dispatch arms.** Each `match` arm in `dispatch_request` calls a separate async handler. Without boxing, the parent future's union must be large enough for the *largest* sub-handler's state. `Box::pin()` replaces each arm's inline state with a pointer (~8 bytes), dramatically shrinking the future union and its static POOL storage (BSS: 27,320 → 16,200). LTO still inlines the poll bodies, so code quality is unaffected.

**#5 — Box::pin send_response + static buffer.** Same technique applied to `send_response` (holds a 1024-byte buffer across `.await`). `CLAIMS_BUF` moved to `static mut` to avoid holding 1024 bytes in the measurement state machine.

**#6 — Async/sync phase split.** `generate_claims` was restructured: async phase gathers hardware data (mailbox I/O, RNG), then a `#[inline(never)]` sync function builds all intermediate data structures and encodes. The sync function's locals (~1KB+ of measurement maps, evidence triples, digest entries) live on the call stack instead of in the async state machine. The `#[inline(never)]` is critical — without it, LTO re-inlines the sync function.

---

## 3. What Caused the Bloat

### Async overhead (largest source)

The `async_trait` crate desugars every `async fn` in a trait into `fn method(&self) -> Box<dyn Future<Output = T> + Send + '_>`. This affected 26 methods across 6 traits, causing:

1. **Heap allocation per call** — 3–4 `Box::new(future)` per SPDM message
2. **Opaque state machines** — `dyn Future` prevented cross-call inlining
3. **Monolithic dispatch** — compiler merged sub-handlers into single giant state machines
4. **Unnecessary `Send` bounds** — executor is single-threaded (Tock/Embassy)
5. **Drop glue** — destructor trampolines for each boxed future (980 bytes total)

### Debug derive overhead

`#[derive(Debug)]` on error enums with many variants generates substantial code: per-variant string literals, per-variant match arms, nested `fmt` calls for fields, and pulls in shared `core::fmt` infrastructure. On embedded targets without logging, this is pure waste.

### Build path strings

Rust embeds absolute file paths in panic messages. With long paths like `/home/user/.rustup/toolchains/1.85-x86_64-unknown-linux-gnu/lib/...`, these add up to kilobytes of .rodata.

---

## 4. Experiments That Did NOT Help

| Experiment | Result | Why |
|:--|:--|:--|
| `lto = "thin"` instead of `true` | +6,120 .text | Full LTO is strictly better for single-binary MCU targets |
| `dyn Future` type erasure on dispatch arms | +220 .text | LTO devirtualizes known types, adding vtable overhead without benefit |
| `Box::pin()` on `get_measurement_value` | +562 .text | LTO sees through Box::pin for monomorphized calls; adds alloc overhead |
| Static buffers in `generate_claims` | +482 .text | `fill(0)` initialization loops cost more code than state machine savings |
| `#[inline(never)]` on `encode_eat_claims_with_cti` | +178 .text | Outlining penalty exceeds benefit when called as tail-call from sync fn |
| `#[inline(never)]` on async handler `poll()` fns | no effect | LTO ignores the attribute on compiler-generated poll functions |
| Switching to a different executor | ~0 | Embassy framework is only ~230 bytes; the large `TaskStorage::poll` symbols are compiler-generated poll fns for *your* futures — identical with any executor |

---

## 5. Key Lessons Learned

### Lesson 1: LTO changes the rules for async optimization

With `lto = true`, the compiler inlines aggressively across crate boundaries. Techniques that work without LTO may be ineffective or counterproductive:

- **`#[inline(never)]` on async poll functions**: LTO ignores this on compiler-generated state machine poll fns. The attribute only works on *your* functions, not the generated ones.
- **`Box::pin()` on monomorphized calls**: LTO sees through the boxing for known concrete types. The poll body is still inlined; only the future's *union layout* shrinks (pointer instead of full sub-future).
- **`dyn Future` type erasure**: LTO devirtualizes it back, adding vtable overhead for zero benefit.

**What DOES work with LTO:**
- `Box::pin()` on `match` arms in dispatch functions — shrinks the future union even though poll bodies are inlined, reducing BSS (POOL size) significantly.
- `#[inline(never)]` on **sync** helper functions called from async code — LTO respects this, keeping locals on the call stack instead of in the state machine.
- Splitting async functions into async gather + sync builder phases.

### Lesson 2: Async/sync phase splitting is the most reliable technique

When an async function does I/O then processes results, splitting it into two phases is effective:

```rust
// BEFORE: all locals live in the async state machine
async fn generate_claims(buf: &mut [u8], nonce: &[u8]) -> Result<usize> {
    let data = fetch_from_hardware().await?;    // async I/O
    let mut big_array = [0u8; 1024];            // held across .await below
    // ... build complex data structures ...
    encode_claims(&big_array, buf).await        // more async I/O
}

// AFTER: sync builder's locals live on the call stack
async fn generate_claims(buf: &mut [u8], nonce: &[u8]) -> Result<usize> {
    let data = fetch_from_hardware().await?;    // async I/O
    let cti = generate_random().await?;         // async I/O
    build_and_encode(&data, &cti, nonce, buf)   // sync — locals on stack
}

#[inline(never)]  // critical: prevents LTO from re-inlining
fn build_and_encode(data: &Data, cti: &[u8], nonce: &[u8], buf: &mut [u8]) -> Result<usize> {
    let mut big_array = [0u8; 1024];  // on call stack, not in future
    // ... build complex data structures ...
    Ok(encoded_len)
}
```

The `#[inline(never)]` is **critical** — without it, LTO re-inlines the sync function and the locals end up back in the state machine.

### Lesson 3: Manual Debug impls are disproportionately effective

Replacing `#[derive(Debug)]` with `impl Debug { f.write_str("TypeName") }` on 16 error types saved **14,328 bytes** (8.4% of flash) — the single largest win. The savings come from eliminating:
- Per-variant string literals in .rodata
- Per-variant match dispatch code in .text
- Nested `Debug::fmt` calls for fields
- Shared `core::fmt` infrastructure (`pad`, `write_str`, `debug_tuple_field1_finish`) that gets pulled in

For error types where you only need `Debug` to satisfy trait bounds (e.g., `Result` propagation), a simple type-name string is sufficient.

### Lesson 4: `--remap-path-prefix` is free savings

Adding `--remap-path-prefix` flags to `.cargo/config.toml` strips absolute build paths from panic messages and debug info embedded in .rodata. This saved **4,134 bytes** with zero behavioral change:

```toml
[build]
rustflags = [
    "--remap-path-prefix=/home/user/src/project=",
    "--remap-path-prefix=/home/user/.rustup/toolchains/1.85-x86_64-unknown-linux-gnu=rustc",
    "--remap-path-prefix=/home/user/.cargo/registry/src=crates",
]
```

### Lesson 5: Box::pin reduces BSS more than .text

`Box::pin()` on dispatch match arms reduced BSS (POOL) by **11,120 bytes** (27,320 → 16,200) but only saved 3,062 bytes of .text. The main benefit is that each match arm stores a `Box<impl Future>` pointer (~8 bytes) in the parent future's union instead of the full sub-future state (hundreds of bytes each). Since only one arm is active at a time, the union's max variant shrinks dramatically.

### Lesson 6: Static buffers need careful evaluation

Moving stack buffers to `static mut` eliminates them from async state machines but introduces initialization code (`fill(0)`) that can cost more than the savings. Only use static buffers when:
- The buffer is large (>256 bytes)
- The buffer spans multiple `.await` points
- No re-initialization is needed (or the init is trivial)

### Lesson 7: Crate-level measurements overstate impact under LTO

The spdm-lib crate showed −34,247 bytes (−58.3%) when measured without LTO. The full binary with LTO showed −20,324 bytes (−11.9%). The gap exists because LTO was already independently recovering some of the same waste — inlining through `dyn Future` boundaries, stripping unreachable code, and folding identical monomorphizations. Our changes make the optimizations *structural and reliable* rather than dependent on LTO heuristics, but the incremental binary savings are smaller than crate-level numbers suggest.

---

## 6. Remaining Opportunities and Limits

### Top .text contributors (current)

| Symbol | Size (bytes) | % of .text |
|:--|--:|--:|
| `dispatch_request` closure (poll) | 49,144 | 37.0% |
| `spdm_mctp_responder` TaskStorage poll | 8,378 | 6.3% |
| `fetch_measurement_block` closure | 5,880 | 4.4% |
| `MeasurementsResponse::encode_response` closure | 4,506 | 3.4% |
| `build_and_encode_claims` | 3,618 | 2.7% |
| `send_response` Box::pin poll | 3,404 | 2.6% |
| `CaliptraVdmHandler::handle_request` closure | 3,002 | 2.3% |
| `Transcript::append` closure | 2,140 | 1.6% |
| `spdm_task` TaskStorage poll | 2,000 | 1.5% |
| `SharedCertStore::get_cert_chain` closure | 1,804 | 1.4% |

### Top .bss contributors (current)

| Symbol | Size (bytes) | Notes |
|:--|--:|:--|
| `HEAP_MEM` | 24,576 | Global heap — could reduce if heap usage is profiled |
| `spdm_mctp_responder` POOL | 15,184 | Embassy task storage for responder future |
| `LARGE_MSG_BUF_STORAGE` | 4,097 | Shared large message buffer |
| `SHARED_DPE_LEAF_CERT` | 2,072 | Cached DPE leaf certificate |
| `spdm_task` POOL | 1,704 | Embassy task storage for inner task |
| `CLAIMS_BUF` | 1,024 | Static measurement claims buffer |

### Diminishing returns

The remaining `.text` is dominated by:

1. **`dispatch_request` (49,144 bytes, 37%)** — the entire SPDM protocol logic for 12 command handlers, merged by LTO into a single monolithic state machine. Each handler is already a separate `async fn` wrapped in `Box::pin`, but LTO re-inlines the poll bodies. Only feature-gating commands could reduce this further.

2. **Task poll functions (~10,378 bytes, 8%)** — compiler-generated state machine poll code for Embassy tasks. Not executor overhead — any executor polling the same futures generates the same code. Embassy's actual framework is only ~230 bytes.

3. **Crypto/cert operations (~8,000 bytes, 6%)** — AES-GCM, cert chain operations, HMAC, signature verification. Core functionality.

4. **Measurement pipeline (~14,000 bytes, 10.5%)** — `fetch_measurement_block` + `encode_response` + `build_and_encode_claims`. Mostly OCP-EAT CBOR encoding logic.

### Full sync conversion (high-confidence savings, high cost)

The codebase uses **zero concurrency** — no `join!`, `select!`, or `race` anywhere. Every `.await` is purely sequential I/O, ultimately bottoming out at `TockSubscribe → yield_wait`. Embassy's `Signal` is used only as a "block forever" primitive, `embassy_time` is unused, and the critical section is a no-op. Async provides no functional benefit.

A full conversion to synchronous Tock-native code (blocking `subscribe → command → yield_wait` instead of `Future`-based I/O) would eliminate:

| Category | Current cost | Sync replacement | Est. savings |
|:--|:--|:--|:--|
| Async state machines | ~80KB of poll fns (discriminant checks, pin projections, ~22 await points per handler) | Regular function calls | **12,000–20,000 .text** |
| Box::pin / Box::new | 12 `Box::pin` + 4 `Box::new(TockSubscribe)` per message | Direct calls + blocking Tock syscalls | ~1,500–2,000 .text |
| Embassy task POOLs | 15,184 + 1,704 = 16,888 BSS | 0 | **~17,000 BSS** |
| HEAP_MEM | 24,576 BSS | Potentially 0 if no other heap users | **up to 24,576 BSS** |
| Embassy executor + alloc crate | ~730 .text | Not needed | ~730 .text |

**Estimated total: ~15,000–23,000 bytes .text (11–17%) and ~17,000–41,000 bytes BSS (35–84%).** This would bring flash from ~151KB to ~128–136KB and BSS from ~49KB to ~8–32KB.

**Why it's costly:**

1. **Full-stack refactor** — every async trait (`SpdmTransport`, `SpdmCertStore`, `SpdmMeasurementValue`, `IdeDriver`, `TdispDriver`, `VdmResponder` — 26 async methods) needs sync versions.
2. **Tock driver rewrite** — `TockSubscribe` (the Future wrapper for Tock upcalls) goes away, replaced by blocking wrappers in every driver in `runtime/userspace/syscall/`.
3. **Stack pressure** — async futures store locals in heap-allocated state machines. Sync code puts everything on the call stack. Handlers with ~17 sequential I/O calls and intermediate buffers could increase `.stack` requirements, partially offsetting BSS savings.
4. **Test infrastructure** — the test harness depends on async APIs and would need sync-compatible equivalents.
5. **Future extensibility** — if concurrent I/O is ever needed (timeouts, background cert refresh), async would need to be re-added or manual state machines built.

**Verdict:** This is the single largest remaining optimization opportunity, roughly doubling the savings from all current work. However, the refactor scope is substantial and the code becomes harder to evolve. Recommended only if binary size must drop below ~130KB flash.

### What's left to try (incremental, low-confidence)

| Opportunity | Est. savings | Risk/cost |
|:--|:--|:--|
| Feature-gate unused SPDM commands | up to ~5,000 | Requires platform-specific feature flags |
| Replace `buf[..n]` with `buf.get(..n)` to avoid IndexMut panic code | up to ~2,000 | Widespread change, changes error semantics |
| Reduce `HEAP_MEM` from 24KB | BSS only | Requires profiling actual heap usage |
| Reduce `.stack` from 44KB | RAM only | Requires stack usage analysis |
| Upgrade to newer Rust toolchain | unknown | Newer LLVM may optimize async better |

### Bottom line

The binary is at **~151KB flash**, down from **~171KB** (−11.9%). The remaining code is almost entirely core protocol logic, crypto, and executor infrastructure — there is no significant "waste" left to eliminate. Further meaningful reductions would require **removing functionality** (feature-gating commands) rather than optimizing how existing code is compiled.

---

## 7. Guidelines: Keeping the Binary Lean

### Rule 1: Never use `async_trait` on traits with a single concrete implementation

Use generic parameters + native AFIT instead. Reserve `async_trait` (or `dyn` dispatch) for genuinely heterogeneous runtime dispatch.

```rust
// BAD — boxes every call, prevents inlining
#[async_trait]
trait Transport { async fn send(&mut self, data: &[u8]) -> Result<()>; }
fn process(t: &mut dyn Transport) { ... }

// GOOD — zero-cost, compiler inlines across boundary
trait Transport { async fn send(&mut self, data: &[u8]) -> Result<()>; }
fn process<T: Transport>(t: &mut T) { ... }
```

### Rule 2: Drop `Send` on single-threaded executors

Use `#[async_trait(?Send)]` if you must use `async_trait` at all.

### Rule 3: Avoid `.await` inside large `match` arms

Split each arm into a separate `async fn` and use `Box::pin()` on calls in the dispatch function to shrink the future union.

### Rule 4: Audit `#[derive(Debug)]` on protocol types

Only derive `Debug` on error types needed for `Result` propagation, and prefer manual `impl Debug { f.write_str("TypeName") }`. For optional debugging, use `#[cfg_attr(feature = "debug-types", derive(Debug))]`.

### Rule 5: Split async functions into async gather + sync builder

When an async function does I/O then builds large data structures, factor the building into a `#[inline(never)]` sync helper. This keeps intermediate locals on the call stack instead of in the state machine.

### Rule 6: Use `--remap-path-prefix`

Strip absolute build paths from panic strings via `.cargo/config.toml`.

### Rule 7: Prefer `CommonCodec` over manual `Codec` impls

Use shared non-generic helpers to avoid monomorphization bloat.

### Rule 8: Monitor code size in CI

Add a CI step that measures `.text` size and fails on regressions beyond a threshold (e.g., 5%):

```bash
size -A target/riscv32imc-unknown-none-elf/release/user-app | awk '/.text/{print $2}'
```

---

## Appendix A: spdm-lib Crate-Level Analysis

*The following measurements were taken with LTO disabled to isolate spdm-lib's contribution. With full LTO enabled, many of these savings overlap with compiler optimizations (see Lesson 7).*

### Crate-level results

| Metric | Before | After | Delta |
|:--|--:|--:|:--|
| **.text size** | **58,700** | **24,453** | **−34,247 (−58.3%)** |
| Async closures | 13,230 | 3,078 | −10,152 |
| Debug fmt impls | 9,632 | 0 | −9,632 |
| Codec monomorphizations | ~7,520 | ~3,818 | −3,702 |
| Drop glue | 980 | 274 | −706 |
| Heap allocs/request | 3–4 | 1 | −2–3 |

### Comparison with C

| Implementation | .text (bytes) | Ratio vs C |
|:--|--:|--:|
| C (libspdm-based) | 27,560 | 1.0× |
| Rust — original | 58,700 | 2.13× |
| Rust — after all optimizations | **24,453** | **0.89×** |

The Rust spdm-lib is now **11% smaller than the C equivalent** despite implementing additional features (IDE-KM, TDISP, Caliptra VDM, OCP). The comparable Rust SPDM core (excluding VDM handlers) is ~15,600 bytes vs C's 27,560 — **43% smaller** for equivalent functionality.

### Why crate savings ≠ binary savings

The spdm-lib showed −34,247 bytes (−58.3%) at crate level but the full binary showed −20,324 bytes (−11.9%). The gap arises because:

1. **LTO pre-optimizes async overhead** — with `lto = true`, the compiler partially inlines through `dyn Future` boundaries and devirtualizes vtable calls, recovering ~14,000 bytes of async_trait overhead independently.
2. **Dead code elimination** — LTO strips unreachable functions that exist in the `.rlib` but never appear in the final binary.
3. **Identical Code Folding** — the linker's `-icf=all` merges identical monomorphized functions that our Codec dedup also addressed.
4. **Exception: Debug removal** — this saved *more* in the binary (−14,328) than at crate level (−9,632) because LTO could not eliminate reachable Debug impls, and the binary measurement captured .rodata savings too.
