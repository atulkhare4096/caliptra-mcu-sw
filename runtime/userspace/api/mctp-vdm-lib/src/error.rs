// Licensed under the Apache-2.0 license

/// Errors that can occur in the VDM library.
#[cfg_attr(feature = "debug", derive(Debug))]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum VdmLibError {
    /// Transport error occurred.
    TransportError,
    /// Invalid request received.
    InvalidRequest,
    /// Invalid command code.
    InvalidCommand,
    /// Buffer too small.
    BufferTooSmall,
    /// Command handler error.
    CommandHandlerError,
    /// Encoding error.
    EncodingError,
    /// Decoding error.
    DecodingError,
    /// Service not ready.
    NotReady,
    /// Internal error.
    InternalError,
}

#[cfg(not(feature = "debug"))]
impl core::fmt::Debug for VdmLibError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("VdmLibError")
    }
}
