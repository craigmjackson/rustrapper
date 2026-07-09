#![allow(non_camel_case_types, dead_code)]

use core::ffi::c_void;

pub type BOOLEAN = u8;
pub type UINTN = u64;
pub type EFI_STATUS = UINTN;
pub type EFI_HANDLE = *mut c_void;

pub const EFI_SUCCESS: EFI_STATUS = 0;
pub const EFI_INVALID_PARAMETER: EFI_STATUS = 2;
pub const EFI_NOT_FOUND: EFI_STATUS = 14;
pub const EFI_NOT_STARTED: EFI_STATUS = 19;
pub const EFI_ALREADY_STARTED: EFI_STATUS = 20;

pub const EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL: u32 = 0x00000001;
pub const EFI_OPEN_PROTOCOL_GET_PROTOCOL: u32 = 0x00000002;

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

pub static SNP_GUID: EFI_GUID = EFI_GUID {
    data1: 0xA19832B9,
    data2: 0xAC25,
    data3: 0x11D3,
    data4: [0x9A, 0x2D, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D],
};

pub type EFI_EVENT = *mut c_void;

#[repr(C)]
pub struct EFI_MAC_ADDRESS {
    pub addr: [u8; 32],
}

#[repr(C)]
pub struct EFI_IP4_ADDRESS {
    pub addr: [u8; 4],
}

#[repr(C)]
pub struct EFI_SIMPLE_NETWORK_MODE {
    pub state: u32,
    pub hw_address_size: u32,
    pub media_header_size: u32,
    pub max_packet_size: u32,
    pub nv_ram_size: u32,
    pub nv_ram_access_size: u32,
    pub receive_filter_mask: u32,
    pub receive_filter_setting: u32,
    pub max_mcast_filter_count: u32,
    pub mcast_filter_count: u32,
    pub mcast_filter: [EFI_MAC_ADDRESS; 16],
    pub current_address: EFI_MAC_ADDRESS,
    pub broadcast_address: EFI_MAC_ADDRESS,
    pub permanent_address: EFI_MAC_ADDRESS,
    pub if_type: u8,
    pub mac_address_changeable: u8,
    pub multiple_tx_supported: u8,
    pub media_present_supported: u8,
    pub media_present: u8,
}

#[repr(C)]
pub struct EFI_SIMPLE_NETWORK_PROTOCOL {
    pub revision: u64,
    pub start: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL) -> EFI_STATUS,
    pub stop: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL) -> EFI_STATUS,
    pub initialize: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL, extra_rx: UINTN, extra_tx: UINTN) -> EFI_STATUS,
    pub reset: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL, extended_verification: BOOLEAN) -> EFI_STATUS,
    pub shutdown: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL) -> EFI_STATUS,
    pub receive_filters: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL, enable_mask: u32, disable_mask: u32, reset_mcast_filter: BOOLEAN, mcast_filter_count: UINTN, mcast_filter: *mut EFI_MAC_ADDRESS) -> EFI_STATUS,
    pub station_address: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL, reset: BOOLEAN, new_mac: *mut EFI_MAC_ADDRESS) -> EFI_STATUS,
    pub statistics: *mut c_void,
    pub mcast_ip_to_mac: *mut c_void,
    pub nvdata: *mut c_void,
    pub get_status: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL, interrupt_status: *mut u32, tx_buf: *mut *mut c_void) -> EFI_STATUS,
    pub transmit: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL, header_size: UINTN, buffer_size: UINTN, buffer: *mut c_void, src_addr: *mut EFI_MAC_ADDRESS, dest_addr: *mut EFI_MAC_ADDRESS, protocol: *mut u16) -> EFI_STATUS,
    pub receive: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_NETWORK_PROTOCOL, header_size: *mut UINTN, buffer_size: *mut UINTN, buffer: *mut c_void, src_addr: *mut EFI_MAC_ADDRESS, dest_addr: *mut EFI_MAC_ADDRESS, protocol: *mut u16) -> EFI_STATUS,
    pub wait_for_packet: EFI_EVENT,
    pub mode: *mut EFI_SIMPLE_NETWORK_MODE,
}

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
pub struct EFI_INPUT_KEY {
    pub scan_code: u16,
    pub unicode_char: u16,
}

#[repr(C)]
pub struct EFI_SIMPLE_TEXT_INPUT_PROTOCOL {
    pub reset: *mut c_void,
    pub read_key_stroke: unsafe extern "efiapi" fn(*mut EFI_SIMPLE_TEXT_INPUT_PROTOCOL, *mut EFI_INPUT_KEY) -> EFI_STATUS,
    pub wait_for_key: EFI_EVENT,
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
    fn test_snp_guid() {
        assert_eq!(SNP_GUID.data1, 0xA19832B9);
        assert_eq!(SNP_GUID.data2, 0xAC25);
        assert_eq!(SNP_GUID.data3, 0x11D3);
        assert_eq!(SNP_GUID.data4, [0x9A, 0x2D, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D]);
    }

    #[test]
    fn test_snp_mode_size() {
        // 10*u32 + 19*EFI_MAC_ADDRESS + 1*u8 + 4*BOOLEAN + pad(3)
        assert_eq!(mem::size_of::<EFI_SIMPLE_NETWORK_MODE>(), 656);
    }

    #[test]
    fn test_mac_address_size() {
        assert_eq!(mem::size_of::<EFI_MAC_ADDRESS>(), 32);
    }

    #[test]
    fn test_input_key_size() {
        assert_eq!(mem::size_of::<EFI_INPUT_KEY>(), 4);
    }

    #[test]
    fn test_simple_text_input_size() {
        // 2 ptrs + 1 EFI_EVENT = 3*8 = 24
        assert_eq!(mem::size_of::<EFI_SIMPLE_TEXT_INPUT_PROTOCOL>(), 24);
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
