//! BIOS memory: the E820 map captured by the real-mode entry stub, plus TFTP
//! download buffer placement.
//!
//! The entry stub (`stage2_entry.nasm`) calls INT 15h/AX=E820 while still in
//! real mode — it cannot be called from the protected-mode stage2, which runs
//! with interrupts disabled and no IDT — and stores the map at a fixed low
//! address. `_start` records the address/count via [`init`]. Downloads are
//! then placed at the start of the largest usable RAM region above the
//! bootloader (type 1, below 4 GB), instead of a hardcoded address.

use common::tftp::TftpSink;

/// Download buffer size used for the TFTP transfer plan.
pub const TFTP_SIZE_HINT: usize = 16 * 1024 * 1024;

/// A 24-byte BIOS E820 entry (base, length, type, ACPI extended attributes).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct E820Entry {
    pub base: u64,
    pub length: u64,
    pub entry_type: u32,
    pub acpi_ext: u32,
}

/// Layout of the map captured by the entry stub.
pub mod stub {
    /// Physical address of the captured map.
    pub const E820_ADDR: usize = 0x500;
    /// Maximum number of entries the stub captures.
    pub const E820_MAX: usize = 64;
}

static mut E820_ADDR: usize = 0;
static mut E820_COUNT: usize = 0;

#[cfg(not(test))]
extern "C" {
    static __bss_end: u8;
}

/// Record the E820 map location passed to `_start` by the entry stub. The
/// address is validated against the stub's fixed location so a garbage value
/// can never be dereferenced.
pub fn init(addr: u32, count: u32) {
    let addr = if addr as usize == stub::E820_ADDR {
        addr as usize
    } else {
        0
    };
    let count = (count as usize).min(stub::E820_MAX);
    unsafe {
        E820_ADDR = addr;
        E820_COUNT = count;
    }
}

/// The captured memory map (empty if the BIOS did not provide E820).
pub fn e820_map() -> &'static [E820Entry] {
    let addr = unsafe { E820_ADDR };
    let count = unsafe { E820_COUNT };
    if addr == 0 || count == 0 {
        return &[];
    }
    unsafe { core::slice::from_raw_parts(addr as *const E820Entry, count) }
}

/// First address the bootloader does not use (linker `__bss_end`), rounded up
/// to a page.
fn bootloader_end() -> u64 {
    #[cfg(not(test))]
    {
        let end = core::ptr::addr_of!(__bss_end) as usize as u64;
        (end + 0xFFF) & !0xFFF
    }
    #[cfg(test)]
    {
        0x11B000
    }
}

/// Choose a `want`-byte buffer at or above `min_addr` from an E820 map.
///
/// Only type-1 (usable) RAM below 4 GB is considered: the stage2 runs in
/// 32-bit protected mode with paging off, so addresses at or above 4 GB would
/// truncate. Returns `(base, available)` for the largest qualifying region.
pub fn choose_region(entries: &[E820Entry], min_addr: u64, want: u64) -> Option<(u64, u64)> {
    const FOUR_GB: u64 = 0x1_0000_0000;
    let mut best: Option<(u64, u64)> = None;
    for e in entries {
        if e.entry_type != 1 {
            continue;
        }
        let end = e.base.saturating_add(e.length).min(FOUR_GB);
        let start = e.base.max(min_addr);
        if start >= end {
            continue;
        }
        let len = end - start;
        if len < want {
            continue;
        }
        if best.map_or(true, |(_, best_len)| len > best_len) {
            best = Some((start, len));
        }
    }
    best
}

/// Placement for a `size_hint`-byte download: `(base, capacity)`.
fn allocate(size_hint: usize) -> (u64, usize) {
    let start = bootloader_end().max(0x10_0000);
    let want = size_hint as u64;
    match choose_region(e820_map(), start, want) {
        Some((base, len)) => (base, want.min(len) as usize),
        // No usable map (capture failed or an E820-less BIOS): keep the
        // historical fixed 2 MB buffer.
        None => (start.max(0x20_0000), size_hint),
    }
}

/// Print the planned TFTP download buffer (base and capacity).
pub fn print_summary() {
    use common::print::{print_dec, print_hex, puts};
    let (base, cap) = allocate(TFTP_SIZE_HINT);
    puts("TFTP buffer: ");
    print_hex(base, 8);
    puts(" (");
    print_dec(cap as u64 / (1024 * 1024));
    puts(" MB)\n");
}

/// Memory sink for BIOS TFTP downloads, placed in an E820-usable region.
pub struct BiosExtendedMemorySink {
    base_addr: u64,
    current_offset: usize,
    capacity: usize,
}

impl BiosExtendedMemorySink {
    /// Create a memory sink for a download of up to `size_hint` bytes.
    pub fn new(size_hint: usize) -> Self {
        let (base_addr, capacity) = allocate(size_hint);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn ent(base: u64, length: u64, entry_type: u32) -> E820Entry {
        E820Entry {
            base,
            length,
            entry_type,
            acpi_ext: 0,
        }
    }

    #[test]
    fn picks_region_above_min_addr() {
        let map = [
            ent(0, 0x9FC00, 1),          // low memory: below min_addr
            ent(0x100000, 0x7F00000, 1), // 1 MB .. 128 MB usable
            ent(0x8000000, 0x1000000, 2),// reserved
        ];
        let (base, len) = choose_region(&map, 0x20_0000, 16 * 1024 * 1024).unwrap();
        assert_eq!(base, 0x20_0000);
        assert_eq!(len, 0x8000000 - 0x20_0000);
    }

    #[test]
    fn uses_region_start_when_above_min() {
        let map = [ent(0x40_0000, 0x100_0000, 1)];
        let (base, len) = choose_region(&map, 0x20_0000, 0x1000).unwrap();
        assert_eq!(base, 0x40_0000);
        assert_eq!(len, 0x100_0000);
    }

    #[test]
    fn ignores_non_usable_and_small_regions() {
        let map = [
            ent(0x100000, 0x100000, 2), // reserved
            ent(0x200000, 0x1000, 1),   // too small
        ];
        assert_eq!(choose_region(&map, 0x20_0000, 0x10000), None);
    }

    #[test]
    fn picks_the_largest_qualifying_region() {
        let map = [
            ent(0x20_0000, 0x20_0000, 1), // 2 MB
            ent(0x10_00000, 0x40_00000, 1), // 64 MB
            ent(0x100_00000, 0x10_00000, 1), // 16 MB
        ];
        let (base, len) = choose_region(&map, 0x20_0000, 16 * 1024 * 1024).unwrap();
        assert_eq!(base, 0x10_00000);
        assert_eq!(len, 0x40_00000);
    }

    #[test]
    fn ignores_memory_at_or_above_4gb() {
        let map = [ent(0x1_0000_0000, 0x1_0000_0000, 1)];
        assert_eq!(choose_region(&map, 0x20_0000, 0x1000), None);
    }

    #[test]
    fn clamps_region_at_4gb_boundary() {
        let map = [ent(0xFFFF_F000u64, 0x2000, 1)];
        let (base, len) = choose_region(&map, 0x20_0000, 0x1000).unwrap();
        assert_eq!(base, 0xFFFF_F000u64);
        assert_eq!(len, 0x1000);
    }

    #[test]
    fn empty_map_has_no_choices() {
        assert_eq!(choose_region(&[], 0x20_0000, 1), None);
    }
}
