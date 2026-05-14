// Licensed under the Apache-2.0 license

use crate::cmd_interface::CmdInterface;
use crate::transport::McuMboxTransport;
use caliptra_mcu_common_commands::{CaliptraCmdHandler, CommandAuthorizer};
use caliptra_mcu_libsyscall_caliptra::DefaultSyscalls;
use caliptra_mcu_libtock_console::Console;
use caliptra_mcu_mbox_common::messages::{McuMailboxReq, McuMailboxResp};
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug)]
pub enum McuMboxServiceError {
    StartError,
    StopError,
}

/// MCU mailbox service.
///
/// Encapsulates the command interface and running state for the MCU mailbox service.
pub struct McuMboxService<'a> {
    cmd_interface: CmdInterface<'a>,
    running: &'static AtomicBool,
}

impl<'a> McuMboxService<'a> {
    pub fn init(
        non_crypto_cmd_handler: &'a dyn CaliptraCmdHandler,
        cmd_authorizer: &'a mut dyn CommandAuthorizer,
        transport: &'a mut McuMboxTransport,
    ) -> Self {
        let cmd_interface = CmdInterface::new(transport, non_crypto_cmd_handler, cmd_authorizer);
        Self {
            cmd_interface,
            running: {
                static RUNNING: AtomicBool = AtomicBool::new(false);
                &RUNNING
            },
        }
    }

    pub fn start(&mut self) -> Result<(), McuMboxServiceError> {
        if self.running.load(Ordering::SeqCst) {
            return Err(McuMboxServiceError::StartError);
        }

        self.running.store(true, Ordering::SeqCst);

        let cmd_interface: &'static mut CmdInterface<'static> =
            unsafe { core::mem::transmute(&mut self.cmd_interface) };

        mcu_mbox_responder_task(cmd_interface, self.running);

        Ok(())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
    }
}

pub fn mcu_mbox_responder_task(
    cmd_interface: &'static mut CmdInterface<'static>,
    running: &'static AtomicBool,
) {
    mcu_mbox_responder(cmd_interface, running);
}

pub fn mcu_mbox_responder(
    cmd_interface: &'static mut CmdInterface<'static>,
    running: &'static AtomicBool,
) {
    let mut req_buf = [0; size_of::<McuMailboxReq>()];
    let mut resp_buf = [0; size_of::<McuMailboxResp>()];
    while running.load(Ordering::SeqCst) {
        if let Err(e) = cmd_interface
            .handle_responder_msg(&mut req_buf, &mut resp_buf)
            
        {
            // Debug print on error
            writeln!(
                Console::<DefaultSyscalls>::writer(),
                "mcu_mbox_responder error: {:?}",
                e
            )
            .unwrap();
        }
    }
}
