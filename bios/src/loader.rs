//! BIOS executable loader (ELF32, Multiboot)

use common::loader::{FileFormat, detect_format};

/// ELF32 header
#[repr(C, packed)]
struct Elf32Header {
    e_ident: [u8; 16],
    e_type: u16,
    e_machine: u16,
    e_version: u32,
    e_entry: u32,
    e_phoff: u32,
    e_shoff: u32,
    e_flags: u32,
    e_ehsize: u16,
    e_phentsize: u16,
    e_phnum: u16,
    e_shentsize: u16,
    e_shnum: u16,
    e_shstrndx: u16,
}

/// ELF32 program header
#[repr(C, packed)]
struct Elf32Phdr {
    p_type: u32,
    p_offset: u32,
    p_vaddr: u32,
    p_paddr: u32,
    p_filesz: u32,
    p_memsz: u32,
    p_flags: u32,
    p_align: u32,
}

const PT_LOAD: u32 = 1;

/// Execute an ELF32 binary
pub fn execute_elf32(buffer: *mut u8, size: usize) {
    if size < core::mem::size_of::<Elf32Header>() {
        return;
    }
    
    let header = unsafe { &*(buffer as *const Elf32Header) };
    
    // Verify ELF magic
    if header.e_ident[0] != 0x7F || 
       header.e_ident[1] != b'E' || 
       header.e_ident[2] != b'L' || 
       header.e_ident[3] != b'F' {
        return;
    }
    
    // Verify 32-bit
    if header.e_ident[4] != 1 {
        return;
    }
    
    let entry = header.e_entry;
    let phoff = header.e_phoff as usize;
    let phnum = header.e_phnum as usize;
    let phentsize = header.e_phentsize as usize;
    
    // Load program segments
    for i in 0..phnum {
        let phdr_offset = phoff + i * phentsize;
        if phdr_offset + core::mem::size_of::<Elf32Phdr>() > size {
            break;
        }
        
        let phdr = unsafe { &*((buffer as usize + phdr_offset) as *const Elf32Phdr) };
        
        if phdr.p_type == PT_LOAD {
            let src = unsafe { buffer.add(phdr.p_offset as usize) };
            let dst = phdr.p_paddr as *mut u8;
            let copy_size = phdr.p_filesz as usize;
            
            if (phdr.p_offset as usize) + copy_size <= size {
                unsafe {
                    core::ptr::copy_nonoverlapping(src, dst, copy_size);
                }
                
                // Zero BSS if memsz > filesz
                if phdr.p_memsz > phdr.p_filesz {
                    let bss_start = unsafe { dst.add(copy_size) };
                    let bss_size = (phdr.p_memsz - phdr.p_filesz) as usize;
                    unsafe {
                        core::ptr::write_bytes(bss_start, 0, bss_size);
                    }
                }
            }
        }
    }
    
    // Jump to entry point
    unsafe {
        let entry_fn: extern "C" fn() = core::mem::transmute(entry as usize);
        entry_fn();
    }
}

/// Multiboot header
#[repr(C, packed)]
struct MultibootHeader {
    magic: u32,
    flags: u32,
    checksum: u32,
}

/// Multiboot info structure (minimal)
#[repr(C, packed)]
struct MultibootInfo {
    flags: u32,
    mem_lower: u32,
    mem_upper: u32,
}

const MULTIBOOT_MAGIC: u32 = 0x1BADB002;

/// Execute a Multiboot-compliant kernel
pub fn execute_multiboot(buffer: *mut u8, size: usize) {
    if size < core::mem::size_of::<MultibootHeader>() {
        return;
    }
    
    let header = unsafe { &*(buffer as *const MultibootHeader) };
    
    // Verify multiboot magic
    if header.magic != MULTIBOOT_MAGIC {
        return;
    }
    
    // Verify checksum
    if header.magic.wrapping_add(header.flags).wrapping_add(header.checksum) != 0 {
        return;
    }
    
    // Create minimal multiboot info structure
    let _mbi = MultibootInfo {
        flags: 1, // memory map valid
        mem_lower: 640, // 640KB conventional memory
        mem_upper: 0, // extended memory (would need to query)
    };
    
    // For a real implementation, we'd need to:
    // 1. Parse the multiboot header to find the entry point
    // 2. Set up a proper multiboot info structure with memory map
    // 3. Switch to a clean state and jump to the kernel
    
    // For now, just indicate we detected it
    // Actual execution would require more complex setup
    
    // The entry point is typically at the start of the file for simple kernels
    // but multiboot specifies it should be in the header
    // Since we don't have the full header parsing, we'll skip actual execution
}

/// Execute a file based on its detected format
pub fn execute_file(
    buffer: *mut u8,
    size: usize,
    puts: fn(&str),
) {
    let data = unsafe { core::slice::from_raw_parts(buffer, size) };
    let format = detect_format(data);
    
    match format {
        FileFormat::Elf32 => {
            puts("Executing ELF32 binary...\n");
            execute_elf32(buffer, size);
        }
        FileFormat::Multiboot => {
            puts("Executing Multiboot kernel...\n");
            execute_multiboot(buffer, size);
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
