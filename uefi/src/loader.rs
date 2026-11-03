//! UEFI executable loader

use core::ffi::c_void;
use crate::efi::*;
use common::loader::{FileFormat, detect_format};

// Boot Services function offsets (UEFI Spec 2.10)
const BOOT_SVC_LOAD_IMAGE: usize = 0xC8;
const BOOT_SVC_START_IMAGE: usize = 0xD0;

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

/// Execute a downloaded PXE file: `.lua` scripts run through the built-in
/// `lua::run` interpreter (shared with the ARM64 bare-metal target), all other
/// formats fall through to [`execute_file`].
pub fn execute_pxe_file(
    system_table: &EFI_SYSTEM_TABLE,
    image_handle: EFI_HANDLE,
    buffer: *mut u8,
    size: usize,
    filename: &str,
    puts: fn(&str),
) {
    if filename.ends_with(".lua") {
        puts("Executing Lua script\n");
        let data = unsafe { core::slice::from_raw_parts(buffer, size) };
        let mut state = lua::LuaState::new();
        crate::fetch::set_io_callbacks(&mut state);
        match state.run_with_fetch_load(
            data,
            crate::u16_putc,
            crate::fetch::fetch_file,
            crate::fetch::load_file,
        ) {
            Ok(()) => puts("Lua script done\n"),
            Err(e) => {
                puts("Lua error: ");
                lua::eval::emit_error(&state, e, crate::u16_putc);
                puts("\n");
            }
        }
    } else {
        execute_file(system_table, image_handle, buffer, size, puts);
    }
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


const EFI_RESET_COLD: u32 = 0;

type AllocatePoolFn = unsafe extern "efiapi" fn(u32, UINTN, *mut *mut c_void) -> EFI_STATUS;
type SetVariableFn = unsafe extern "efiapi" fn(
    *const u16,
    *const EFI_GUID,
    u32,
    UINTN,
    *mut c_void,
) -> EFI_STATUS;
type ResetSystemFn = unsafe extern "efiapi" fn(u32, EFI_STATUS, UINTN, *mut c_void);

const EFI_LOADER_DATA: u32 = 2;
const BOOT_SVC_ALLOCATE_POOL: usize = 0x40;

fn read_boot_svc_fn<T>(gbs: *const c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

/// Hand off to an EFI application referenced by a device path (e.g. a
/// `Boot####` option's file path list). Returns only if the image exits.
/// `option_number` is used for the BootNext+Reset fallback.
pub fn boot_from_device_path(
    system_table: &EFI_SYSTEM_TABLE,
    image_handle: EFI_HANDLE,
    device_path: *const u8,
    device_path_size: usize,
    option_number: u16,
    puts: fn(&str),
) -> Result<(), &'static str> {
    let gbs = system_table.boot_services;

    let load_image: LoadImageFn = read_boot_svc_fn(gbs, BOOT_SVC_LOAD_IMAGE);
    let start_image: StartImageFn = read_boot_svc_fn(gbs, BOOT_SVC_START_IMAGE);

    // Try the original path first.
    let mut loaded: EFI_HANDLE = core::ptr::null_mut();
    let mut status = unsafe {
        load_image(
            false,
            image_handle,
            device_path as *mut c_void,
            core::ptr::null_mut(),
            0,
            &mut loaded,
        )
    };

    // If that fails with NOT_FOUND, try appending a default file path node
    // (\EFI\BOOT\BOOTX64.EFI) for device-only paths (e.g. CD-ROM).
    if status == EFI_NOT_FOUND {
        let new_path = unsafe {
            build_default_file_path(system_table, device_path, device_path_size)
        };
        if !new_path.is_null() {
            let mut loaded2: EFI_HANDLE = core::ptr::null_mut();
            let s2 = unsafe {
                load_image(
                    false,
                    image_handle,
                    new_path as *mut c_void,
                    core::ptr::null_mut(),
                    0,
                    &mut loaded2,
                )
            };
            if s2 == EFI_SUCCESS {
                loaded = loaded2;
                status = s2;
            } else {
                status = s2;
            }
        }
    }

    if status != EFI_SUCCESS {
        // Fallback: set BootNext and reset so the firmware boots the option.
        unsafe {
            let rt = system_table.runtime_services as *mut EFI_RUNTIME_SERVICES;
            if !rt.is_null() {
                let set_variable: SetVariableFn = core::mem::transmute((*rt).set_variable);
                let reset_system: ResetSystemFn = core::mem::transmute((*rt).reset_system);
                let mut name = [0u16; 9];
                let s = b"BootNext\0";
                for (i, &b) in s.iter().enumerate() {
                    name[i] = b as u16;
                }
                let mut num = option_number;
                let st = set_variable(
                    name.as_ptr(),
                    &GLOBAL_VARIABLE_GUID,
                    0x00000007,
                    2,
                    &mut num as *mut u16 as *mut c_void,
                );
                if st == EFI_SUCCESS {
                    reset_system(EFI_RESET_COLD, EFI_SUCCESS, 0, core::ptr::null_mut());
                    loop {}
                }
            }
        }
        puts("LoadImage failed for selected boot option.\n");
        return Err("LoadImage failed");
    }

    let mut exit_data_size: UINTN = 0;
    let mut exit_data: *mut u16 = core::ptr::null_mut();
    let st = unsafe {
        start_image(loaded, &mut exit_data_size, &mut exit_data)
    };
    if st != EFI_SUCCESS {
        puts("StartImage failed for selected boot option.\n");
        return Err("StartImage failed");
    }
    Ok(())
}

unsafe fn build_default_file_path(
    system_table: &EFI_SYSTEM_TABLE,
    src: *const u8,
    src_size: usize,
) -> *mut u8 {
    // Check if the path already contains a file path node (type 4, subtype 4).
    let mut has_file = false;
    let mut i = 0usize;
    while i + 4 <= src_size {
        let ty = *src.add(i);
        let sub = *src.add(i + 1);
        let len = u16::from_le_bytes([*src.add(i + 2), *src.add(i + 3)]) as usize;
        if len < 4 || i + len > src_size {
            break;
        }
        if ty == 0x04 && sub == 0x04 {
            has_file = true;
            break;
        }
        if ty == 0x7F {
            break;
        }
        i += len;
    }
    if has_file {
        return core::ptr::null_mut();
    }

    // Find end of path (first End node or src_size).
    let mut path_end = src_size;
    let mut i = 0usize;
    while i + 4 <= src_size {
        let ty = *src.add(i);
        let len = u16::from_le_bytes([*src.add(i + 2), *src.add(i + 3)]) as usize;
        if len < 4 || i + len > src_size {
            break;
        }
        if ty == 0x7F {
            path_end = i;
            break;
        }
        i += len;
    }

    // File path string \EFI\BOOT\BOOTX64.EFI
    let path_str: [u16; 25] = [
        b'\\' as u16, b'E' as u16, b'F' as u16, b'I' as u16, b'\\' as u16,
        b'B' as u16, b'O' as u16, b'O' as u16, b'T' as u16, b'\\' as u16,
        b'B' as u16, b'O' as u16, b'O' as u16, b'T' as u16, b'X' as u16,
        b'6' as u16, b'4' as u16, b'.' as u16, b'E' as u16, b'F' as u16,
        b'I' as u16, 0, 0, 0, 0,
    ];
    // actual length excluding extra zeros: count non-zero until first 0
    let mut char_count = 0usize;
    while char_count < path_str.len() && path_str[char_count] != 0 {
        char_count += 1;
    }
    let file_node_len = 4 + (char_count + 1) * 2;

    let new_size = path_end + file_node_len + 4;
    let gbs = system_table.boot_services;
    let allocate_pool: AllocatePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_ALLOCATE_POOL);
    let mut buf: *mut u8 = core::ptr::null_mut();
    let st = allocate_pool(EFI_LOADER_DATA, new_size as UINTN, &mut buf as *mut *mut u8 as *mut *mut c_void);
    if st != EFI_SUCCESS || buf.is_null() {
        return core::ptr::null_mut();
    }

    // copy original path (without end node)
    core::ptr::copy_nonoverlapping(src, buf, path_end);

    // file path node
    let off = path_end;
    *buf.add(off) = 0x04;
    *buf.add(off + 1) = 0x04;
    let ln = file_node_len as u16;
    *buf.add(off + 2) = (ln & 0xFF) as u8;
    *buf.add(off + 3) = (ln >> 8) as u8;
    let str_ptr = buf.add(off + 4) as *mut u16;
    for i in 0..=char_count {
        *str_ptr.add(i) = path_str[i];
    }

    // end entire node
    let eoff = off + file_node_len;
    *buf.add(eoff) = 0x7F;
    *buf.add(eoff + 1) = 0xFF;
    *buf.add(eoff + 2) = 0x04;
    *buf.add(eoff + 3) = 0x00;

    buf
}
