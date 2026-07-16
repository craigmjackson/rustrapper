use core::ffi::c_void;

use crate::efi::*;

type LocateHandleBufferFn = unsafe extern "efiapi" fn(
    search_type: u32,
    protocol: *const EFI_GUID,
    search_key: *mut c_void,
    handle_count: *mut UINTN,
    buffer: *mut *mut EFI_HANDLE,
) -> EFI_STATUS;

type OpenProtocolFn = unsafe extern "efiapi" fn(
    handle: EFI_HANDLE,
    protocol: *const EFI_GUID,
    interface: *mut *mut c_void,
    agent_handle: EFI_HANDLE,
    controller_handle: EFI_HANDLE,
    attributes: u32,
) -> EFI_STATUS;

type FreePoolFn = unsafe extern "efiapi" fn(buffer: *mut c_void) -> EFI_STATUS;

const BOOT_SVC_LOCATE_HANDLE_BUFFER: usize = 0x138;
const BOOT_SVC_OPEN_PROTOCOL: usize = 0x118;
const BOOT_SVC_FREE_POOL: usize = 0x48;

fn read_boot_svc_fn<T>(gbs: *const c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

fn w16(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, s: &str) {
    let mut buf = [0u16; 256];
    let bytes = s.as_bytes();
    let len = bytes.len().min(255);
    let mut i = 0;
    while i < len {
        buf[i] = bytes[i] as u16;
        i += 1;
    }
    buf[i] = 0;
    unsafe {
        (con_out.output_string)(con_out as *const _ as *mut _, buf.as_ptr());
    }
}

fn put_dec(con_out: &SIMPLE_TEXT_OUTPUT_PROTOCOL, val: u64) {
    let mut rev = [0u16; 24];
    let mut n = 0usize;
    let mut v = val;
    if v == 0 {
        rev[n] = b'0' as u16;
        n = 1;
    } else {
        while v > 0 {
            rev[n] = (b'0' as u16) + (v % 10) as u16;
            v /= 10;
            n += 1;
        }
    }
    let mut buf = [0u16; 24];
    let mut j = 0usize;
    while n > 0 {
        n -= 1;
        buf[j] = rev[n];
        j += 1;
    }
    buf[j] = 0;
    unsafe {
        (con_out.output_string)(con_out as *const _ as *mut _, buf.as_ptr());
    }
}

pub fn scan_storage_devices(
    image_handle: EFI_HANDLE,
    system_table: &EFI_SYSTEM_TABLE,
) {
    let con_out = unsafe { &*system_table.con_out };
    let gbs = system_table.boot_services;

    let locate_handle_buffer: LocateHandleBufferFn =
        read_boot_svc_fn(gbs, BOOT_SVC_LOCATE_HANDLE_BUFFER);
    let open_protocol: OpenProtocolFn = read_boot_svc_fn(gbs, BOOT_SVC_OPEN_PROTOCOL);
    let free_pool: FreePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_FREE_POOL);

    w16(con_out, "Scanning for storage devices...\r\n\r\n");

    let mut handle_count: UINTN = 0;
    let mut handle_buffer: *mut EFI_HANDLE = core::ptr::null_mut();
    let status = unsafe {
        locate_handle_buffer(
            2,
            &BLOCK_IO_GUID as *const EFI_GUID,
            core::ptr::null_mut(),
            &mut handle_count,
            &mut handle_buffer,
        )
    };

    if status != EFI_SUCCESS || handle_count == 0 {
        w16(con_out, "No storage devices found.\r\n");
        unsafe {
            free_pool(handle_buffer as *mut c_void);
        }
        return;
    }

    w16(con_out, "Found ");
    put_dec(con_out, handle_count);
    w16(con_out, " storage device(s):\r\n\r\n");

    for i in 0..handle_count {
        let handle = unsafe { *handle_buffer.add(i as usize) };

        w16(con_out, "Device ");
        put_dec(con_out, i + 1);
        w16(con_out, ":\r\n");

        let mut block_io_ptr: *mut c_void = core::ptr::null_mut();
        let st = unsafe {
            open_protocol(
                handle,
                &BLOCK_IO_GUID as *const EFI_GUID,
                &mut block_io_ptr,
                image_handle,
                core::ptr::null_mut(),
                EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL,
            )
        };

        if st == EFI_SUCCESS {
            let block_io = unsafe { &*(block_io_ptr as *const EFI_BLOCK_IO_PROTOCOL) };
            let media = unsafe { &*block_io.media };

            w16(con_out, "  Block IO Protocol:\r\n");
            w16(con_out, "    Removable: ");
            w16(con_out, if media.removable_media != 0 { "Yes\r\n" } else { "No\r\n" });
            w16(con_out, "    Media Present: ");
            w16(con_out, if media.media_present != 0 { "Yes\r\n" } else { "No\r\n" });

            if media.media_present != 0 {
                w16(con_out, "    Block Size: ");
                put_dec(con_out, media.block_size as u64);
                w16(con_out, "\r\n");
                w16(con_out, "    Total Blocks: ");
                put_dec(con_out, media.last_block + 1);
                w16(con_out, "\r\n");

                let total_size = (media.last_block + 1) * media.block_size as u64;
                let size_mb = total_size / (1024 * 1024);
                let size_gb = size_mb / 1024;

                w16(con_out, "    Size: ");
                if size_gb > 0 {
                    put_dec(con_out, size_gb);
                    w16(con_out, " GB");
                } else if size_mb > 0 {
                    put_dec(con_out, size_mb);
                    w16(con_out, " MB");
                } else {
                    put_dec(con_out, total_size / 1024);
                    w16(con_out, " KB");
                }
                w16(con_out, "\r\n");

                w16(con_out, "    Read Only: ");
                w16(con_out, if media.read_only != 0 { "Yes\r\n" } else { "No\r\n" });
            }
        }

        let mut dev_path_ptr: *mut c_void = core::ptr::null_mut();
        let st = unsafe {
            open_protocol(
                handle,
                &DEVICE_PATH_GUID as *const EFI_GUID,
                &mut dev_path_ptr,
                image_handle,
                core::ptr::null_mut(),
                EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL,
            )
        };

        if st == EFI_SUCCESS {
            let dp = unsafe { &*(dev_path_ptr as *const EFI_DEVICE_PATH_PROTOCOL) };

            w16(con_out, "  Device Path: Type=");
            put_dec(con_out, dp.type_ as u64);
            w16(con_out, " SubType=");
            put_dec(con_out, dp.sub_type as u64);

            match dp.type_ {
                1 => w16(con_out, " (Hardware)"),
                2 => w16(con_out, " (ACPI)"),
                3 => w16(con_out, " (Messaging)"),
                4 => {
                    w16(con_out, " (Media)");
                    match dp.sub_type {
                        1 => w16(con_out, " - Hard Drive"),
                        2 => w16(con_out, " - CD-ROM"),
                        _ => {}
                    }
                }
                5 => w16(con_out, " (BBS)"),
                0x7F => w16(con_out, " (End)"),
                _ => {}
            }
            w16(con_out, "\r\n");
        }

        w16(con_out, "\r\n");
    }

    unsafe {
        free_pool(handle_buffer as *mut c_void);
    }

    w16(con_out, "Scan complete.\r\n");
}
