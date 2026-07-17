//! UEFI executable loader

use core::ffi::c_void;
use crate::efi::*;
use common::loader::{FileFormat, detect_format};

// Boot Services function offsets (UEFI Spec 2.10)
const BOOT_SVC_LOAD_IMAGE: usize = 0x68;
const BOOT_SVC_START_IMAGE: usize = 0x70;

type LoadImageFn = unsafe extern "efiapi" fn(
    boot_policy: bool,
    parent_image_handle: EFI_HANDLE,
    device_path: *mut c_void,
    source_buffer: *mut c_void,
    source_size: UINTN,
    image_handle: *mut EFI_HANDLE,
) -> EFI_STATUS;

type StartImageFn = unsafe extern "efiapi" fn(
    image_handle: EFI_HANDLE,
    exit_data_size: *mut UINTN,
    exit_data: *mut *mut u16,
) -> EFI_STATUS;

fn read_boot_svc_fn<T>(gbs: *const c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

/// Execute a PE/COFF EFI application
pub fn execute_pe_coff(
    system_table: &EFI_SYSTEM_TABLE,
    image_handle: EFI_HANDLE,
    buffer: *mut u8,
    size: usize,
) -> Result<(), &'static str> {
    let gbs = system_table.boot_services;
    
    let load_image: LoadImageFn = read_boot_svc_fn(gbs, BOOT_SVC_LOAD_IMAGE);
    let start_image: StartImageFn = read_boot_svc_fn(gbs, BOOT_SVC_START_IMAGE);
    
    let mut loaded_image_handle: EFI_HANDLE = core::ptr::null_mut();
    
    // Load the image from memory buffer
    let status = unsafe {
        load_image(
            false, // boot_policy
            image_handle,
            core::ptr::null_mut(), // device_path (not needed for memory buffer)
            buffer as *mut c_void,
            size as UINTN,
            &mut loaded_image_handle,
        )
    };
    
    if status != EFI_SUCCESS {
        return Err("LoadImage failed");
    }
    
    // Start the image
    let mut exit_data_size: UINTN = 0;
    let mut exit_data: *mut u16 = core::ptr::null_mut();
    
    let status = unsafe {
        start_image(
            loaded_image_handle,
            &mut exit_data_size,
            &mut exit_data,
        )
    };
    
    if status != EFI_SUCCESS {
        return Err("StartImage failed");
    }
    
    Ok(())
}

/// Execute a file based on its detected format
pub fn execute_file(
    system_table: &EFI_SYSTEM_TABLE,
    image_handle: EFI_HANDLE,
    buffer: *mut u8,
    size: usize,
    puts: fn(&str),
) {
    let data = unsafe { core::slice::from_raw_parts(buffer, size) };
    let format = detect_format(data);
    
    match format {
        FileFormat::PeCoff => {
            puts("Executing PE/COFF EFI application...\n");
            match execute_pe_coff(system_table, image_handle, buffer, size) {
                Ok(()) => puts("Execution completed\n"),
                Err(e) => {
                    puts("Execution failed: ");
                    puts(e);
                    puts("\n");
                }
            }
        }
        FileFormat::Text => {
            puts("Text file contents:\n");
            if let Ok(text) = core::str::from_utf8(data) {
                puts(text);
                if !text.ends_with('\n') {
                    puts("\n");
                }
            } else {
                puts("(Unable to decode as UTF-8)\n");
            }
        }
        _ => {
            puts("Binary file, size: ");
            // Simple decimal print
            let mut val = size;
            let mut digits = [0u8; 20];
            let mut i = 0;
            if val == 0 {
                digits[0] = b'0';
                i = 1;
            } else {
                while val > 0 {
                    digits[i] = b'0' + (val % 10) as u8;
                    val /= 10;
                    i += 1;
                }
            }
            let mut j = i;
            while j > 0 {
                j -= 1;
                let c = digits[j];
                puts(core::str::from_utf8(&[c]).unwrap_or("?"));
            }
            puts(" bytes\n");
        }
    }
}
