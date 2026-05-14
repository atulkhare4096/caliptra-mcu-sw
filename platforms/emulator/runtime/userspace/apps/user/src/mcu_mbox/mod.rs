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
