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
pub const EFI_OPEN_PROTOCOL_BY_DRIVER: u32 = 0x00000004;
pub const EFI_OPEN_PROTOCOL_EXCLUSIVE: u32 = 0x00000008;

pub static PCI_IO_GUID: EFI_GUID = EFI_GUID {
    data1: 0x4CF5B200,
    data2: 0x68B8,
    data3: 0x4CA5,
    data4: [0x9E, 0xEC, 0xB2, 0x3E, 0x3F, 0x50, 0x02, 0x9A],
};

#[repr(u8)]
pub enum EFI_PCI_IO_PROTOCOL_WIDTH {
    Uint8 = 0,
    Uint16 = 1,
    Uint32 = 2,
    Uint64 = 3,
    FifoUint8 = 4,
    FifoUint16 = 5,
    FifoUint32 = 6,
    FifoUint64 = 7,
    FillUint8 = 8,
    FillUint16 = 9,
    FillUint32 = 10,
    FillUint64 = 11,
}

#[repr(C)]
pub struct EFI_PCI_IO_PROTOCOL {
    pub poll_mem: *mut c_void,
    pub poll_io: *mut c_void,
    pub mem: EFI_PCI_IO_PROTOCOL_ACCESS,
    pub io: EFI_PCI_IO_PROTOCOL_ACCESS,
    pub pci: EFI_PCI_IO_PROTOCOL_PCI,
    pub copy_mem: *mut c_void,
    pub map: *mut c_void,
    pub unmap: *mut c_void,
    pub allocate_buffer: *mut c_void,
    pub free_buffer: *mut c_void,
    pub flush: *mut c_void,
    pub get_location: *mut c_void,
    pub attributes: *mut c_void,
    pub get_bar_attributes: *mut c_void,
    pub set_bar_attributes: *mut c_void,
    pub rom_size: u64,
    pub rom_image: *mut c_void,
}

#[repr(C)]
pub struct EFI_PCI_IO_PROTOCOL_ACCESS {
    pub read: unsafe extern "efiapi" fn(*mut EFI_PCI_IO_PROTOCOL, EFI_PCI_IO_PROTOCOL_WIDTH, u64, UINTN, *mut c_void) -> EFI_STATUS,
    pub write: unsafe extern "efiapi" fn(*mut EFI_PCI_IO_PROTOCOL, EFI_PCI_IO_PROTOCOL_WIDTH, u64, UINTN, *const c_void) -> EFI_STATUS,
}

#[repr(C)]
pub struct EFI_PCI_IO_PROTOCOL_PCI {
    pub read: unsafe extern "efiapi" fn(*mut EFI_PCI_IO_PROTOCOL, EFI_PCI_IO_PROTOCOL_WIDTH, u64, *mut c_void) -> EFI_STATUS,
    pub write: unsafe extern "efiapi" fn(*mut EFI_PCI_IO_PROTOCOL, EFI_PCI_IO_PROTOCOL_WIDTH, u64, *const c_void) -> EFI_STATUS,
}

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

pub static LOADED_IMAGE_GUID: EFI_GUID = EFI_GUID {
    data1: 0x5B1B31A1,
    data2: 0x9562,
    data3: 0x11D2,
    data4: [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B],
};

pub static PCI_ROOT_BRIDGE_IO_GUID: EFI_GUID = EFI_GUID {
    data1: 0x2F707EBB,
    data2: 0x4A1A,
    data3: 0x11D4,
    data4: [0x9A, 0x38, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D],
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
pub struct EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL {
    pub parent_handle: EFI_HANDLE,
    pub poll_io: *mut c_void,
    pub poll_mem: *mut c_void,
    pub io: EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL_IO,
    pub mem: EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL_IO,
    pub pci: EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL_PCI,
    pub copy_mem: *mut c_void,
    pub map: *mut c_void,
    pub unmap: *mut c_void,
    pub allocate_buffer: *mut c_void,
    pub free_buffer: *mut c_void,
    pub flush: *mut c_void,
    pub get_attributes: *mut c_void,
    pub set_attributes: *mut c_void,
    pub configuration: *mut c_void,
    pub segment_number: u64,
}

#[repr(C)]
pub struct EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL_IO {
    pub read: *mut c_void,
    pub write: *mut c_void,
}

#[repr(C)]
pub struct EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL_PCI {
    pub read: *mut c_void,
    pub write: *mut c_void,
}

#[repr(C)]
pub struct EFI_LOADED_IMAGE_PROTOCOL {
    pub revision: u32,
    pub parent_handle: EFI_HANDLE,
    pub system_table: *mut EFI_SYSTEM_TABLE,
    pub device_handle: EFI_HANDLE,
    pub file_path: *mut EFI_DEVICE_PATH_PROTOCOL,
    pub reserved: *mut c_void,
    pub load_options_size: u32,
    pub load_options: *mut c_void,
    pub image_base: *mut c_void,
    pub image_size: u64,
    pub image_code_type: u32,
    pub image_data_type: u32,
    pub unload: unsafe extern "efiapi" fn(EFI_HANDLE) -> EFI_STATUS,
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

    #[test]
    fn test_pci_io_guid_value() {
        assert_eq!(PCI_IO_GUID.data1, 0x4CF5B200);
        assert_eq!(PCI_IO_GUID.data2, 0x68B8);
        assert_eq!(PCI_IO_GUID.data3, 0x4CA5);
        assert_eq!(PCI_IO_GUID.data4, [0x9E, 0xEC, 0xB2, 0x3E, 0x3F, 0x50, 0x02, 0x9A]);
    }

    #[test]
    fn test_loaded_image_guid_value() {
        assert_eq!(LOADED_IMAGE_GUID.data1, 0x5B1B31A1);
        assert_eq!(LOADED_IMAGE_GUID.data2, 0x9562);
        assert_eq!(LOADED_IMAGE_GUID.data3, 0x11D2);
        assert_eq!(LOADED_IMAGE_GUID.data4, [0x8E, 0x3F, 0x00, 0xA0, 0xC9, 0x69, 0x72, 0x3B]);
    }

    #[test]
    fn test_pci_root_bridge_io_guid_value() {
        assert_eq!(PCI_ROOT_BRIDGE_IO_GUID.data1, 0x2F707EBB);
        assert_eq!(PCI_ROOT_BRIDGE_IO_GUID.data2, 0x4A1A);
        assert_eq!(PCI_ROOT_BRIDGE_IO_GUID.data3, 0x11D4);
        assert_eq!(PCI_ROOT_BRIDGE_IO_GUID.data4, [0x9A, 0x38, 0x00, 0x90, 0x27, 0x3F, 0xC1, 0x4D]);
    }

    #[test]
    fn test_pci_io_protocol_width_values() {
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::Uint8 as u8, 0);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::Uint16 as u8, 1);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::Uint32 as u8, 2);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::Uint64 as u8, 3);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FifoUint8 as u8, 4);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FifoUint16 as u8, 5);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FifoUint32 as u8, 6);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FifoUint64 as u8, 7);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FillUint8 as u8, 8);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FillUint16 as u8, 9);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FillUint32 as u8, 10);
        assert_eq!(EFI_PCI_IO_PROTOCOL_WIDTH::FillUint64 as u8, 11);
    }

    #[test]
    fn test_pci_io_protocol_size() {
        assert_eq!(mem::size_of::<EFI_PCI_IO_PROTOCOL>(), 160);
    }

    #[test]
    fn test_pci_io_protocol_access_size() {
        assert_eq!(mem::size_of::<EFI_PCI_IO_PROTOCOL_ACCESS>(), 16);
    }

    #[test]
    fn test_pci_io_protocol_pci_size() {
        assert_eq!(mem::size_of::<EFI_PCI_IO_PROTOCOL_PCI>(), 16);
    }

    #[test]
    fn test_loaded_image_protocol_size() {
        assert_eq!(mem::size_of::<EFI_LOADED_IMAGE_PROTOCOL>(), 96);
    }

    #[test]
    fn test_simple_network_protocol_size() {
        assert_eq!(mem::size_of::<EFI_SIMPLE_NETWORK_PROTOCOL>(), 128);
    }

    #[test]
    fn test_open_protocol_constants() {
        assert_eq!(EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL, 0x00000001);
        assert_eq!(EFI_OPEN_PROTOCOL_GET_PROTOCOL, 0x00000002);
        assert_eq!(EFI_OPEN_PROTOCOL_BY_DRIVER, 0x00000004);
        assert_eq!(EFI_OPEN_PROTOCOL_EXCLUSIVE, 0x00000008);
    }

    #[test]
    fn test_efi_status_constants() {
        assert_eq!(EFI_NOT_STARTED, 19);
        assert_eq!(EFI_ALREADY_STARTED, 20);
    }

    #[test]
    fn test_boot_svc_offsets_consistent() {
        // These offsets must be unique and in increasing order
        let offs = [0x48usize, 0x118, 0x138];
        let mut sorted = offs;
        sorted.sort();
        assert_eq!(offs, sorted);
        for i in 0..offs.len() {
            for j in (i + 1)..offs.len() {
                assert_ne!(offs[i], offs[j], "offset {} duplicated", offs[i]);
            }
        }
    }

    #[test]
    fn test_pci_root_bridge_io_size() {
        assert_eq!(mem::size_of::<EFI_PCI_ROOT_BRIDGE_IO_PROTOCOL>(), 152);
    }
}
