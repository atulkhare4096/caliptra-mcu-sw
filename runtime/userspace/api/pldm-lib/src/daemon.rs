// Licensed under the Apache-2.0 license

use crate::cmd_interface::CmdInterface;
use crate::config;
use crate::firmware_device::fd_context::FirmwareDeviceContext;
use crate::firmware_device::fd_ops::FdOps;
use crate::firmware_device::transfer_session::TransferSession;
use crate::timer::AsyncAlarm;
use crate::transport::MctpTransport;
use caliptra_mcu_libsyscall_caliptra::mctp::driver_num;
use caliptra_mcu_libsyscall_caliptra::DefaultSyscalls;
use caliptra_mcu_libtock_console::Console;
use caliptra_mcu_pldm_common::codec::PldmCodec;
use caliptra_mcu_pldm_common::message::firmware_update::request_fw_data::{
    RequestFirmwareDataRequest, RequestFirmwareDataResponseFixed,
};
use caliptra_mcu_pldm_common::message::firmware_update::transfer_complete::TransferResult;
use caliptra_mcu_pldm_common::protocol::base::{PldmBaseCompletionCode, PldmMsgType};
use caliptra_mcu_pldm_common::protocol::firmware_update::{FwUpdateCmd, FwUpdateCompletionCode};
use caliptra_mcu_pldm_common::util::mctp_transport::{
    construct_mctp_pldm_msg, extract_pldm_msg, MAX_MCTP_PLDM_MSG_SIZE, MCTP_PLDM_MSG_HDR_LEN,
};
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use core::sync::atomic::AtomicU8;
use caliptra_mcu_libtock_platform::Syscalls;
const YIELD_EVERY_ITERATIONS: u32 = 32;

#[derive(Debug)]
pub enum PldmServiceError {
    StartError,
    StopError,
}

/// Represents a PLDM (Platform Level Data Model) service.
///
/// The `PldmService` struct encapsulates the command interface and the running state
/// of the PLDM service.
///
/// # Type Parameters
///
/// * `'a` - A lifetime parameter for the command interface.
///
/// # Fields
///
/// * `cmd_interface` - The command interface used by the PLDM service.
/// * `running` - An atomic boolean indicating whether the PLDM service is currently running.
/// * `initiator_signal` - A signal used to activate the PLDM initiator task.
/// Simple flag for inter-task signaling (replaces embassy Signal).
/// 0 = not signaled, 1 = signaled.
static INITIATOR_FLAG: AtomicU8 = AtomicU8::new(0);

pub struct PldmService<'a> {
    cmd_interface: CmdInterface<'a>,
    running: &'static AtomicBool,
}

// Note: This implementation is a starting point for integration testing.
// It will be extended and refactored to support additional PLDM commands in both responder and requester modes.
impl<'a> PldmService<'a> {
    pub fn init(fdops: &'a dyn FdOps) -> Self {
        let cmd_interface = CmdInterface::new(
            config::PLDM_PROTOCOL_CAPABILITIES.get(),
            FirmwareDeviceContext::new(fdops),
        );
        Self {
            cmd_interface,
            running: {
                static RUNNING: AtomicBool = AtomicBool::new(false);
                &RUNNING
            },
        }
    }

    pub fn start(&mut self) -> Result<(), PldmServiceError> {
        if self.running.load(Ordering::SeqCst) {
            return Err(PldmServiceError::StartError);
        }

        self.running.store(true, Ordering::SeqCst);

        let cmd_interface: &'static CmdInterface<'static> =
            unsafe { core::mem::transmute(&self.cmd_interface) };

        // Run combined responder+initiator loop (blocks)
        pldm_service_loop(cmd_interface, self.running);
        Ok(())
    }

    /// Start the service and process messages until `done` returns true.
    ///
    /// Unlike `start()`, this returns control to the caller once the condition
    /// is met. The service remains in the running state so it can be resumed
    /// with another call to `run_until`.
    pub fn run_until<F: Fn() -> bool>(&mut self, done: F) -> Result<(), PldmServiceError> {
        if !self.running.load(Ordering::SeqCst) {
            self.running.store(true, Ordering::SeqCst);
        }

        let cmd_interface: &'static CmdInterface<'static> =
            unsafe { core::mem::transmute(&self.cmd_interface) };

        let mut transport = MctpTransport::new(driver_num::MCTP_PLDM);
        let mut msg_buffer = [0; MAX_MCTP_PLDM_MSG_SIZE];
        let mut console_writer = Console::<DefaultSyscalls>::writer();

        while self.running.load(Ordering::SeqCst) && !done() {
            match cmd_interface.handle_responder_msg(&mut transport, &mut msg_buffer) {
                Ok(_) => {}
                Err(e) => {
                    writeln!(console_writer, "PLDM_APP: Error handling responder msg: {:?}", e)
                        .unwrap();
                }
            }

            if cmd_interface.should_start_initiator_mode() {
                pldm_initiator_inline(cmd_interface, self.running, &mut transport, &mut msg_buffer);
            }
        }
        Ok(())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

pub fn pldm_responder_task(
    cmd_interface: &'static CmdInterface<'static>,
    running: &'static AtomicBool,
) {
    pldm_responder(cmd_interface, running);
}

/// Combined service loop that runs both responder and initiator in one thread.
///
/// The responder processes one incoming message per iteration.
/// When the responder detects download state, the initiator runs inline.
fn pldm_service_loop(
    cmd_interface: &'static CmdInterface<'static>,
    running: &'static AtomicBool,
) {
    let mut transport = MctpTransport::new(driver_num::MCTP_PLDM);
    let mut msg_buffer = [0; MAX_MCTP_PLDM_MSG_SIZE];
    let mut console_writer = Console::<DefaultSyscalls>::writer();

    while running.load(Ordering::SeqCst) {
        // Handle one responder message
        match cmd_interface.handle_responder_msg(&mut transport, &mut msg_buffer) {
            Ok(_) => {}
            Err(e) => {
                writeln!(console_writer, "PLDM_APP: Error handling responder msg: {:?}", e)
                    .unwrap();
            }
        }

        // When FD state is download state, run the initiator inline
        if cmd_interface.should_start_initiator_mode() {
            pldm_initiator_inline(cmd_interface, running, &mut transport, &mut msg_buffer);
        }
    }
}

pub fn pldm_initiator_inline(
    cmd_interface: &'static CmdInterface<'static>,
    running: &'static AtomicBool,
    transport: &mut MctpTransport,
    msg_buffer: &mut [u8; MAX_MCTP_PLDM_MSG_SIZE],
) {
    let mut console_writer = Console::<DefaultSyscalls>::writer();

    // Transfer session for optimized download - created lazily when entering download phase
    let mut session: Option<TransferSession> = None;
    let mut counter: u32 = 0;

    while running.load(Ordering::SeqCst) {
        if cmd_interface.should_stop_initiator_mode() {
            break;
        }

        // Use optimized download path when we have an active session
        if let Some(ref mut sess) = session {
            match run_optimized_download(cmd_interface, transport, msg_buffer, sess)
                    
                {
                    Ok(download_complete) => {
                        if download_complete {
                            // Sync session state back to internal state
                            cmd_interface.sync_transfer_session(sess);
                            session = None;
                            // Fall through to regular handling for TransferComplete/Verify/Apply
                        }
                    }
                    Err(e) => {
                        writeln!(
                            console_writer,
                            "PLDM_APP: Error in optimized download: {:?}",
                            e
                        )
                        .unwrap();
                        // Sync and fall back to regular path
                        cmd_interface.sync_transfer_session(sess);
                        session = None;
                    }
                }
            }

            if session.is_some() {
                // yield every so often still so that we handle cancelations
                counter = counter.wrapping_add(1);
                if counter % YIELD_EVERY_ITERATIONS == 0 {
                    let _ = AsyncAlarm::<DefaultSyscalls>::sleep_ticks(1);
                }
            } else {
                // Handle phases via regular path, which will properly wait for Download state
                match cmd_interface
                    .handle_initiator_msg(transport, msg_buffer)
                    
                {
                    Ok(_) => {
                        // After successful handling, check if we should switch to optimized download
                        // The regular handler will have processed the first chunk; now create session
                        // for subsequent chunks if we're still in download phase
                        if cmd_interface.should_start_initiator_mode() {
                            session = Some(cmd_interface.create_transfer_session());
                            counter = 0;
                        }
                    }
                    Err(e) => {
                        writeln!(
                            console_writer,
                            "PLDM_APP: Error handling initiator msg: {:?}",
                            e
                        )
                        .unwrap();
                    }
                }

                // Sleep to yield control (only in non-optimized path)
                let _ = AsyncAlarm::<DefaultSyscalls>::sleep_ticks(1);
            }
        }
    }

#[allow(dead_code)]
pub fn pldm_responder(
    cmd_interface: &'static CmdInterface<'static>,
    running: &'static AtomicBool,
) {
    let mut transport = MctpTransport::new(driver_num::MCTP_PLDM);

    let mut msg_buffer = [0; MAX_MCTP_PLDM_MSG_SIZE];
    let mut console_writer = Console::<DefaultSyscalls>::writer();

    while running.load(Ordering::SeqCst) {
        match cmd_interface
            .handle_responder_msg(&mut transport, &mut msg_buffer)
            
        {
            Ok(_) => {}
            Err(e) => {
                writeln!(
                    console_writer,
                    "PLDM_APP: Error handling responder msg: {:?}",
                    e
                )
                .unwrap();
            }
        }

        // When FD state is download state, flag for the initiator
        if cmd_interface.should_start_initiator_mode() {
            INITIATOR_FLAG.store(1, Ordering::SeqCst);
        }
    }
}

/// Optimized download loop that uses a local TransferSession to minimize mutex acquisitions.
///
/// This function runs the download phase with the session state kept outside the async mutex,
/// only syncing back periodically or when the transfer completes/is cancelled.
fn run_optimized_download(
    cmd_interface: &'static CmdInterface<'static>,
    transport: &mut MctpTransport,
    msg_buffer: &mut [u8],
    session: &mut TransferSession,
) -> Result<bool, crate::error::MsgHandlerError> {
    let ua_eid: u8 = crate::config::UA_EID;
    let ops = cmd_interface.ops();

    // Check for cancellation (atomic, no mutex)
    if cmd_interface.is_cancelled() {
        session.mark_complete(TransferResult::FdAbortedTransfer);
        return Ok(true); // Signal that download phase is done
    }

    let now = cmd_interface.now();

    // Check T1 timeout
    if session.is_t1_timeout(now) {
        session.mark_failed(TransferResult::FdAbortedTransfer);
        return Ok(true);
    }

    // Check if we should send a request
    if !session.should_send_request(now) {
        return Ok(false);
    }

    // If transfer is complete, signal done (TransferComplete will be handled by fallback path)
    if session.complete {
        return Ok(true);
    }

    // Query offset and length from ops (this is an async call but necessary)
    let (requested_offset, requested_length) = ops
        .query_download_offset_and_length(&session.component)
        
        .map_err(crate::error::MsgHandlerError::FdOps)?;

    // Calculate chunk parameters using local session state
    let (chunk_offset, chunk_length) =
        match session.get_download_chunk(requested_offset as u32, requested_length as u32) {
            Some(chunk) => chunk,
            None => {
                session.mark_failed(TransferResult::FdAbortedTransfer);
                return Ok(true);
            }
        };

    // Update session state
    session.offset = chunk_offset;
    session.length = chunk_length;

    // Build request message
    let instance_id = session.alloc_next_instance_id();
    let payload =
        construct_mctp_pldm_msg(msg_buffer).map_err(crate::error::MsgHandlerError::Util)?;

    let msg_len = RequestFirmwareDataRequest::new(
        instance_id,
        PldmMsgType::Request,
        chunk_offset,
        chunk_length,
    )
    .encode(payload)
    .map_err(crate::error::MsgHandlerError::Codec)?;

    // Mark as sent
    session.mark_sent(cmd_interface.now(), FwUpdateCmd::RequestFirmwareData as u8);

    // Send request
    transport
        .send_request(ua_eid, &msg_buffer[..msg_len + MCTP_PLDM_MSG_HDR_LEN])
        
        .map_err(crate::error::MsgHandlerError::Transport)?;

    // Receive response
    transport
        .receive_response(msg_buffer)
        
        .map_err(crate::error::MsgHandlerError::Transport)?;

    // Process response
    let resp_payload = extract_pldm_msg(msg_buffer).map_err(crate::error::MsgHandlerError::Util)?;

    let rsp_fixed = RequestFirmwareDataResponseFixed::decode(resp_payload)
        .map_err(crate::error::MsgHandlerError::Codec)?;

    // Update T1 timestamp on response
    session.update_t1_timestamp(cmd_interface.now());

    match rsp_fixed.completion_code {
        code if code == PldmBaseCompletionCode::Success as u8 => {
            // Extract firmware data and pass to ops
            let fw_data = &resp_payload[core::mem::size_of::<RequestFirmwareDataResponseFixed>()..]
                .get(..chunk_length as usize)
                .ok_or(crate::error::MsgHandlerError::Codec(
                    caliptra_mcu_pldm_common::codec::PldmCodecError::BufferTooShort,
                ))?;

            let result = ops
                .download_fw_data(chunk_offset as usize, fw_data, &session.component)
                
                .map_err(crate::error::MsgHandlerError::FdOps)?;

            if result == TransferResult::TransferSuccess {
                if ops.is_download_complete(&session.component) {
                    session.mark_complete(TransferResult::TransferSuccess);
                    return Ok(true);
                } else {
                    session.mark_ready_for_next();
                }
            } else {
                session.mark_complete(result);
                return Ok(true);
            }
        }
        code if code == FwUpdateCompletionCode::RetryRequestFwData as u8 => {
            // Retry - keep state as ready
            session.mark_ready_for_next();
        }
        _ => {
            session.mark_complete(TransferResult::FdAbortedTransfer);
            return Ok(true);
        }
    }

    Ok(false)
}
