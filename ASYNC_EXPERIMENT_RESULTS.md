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

```
Async handlers total .text contribution: 88,216 bytes
Estimated sync handler code (from sync branch): ~39,000 bytes
Pure async state machine overhead: ~49,000 bytes
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

## Conclusion

**Async code size cannot be made competitive with sync through structural optimizations.**

The only way to eliminate the ~49KB overhead is to convert handlers from `async fn` to
regular `fn` (the sync conversion approach on `dev/atul/sync_conversion`).

### Estimated sizes if handlers were sync but transport stayed async:
- .text: ~84,000 (44,642 base + ~39,000 sync handler code)
- This matches the sync branch's 83,040 almost exactly
- The async transport shell (embassy executor, TockSubscribe) adds minimal overhead (~1-2KB)

### Recommendation

Proceed with the sync conversion approach (`dev/atul/sync_conversion`). The experiment
conclusively demonstrates that:
1. The sync branch's architecture (blocking mailbox + sync handlers + async transport shell)
   is the optimal design for this code size constraint
2. No amount of optimization can make the async handlers fit within the 98KB flash budget
   (they alone contribute 88KB of .text before any other code)
