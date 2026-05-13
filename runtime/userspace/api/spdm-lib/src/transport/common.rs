// Licensed under the Apache-2.0 license

use crate::codec::CodecError;
use crate::codec::MessageBuf;
use caliptra_mcu_libtock_platform::ErrorCode;

pub type TransportResult<T> = Result<T, TransportError>;

pub trait SpdmTransport {
    async fn send_request<'a>(
        &mut self,
        dest_eid: u8,
        req: &mut MessageBuf<'a>,
        secure: Option<bool>,
    ) -> TransportResult<()>;
    async fn receive_response<'a>(&mut self, rsp: &mut MessageBuf<'a>) -> TransportResult<bool>;
    async fn receive_request<'a>(&mut self, req: &mut MessageBuf<'a>) -> TransportResult<bool>;
    async fn send_response<'a>(
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
