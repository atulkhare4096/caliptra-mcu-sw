// Licensed under the Apache-2.0 license

use caliptra_mcu_libsyscall_caliptra::DefaultSyscalls;
use caliptra_mcu_libtock_alarm::{Convert, Hz, Milliseconds};
use caliptra_mcu_libtock_platform::{self as platform};
use caliptra_mcu_libtock_platform::{DefaultConfig, ErrorCode, Syscalls};
use caliptra_mcu_libtockasync::blocking;

pub struct AsyncAlarm<S: Syscalls = DefaultSyscalls, C: platform::subscribe::Config = DefaultConfig>(
    S,
    C,
);

impl<S: Syscalls, C: platform::subscribe::Config> AsyncAlarm<S, C> {
    /// Run a check against the console capsule to ensure it is present.
    #[inline(always)]
    #[allow(dead_code)]
    pub fn exists() -> Result<(), ErrorCode> {
        S::command(DRIVER_NUM, command::EXISTS, 0, 0).to_result()
    }

    pub fn get_frequency() -> Result<Hz, ErrorCode> {
        S::command(DRIVER_NUM, command::FREQUENCY, 0, 0)
            .to_result()
            .map(Hz)
    }

    #[allow(dead_code)]
    pub fn get_ticks() -> Result<u32, ErrorCode> {
        S::command(DRIVER_NUM, command::TIME, 0, 0).to_result()
    }

    pub fn get_milliseconds() -> Result<u64, ErrorCode> {
        let ticks = Self::get_ticks()? as u64;
        let freq = (Self::get_frequency()?).0 as u64;

        Ok(ticks.saturating_div(freq / 1000))
    }

    pub fn sleep_for<T: Convert>(time: T) -> Result<(), ErrorCode> {
        let freq = Self::get_frequency()?;
        let ticks = time.to_ticks(freq).0;
        Self::sleep_ticks(ticks)
    }

    pub fn sleep_ticks(ticks: u32) -> Result<(), ErrorCode> {
        blocking::subscribe_and_wait::<S>(DRIVER_NUM, 0, command::SET_RELATIVE, ticks, 0)?;
        Ok(())
    }

    pub fn sleep(time: Milliseconds) {
        // sleep_ticks handles mutex acquisition internally
        let _ = AsyncAlarm::<DefaultSyscalls>::sleep_for(time);
    }
}

// -----------------------------------------------------------------------------
// Driver number and command IDs
// -----------------------------------------------------------------------------

const DRIVER_NUM: u32 = 0;

// Command IDs
#[allow(unused)]
mod command {
    pub const EXISTS: u32 = 0;
    pub const FREQUENCY: u32 = 1;
    pub const TIME: u32 = 2;
    pub const STOP: u32 = 3;

    pub const SET_RELATIVE: u32 = 5;
    pub const SET_ABSOLUTE: u32 = 6;
}

#[allow(unused)]
mod subscribe {
    pub const CALLBACK: u32 = 0;
}
