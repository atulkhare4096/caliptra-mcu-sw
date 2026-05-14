// Licensed under the Apache-2.0 license

extern crate alloc;
use crate::image_loading::pldm_context::State;
use crate::image_loading::pldm_fdops::StreamingFdOps;
use caliptra_mcu_flash_image::{FlashHeader, ImageHeader};

use caliptra_mcu_libsyscall_caliptra::dma::{AXIAddr, DMAMapping};

use caliptra_mcu_libtock_platform::ErrorCode;

use caliptra_mcu_pldm_common::message::firmware_update::get_fw_params::FirmwareParameters;
use caliptra_mcu_pldm_common::message::firmware_update::verify_complete::VerifyResult;
use caliptra_mcu_pldm_common::protocol::firmware_update::Descriptor;
use caliptra_mcu_pldm_lib::daemon::PldmService;
use caliptra_mcu_pldm_lib::firmware_device::fd_ops::FdOps;

use zerocopy::FromBytes;

use super::pldm_context::{DOWNLOAD_CTX, PLDM_STATE};

const MAX_IMAGE_COUNT: u32 = 127;

fn get_pldm_state() -> State {
    PLDM_STATE.lock(|state| *state.borrow())
}

fn pldm_download_header(service: &mut PldmService<'_>) -> Result<(), ErrorCode> {
    PLDM_STATE.lock(|state| {
        let mut state = state.borrow_mut();
        *state = State::DownloadingHeader;
    });
    DOWNLOAD_CTX.lock(|ctx| {
        let mut ctx = ctx.borrow_mut();
        ctx.total_length = core::mem::size_of::<FlashHeader>();
        ctx.initial_offset = 0;
        ctx.current_offset = 0;
        ctx.total_downloaded = 0;
    });

    // Drive the PLDM service until header download completes
    service
        .run_until(|| get_pldm_state() == State::HeaderDownloadComplete)
        .map_err(|_| ErrorCode::Fail)?;

    let state = get_pldm_state();
    if state != State::HeaderDownloadComplete {
        return Err(ErrorCode::Fail);
    }

    let num_images = DOWNLOAD_CTX.lock(|ctx| {
        let ctx = ctx.borrow();
        let (header, _rest) = FlashHeader::ref_from_prefix(&ctx.header).unwrap();
        header.image_count as usize
    });

    if num_images > MAX_IMAGE_COUNT as usize {
        return Err(ErrorCode::Fail);
    }
    Ok(())
}

pub fn pldm_download_toc(service: &mut PldmService<'_>, component_id: u32) -> Result<(u32, u32), ErrorCode> {
    let num_images = DOWNLOAD_CTX.lock(|ctx| {
        let ctx = ctx.borrow();
        let (header, _rest) = FlashHeader::ref_from_prefix(&ctx.header).unwrap();
        header.image_count as usize
    });

    // Set State to DownloadingToc
    PLDM_STATE.lock(|state| {
        let mut state = state.borrow_mut();
        *state = State::DownloadingToc;
    });

    let mut image_offset_and_size = None;
    for index in 0..num_images {
        DOWNLOAD_CTX.lock(|ctx| {
            let mut ctx = ctx.borrow_mut();
            ctx.total_length = core::mem::size_of::<ImageHeader>(); // image info length
            ctx.initial_offset =
                core::mem::size_of::<FlashHeader>() + index * core::mem::size_of::<ImageHeader>();
            ctx.current_offset = ctx.initial_offset;
            ctx.total_downloaded = 0;
        });

        // Wait for TOC DownloadComplete to be ready
        loop {
            // Drive the service until TOC download completes
            service
                .run_until(|| {
                    let s = get_pldm_state();
                    s == State::TocDownloadComplete || s == State::ImageDownloadReady
                })
                .map_err(|_| ErrorCode::Fail)?;

            let is_dowload_complete = PLDM_STATE.lock(|state| {
                let mut state = state.borrow_mut();
                if *state == State::TocDownloadComplete {
                    DOWNLOAD_CTX.lock(|ctx| {
                        let ctx = ctx.borrow();
                        let (info, _rest) = ImageHeader::ref_from_prefix(&ctx.image_info).unwrap();
                        if info.identifier == component_id {
                            image_offset_and_size = Some((info.offset, info.size));
                            *state = State::ImageDownloadReady;
                        } else {
                            *state = State::DownloadingToc;
                        }
                    });

                    true
                } else {
                    false
                }
            });
            if is_dowload_complete {
                break;
            }
        }

        if image_offset_and_size.is_some() {
            break;
        }
    }

    match image_offset_and_size {
        Some(offset_size) => Ok(offset_size),
        None => Err(ErrorCode::Fail),
    }
}

pub fn pldm_download_image(
    service: &mut PldmService<'_>,
    load_address: AXIAddr,
    offset: u32,
    size: u32,
) -> Result<(), ErrorCode> {
    PLDM_STATE.lock(|state| {
        let mut state = state.borrow_mut();
        *state = State::DownloadingImage;
    });

    DOWNLOAD_CTX.lock(|ctx| {
        let mut ctx = ctx.borrow_mut();
        ctx.total_length = size as usize;
        ctx.initial_offset = offset as usize;
        ctx.current_offset = offset as usize;
        ctx.total_downloaded = 0;
        ctx.load_address = load_address;
    });

    // Drive the service until image download completes
    service
        .run_until(|| get_pldm_state() == State::ImageDownloadComplete)
        .map_err(|_| ErrorCode::Fail)?;

    let state = get_pldm_state();
    if state != State::ImageDownloadComplete {
        return Err(ErrorCode::Fail);
    }
    Ok(())
}

pub fn initialize_pldm<'a, D: DMAMapping + 'static>(
    descriptors: &'a [Descriptor],
    fw_params: &'a FirmwareParameters,
    dma_mapping: &'a D,
) -> Result<PldmService<'a>, ErrorCode> {
    let is_initialiazed = PLDM_STATE.lock(|state| {
        let mut state = state.borrow_mut();
        if *state == State::NotRunning {
            *state = State::Initializing;
            false
        } else {
            true
        }
    });
    if !is_initialiazed {
        if descriptors.is_empty() {
            panic!("PLDM descriptors cannot be empty");
        }
        let mut stud_fd_ops = StreamingFdOps::new(descriptors, fw_params, dma_mapping);
        let stud_fd_ops: &'static mut StreamingFdOps<D> =
            unsafe { core::mem::transmute(&mut stud_fd_ops) };

        let mut service = PldmService::init(stud_fd_ops);

        // Drive the service until it reaches Initialized state
        service
            .run_until(|| get_pldm_state() == State::Initialized)
            .map_err(|_| ErrorCode::Fail)?;

        let state = get_pldm_state();
        if state != State::Initialized {
            return Err(ErrorCode::Fail);
        }

        pldm_download_header(&mut service)?;
        return Ok(service);
    }
    // Already initialized — create a dummy service (shouldn't happen in practice)
    Err(ErrorCode::Already)
}

pub fn finalize(verify_result: VerifyResult) -> Result<(), ErrorCode> {
    DOWNLOAD_CTX.lock(|ctx| {
        let mut ctx = ctx.borrow_mut();
        ctx.download_complete = true;
        ctx.verify_result = verify_result;
    });
    Ok(())
}
