// Licensed under the Apache-2.0 license

#![cfg_attr(target_arch = "riscv32", no_std)]
#![cfg_attr(target_arch = "riscv32", no_main)]
#![allow(static_mut_refs)]

use core::fmt::Write;

mod caliptra_cmd_handler;
#[cfg(any(
    feature = "test-firmware-update-streaming",
    feature = "test-firmware-update-flash"
))]
mod firmware_update;
mod image_loader;
mod mcu_mbox;
mod soc_env;
mod spdm;
mod vdm;

#[cfg(target_arch = "riscv32")]
mod riscv;

struct EmulatorWriter {}
static mut EMULATOR_WRITER: EmulatorWriter = EmulatorWriter {};

impl Write for EmulatorWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        print_to_console(s);
        Ok(())
    }
}

fn print_to_console(buf: &str) {
    for b in buf.bytes() {
        // Print to this address for emulator output
        unsafe {
            core::ptr::write_volatile(0x1000_1041 as *mut u8, b);
        }
    }
}

#[cfg(not(target_arch = "riscv32"))]
pub(crate) fn kernel() -> caliptra_mcu_libtock_unittest::fake::Kernel {
    use caliptra_mcu_libtock_unittest::fake;
    let kernel = fake::Kernel::new();
    let console = fake::Console::new();
    kernel.add_driver(&console);
    kernel
}

#[cfg(not(target_arch = "riscv32"))]
fn main() {
    if cfg!(feature = "test-do-nothing") {
        #[allow(clippy::empty_loop)]
        loop {}
    }
    let _kernel = kernel();
    start();
}

fn start() {
    unsafe {
        #[allow(static_mut_refs)]
        caliptra_mcu_romtime::set_printer(&mut EMULATOR_WRITER);
    }
    // Boot-time operations (run to completion, before executor starts)
    image_loader::image_loading_task();

    // When MCU mbox test features are enabled, use the cooperative polling loop
    // so mbox commands are serviced alongside SPDM.
    #[cfg(any(
        feature = "test-mcu-mbox-cmds",
        feature = "test-mcu-mbox-fips-self-test",
        feature = "test-mcu-mbox-fips-periodic",
        feature = "test-caliptra-util-host-validator"
    ))]
    {
        let mbox_enabled = mcu_mbox::init_polling();

        #[cfg(not(any(
            feature = "test-firmware-update-streaming",
            feature = "test-firmware-update-flash"
        )))]
        spdm::spdm_cooperative_main(&mut || {
            if mbox_enabled {
                mcu_mbox::poll_one();
            }
        });

        #[cfg(feature = "test-mcu-mbox-fips-periodic")]
        caliptra_mcu_mbox_lib::fips_periodic::fips_periodic_task();
    }

    // Otherwise, use the hybrid async architecture with embassy executor.
    #[cfg(not(any(
        feature = "test-mcu-mbox-cmds",
        feature = "test-mcu-mbox-fips-self-test",
        feature = "test-mcu-mbox-fips-periodic",
        feature = "test-caliptra-util-host-validator"
    )))]
    caliptra_mcu_libtockasync::start_async(async_main());
}

#[embassy_executor::task]
async fn async_main() {
    // Runtime: SPDM async task — yields to executor on transport I/O,
    // all command handlers are sync (no state machine overhead).
    #[cfg(not(any(
        feature = "test-firmware-update-streaming",
        feature = "test-firmware-update-flash"
    )))]
    spdm::spdm_async_task().await;

    #[cfg(any(
        feature = "test-mctp-vdm-cmds",
        feature = "test-caliptra-util-host-mctp-vdm-validator"
    ))]
    vdm::vdm_task();
}
