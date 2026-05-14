// Licensed under the Apache-2.0 license

use caliptra_mcu_libsyscall_caliptra::mci::Mci;
use caliptra_mcu_libsyscall_caliptra::mcu_mbox::{CmdCode, MbxCmdStatus, McuMbox};
use caliptra_mcu_libsyscall_caliptra::DefaultSyscalls;
use caliptra_mcu_libtockasync::blocking::UpcallNotification;
use caliptra_mcu_mbox_common::messages::{verify_checksum, MailboxReqHeader, MailboxRespHeader};
use core::mem::size_of;
use zerocopy::FromBytes;

pub enum TransportError {
    DriverRxError,
    DriverTxError,
    BufferTooSmall,
    InvalidRequest,
    InvalidResponse,
    ChkSumMismatch,
}

/// MCU Mailbox Transport implementation using the McuMbox syscall interface.
pub struct McuMboxTransport {
    mbox: McuMbox,
    ready_signaled: bool,
}

impl McuMboxTransport {
    pub fn new(drv_num: u32) -> Self {
        Self {
            mbox: McuMbox::new(drv_num),
            ready_signaled: false,
        }
    }

    pub fn receive_request<'a>(
        &mut self,
        buf: &'a mut [u8],
    ) -> Result<(CmdCode, &'a [u8]), TransportError> {
        if buf.len() < size_of::<MailboxReqHeader>() {
            return Err(TransportError::BufferTooSmall);
        }

        buf.fill(0);

        let on_listening_cb = if !self.ready_signaled {
            self.ready_signaled = true;
            Some(|| {
                let mci = Mci::<DefaultSyscalls>::new();
                mci.set_mailbox_ready().unwrap();
            })
        } else {
            None
        };

        let (cmd_opcode, req_len) = self
            .mbox
            .receive_command(buf, on_listening_cb)
            
            .map_err(|_| TransportError::DriverRxError)?;

        if req_len < size_of::<MailboxReqHeader>() {
            return Err(TransportError::InvalidRequest);
        }

        let hdr = MailboxReqHeader::ref_from_bytes(&buf[..size_of::<MailboxReqHeader>()])
            .map_err(|_| TransportError::InvalidRequest)?;
        // Retrieve payload for checksum verification
        let payload = &buf[size_of::<u32>()..req_len];
        if !verify_checksum(hdr.chksum, cmd_opcode, payload) {
            return Err(TransportError::ChkSumMismatch);
        }

        Ok((cmd_opcode, &buf[..req_len]))
    }

    pub fn send_response(&mut self, resp: &[u8]) -> Result<(), TransportError> {
        if resp.len() < size_of::<MailboxRespHeader>() {
            return Err(TransportError::BufferTooSmall);
        }

        let hdr = MailboxRespHeader::ref_from_bytes(&resp[..size_of::<MailboxRespHeader>()])
            .map_err(|_| TransportError::InvalidResponse)?;
        let payload = &resp[size_of::<u32>()..];
        if !verify_checksum(hdr.chksum, 0, payload) {
            return Err(TransportError::ChkSumMismatch);
        }

        self.mbox
            .send_response(resp)
            
            .map_err(|_| TransportError::DriverTxError)?;

        Ok(())
    }

    pub fn finalize_response(&self, status: MbxCmdStatus) -> Result<(), TransportError> {
        self.mbox
            .finish_response(status)
            .map_err(|_| TransportError::DriverTxError)
    }

    // =========================================================================
    // Non-blocking (upcall-driven) API
    // =========================================================================

    /// Set up a non-blocking receive operation.
    ///
    /// Shares `buf` with the kernel, registers the upcall on `notify`, and
    /// issues the RECEIVE_REQUEST command. Returns immediately.
    ///
    /// Also signals mailbox-ready to MCI if not already done.
    pub fn setup_non_blocking(
        &mut self,
        buf: &'static mut [u8],
        notify: &'static UpcallNotification,
    ) -> Result<(), TransportError> {
        self.mbox
            .setup_receive_command(buf, notify)
            .map_err(|_| TransportError::DriverRxError)?;
        if !self.ready_signaled {
            self.ready_signaled = true;
            let mci = Mci::<DefaultSyscalls>::new();
            mci.set_mailbox_ready().map_err(|_| TransportError::DriverRxError)?;
        }
        Ok(())
    }

    /// Check if a command has arrived. Returns validated request data or None.
    ///
    /// `nb_buf` must be the same buffer passed to `setup_non_blocking()`.
    pub fn try_receive_request<'a>(
        &self,
        nb_buf: &'a [u8],
        notify: &UpcallNotification,
    ) -> Result<Option<(CmdCode, usize)>, TransportError> {
        if !notify.is_ready() {
            return Ok(None);
        }
        let (cmd_opcode, recv_len_raw, _) = notify.args();
        let recv_len = recv_len_raw as usize;
        if recv_len < size_of::<MailboxReqHeader>() {
            return Err(TransportError::InvalidRequest);
        }
        let hdr = MailboxReqHeader::ref_from_bytes(&nb_buf[..size_of::<MailboxReqHeader>()])
            .map_err(|_| TransportError::InvalidRequest)?;
        let payload = &nb_buf[size_of::<u32>()..recv_len];
        if !verify_checksum(hdr.chksum, cmd_opcode, payload) {
            return Err(TransportError::ChkSumMismatch);
        }
        Ok(Some((cmd_opcode, recv_len)))
    }

    /// Re-arm the non-blocking receive after processing.
    pub fn rearm(&self, notify: &UpcallNotification) {
        notify.clear();
        let _ = self.mbox.arm_receive_command();
    }
}
