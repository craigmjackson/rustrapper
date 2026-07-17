//! File format detection for various executable and data formats.
//!
//! Supports: PE/COFF, ELF32, ELF64, Multiboot, Multiboot2, text, and binary.

/// Detected file format
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    /// PE/COFF executable (Windows/UEFI)
    PeCoff,
    /// ELF32 executable (32-bit Linux/Unix)
    Elf32,
    /// ELF64 executable (64-bit Linux/Unix)
    Elf64,
    /// Multiboot1 compliant kernel
    Multiboot,
    /// Multiboot2 compliant kernel
    Multiboot2,
    /// Plain text file (printable ASCII)
    Text,
    /// Unknown binary data
    Binary,
}

/// Detect the format of a file from its contents
pub fn detect_format(data: &[u8]) -> FileFormat {
    if data.len() < 4 {
        return FileFormat::Binary;
    }
    
    // Check PE/COFF: "MZ" magic
    if data[0] == 0x4D && data[1] == 0x5A {
        return FileFormat::PeCoff;
    }
    
    // Check ELF: 0x7F 'E' 'L' 'F'
    if data[0] == 0x7F && data[1] == 0x45 && data[2] == 0x4C && data[3] == 0x46 {
        if data.len() >= 5 {
            // EI_CLASS at offset 4: 1=32-bit, 2=64-bit
            match data[4] {
                1 => return FileFormat::Elf32,
                2 => return FileFormat::Elf64,
                _ => {}
            }
        }
    }
    
    // Check Multiboot: 0x1BADB002 at offset 0
    if data.len() >= 4 {
        let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        if magic == 0x1BADB002 {
            return FileFormat::Multiboot;
        }
        if magic == 0xE85250D6 {
            return FileFormat::Multiboot2;
        }
    }
    
    // Check if text (printable ASCII in first 1KB)
    let check_len = data.len().min(1024);
    let is_text = data[..check_len].iter().all(|&b| {
        b == 0 || // null bytes allowed
        (b >= 0x20 && b <= 0x7E) || // printable ASCII
        b == 0x09 || // tab
        b == 0x0A || // newline
        b == 0x0D    // carriage return
    });
    
    if is_text && check_len > 0 {
        return FileFormat::Text;
    }
    
    FileFormat::Binary
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_detect_pe_coff() {
        let data = [0x4D, 0x5A, 0x90, 0x00]; // MZ...
        assert_eq!(detect_format(&data), FileFormat::PeCoff);
    }
    
    #[test]
    fn test_detect_elf32() {
        let data = [0x7F, 0x45, 0x4C, 0x46, 1]; // ELF + class 1
        assert_eq!(detect_format(&data), FileFormat::Elf32);
    }
    
    #[test]
    fn test_detect_elf64() {
        let data = [0x7F, 0x45, 0x4C, 0x46, 2]; // ELF + class 2
        assert_eq!(detect_format(&data), FileFormat::Elf64);
    }
    
    #[test]
    fn test_detect_multiboot() {
        let data = 0x1BADB002u32.to_le_bytes();
        assert_eq!(detect_format(&data), FileFormat::Multiboot);
    }
    
    #[test]
    fn test_detect_multiboot2() {
        let data = 0xE85250D6u32.to_le_bytes();
        assert_eq!(detect_format(&data), FileFormat::Multiboot2);
    }
    
    #[test]
    fn test_detect_text() {
        let data = b"Hello, world!\nThis is a test.\n";
        assert_eq!(detect_format(data), FileFormat::Text);
    }
    
    #[test]
    fn test_detect_text_with_nulls() {
        let data = b"Hello\x00World\n";
        assert_eq!(detect_format(data), FileFormat::Text);
    }
    
    #[test]
    fn test_detect_binary() {
        let data = [0x00, 0x01, 0x02, 0x80, 0xFF];
        assert_eq!(detect_format(&data), FileFormat::Binary);
    }
    
    #[test]
    fn test_detect_too_short() {
        let data = [0x4D, 0x5A];
        assert_eq!(detect_format(&data), FileFormat::Binary);
    }
    
    #[test]
    fn test_detect_empty() {
        let data = [];
        assert_eq!(detect_format(&data), FileFormat::Binary);
    }
}
