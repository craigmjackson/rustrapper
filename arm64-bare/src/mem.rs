//! ARM64 bare-metal memory allocation for TFTP transfers

use common::tftp::TftpSink;

/// Memory sink for ARM64 bare-metal
/// Uses a fixed region of RAM for transfers
pub struct Arm64MemorySink {
    base_addr: u64,
    current_offset: usize,
    capacity: usize,
}

impl Arm64MemorySink {
    pub fn new(size_hint: usize) -> Self {
        // Use a fixed region of RAM above the kernel load address
        // QEMU virt machine has RAM starting at 0x40000000
        // We'll use 0x50000000 (1.25GB) as a safe location
        let base_addr = 0x50000000u64;
        let capacity = size_hint;
        
        Self {
            base_addr,
            current_offset: 0,
            capacity,
        }
    }
    
    pub fn buffer_addr(&self) -> u64 {
        self.base_addr
    }
}

impl TftpSink for Arm64MemorySink {
    fn write_block(&mut self, data: &[u8]) -> Result<(), ()> {
        let new_offset = self.current_offset + data.len();
        
        if new_offset > self.capacity {
            return Err(());
        }
        
        // Write to physical memory
        let addr = (self.base_addr as usize) + self.current_offset;
        unsafe {
            let dst = addr as *mut u8;
            core::ptr::copy_nonoverlapping(data.as_ptr(), dst, data.len());
        }
        
        self.current_offset = new_offset;
        Ok(())
    }
    
    fn finalize(&mut self, _size: usize) -> Result<(), ()> {
        Ok(())
    }
}
