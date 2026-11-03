//! UEFI boot-option enumeration and handoff helpers.
//!
//! The firmware stores its boot entries as NVRAM variables named `Boot0000`,
//! `Boot0001`, ... with the global variable GUID. `BootOrder` is a `UINT16`
//! array giving the preferred order. Each `Boot####` variable is an
//! `EFI_LOAD_OPTION`.

use core::ffi::c_void;

use crate::efi::*;

pub const MAX_BOOT_OPTIONS: usize = 32;

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub struct BootOption {
    pub number: u16,
    pub attributes: u32,
    pub description_ptr: *const u16,
    pub description_len: usize,
    pub file_path_ptr: *const u8,
    pub file_path_size: usize,
    pub data_ptr: *mut u8,
}

impl BootOption {
    pub const ZERO: BootOption = BootOption {
        number: 0,
        attributes: 0,
        description_ptr: core::ptr::null(),
        description_len: 0,
        file_path_ptr: core::ptr::null(),
        file_path_size: 0,
        data_ptr: core::ptr::null_mut(),
    };
}

#[allow(dead_code)]
pub struct LoadOptionView<'a> {
    pub attributes: u32,
    pub description: &'a [u16],
    pub file_path: &'a [u8],
    pub optional_data: &'a [u8],
}

pub fn parse_boot_order(data: &[u8]) -> &[u16] {
    if data.len() < 2 {
        return &[];
    }
    let count = data.len() / 2;
    // UEFI pool data is suitably aligned; tests use aligned buffers.
    unsafe { core::slice::from_raw_parts(data.as_ptr() as *const u16, count) }
}

pub fn parse_load_option(data: &[u8]) -> Option<LoadOptionView<'_>> {
    if data.len() < 6 {
        return None;
    }
    let attributes = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let file_path_list_length = u16::from_le_bytes([data[4], data[5]]) as usize;

    // Description is a null-terminated UTF-16 string starting at offset 6.
    let mut i = 6usize;
    let desc_start = i;
    let mut found_null = false;
    while i + 1 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 {
            found_null = true;
            break;
        }
        i += 2;
    }
    if !found_null {
        return None;
    }
    let desc_bytes = &data[desc_start..i];
    if desc_bytes.len() % 2 != 0 {
        return None;
    }
    let description = unsafe {
        core::slice::from_raw_parts(desc_bytes.as_ptr() as *const u16, desc_bytes.len() / 2)
    };
    i += 2; // skip terminating null

    if i + file_path_list_length > data.len() {
        return None;
    }
    let file_path = &data[i..i + file_path_list_length];
    i += file_path_list_length;
    let optional_data = &data[i..];

    Some(LoadOptionView {
        attributes,
        description,
        file_path,
        optional_data,
    })
}

#[allow(dead_code)]
pub fn is_active(view: &LoadOptionView<'_>) -> bool {
    view.attributes & 0x0000_0001 != 0
}

// --- UEFI-only helpers below ---

type AllocatePoolFn = unsafe extern "efiapi" fn(u32, UINTN, *mut *mut c_void) -> EFI_STATUS;

const EFI_LOADER_DATA: u32 = 2;

fn read_boot_svc_fn<T>(gbs: *const c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

const BOOT_SVC_ALLOCATE_POOL: usize = 0x40;

pub unsafe fn read_variable(
    system_table: &EFI_SYSTEM_TABLE,
    name: &[u16],
    guid: &EFI_GUID,
) -> Option<(*mut u8, usize)> {
    let rt = system_table.runtime_services as *mut EFI_RUNTIME_SERVICES;
    if rt.is_null() {
        return None;
    }
    let get_variable = (*rt).get_variable;

    // First probe the required size.
    let mut size: UINTN = 0;
    let mut attrs: u32 = 0;
    let status = get_variable(
        name.as_ptr(),
        guid,
        &mut attrs,
        &mut size,
        core::ptr::null_mut(),
    );
    if status != EFI_BUFFER_TOO_SMALL {
        // Accept the odd case of a zero-sized variable.
        if status == EFI_SUCCESS && size == 0 {
            // allocate 1 byte so the caller gets a non-null buffer
            size = 1;
        } else {
            return None;
        }
    }

    let gbs = system_table.boot_services;
    let allocate_pool: AllocatePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_ALLOCATE_POOL);
    let mut buf: *mut u8 = core::ptr::null_mut();
    let alloc_status = allocate_pool(EFI_LOADER_DATA, size, &mut buf as *mut *mut u8 as *mut *mut c_void);
    if alloc_status != EFI_SUCCESS || buf.is_null() {
        return None;
    }

    let status = get_variable(
        name.as_ptr(),
        guid,
        &mut attrs,
        &mut size,
        buf as *mut c_void,
    );
    if status != EFI_SUCCESS {
        return None;
    }
    Some((buf, size as usize))
}

fn format_boot_name(num: u16, out: &mut [u16; 9]) {
    let hex = b"0123456789ABCDEF";
    out[0] = b'B' as u16;
    out[1] = b'o' as u16;
    out[2] = b'o' as u16;
    out[3] = b't' as u16;
    for i in 0..4usize {
        let shift = 12 - i * 4;
        let nibble = ((num >> shift) & 0xF) as usize;
        out[4 + i] = hex[nibble] as u16;
    }
    out[8] = 0;
}

pub unsafe fn enumerate_boot_options(
    system_table: &EFI_SYSTEM_TABLE,
) -> ([BootOption; MAX_BOOT_OPTIONS], usize) {
    let mut options = [BootOption::ZERO; MAX_BOOT_OPTIONS];
    let mut count = 0usize;

    let order_name: [u16; 11] = {
        let mut a = [0u16; 11];
        let s = b"BootOrder\0";
        for (i, &b) in s.iter().enumerate() {
            a[i] = b as u16;
        }
        a
    };

    let (order_buf, order_size) =
        match read_variable(system_table, &order_name, &GLOBAL_VARIABLE_GUID) {
            Some(x) => x,
            None => return (options, 0),
        };
    let order = parse_boot_order(core::slice::from_raw_parts(order_buf, order_size as usize));

    for &num in order {
        if count >= MAX_BOOT_OPTIONS {
            break;
        }
        let mut name_buf = [0u16; 9];
        format_boot_name(num, &mut name_buf);

        let (var_buf, var_size) = match read_variable(system_table, &name_buf, &GLOBAL_VARIABLE_GUID) {
            Some(x) => x,
            None => continue,
        };
        let view = match parse_load_option(core::slice::from_raw_parts(var_buf, var_size as usize)) {
            Some(v) => v,
            None => continue,
        };

        options[count] = BootOption {
            number: num,
            attributes: view.attributes,
            description_ptr: view.description.as_ptr(),
            description_len: view.description.len(),
            file_path_ptr: view.file_path.as_ptr(),
            file_path_size: view.file_path.len(),
            data_ptr: var_buf,
        };
        count += 1;
    }

    (options, count)
}

pub unsafe fn description_ascii(opt: &BootOption, out: &mut [u8]) -> usize {
    if opt.description_ptr.is_null() {
        return 0;
    }
    let slice = core::slice::from_raw_parts(opt.description_ptr, opt.description_len);
    let n = out.len().min(slice.len());
    for i in 0..n {
        let c = slice[i];
        out[i] = if c < 0x80 { c as u8 } else { b'?' };
    }
    // null terminate if space
    if n < out.len() {
        out[n] = 0;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_boot_order_empty() {
        assert!(parse_boot_order(&[]).is_empty());
        assert!(parse_boot_order(&[1]).is_empty());
    }

    #[test]
    fn parse_boot_order_values() {
        // 0x0001, 0x0002 in little-endian
        let data = [0x01, 0x00, 0x02, 0x00];
        let v = parse_boot_order(&data);
        assert_eq!(v, &[1u16, 2u16]);
    }

    #[test]
    fn parse_load_option_basic() {
        // attributes=1, file_path_list_length=4, description "Hi\0", then 4 bytes path, then optional
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&4u16.to_le_bytes());
        // description "Hi\0"
        data.extend_from_slice(&[b'H' as u8, 0, b'i' as u8, 0, 0, 0]);
        // file path (4 bytes)
        data.extend_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD]);
        // optional
        data.extend_from_slice(&[0x11, 0x22]);
        let v = parse_load_option(&data).unwrap();
        assert_eq!(v.attributes, 1);
        // description as u16
        assert_eq!(v.description, &[b'H' as u16, b'i' as u16]);
        assert_eq!(v.file_path, &[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(v.optional_data, &[0x11, 0x22]);
    }

    #[test]
    fn parse_load_option_no_null_desc() {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&[b'H' as u8, 0]); // no terminator
        assert!(parse_load_option(&data).is_none());
    }

    #[test]
    fn parse_load_option_fp_too_long() {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&10u16.to_le_bytes());
        data.extend_from_slice(&[0, 0]); // empty desc
        data.extend_from_slice(&[0xAA; 4]);
        assert!(parse_load_option(&data).is_none());
    }

    #[test]
    fn is_active_works() {
        let v = LoadOptionView {
            attributes: 1,
            description: &[],
            file_path: &[],
            optional_data: &[],
        };
        assert!(is_active(&v));
        let v2 = LoadOptionView { attributes: 0, ..v };
        assert!(!is_active(&v2));
    }
}
