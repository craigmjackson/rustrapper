#![allow(non_camel_case_types, dead_code)]

use core::ffi::c_void;

pub type BOOLEAN = u8;
pub type UINTN = u64;
pub type EFI_STATUS = UINTN;
pub type EFI_HANDLE = *mut c_void;

pub const EFI_SUCCESS: EFI_STATUS = 0;
pub const EFI_INVALID_PARAMETER: EFI_STATUS = 2;
pub const EFI_NOT_FOUND: EFI_STATUS = 14;

pub const EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL: u32 = 0x00000001;

#[repr(C)]
pub struct EFI_GUID {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

pub static BLOCK_IO_GUID: EFI_GUID = EFI_GUID {
    data1: 0x964E5B21,
    data2: 0x6459,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

pub static DEVICE_PATH_GUID: EFI_GUID = EFI_GUID {
    data1: 0x09576E91,
    data2: 0x6D3F,
    data3: 0x11D2,
    data4: [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

#[repr(C)]
pub struct EFI_TABLE_HEADER {
    pub signature: u64,
    pub revision: u32,
    pub header_size: u32,
    pub crc32: u32,
    pub reserved: u32,
}

#[repr(C)]
pub struct EFI_BLOCK_IO_MEDIA {
    pub media_id: u32,
    pub removable_media: u8,
    pub media_present: u8,
    pub logical_partition: u8,
    pub read_only: u8,
    pub write_caching: u8,
    pub pad: [u8; 3],
    pub block_size: u32,
    pub io_align: u32,
    pub last_block: u64,
}

#[repr(C)]
pub struct EFI_BLOCK_IO_PROTOCOL {
    pub revision: u64,
    pub media: *mut EFI_BLOCK_IO_MEDIA,
    pub reset: *mut c_void,
    pub read_blocks: *mut c_void,
    pub write_blocks: *mut c_void,
    pub flush_blocks: *mut c_void,
}

#[repr(C)]
pub struct EFI_DEVICE_PATH_PROTOCOL {
    pub type_: u8,
    pub sub_type: u8,
    pub length: u16,
}

#[repr(C)]
pub struct SIMPLE_TEXT_OUTPUT_PROTOCOL {
    pub reset: *mut c_void,
    pub output_string: unsafe extern "efiapi" fn(*mut SIMPLE_TEXT_OUTPUT_PROTOCOL, *const u16) -> EFI_STATUS,
}

#[repr(C)]
pub struct EFI_SYSTEM_TABLE {
    pub hdr: EFI_TABLE_HEADER,
    pub firmware_vendor: *mut u16,
    pub firmware_revision: u32,
    pub __pad1: u32,
    pub console_in_handle: EFI_HANDLE,
    pub con_in: *mut c_void,
    pub console_out_handle: EFI_HANDLE,
    pub con_out: *mut SIMPLE_TEXT_OUTPUT_PROTOCOL,
    pub standard_error_handle: EFI_HANDLE,
    pub std_err: *mut c_void,
    pub runtime_services: *mut c_void,
    pub boot_services: *mut c_void,
    pub number_of_table_entries: u64,
    pub configuration_table: *mut c_void,
}

#[cfg(test)]
mod tests {
    use core::mem;
    use super::*;

    #[test]
    fn test_guid_size() {
        assert_eq!(mem::size_of::<EFI_GUID>(), 16);
    }

    #[test]
    fn test_block_io_media_size() {
        // 4 + 5 + 3 + 4 + 4 + pad(4) + 8 = 32 (last_block requires 8-byte alignment)
        assert_eq!(mem::size_of::<EFI_BLOCK_IO_MEDIA>(), 32);
    }

    #[test]
    fn test_block_io_protocol_size() {
        assert_eq!(mem::size_of::<EFI_BLOCK_IO_PROTOCOL>(), 48);
    }

    #[test]
    fn test_device_path_size() {
        assert_eq!(mem::size_of::<EFI_DEVICE_PATH_PROTOCOL>(), 4);
    }

    #[test]
    fn test_system_table_size() {
        // 24 (hdr) + 6 ptrs*8 + 2 u32*4 + u64 + ptr = 120
        assert_eq!(mem::size_of::<EFI_SYSTEM_TABLE>(), 120);
    }

    #[test]
    fn test_table_header_size() {
        assert_eq!(mem::size_of::<EFI_TABLE_HEADER>(), 24);
    }

    #[test]
    fn test_simple_text_output_size() {
        assert_eq!(mem::size_of::<SIMPLE_TEXT_OUTPUT_PROTOCOL>(), 16);
    }

    #[test]
    fn test_block_io_guid_value() {
        assert_eq!(BLOCK_IO_GUID.data1, 0x964E5B21);
        assert_eq!(BLOCK_IO_GUID.data2, 0x6459);
        assert_eq!(BLOCK_IO_GUID.data3, 0x11D2);
        assert_eq!(BLOCK_IO_GUID.data4, [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B]);
    }

    #[test]
    fn test_device_path_guid_value() {
        assert_eq!(DEVICE_PATH_GUID.data1, 0x09576E91);
        assert_eq!(DEVICE_PATH_GUID.data2, 0x6D3F);
        assert_eq!(DEVICE_PATH_GUID.data3, 0x11D2);
        assert_eq!(DEVICE_PATH_GUID.data4, [0x8E, 0x39, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B]);
    }

    #[test]
    fn test_efi_success() {
        assert_eq!(EFI_SUCCESS, 0);
    }

    #[test]
    fn test_efi_status_size() {
        assert_eq!(mem::size_of::<EFI_STATUS>(), mem::size_of::<UINTN>());
    }

    #[test]
    fn test_uintn_is_64() {
        assert_eq!(mem::size_of::<UINTN>(), 8);
    }

    #[test]
    fn test_efi_handle_size() {
        assert_eq!(mem::size_of::<EFI_HANDLE>(), mem::size_of::<*mut core::ffi::c_void>());
    }

    #[test]
    fn test_efi_constants() {
        assert_eq!(EFI_INVALID_PARAMETER, 2);
        assert_eq!(EFI_NOT_FOUND, 14);
        assert_eq!(EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL, 0x00000001);
    }

    #[test]
    fn test_guid_unique() {
        assert!(
            BLOCK_IO_GUID.data1 != DEVICE_PATH_GUID.data1
                || BLOCK_IO_GUID.data2 != DEVICE_PATH_GUID.data2
                || BLOCK_IO_GUID.data3 != DEVICE_PATH_GUID.data3
        );
    }
}
