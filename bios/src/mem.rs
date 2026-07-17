//! BIOS memory allocation for TFTP transfers (fixed address above 1MB)

use common::tftp::TftpSink;

/// Memory sink for BIOS that uses a fixed address above the bootloader.
pub struct BiosExtendedMemorySink {
    base_addr: u64,
    current_offset: usize,
    capacity: usize,
}

impl BiosExtendedMemorySink {
    /// Create a memory sink for TFTP downloads.
    ///
    /// We use a fixed address at 2 MB — safely above the bootloader payload
    /// (0x100000) and its BSS, and well within the first 128 MB that QEMU
    /// always provides.
    ///
    /// NOTE: We cannot call INT 15h E820 from 32-bit protected mode (no IDT),
    /// so we skip the BIOS memory map query and use a fixed base address.
    pub fn new(size_hint: usize) -> Self {
        Self {
            base_addr: 0x200000, // 2 MB — above bootloader code, safe on all configs
            current_offset: 0,
            capacity: size_hint,
        }
    }

    pub fn buffer_addr(&self) -> u64 {
        self.base_addr
    }
}

impl TftpSink for BiosExtendedMemorySink {
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
