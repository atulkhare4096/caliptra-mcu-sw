// Licensed under the Apache-2.0 license

// Flash userspace library

use crate::DefaultSyscalls;
use caliptra_mcu_libtock_platform::{ErrorCode, Syscalls};
use caliptra_mcu_libtockasync::blocking;
use core::marker::PhantomData;

pub struct SpiFlash<S: Syscalls = DefaultSyscalls> {
    syscall: PhantomData<S>,
    driver_num: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct FlashCapacity(pub u32);

/// Represents an asynchronous SPI flash memory interface.
///
/// This struct provides methods to interact with SPI flash memory in an asynchronous manner,
/// allowing for non-blocking read, write and erase operations.
impl<S: Syscalls> SpiFlash<S> {
    /// Creates a new instance of `SpiFlash`.
    ///
    /// # Arguments
    ///
    /// * `driver_num` - The driver number associated with the SPI flash.
    ///
    /// # Returns
    /// A new instance of `SpiFlash`.
    pub fn new(driver_num: u32) -> Self {
        Self {
            syscall: PhantomData,
            driver_num,
        }
    }

    /// Checks if the SPI flash exists.
    ///
    /// # Returns
    ///
    /// * `Ok(())` if the SPI flash exists.
    /// * `Err(ErrorCode)` if there is an error.
    pub fn exists(&self) -> Result<(), ErrorCode> {
        S::command(self.driver_num, flash_storage_cmd::EXISTS, 0, 0).to_result()
    }

    /// Gets the capacity of the SPI flash memory that is available to userspace.
    ///
    /// # Returns
    ///
    /// * `Ok(FlashCapacity)` with the capacity of the SPI flash memory.
    /// * `Err(ErrorCode)` if there is an error.
    pub fn get_capacity(&self) -> Result<FlashCapacity, ErrorCode> {
        S::command(self.driver_num, flash_storage_cmd::GET_CAPACITY, 0, 0)
            .to_result()
            .map(FlashCapacity)
    }

    /// Gets the chunk size for read and write operations.
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` with the chunk size for read and write operations.
    /// * `Err(ErrorCode)` if there is an error.
    pub fn get_chunk_size(&self) -> Result<usize, ErrorCode> {
        S::command(self.driver_num, flash_storage_cmd::GET_CHUNK_SIZE, 0, 0)
            .to_result()
            .map(|x: u32| x as usize)
    }

    fn read_chunk(
        &self,
        address: usize,
        len: usize,
        buf: &mut [u8],
    ) -> Result<(), ErrorCode> {
        // Check if the buffer is large enough and the length is within the chunk size
        if buf.len() < len || len > self.get_chunk_size()? {
            return Err(ErrorCode::NoMem);
        }

        blocking::subscribe_allow_rw_and_wait::<S>(
            self.driver_num,
            subscribe::READ_DONE,
            rw_allow::READ,
            buf,
            flash_storage_cmd::READ,
            address as u32,
            len as u32,
        )?;

        S::unallow_rw(self.driver_num, rw_allow::READ);
        Ok(())
    }

    pub fn read(&self, address: usize, len: usize, buf: &mut [u8]) -> Result<(), ErrorCode> {
        if buf.len() < len {
            return Err(ErrorCode::NoMem);
        }

        let chunk_size = self.get_chunk_size()?;
        let mut remaining = len;
        let mut offset = 0;
        while remaining > 0 {
            let len = core::cmp::min(remaining, chunk_size);
            self.read_chunk(address + offset, len, &mut buf[offset..offset + len])?;
            remaining -= len;
            offset += len;
        }

        Ok(())
    }

    fn write_chunk(&self, address: usize, len: usize, buf: &[u8]) -> Result<(), ErrorCode> {
        if buf.len() < len || len > self.get_chunk_size()? {
            return Err(ErrorCode::NoMem);
        }

        blocking::subscribe_allow_ro_and_wait::<S>(
            self.driver_num,
            subscribe::WRITE_DONE,
            ro_allow::WRITE,
            buf,
            flash_storage_cmd::WRITE,
            address as u32,
            len as u32,
        )?;

        S::unallow_ro(self.driver_num, ro_allow::WRITE);
        Ok(())
    }

    pub fn write(&self, address: usize, len: usize, buf: &[u8]) -> Result<(), ErrorCode> {
        if buf.len() < len {
            return Err(ErrorCode::NoMem);
        }

        let chunk_size = self.get_chunk_size()?;
        let mut remaining = len;
        let mut offset = 0;
        while remaining > 0 {
            let len = core::cmp::min(remaining, chunk_size);
            self.write_chunk(address + offset, len, &buf[offset..offset + len])?;
            remaining -= len;
            offset += len;
        }

        Ok(())
    }

    pub fn erase(&self, address: usize, len: usize) -> Result<(), ErrorCode> {
        blocking::subscribe_and_wait::<S>(
            self.driver_num,
            subscribe::ERASE_DONE,
            flash_storage_cmd::ERASE,
            address as u32,
            len as u32,
        )?;
        Ok(())
    }
}

// -----------------------------------------------------------------------------
// Driver number and command IDs
// -----------------------------------------------------------------------------

mod subscribe {
    /// Read done callback.
    pub const READ_DONE: u32 = 0;
    /// Write done callback.
    pub const WRITE_DONE: u32 = 1;
    /// Erase done callback
    pub const ERASE_DONE: u32 = 2;
}

/// Ids for read-only allow buffers
mod ro_allow {
    /// Setup a buffer to write bytes to the flash storage.
    pub const WRITE: u32 = 0;
}

/// Ids for read-write allow buffers
mod rw_allow {
    /// Setup a buffer to read from the flash storage into.
    pub const READ: u32 = 0;
}

/// Command IDs for flash partition driver capsule
///
/// - `0`: Return Ok(()) if this driver is included on the platform.
/// - `1`: Return flash capacity available to userspace.
/// - `2`: Start a read
/// - `3`: Start a write
/// - `4`: Start an erase
/// - `5`: Get the chunk size for read/write operations.
mod flash_storage_cmd {
    pub const EXISTS: u32 = 0;
    pub const GET_CAPACITY: u32 = 1;
    pub const READ: u32 = 2;
    pub const WRITE: u32 = 3;
    pub const ERASE: u32 = 4;
    pub const GET_CHUNK_SIZE: u32 = 5;
}
