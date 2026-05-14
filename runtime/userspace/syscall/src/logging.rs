// Licensed under the Apache-2.0 license

use crate::DefaultSyscalls;
use caliptra_mcu_libtock_platform::{ErrorCode, Syscalls};
use caliptra_mcu_libtockasync::blocking;
use core::marker::PhantomData;

pub struct LoggingSyscall<S: Syscalls = DefaultSyscalls> {
    syscall: PhantomData<S>,
    driver_num: u32,
}

impl<S: Syscalls> Default for LoggingSyscall<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// Represents an asynchronous logging interface.
impl<S: Syscalls> LoggingSyscall<S> {
    /// Creates a new LoggingSyscall instance with the default driver number.
    ///
    /// # Returns
    /// A new `LoggingSyscall` instance.
    pub fn new() -> Self {
        Self {
            syscall: PhantomData,
            driver_num: driver_num::LOGGING_FLASH,
        }
    }

    /// Checks if the logging driver exists.
    ///
    /// # Returns
    /// - `Ok(())` - If the driver exists.
    /// - `Err(ErrorCode)` - An error code if the operation fails.
    pub fn exists(&self) -> Result<(), ErrorCode> {
        S::command(self.driver_num, logging_cmd::EXISTS, 0, 0).to_result()
    }
    /// Gets the capacity of the logging storage.
    ///
    /// # Returns
    /// - `Ok(capacity)` - The capacity in bytes.
    /// - `Err(ErrorCode)` - An error code if the operation fails.
    pub fn get_capacity(&self) -> Result<usize, ErrorCode> {
        S::command(self.driver_num, logging_cmd::GET_CAP, 0, 0)
            .to_result()
            .map(|x: u32| x as usize)
    }

    /// Appends an entry to the log asynchronously.
    ///
    /// # Arguments
    /// - `entry`: The data to append.
    ///
    /// # Returns
    /// - `Ok(())` on success
    /// - `Err(ErrorCode)` - An error code if the operation fails.
    pub fn append_entry(&self, entry: &[u8]) -> Result<(), ErrorCode> {
        blocking::subscribe_allow_ro_and_wait::<S>(
            self.driver_num,
            subscribe::APPEND_DONE,
            ro_allow::APPEND,
            entry,
            logging_cmd::APPEND,
            entry.len() as u32,
            0,
        )?;
        S::unallow_ro(self.driver_num, ro_allow::APPEND);
        Ok(())
    }

    pub fn read_entry(&self, buffer: &mut [u8]) -> Result<usize, ErrorCode> {
        let (len, _, _) = blocking::subscribe_allow_rw_and_wait::<S>(
            self.driver_num,
            subscribe::READ_DONE,
            rw_allow::READ,
            buffer,
            logging_cmd::READ,
            buffer.len() as u32,
            0,
        )?;
        S::unallow_rw(self.driver_num, rw_allow::READ);
        Ok(len as usize)
    }

    pub fn sync(&self) -> Result<(), ErrorCode> {
        blocking::subscribe_and_wait::<S>(
            self.driver_num,
            subscribe::SYNC_DONE,
            logging_cmd::SYNC,
            0,
            0,
        )?;
        Ok(())
    }

    pub fn clear(&self) -> Result<(), ErrorCode> {
        blocking::subscribe_and_wait::<S>(
            self.driver_num,
            subscribe::ERASE_DONE,
            logging_cmd::ERASE,
            0,
            0,
        )?;
        Ok(())
    }

    /// Seeks to the beginning of the log asynchronously. Used by the logging system to reset the read position.
    ///
    /// # Returns
    /// * `Ok(())` - On success.
    /// * `Err(ErrorCode)` - An error code if the operation fails.
    pub fn seek_beginning(&self) -> Result<(), ErrorCode> {
        blocking::subscribe_and_wait::<S>(
            self.driver_num,
            subscribe::SEEK_DONE,
            logging_cmd::SEEK,
            0,
            0,
        )?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Driver number and command IDs
// -----------------------------------------------------------------------------

pub mod driver_num {
    pub const LOGGING_FLASH: u32 = 0x9001_0000;
}

// Upcalls
mod subscribe {
    /// Read done callback.
    pub const READ_DONE: u32 = 0;
    /// Seek done callback.
    pub const SEEK_DONE: u32 = 1;
    /// Append done callback.
    pub const APPEND_DONE: u32 = 2;
    /// Sync done callback.
    pub const SYNC_DONE: u32 = 3;
    /// Erase done callback
    pub const ERASE_DONE: u32 = 4;
}

mod ro_allow {
    /// Read-only buffer containing the entry to be appended to the log.
    pub const APPEND: u32 = 0;
}

mod rw_allow {
    /// Read-write buffer for receiving the entry to be read from the log.
    pub const READ: u32 = 0;
}

/// Command IDs for logging driver capsule
///
/// - `0`: Return Ok(()) if this driver is included on the platform.
/// - `1`: Read an entry from the log.
/// - `2`: Append an entry to the log.
/// - `3`: Seek to the beginning of the log.
/// - `4`: Synchronize the log.
/// - `5`: Clear the log.
/// - `6`: Get the capacity of the logging storage.
mod logging_cmd {
    pub const EXISTS: u32 = 0;
    pub const READ: u32 = 1;
    pub const APPEND: u32 = 2;
    pub const SEEK: u32 = 3;
    pub const SYNC: u32 = 4;
    pub const ERASE: u32 = 5;
    pub const GET_CAP: u32 = 6;
}
