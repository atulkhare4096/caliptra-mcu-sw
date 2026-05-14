// Licensed under the Apache-2.0 license

//! Periodic FIPS self-test module.
//!
//! This module provides functionality to run FIPS self-tests periodically
//! in the background. It can be enabled/disabled via MCU mailbox commands.

use caliptra_api::mailbox::CommandId as CaliptraCommandId;
use caliptra_mcu_libapi_caliptra::mailbox_api::execute_mailbox_cmd;
use caliptra_mcu_libsyscall_caliptra::mailbox::Mailbox;
use caliptra_mcu_libsyscall_caliptra::DefaultSyscalls;
use caliptra_mcu_libtock_alarm::{Convert, Hz, Milliseconds};
use caliptra_mcu_libtock_console::Console;
use caliptra_mcu_libtock_platform::Syscalls;
use caliptra_mcu_libtockasync::blocking;
use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering};

/// Periodic FIPS self-test interval in milliseconds.
/// Default: 60 seconds (60000 ms)
pub const FIPS_PERIODIC_INTERVAL_MS: u32 = 60_000;

/// Result status values
pub const RESULT_NOT_RUN: u32 = 0;
pub const RESULT_PASS: u32 = 1;
pub const RESULT_FAIL: u32 = 2;

/// Global state for periodic FIPS self-test
static ENABLED: AtomicU32 = AtomicU32::new(0);
static ITERATIONS: AtomicU32 = AtomicU32::new(0);
static LAST_RESULT: AtomicU32 = AtomicU32::new(RESULT_NOT_RUN);

/// Flag to wake up the periodic task when state changes
static STATE_CHANGED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Driver number for alarm
const DRIVER_NUM: u32 = 0;

/// Command IDs for alarm
mod command {
    pub const FREQUENCY: u32 = 1;
    pub const SET_RELATIVE: u32 = 5;
}

/// Check if periodic FIPS self-test is enabled.
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::SeqCst) != 0
}

/// Enable or disable periodic FIPS self-test.
pub fn set_enabled(enable: bool) {
    let new_value = if enable { 1 } else { 0 };
    ENABLED.store(new_value, Ordering::SeqCst);
    STATE_CHANGED.store(true, core::sync::atomic::Ordering::SeqCst);
}

/// Get the number of completed iterations.
pub fn get_iterations() -> u32 {
    ITERATIONS.load(Ordering::SeqCst)
}

/// Get the last result status.
pub fn get_last_result() -> u32 {
    LAST_RESULT.load(Ordering::SeqCst)
}

/// Get full status: (enabled, iterations, last_result)
pub fn get_status() -> (bool, u32, u32) {
    (is_enabled(), get_iterations(), get_last_result())
}

/// Blocking sleep helper
fn sleep_ms(ms: u32) {
    use caliptra_mcu_libtock_platform::ErrorCode;

    let freq: Result<u32, ErrorCode> =
        DefaultSyscalls::command(DRIVER_NUM, command::FREQUENCY, 0, 0).to_result();
    let freq = freq.map(Hz).unwrap_or(Hz(1000));

    let ticks = Milliseconds(ms).to_ticks(freq).0;

    let _ = blocking::subscribe_and_wait::<DefaultSyscalls>(
        DRIVER_NUM,
        0,
        command::SET_RELATIVE,
        ticks,
        0,
    );
}

/// Run a single FIPS self-test iteration using the Caliptra mailbox.
///
/// This function:
/// 1. Sends SELF_TEST_START to Caliptra
/// 2. Polls SELF_TEST_GET_RESULTS until completion
/// 3. Returns true on success, false on failure
fn run_fips_self_test(caliptra_mbox: &Mailbox) -> bool {
    // Start the self-test
    let mut req_buf = [0u8; 8]; // Minimal request buffer (just header)
    let mut resp_buf = [0u8; 8]; // Response buffer

    let start_result = execute_mailbox_cmd(
        caliptra_mbox,
        CaliptraCommandId::SELF_TEST_START.into(),
        &mut req_buf,
        &mut resp_buf,
    )
    ;

    if start_result.is_err() {
        writeln!(
            Console::<DefaultSyscalls>::writer(),
            "Periodic FIPS: SELF_TEST_START failed"
        )
        .ok();
        return false;
    }

    // Poll for completion (with timeout via iteration limit)
    const MAX_POLL_ITERATIONS: u32 = 100;
    for _ in 0..MAX_POLL_ITERATIONS {
        // Wait a bit between polls
        sleep_ms(100);

        // Get results
        let get_results = execute_mailbox_cmd(
            caliptra_mbox,
            CaliptraCommandId::SELF_TEST_GET_RESULTS.into(),
            &mut req_buf,
            &mut resp_buf,
        )
        ;

        match get_results {
            Ok(_) => {
                // Success - self-test completed
                return true;
            }
            Err(_) => {
                // Still in progress or error - continue polling
                continue;
            }
        }
    }

    writeln!(
        Console::<DefaultSyscalls>::writer(),
        "Periodic FIPS: self-test timeout"
    )
    .ok();
    false
}

/// Embassy task for periodic FIPS self-test.
///
/// This task runs in the background and periodically executes FIPS self-tests
/// when enabled.
pub fn fips_periodic_task() {
    let caliptra_mbox = Mailbox::new();

    writeln!(
        Console::<DefaultSyscalls>::writer(),
        "Periodic FIPS self-test task started"
    )
    .ok();

    loop {
        if is_enabled() {
            // Run self-test
            let result = run_fips_self_test(&caliptra_mbox);

            // Update state (load-modify-store since fetch_add not available on riscv32)
            let current = ITERATIONS.load(Ordering::SeqCst);
            ITERATIONS.store(current.wrapping_add(1), Ordering::SeqCst);
            LAST_RESULT.store(
                if result { RESULT_PASS } else { RESULT_FAIL },
                Ordering::SeqCst,
            );

            let iterations = get_iterations();
            writeln!(
                Console::<DefaultSyscalls>::writer(),
                "Periodic FIPS: iteration {} result: {}",
                iterations,
                if result { "PASS" } else { "FAIL" }
            )
            .ok();

            // Wait for the interval before next test
            sleep_ms(FIPS_PERIODIC_INTERVAL_MS);
        } else {
            // Wait for enable signal (poll + yield)
            while !STATE_CHANGED.load(core::sync::atomic::Ordering::SeqCst) {
                DefaultSyscalls::yield_wait();
            }
            STATE_CHANGED.store(false, core::sync::atomic::Ordering::SeqCst);
        }
    }
}
