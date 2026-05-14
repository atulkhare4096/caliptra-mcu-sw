// Licensed under the Apache-2.0 license

use crate::error::VdmLibError;
use caliptra_mcu_libsyscall_caliptra::mctp::{driver_num, Mctp, MessageInfo};
use caliptra_mcu_libtockasync::blocking::UpcallNotification;
use caliptra_mcu_mctp_vdm_common::util::mctp_transport::{
    MctpCommonHeader, MCTP_COMMON_HEADER_OFFSET, MCTP_VDM_MSG_TYPE,
};

/// Transport error types.
#[derive(Debug)]
pub enum TransportError {
    DriverError,
    BufferTooSmall,
    UnexpectedMessageType,
    ReceiveError,
    SendError,
    NoRequestInFlight,
}

impl From<TransportError> for VdmLibError {
    fn from(_: TransportError) -> Self {
        VdmLibError::TransportError
    }
}

/// MCTP transport for VDM messages.
pub struct MctpVdmTransport {
    mctp: Mctp,
    cur_resp_ctx: Option<MessageInfo>,
}

impl MctpVdmTransport {
    /// Create a new MCTP VDM transport with a specific driver number.
    pub fn new(drv_num: u32) -> Self {
        Self {
            mctp: Mctp::new(drv_num),
            cur_resp_ctx: None,
        }
    }

    /// Check if the MCTP driver exists.
    pub fn exists(&self) -> bool {
        self.mctp.exists()
    }

    /// Receive a VDM request.
    /// Returns the length of the received request.
    pub fn receive_request(&mut self, req: &mut [u8]) -> Result<usize, TransportError> {
        // Reset msg buffer
        req.fill(0);
        let (req_len, msg_info) = self
            .mctp
            .receive_request(req)
            
            .map_err(|_| TransportError::ReceiveError)?;

        if req_len == 0 {
            return Err(TransportError::BufferTooSmall);
        }

        // Check common header
        let mctp_hdr = MctpCommonHeader(req[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_VDM_MSG_TYPE {
            return Err(TransportError::UnexpectedMessageType);
        }

        self.cur_resp_ctx = Some(msg_info);

        Ok(req_len as usize)
    }

    /// Send a VDM response.
    pub fn send_response(&mut self, resp: &[u8]) -> Result<(), TransportError> {
        // Ensure the response buffer is large enough to contain the MCTP common header.
        if resp.is_empty() {
            return Err(TransportError::BufferTooSmall);
        }

        let mctp_hdr = MctpCommonHeader(resp[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_VDM_MSG_TYPE {
            return Err(TransportError::UnexpectedMessageType);
        }

        if let Some(msg_info) = self.cur_resp_ctx.clone() {
            self.mctp
                .send_response(resp, msg_info)
                
                .map_err(|_| TransportError::SendError)?;
        } else {
            return Err(TransportError::NoRequestInFlight);
        }

        self.cur_resp_ctx = None;

        Ok(())
    }

    /// Get the maximum message size supported by the transport.
    pub fn max_message_size(&self) -> Result<u32, TransportError> {
        self.mctp
            .max_message_size()
            .map_err(|_| TransportError::DriverError)
    }

    // =========================================================================
    // Non-blocking (upcall-driven) API
    // =========================================================================

    /// Set up a non-blocking receive-request operation.
    pub fn setup_non_blocking(
        &mut self,
        buf: &'static mut [u8],
        notify: &'static UpcallNotification,
    ) -> Result<(), TransportError> {
        self.mctp
            .setup_receive_request(buf, notify)
            .map_err(|_| TransportError::DriverError)
    }

    /// Check if a request has arrived and populate `msg_buf` from the shared buffer.
    pub fn try_receive_from_buffer(
        &mut self,
        notify: &UpcallNotification,
        nb_buf: &[u8],
        msg_buf: &mut [u8],
    ) -> Result<Option<usize>, TransportError> {
        if !notify.is_ready() {
            return Ok(None);
        }
        let (recv_len_raw, _, msg_info_raw) = notify.args();
        let recv_len = recv_len_raw as usize;
        if recv_len == 0 {
            return Err(TransportError::BufferTooSmall);
        }
        msg_buf[..recv_len].copy_from_slice(&nb_buf[..recv_len]);
        msg_buf[recv_len..].fill(0);
        let mctp_hdr = MctpCommonHeader(msg_buf[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_VDM_MSG_TYPE {
            return Err(TransportError::UnexpectedMessageType);
        }
        self.cur_resp_ctx = Some(msg_info_raw.into());
        Ok(Some(recv_len))
    }

    /// Re-arm the non-blocking receive after processing.
    pub fn rearm(&self, notify: &UpcallNotification) {
        notify.clear();
        let _ = self.mctp.arm_receive_request();
    }
}

impl Default for MctpVdmTransport {
    fn default() -> Self {
        Self::new(driver_num::MCTP_CALIPTRA)
    }
}
