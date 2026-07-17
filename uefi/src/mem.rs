//! UEFI memory allocation for TFTP transfers

use core::ffi::c_void;
use crate::efi::*;
use common::tftp::TftpSink;

// Boot Services function offsets (UEFI Spec 2.10)
const BOOT_SVC_ALLOCATE_POOL: usize = 0x30;
const BOOT_SVC_FREE_POOL: usize = 0x38;

// Memory types
const EFI_LOADER_DATA: u32 = 2;

type AllocatePoolFn = unsafe extern "efiapi" fn(
    pool_type: u32,
    size: UINTN,
    buffer: *mut *mut c_void,
) -> EFI_STATUS;

type FreePoolFn = unsafe extern "efiapi" fn(
    buffer: *mut c_void,
) -> EFI_STATUS;

fn read_boot_svc_fn<T>(gbs: *const c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

/// Memory sink for UEFI that uses AllocatePool for dynamic allocation
pub struct UefiMemorySink {
    buffer: *mut u8,
    capacity: usize,
    current_offset: usize,
    system_table: *const EFI_SYSTEM_TABLE,
}

impl UefiMemorySink {
    pub fn new(system_table: &EFI_SYSTEM_TABLE, size_hint: usize) -> Self {
        let gbs = system_table.boot_services;
        let allocate_pool: AllocatePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_ALLOCATE_POOL);
        
        let mut buffer: *mut c_void = core::ptr::null_mut();
        let status = unsafe {
            allocate_pool(EFI_LOADER_DATA, size_hint as UINTN, &mut buffer)
        };
        
        if status != EFI_SUCCESS {
            Self {
                buffer: core::ptr::null_mut(),
                capacity: 0,
                current_offset: 0,
                system_table: system_table as *const _,
            }
        } else {
            Self {
                buffer: buffer as *mut u8,
                capacity: size_hint,
                current_offset: 0,
                system_table: system_table as *const _,
            }
        }
    }
    
    pub fn buffer(&self) -> *mut u8 {
        self.buffer
    }
}

impl Drop for UefiMemorySink {
    fn drop(&mut self) {
        if !self.buffer.is_null() {
            let gbs = unsafe { (*self.system_table).boot_services };
            let free_pool: FreePoolFn = read_boot_svc_fn(gbs, BOOT_SVC_FREE_POOL);
            unsafe {
                free_pool(self.buffer as *mut c_void);
            }
        }
    }
}

impl TftpSink for UefiMemorySink {
    fn write_block(&mut self, data: &[u8]) -> Result<(), ()> {
        if self.buffer.is_null() {
            return Err(());
        }
        
        let new_offset = self.current_offset + data.len();
        
        // Check if we need to grow the buffer
        if new_offset > self.capacity {
            // For now, just fail if we exceed capacity
            // In a more sophisticated implementation, we could reallocate
            return Err(());
        }
        
        // Copy data to buffer
        unsafe {
            let dst = self.buffer.add(self.current_offset);
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        
        self.current_offset = new_offset;
        Ok(())
    }
    
    fn finalize(&mut self, _size: usize) -> Result<(), ()> {
        Ok(())
    }
}
