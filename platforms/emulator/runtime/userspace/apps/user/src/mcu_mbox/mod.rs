// Licensed under the Apache-2.0 license

#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
mod cmd_auth_mock;
#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
mod cmd_handler_mock;

use caliptra_mcu_libsyscall_caliptra::system::System;
use caliptra_mcu_libsyscall_caliptra::DefaultSyscalls;
use caliptra_mcu_libtock_console::Console;
use caliptra_mcu_libtock_platform::ErrorCode;
use caliptra_mcu_libtock_platform::Syscalls;
use core::fmt::Write;

#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
use caliptra_mcu_libtockasync::blocking::UpcallNotification;

#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
use caliptra_mcu_mbox_common::messages::{McuMailboxReq, McuMailboxResp};

/// Non-blocking notification for the MCU Mbox cooperative path.
#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
static MBOX_NOTIFY: UpcallNotification = UpcallNotification::new();

// Module-level statics for the non-blocking cooperative path.
// SAFETY: single-threaded Tock userspace — no concurrent access.
#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
static mut MBOX_NB_BUF: [u8; core::mem::size_of::<McuMailboxReq>()] =
    [0; core::mem::size_of::<McuMailboxReq>()];

#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
static mut MBOX_RESP_BUF: [u8; core::mem::size_of::<McuMailboxResp>()] =
    [0; core::mem::size_of::<McuMailboxResp>()];

#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
static mut CMD_IFACE: core::mem::MaybeUninit<
    caliptra_mcu_mbox_lib::cmd_interface::CmdInterface<'static>,
> = core::mem::MaybeUninit::uninit();

pub fn mcu_mbox_task() {
    match start_mcu_mbox_service() {
        Ok(_) => {}
        Err(_) => System::exit(1),
    }
}

#[allow(dead_code)]
#[allow(unused_variables)]
fn start_mcu_mbox_service() -> Result<(), ErrorCode> {
    let mut console_writer = Console::<DefaultSyscalls>::writer();
    writeln!(console_writer, "Starting MCU_MBOX task...").unwrap();

    #[cfg(any(
        feature = "test-mcu-mbox-cmds",
        feature = "test-mcu-mbox-fips-self-test",
        feature = "test-mcu-mbox-fips-periodic",
        feature = "test-caliptra-util-host-validator"
    ))]
    {
        let handler = cmd_handler_mock::NonCryptoCmdHandlerMock::default();
        let mut cmd_authorizer = cmd_auth_mock::MockCommandAuthorizer::default();
        let mut transport = caliptra_mcu_mbox_lib::transport::McuMboxTransport::new(
            caliptra_mcu_libsyscall_caliptra::mcu_mbox::MCU_MBOX0_DRIVER_NUM,
        );
        let mut mcu_mbox_service = caliptra_mcu_mbox_lib::daemon::McuMboxService::init(
            &handler,
            &mut cmd_authorizer,
            &mut transport,
        );
        writeln!(
            console_writer,
            "Starting MCU_MBOX service for integration tests..."
        )
        .unwrap();

        if let Err(e) = mcu_mbox_service.start() {
            writeln!(
                console_writer,
                "USER_APP: Error starting MCU_MBOX service: {:?}",
                e
            )
            .unwrap();
        }
        // Service blocks in start(); if it returns, suspend here
        loop { DefaultSyscalls::yield_wait(); }
    }

    Ok(())
}

/// Initialize MCU Mbox for non-blocking cooperative polling.
///
/// Sets up the transport with a static buffer and notification, and creates
/// the command interface. Returns `true` if setup succeeded.
///
/// After calling this, use [`poll_one()`] in the cooperative loop.
#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
pub(crate) fn init_polling() -> bool {
    use static_cell::StaticCell;

    let mut cw = Console::<DefaultSyscalls>::writer();

    static HANDLER: StaticCell<cmd_handler_mock::NonCryptoCmdHandlerMock> = StaticCell::new();
    static AUTHORIZER: StaticCell<cmd_auth_mock::MockCommandAuthorizer> = StaticCell::new();
    static TRANSPORT: StaticCell<caliptra_mcu_mbox_lib::transport::McuMboxTransport> =
        StaticCell::new();

    let handler = HANDLER.init(cmd_handler_mock::NonCryptoCmdHandlerMock::default());
    let authorizer = AUTHORIZER.init(cmd_auth_mock::MockCommandAuthorizer::default());
    let transport = TRANSPORT.init(caliptra_mcu_mbox_lib::transport::McuMboxTransport::new(
        caliptra_mcu_libsyscall_caliptra::mcu_mbox::MCU_MBOX0_DRIVER_NUM,
    ));

    // Set up non-blocking receive using module-level static buffer
    #[allow(static_mut_refs)]
    let nb_buf: &'static mut [u8] = unsafe { &mut MBOX_NB_BUF };

    if transport.setup_non_blocking(nb_buf, &MBOX_NOTIFY).is_err() {
        writeln!(cw, "MCU_MBOX: setup_non_blocking failed").unwrap();
        return false;
    }

    // Store the CmdInterface in the module-level static.
    // SAFETY: single-threaded, init_polling called once before poll_one.
    unsafe {
        CMD_IFACE.write(caliptra_mcu_mbox_lib::cmd_interface::CmdInterface::new(
            transport, handler, authorizer,
        ));
    }

    writeln!(cw, "MCU_MBOX: polling initialized").unwrap();
    true
}

/// Poll the MCU Mbox for one pending command. Returns `true` if handled.
///
/// Must only be called after a successful [`init_polling()`].
#[cfg(any(
    feature = "test-mcu-mbox-cmds",
    feature = "test-mcu-mbox-fips-self-test",
    feature = "test-mcu-mbox-fips-periodic",
    feature = "test-caliptra-util-host-validator"
))]
pub(crate) fn poll_one() -> bool {
    // SAFETY: single-threaded Tock userspace. init_polling() must have
    // been called and CMD_IFACE initialized before this function runs.
    #[allow(static_mut_refs)]
    let cmd_iface = unsafe { CMD_IFACE.assume_init_mut() };

    #[allow(static_mut_refs)]
    let nb_ref: &[u8] = unsafe { &MBOX_NB_BUF };

    #[allow(static_mut_refs)]
    let resp_buf = unsafe { &mut MBOX_RESP_BUF };

    cmd_iface.poll_one(nb_ref, &MBOX_NOTIFY, resp_buf)
}
