// Licensed under the Apache-2.0 license

//! # Mailbox Interface
use crate::DefaultSyscalls;
use caliptra_api::mailbox::MailboxReqHeader;
use caliptra_mcu_libtock_platform::{ErrorCode, Syscalls};
use caliptra_mcu_libtockasync::blocking;
use core::marker::PhantomData;

const PAYLOAD_CHUNK_SIZE: usize = 256;

/// Mailbox interface user interface.
///
/// # Generics
/// - `S`: The syscall implementation.
pub struct Mailbox<S: Syscalls = DefaultSyscalls> {
    _syscall: PhantomData<S>,
    driver_num: u32,
}

impl<S: Syscalls> Default for Mailbox<S> {
    fn default() -> Self {
        Self::new()
    }
}

// Populate the checksum for a mailbox request.
pub fn populate_checksum(cmd: u32, data: &mut [u8]) -> Result<(), ErrorCode> {
    // Calc checksum, use the size override if provided
    let checksum = caliptra_api::calc_checksum(cmd, data);

    if data.len() < size_of::<MailboxReqHeader>() {
        Err(ErrorCode::Invalid)?;
    }
    data[..size_of::<MailboxReqHeader>()].copy_from_slice(&checksum.to_le_bytes());
    Ok(())
}

impl<S: Syscalls> Mailbox<S> {
    pub fn new() -> Self {
        Self {
            _syscall: PhantomData,
            driver_num: MAILBOX_DRIVER_NUM,
        }
    }

    // Populate the checksum for a mailbox request.
    pub fn populate_checksum(&self, cmd: u32, data: &mut [u8]) -> Result<(), ErrorCode> {
        populate_checksum(cmd, data)
    }

    /// Executes a mailbox command and returns the response.
    ///
    /// This method sends a mailbox command to the kernel, then waits
    /// asynchronously for the command to complete. The response buffer is filled with
    /// the result from the kernel.
    ///
    /// # Arguments
    /// - `command`: The mailbox command ID to execute.
    /// - `input_data`: A read-only buffer containing the mailbox command parameters.
    /// - `response_buffer`: A writable buffer to store the response data.
    ///
    /// # Returns
    /// - `Ok(usize)` on success, containing the number of bytes written to the response buffer.
    /// - `Err(ErrorCode)` if the command fails.
    pub fn execute(
        &self,
        command: u32,
        input_data: &[u8],
        response_buffer: &mut [u8],
    ) -> Result<usize, MailboxError> {
        let (bytes, error_code, _) = blocking::subscribe_allow_ro_rw_and_wait::<S>(
            self.driver_num,
            mailbox_subscribe::COMMAND_DONE,
            mailbox_ro_buffer::INPUT,
            input_data,
            mailbox_rw_buffer::RESPONSE,
            response_buffer,
            mailbox_cmd::EXECUTE_COMMAND,
            command,
            0,
        )
        .map_err(MailboxError::ErrorCode)?;

        if error_code != 0 {
            Err(MailboxError::MailboxError(error_code))
        } else {
            Ok(bytes as usize)
        }
    }

    pub fn execute_with_payload_stream(
        &self,
        command: u32,
        header: Option<&[u8]>,
        payload: &mut dyn PayloadStream,
        response_buffer: &mut [u8],
    ) -> Result<usize, MailboxError> {
        let request_len = payload.size() + header.map_or(0, |h| h.len());

        // Send the command to initiate mailbox request
        S::command(
            self.driver_num,
            mailbox_cmd::START_CHUNKED_REQUEST,
            command,
            request_len as u32,
        )
        .to_result::<(), ErrorCode>()
        .map_err(MailboxError::ErrorCode)?;

        // Send the header if provided
        let mut buffer = [0u8; PAYLOAD_CHUNK_SIZE];
        if let Some(header) = header {
            buffer[..header.len()].copy_from_slice(header);
            self.send_chunk(buffer[..header.len()].as_ref())?;
        }

        // Send the payload in chunks
        loop {
            let sz = payload
                .read(&mut buffer)
                .map_err(MailboxError::ErrorCode)?;
            if sz == 0 {
                break;
            }
            self.send_chunk(buffer[..sz].as_ref())?;
        }

        // Execute the command
        let (bytes, error_code, _) = blocking::subscribe_allow_rw_and_wait::<S>(
            self.driver_num,
            mailbox_subscribe::COMMAND_DONE,
            mailbox_rw_buffer::RESPONSE,
            response_buffer,
            mailbox_cmd::EXECUTE_CHUNKED_REQUEST,
            command,
            0,
        )
        .map_err(MailboxError::ErrorCode)?;

        if error_code != 0 {
            Err(MailboxError::MailboxError(error_code))
        } else {
            Ok(bytes as usize)
        }
    }

    fn send_chunk(&self, buffer: &[u8]) -> Result<(u32, u32, u32), MailboxError> {
        blocking::subscribe_allow_ro_and_wait::<S>(
            self.driver_num,
            mailbox_subscribe::COMMAND_DONE,
            mailbox_ro_buffer::INPUT,
            buffer,
            mailbox_cmd::NEXT_PAYLOAD_CHUNK,
            0,
            0,
        )
        .map_err(MailboxError::ErrorCode)
    }
}
pub trait PayloadStream {
    /// Returns the size of the payload in bytes.
    fn size(&self) -> usize;

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, ErrorCode>;
}

// -----------------------------------------------------------------------------
// Command IDs and Mailbox-specific constants
// -----------------------------------------------------------------------------

// Driver number for the Mailbox interface
pub const MAILBOX_DRIVER_NUM: u32 = 0x8000_0009;

/// Command IDs for mailbox operations.
mod mailbox_cmd {
    pub const _STATUS: u32 = 0;
    /// Execute a command with input and response buffers.
    pub const EXECUTE_COMMAND: u32 = 1;
    pub const START_CHUNKED_REQUEST: u32 = 2;
    pub const NEXT_PAYLOAD_CHUNK: u32 = 3;
    pub const EXECUTE_CHUNKED_REQUEST: u32 = 4;
}

/// Buffer IDs for mailbox read operations.
mod mailbox_ro_buffer {
    /// Buffer ID for the input buffer (read-only).
    pub const INPUT: u32 = 0;
}

/// Buffer IDs for mailbox read-write operations.
mod mailbox_rw_buffer {
    /// Buffer ID for the response buffer (read-write).
    pub const RESPONSE: u32 = 0;
}

/// Subscription IDs for asynchronous mailbox events.
mod mailbox_subscribe {
    /// Subscription ID for the `COMMAND_DONE` event.
    pub const COMMAND_DONE: u32 = 0;
}

#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(PartialEq)]
pub enum MailboxError {
    ErrorCode(ErrorCode),
    MailboxError(u32),
}

#[cfg(not(feature = "debug"))]
impl core::fmt::Debug for MailboxError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("MailboxError")
    }
}
