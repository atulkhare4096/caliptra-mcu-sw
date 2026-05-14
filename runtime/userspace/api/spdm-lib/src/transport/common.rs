// Licensed under the Apache-2.0 license

extern crate alloc;
use crate::codec::CodecError;
use crate::codec::MessageBuf;
use alloc::boxed::Box;
use caliptra_mcu_libtock_platform::ErrorCode;

pub type TransportResult<T> = Result<T, TransportError>;

pub trait SpdmTransport {
    fn send_request<'a>(
        &mut self,
        dest_eid: u8,
        req: &mut MessageBuf<'a>,
        secure: Option<bool>,
    ) -> TransportResult<()>;
    fn receive_response<'a>(&mut self, rsp: &mut MessageBuf<'a>) -> TransportResult<bool>;
    fn receive_request<'a>(&mut self, req: &mut MessageBuf<'a>) -> TransportResult<bool>;
    fn send_response<'a>(
        &mut self,
        resp: &mut MessageBuf<'a>,
        secure: bool,
    ) -> TransportResult<()>;
    fn max_message_size(&self) -> TransportResult<usize>;
    fn header_size(&self) -> usize;
    fn sequence_num_size_bytes(&self) -> usize {
        0 // No secure message sequence number by default
    }
    fn random_data_size_bytes(&self) -> usize {
        0 // No secure message random data by default
    }

    /// Populate `req` from a pre-received buffer (non-blocking path).
    ///
    /// `nb_buf` is the kernel-shared buffer containing raw transport data.
    /// `upcall_args` are the (arg0, arg1, arg2) from the `UpcallNotification`.
    /// Returns the `secure` flag, same as `receive_request`.
    fn receive_from_buffer<'a>(
        &mut self,
        _req: &mut MessageBuf<'a>,
        _nb_buf: &[u8],
        _upcall_args: (u32, u32, u32),
    ) -> TransportResult<bool> {
        Err(TransportError::OperationNotSupported)
    }

    /// Re-arm the non-blocking receive after processing.
    fn rearm_receive(&self) -> TransportResult<()> {
        Err(TransportError::OperationNotSupported)
    }
}

#[derive(Debug)]
pub enum TransportError {
    DriverError(ErrorCode),
    Codec(CodecError),
    UnexpectedMessageType,
    UnsupportedMessageType,
    ResponseNotExpected,
    NoRequestInFlight,
    InvalidMessage,
    OperationNotSupported,
}
