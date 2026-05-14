// Licensed under the Apache-2.0 license

use caliptra_mcu_libsyscall_caliptra::mctp::{Mctp, MessageInfo};
use caliptra_mcu_libtockasync::blocking::UpcallNotification;
use caliptra_mcu_pldm_common::util::mctp_transport::{
    MctpCommonHeader, MCTP_COMMON_HEADER_OFFSET, MCTP_PLDM_MSG_TYPE,
};

pub enum PldmTransportType {
    Mctp,
}

#[derive(Debug)]
pub enum TransportError {
    DriverError,
    BufferTooSmall,
    UnexpectedMessageType,
    ReceiveError,
    SendError,
    ResponseNotExpected,
    NoRequestInFlight,
}

pub struct MctpTransport {
    mctp: Mctp,
    cur_resp_ctx: Option<MessageInfo>,
    cur_req_ctx: Option<MessageInfo>,
}

impl MctpTransport {
    pub fn new(drv_num: u32) -> Self {
        Self {
            mctp: Mctp::new(drv_num),
            cur_resp_ctx: None,
            cur_req_ctx: None,
        }
    }

    pub fn send_request(&mut self, dest_eid: u8, req: &[u8]) -> Result<(), TransportError> {
        let mctp_hdr = MctpCommonHeader(req[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_PLDM_MSG_TYPE {
            Err(TransportError::UnexpectedMessageType)?;
        }

        let tag = self
            .mctp
            .send_request(dest_eid, req)
            
            .map_err(|_| TransportError::SendError)?;

        self.cur_req_ctx = Some(MessageInfo { eid: dest_eid, tag });

        Ok(())
    }

    pub fn receive_response(&mut self, rsp: &mut [u8]) -> Result<(), TransportError> {
        // Reset msg buffer
        rsp.fill(0);
        let (rsp_len, _msg_info) = if let Some(msg_info) = &self.cur_req_ctx {
            self.mctp
                .receive_response(rsp, msg_info.tag, msg_info.eid)
                
                .map_err(|_| TransportError::ReceiveError)
        } else {
            Err(TransportError::ResponseNotExpected)
        }?;

        if rsp_len == 0 {
            Err(TransportError::BufferTooSmall)?;
        }

        // Check common header
        let mctp_hdr = MctpCommonHeader(rsp[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_PLDM_MSG_TYPE {
            Err(TransportError::UnexpectedMessageType)?;
        }

        self.cur_req_ctx = None;
        Ok(())
    }

    pub fn receive_request(&mut self, req: &mut [u8]) -> Result<(), TransportError> {
        // Reset msg buffer
        req.fill(0);
        let (req_len, msg_info) = self
            .mctp
            .receive_request(req)
            
            .map_err(|_| TransportError::ReceiveError)?;

        if req_len == 0 {
            Err(TransportError::BufferTooSmall)?;
        }

        // Check common header
        let mctp_hdr = MctpCommonHeader(req[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_PLDM_MSG_TYPE {
            Err(TransportError::UnexpectedMessageType)?;
        }

        self.cur_resp_ctx = Some(msg_info);

        Ok(())
    }

    pub fn send_response(&mut self, resp: &[u8]) -> Result<(), TransportError> {
        let mctp_hdr = MctpCommonHeader(resp[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_PLDM_MSG_TYPE {
            Err(TransportError::UnexpectedMessageType)?;
        }

        if let Some(msg_info) = self.cur_resp_ctx.clone() {
            self.mctp
                .send_response(resp, msg_info)
                
                .map_err(|_| TransportError::SendError)?
        } else {
            Err(TransportError::NoRequestInFlight)?;
        }

        self.cur_resp_ctx = None;

        Ok(())
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
    ///
    /// `nb_buf` must be the same buffer passed to `setup_non_blocking()`.
    /// On success, `msg_buf[..recv_len]` contains the request data and the
    /// response context is stored for `send_response()`.
    pub fn try_receive_from_buffer(
        &mut self,
        notify: &UpcallNotification,
        nb_buf: &[u8],
        msg_buf: &mut [u8],
    ) -> Result<Option<()>, TransportError> {
        if !notify.is_ready() {
            return Ok(None);
        }
        let (recv_len_raw, _, msg_info_raw) = notify.args();
        let recv_len = recv_len_raw as usize;
        if recv_len == 0 {
            return Err(TransportError::BufferTooSmall);
        }
        // Copy from shared buffer to processing buffer
        msg_buf[..recv_len].copy_from_slice(&nb_buf[..recv_len]);
        msg_buf[recv_len..].fill(0);
        // Validate MCTP header
        let mctp_hdr = MctpCommonHeader(msg_buf[MCTP_COMMON_HEADER_OFFSET]);
        if mctp_hdr.ic() != 0 || mctp_hdr.msg_type() != MCTP_PLDM_MSG_TYPE {
            return Err(TransportError::UnexpectedMessageType);
        }
        self.cur_resp_ctx = Some(msg_info_raw.into());
        Ok(Some(()))
    }

    /// Re-arm the non-blocking receive after processing.
    pub fn rearm(&self, notify: &UpcallNotification) {
        notify.clear();
        let _ = self.mctp.arm_receive_request();
    }
}
