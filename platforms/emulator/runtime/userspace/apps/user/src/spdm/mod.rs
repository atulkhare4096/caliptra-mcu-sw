// Licensed under the Apache-2.0 license

mod cert_store;
mod device_cert_store;
mod device_measurements;
mod endorsement_certs;
#[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
mod integration_example;
pub(crate) mod shared_large_msg_buf;

#[cfg(not(feature = "pcr-quote-measurements"))]
use crate::spdm::device_measurements::ocp_eat::init_target_env_claims;
use caliptra_mcu_libtockasync::blocking::UpcallNotification;
use caliptra_mcu_libsyscall_caliptra::doe;
use caliptra_mcu_libsyscall_caliptra::mctp;
use caliptra_mcu_libsyscall_caliptra::DefaultSyscalls;
use caliptra_mcu_libtock_console::Console;
use caliptra_mcu_libtock_platform::ErrorCode;
use caliptra_mcu_libtock_platform::Syscalls;
use caliptra_mcu_spdm_lib::codec::MessageBuf;
use caliptra_mcu_spdm_lib::context::{SpdmContext, MAX_SPDM_RESPONDER_BUF_SIZE};
use caliptra_mcu_spdm_lib::error::SpdmError;
use caliptra_mcu_spdm_lib::measurements::SpdmMeasurements;
use caliptra_mcu_spdm_lib::protocol::*;
use caliptra_mcu_spdm_lib::transport::common::SpdmTransport;
use caliptra_mcu_spdm_lib::transport::common::TransportError;
use caliptra_mcu_spdm_lib::transport::doe::DoeTransport;
use caliptra_mcu_spdm_lib::transport::mctp::MctpTransport;
use core::fmt::Write;
use device_cert_store::{initialize_cert_store, SharedCertStore};

// Caliptra supported SPDM and Secure SPDM versions
const SPDM_VERSIONS: &[SpdmVersion] = &[SpdmVersion::V12, SpdmVersion::V13];
const SECURE_SPDM_VERSIONS: &[SpdmVersion] = &[SpdmVersion::V12];

// Caliptra Crypto timeout exponent (2^20 us)
const CALIPTRA_SPDM_CT_EXPONENT: u8 = 20;

pub(crate) fn spdm_task() {
    let mut console_writer = Console::<DefaultSyscalls>::writer();
    writeln!(console_writer, "SPDM_TASK: Running SPDM-TASK...").unwrap();

    // Initialize the shared large message buffer (once, before spawning responders)
    shared_large_msg_buf::init();

    // Initialize the shared certificate store
    if let Err(e) = initialize_cert_store() {
        writeln!(
            console_writer,
            "SPDM_TASK: Failed to initialize certificate store: {:?}",
            e
        )
        .unwrap();
        return;
    }

    // initialize target environment for claims (OCP EAT only)
    #[cfg(not(feature = "pcr-quote-measurements"))]
    init_target_env_claims();

    // Run MCTP responder directly (blocks)
    spdm_mctp_responder();
}

/// Non-blocking notification for the SPDM MCTP responder.
static SPDM_MCTP_NOTIFY: UpcallNotification = UpcallNotification::new();

/// Run the SPDM MCTP responder cooperatively, calling `poll_others` on each
/// iteration so other services can make progress.
///
/// This function never returns. It replaces the blocking `spdm_task()` in the
/// cooperative service loop.
pub(crate) fn spdm_cooperative_main(poll_others: &mut dyn FnMut()) {
    let mut cw = Console::<DefaultSyscalls>::writer();
    writeln!(cw, "SPDM_TASK: Running SPDM-TASK (cooperative)...").unwrap();

    shared_large_msg_buf::init();

    if let Err(e) = initialize_cert_store() {
        writeln!(cw, "SPDM_TASK: Failed to initialize certificate store: {:?}", e).unwrap();
        return;
    }

    #[cfg(not(feature = "pcr-quote-measurements"))]
    init_target_env_claims();

    // Allocate the non-blocking receive buffer as a static.
    // SAFETY: single-threaded Tock userspace — no concurrent access.
    static mut SPDM_NB_BUF: [u8; MAX_SPDM_RESPONDER_BUF_SIZE] =
        [0; MAX_SPDM_RESPONDER_BUF_SIZE];

    let mut raw_buffer = [0; MAX_SPDM_RESPONDER_BUF_SIZE];
    let mut mctp_spdm_transport = MctpTransport::new(mctp::driver_num::MCTP_SPDM);

    // Set up non-blocking receive BEFORE passing transport to SpdmContext.
    #[allow(static_mut_refs)]
    let nb_buf: &'static mut [u8] = unsafe { &mut SPDM_NB_BUF };
    if let Err(e) = mctp_spdm_transport.setup_non_blocking(nb_buf, &SPDM_MCTP_NOTIFY) {
        writeln!(cw, "SPDM_COOPERATIVE: setup_non_blocking failed: {:?}", e).unwrap();
        return;
    }

    let max_mctp_spdm_msg_size =
        (MAX_SPDM_RESPONDER_BUF_SIZE - mctp_spdm_transport.header_size()) as u32;

    let local_capabilities = DeviceCapabilities {
        ct_exponent: CALIPTRA_SPDM_CT_EXPONENT,
        flags: CapabilityFlags::default(),
        data_transfer_size: max_mctp_spdm_msg_size,
        max_spdm_msg_size: shared_large_msg_buf::LARGE_MSG_BUF_SIZE as u32,
    };
    let local_algorithms = LocalDeviceAlgorithms::default();
    let shared_cert_store = SharedCertStore::new();

    #[cfg(not(feature = "pcr-quote-measurements"))]
    let (mut device_manifest, meas_value_info) =
        device_measurements::ocp_eat::create_manifest_with_ocp_eat();
    #[cfg(feature = "pcr-quote-measurements")]
    let (mut device_manifest, meas_value_info) =
        device_measurements::pcr_quote::create_manifest_with_pcr_quote();

    let device_measurements = SpdmMeasurements::new(&meas_value_info, &mut device_manifest);

    let caliptra_cmd_handler = crate::caliptra_cmd_handler::CaliptraCmdBackend;
    let mut caliptra_vdm_handler =
        caliptra_mcu_spdm_lib::vdm_handler::iana::ocp::caliptra_vdm::CaliptraVdmHandler::new(
            &caliptra_cmd_handler,
        );
    let mut handlers_array: [&mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler; 1] =
        [&mut caliptra_vdm_handler as &mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler];
    let vdm_handlers: Option<&mut [&mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler]> =
        Some(&mut handlers_array);

    let large_msg_buf_provider = shared_large_msg_buf::SharedLargeMsgBuf::new();

    let mut ctx = match SpdmContext::new(
        SPDM_VERSIONS,
        SECURE_SPDM_VERSIONS,
        &mut mctp_spdm_transport,
        local_capabilities,
        local_algorithms,
        &shared_cert_store,
        device_measurements,
        vdm_handlers,
        &large_msg_buf_provider,
    ) {
        Ok(ctx) => ctx,
        Err(e) => {
            writeln!(cw, "SPDM_COOPERATIVE: Failed to create context: {:?}", e).unwrap();
            return;
        }
    };

    let mut msg_buffer = MessageBuf::new(&mut raw_buffer);

    // SAFETY: single-threaded Tock userspace. The kernel writes to SPDM_NB_BUF
    // before firing the upcall; we only read after is_ready() returns true.
    #[allow(static_mut_refs)]
    let nb_ref: &[u8] = unsafe { &SPDM_NB_BUF };

    // Cooperative service loop
    loop {
        match ctx.try_process_message(&mut msg_buffer, nb_ref, &SPDM_MCTP_NOTIFY) {
            Ok(true) => {
                writeln!(cw, "SPDM_COOPERATIVE: message handled").unwrap();
            }
            Ok(false) => {} // nothing ready
            Err(e) => {
                writeln!(cw, "SPDM_COOPERATIVE: error: {:?}", e).unwrap();
            }
        }

        // Let other services make progress
        poll_others();

        DefaultSyscalls::yield_wait();
    }
}

fn spdm_mctp_responder() {
    let mut raw_buffer = [0; MAX_SPDM_RESPONDER_BUF_SIZE];
    let mut cw = Console::<DefaultSyscalls>::writer();
    let mut mctp_spdm_transport: MctpTransport = MctpTransport::new(mctp::driver_num::MCTP_SPDM);

    let max_mctp_spdm_msg_size =
        (MAX_SPDM_RESPONDER_BUF_SIZE - mctp_spdm_transport.header_size()) as u32;

    let local_capabilities = DeviceCapabilities {
        ct_exponent: CALIPTRA_SPDM_CT_EXPONENT,
        flags: CapabilityFlags::default(),
        data_transfer_size: max_mctp_spdm_msg_size,
        max_spdm_msg_size: shared_large_msg_buf::LARGE_MSG_BUF_SIZE as u32,
    };

    let local_algorithms = LocalDeviceAlgorithms::default();

    // Create a wrapper for the global certificate store
    let shared_cert_store = SharedCertStore::new();

    // Measurement format: OCP EAT by default, PCR Quote if feature-gated
    #[cfg(not(feature = "pcr-quote-measurements"))]
    let (mut device_manifest, meas_value_info) =
        device_measurements::ocp_eat::create_manifest_with_ocp_eat();

    #[cfg(feature = "pcr-quote-measurements")]
    let (mut device_manifest, meas_value_info) =
        device_measurements::pcr_quote::create_manifest_with_pcr_quote();

    let device_measurements = SpdmMeasurements::new(&meas_value_info, &mut device_manifest);

    // Caliptra VDM handler for SPDM over MCTP transport
    let caliptra_cmd_handler = crate::caliptra_cmd_handler::CaliptraCmdBackend;
    let mut caliptra_vdm_handler =
        caliptra_mcu_spdm_lib::vdm_handler::iana::ocp::caliptra_vdm::CaliptraVdmHandler::new(
            &caliptra_cmd_handler,
        );
    let mut handlers_array: [&mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler; 1] =
        [&mut caliptra_vdm_handler as &mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler];
    let vdm_handlers: Option<&mut [&mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler]> =
        Some(&mut handlers_array);

    // Large message buffer provider — shared across MCTP and DOE transports.
    let large_msg_buf_provider = shared_large_msg_buf::SharedLargeMsgBuf::new();

    let mut ctx = match SpdmContext::new(
        SPDM_VERSIONS,
        SECURE_SPDM_VERSIONS,
        &mut mctp_spdm_transport,
        local_capabilities,
        local_algorithms,
        &shared_cert_store,
        device_measurements,
        vdm_handlers,
        &large_msg_buf_provider,
    ) {
        Ok(ctx) => ctx,
        Err(e) => {
            writeln!(
                cw,
                "SPDM_MCTP_RESPONDER: Failed to create SPDM context: {:?}",
                e
            )
            .unwrap();
            return;
        }
    };

    let mut msg_buffer = MessageBuf::new(&mut raw_buffer);
    loop {
        let result = ctx.process_message(&mut msg_buffer);
        match result {
            Ok(_) => {
                writeln!(cw, "SPDM_MCTP_RESPONDER: Process message successfully").unwrap();
            }
            Err(e) => {
                writeln!(cw, "SPDM_MCTP_RESPONDER: Process message failed: {:?}", e).unwrap();
            }
        }
    }
}

fn spdm_doe_responder() {
    let mut raw_buffer = [0; MAX_SPDM_RESPONDER_BUF_SIZE];
    let mut cw = Console::<DefaultSyscalls>::writer();
    let mut doe_spdm_transport: DoeTransport = DoeTransport::new(doe::driver_num::DOE_SPDM);

    let max_doe_spdm_msg_size =
        (MAX_SPDM_RESPONDER_BUF_SIZE - doe_spdm_transport.header_size()) as u32;

    let mut doe_capability_flags = CapabilityFlags::default();
    doe_capability_flags.set_key_ex_cap(1);
    doe_capability_flags.set_mac_cap(1);
    doe_capability_flags.set_encrypt_cap(1);

    let local_capabilities = DeviceCapabilities {
        ct_exponent: CALIPTRA_SPDM_CT_EXPONENT,
        flags: doe_capability_flags,
        data_transfer_size: max_doe_spdm_msg_size,
        max_spdm_msg_size: shared_large_msg_buf::LARGE_MSG_BUF_SIZE as u32,
    };

    let mut device_doe_algorithms = DeviceAlgorithms::default();
    device_doe_algorithms.set_dhe_group();
    device_doe_algorithms.set_aead_cipher_suite();
    device_doe_algorithms.set_spdm_key_schedule();
    device_doe_algorithms.set_other_param_support();

    let local_algorithms = LocalDeviceAlgorithms::new(device_doe_algorithms);

    // Create a wrapper for the global certificate store
    let shared_cert_store = SharedCertStore::new();

    // Measurement format: OCP EAT by default, PCR Quote if feature-gated
    #[cfg(not(feature = "pcr-quote-measurements"))]
    let (mut device_manifest, meas_value_info) =
        device_measurements::ocp_eat::create_manifest_with_ocp_eat();

    #[cfg(feature = "pcr-quote-measurements")]
    let (mut device_manifest, meas_value_info) =
        device_measurements::pcr_quote::create_manifest_with_pcr_quote();

    let device_measurements = SpdmMeasurements::new(&meas_value_info, &mut device_manifest);

    // Create test drivers and VDM handlers locally for integration testing
    #[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
    let (mut tdisp_driver, mut ide_km_driver) =
        integration_example::vdm_handlers::create_test_pci_sig_drivers();

    #[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
    let mut tdisp_responder =
        caliptra_mcu_spdm_lib::vdm_handler::pci_sig::tdisp::TdispResponder::new(
            integration_example::vdm_handlers::tdisp_driver::SUPPORTED_TDISP_VERSIONS,
            &mut tdisp_driver,
        );

    #[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
    let mut ide_km_responder =
        caliptra_mcu_spdm_lib::vdm_handler::pci_sig::ide_km::IdeKmResponder::new(
            &mut ide_km_driver,
        );

    #[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
    let protocol_handlers: [Option<
        &mut (dyn caliptra_mcu_spdm_lib::vdm_handler::VdmProtocolHandler + Sync),
    >; 2] = [
        tdisp_responder
            .as_mut()
            .map(|r| r as &mut (dyn caliptra_mcu_spdm_lib::vdm_handler::VdmProtocolHandler + Sync)),
        Some(
            &mut ide_km_responder
                as &mut (dyn caliptra_mcu_spdm_lib::vdm_handler::VdmProtocolHandler + Sync),
        ),
    ];

    #[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
    let mut pci_sig_handler = caliptra_mcu_spdm_lib::vdm_handler::pci_sig::PciSigCmdHandler::new(
        0x0001, // TEST_PCI_SIG_VENDOR_ID
        protocol_handlers,
    );

    #[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
    let mut handlers_array: [&mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler; 1] =
        [&mut pci_sig_handler as &mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler];

    #[cfg(feature = "test-doe-spdm-tdisp-ide-validator")]
    let vdm_handlers: Option<&mut [&mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler]> =
        Some(&mut handlers_array);

    #[cfg(not(feature = "test-doe-spdm-tdisp-ide-validator"))]
    let vdm_handlers: Option<&mut [&mut dyn caliptra_mcu_spdm_lib::vdm_handler::VdmHandler]> = None;

    // Large message buffer provider — shared across MCTP and DOE transports.
    let large_msg_buf_provider = shared_large_msg_buf::SharedLargeMsgBuf::new();

    let mut ctx = match SpdmContext::new(
        SPDM_VERSIONS,
        SECURE_SPDM_VERSIONS,
        &mut doe_spdm_transport,
        local_capabilities,
        local_algorithms,
        &shared_cert_store,
        device_measurements,
        vdm_handlers,
        &large_msg_buf_provider,
    ) {
        Ok(ctx) => ctx,
        Err(e) => {
            writeln!(
                cw,
                "SPDM_DOE_RESPONDER: Failed to create SPDM context: {:?}",
                e
            )
            .unwrap();
            return;
        }
    };

    let mut msg_buffer = MessageBuf::new(&mut raw_buffer);
    loop {
        let result = ctx.process_message(&mut msg_buffer);
        match result {
            Ok(_) => {
                writeln!(cw, "SPDM_DOE_RESPONDER: Process message successfully").unwrap();
            }
            Err(SpdmError::Transport(TransportError::DriverError(ErrorCode::NoDevice))) => {
                writeln!(cw, "SPDM_DOE_RESPONDER: No DOE device, exiting task").unwrap();
                break;
            }
            Err(e) => {
                writeln!(cw, "SPDM_DOE_RESPONDER: Process message failed: {:?}", e).unwrap();
            }
        }
    }
}
