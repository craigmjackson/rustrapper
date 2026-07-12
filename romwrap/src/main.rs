use std::env;
use std::fs;
use std::process;

const ROM_HEADER_SIZE: usize = 0x1C;
const PCIR_SIZE: usize = 0x18;
const TOTAL_HEADERS: usize = ROM_HEADER_SIZE + PCIR_SIZE;
const BIOS_ENTRY_SIZE: usize = 3;

fn build_pci_rom(pe_data: &[u8], vendor_id: u16, device_id: u16, is_bios: bool) -> Vec<u8> {
    let pcir_offset: u32 = ROM_HEADER_SIZE as u32;
    let payload_offset = TOTAL_HEADERS + if is_bios { BIOS_ENTRY_SIZE } else { 0 };
    let init_vector: u16 = if is_bios { TOTAL_HEADERS as u16 } else { 0x0000 };

    let raw_total = payload_offset + pe_data.len();
    let total_blocks = ((raw_total + 511) / 512) as u16;
    let padded_len = (total_blocks as usize) * 512;

    let mut rom = Vec::with_capacity(padded_len);

    // ROM header (28 bytes)
    rom.extend_from_slice(&0xAA55u16.to_le_bytes());  // [0-1]: signature
    rom.extend_from_slice(&init_vector.to_le_bytes()); // [2-3]: init vector
    rom.extend_from_slice(&total_blocks.to_le_bytes()); // [4-5]: size in 512-byte blocks
    rom.extend_from_slice(&[0u8; 18]);                 // [6-23]: reserved
    rom.extend_from_slice(&pcir_offset.to_le_bytes()); // [24-27]: PCIR offset

    // PCIR structure (24 bytes)
    rom.extend_from_slice(b"PCIR");
    rom.extend_from_slice(&vendor_id.to_le_bytes());
    rom.extend_from_slice(&device_id.to_le_bytes());
    rom.extend_from_slice(&0x0000u16.to_le_bytes());
    rom.extend_from_slice(&(PCIR_SIZE as u16).to_le_bytes());
    rom.push(0x00);  // revision
    rom.push(if is_bios { 0x00 } else { 0x03 });  // code type: 0x00=PC-AT, 0x03=EFI
    rom.push(0x80);  // indicator (last image)
    rom.push(0x00);  // reserved
    rom.extend_from_slice(&total_blocks.to_le_bytes());  // runtime image length (blocks)
    rom.extend_from_slice(&total_blocks.to_le_bytes());  // storage image length (blocks)
    rom.extend_from_slice(&[0u8; 4]);

    if is_bios {
        // Minimal real-mode entry: xor ax,ax; retf
        rom.push(0x33);
        rom.push(0xC0);
        rom.push(0xCB);
    }

    rom.extend_from_slice(pe_data);

    rom.resize(padded_len, 0);
    rom
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <input.bin> <output.rom> [--bios] [--vendor=VENDOR] [--device=DEVICE]", args[0]);
        process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    let mut vendor_id: u16 = 0x8086;
    let mut device_id: u16 = 0x100E;
    let mut is_bios = false;

    for arg in &args[3..] {
        if *arg == "--bios" {
            is_bios = true;
        } else if let Some(v) = arg.strip_prefix("--vendor=") {
            vendor_id = u16::from_str_radix(v.trim_start_matches("0x"), 16)
                .expect("Invalid vendor ID");
        } else if let Some(d) = arg.strip_prefix("--device=") {
            device_id = u16::from_str_radix(d.trim_start_matches("0x"), 16)
                .expect("Invalid device ID");
        }
    }

    let input_data = fs::read(input_path).expect("Failed to read input file");

    if !is_bios {
        if input_data.len() < 2 || &input_data[0..2] != b"MZ" {
            eprintln!("Error: Input file does not start with MZ (not a valid PE/COFF). Use --bios for non-EFI binaries.");
            process::exit(1);
        }
    }

    let rom = build_pci_rom(&input_data, vendor_id, device_id, is_bios);

    fs::write(output_path, &rom).expect("Failed to write output ROM file");

    println!("Wrote {} bytes to {}", rom.len(), output_path);
    println!("  Vendor: 0x{:04X}, Device: 0x{:04X}", vendor_id, device_id);
    println!("  Type: {}", if is_bios { "BIOS (PC-AT, x86 real-mode)" } else { "UEFI" });
    println!("  Payload: {} bytes", input_data.len());
    println!("  ROM size: {} bytes ({} blocks)", rom.len(), rom.len() / 512);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pe() -> Vec<u8> {
        vec![b'M', b'Z', 0x90, 0x00, 0xAB, 0xCD]
    }

    fn get_pcir_off(rom: &[u8]) -> usize {
        u32::from_le_bytes(rom[0x18..0x1C].try_into().unwrap()) as usize
    }

    // ── Common tests (both UEFI and BIOS) ──

    #[test]
    fn test_rom_starts_with_aa55() {
        let rom = build_pci_rom(&make_pe(), 0x8086, 0x100E, false);
        assert_eq!(rom[0], 0x55);
        assert_eq!(rom[1], 0xAA);
    }

    #[test]
    fn test_pcir_signature() {
        let rom = build_pci_rom(&make_pe(), 0x8086, 0x100E, false);
        let pcir_off = get_pcir_off(&rom);
        assert_eq!(&rom[pcir_off..pcir_off + 4], b"PCIR");
    }

    #[test]
    fn test_last_image_indicator() {
        let rom = build_pci_rom(&make_pe(), 0x8086, 0x100E, false);
        let pcir_off = get_pcir_off(&rom);
        assert_eq!(rom[pcir_off + 0x0E], 0x80);
    }

    #[test]
    fn test_size_512_aligned() {
        let rom = build_pci_rom(&make_pe(), 0x8086, 0x100E, false);
        assert_eq!(rom.len() % 512, 0);
    }

    #[test]
    fn test_vendor_device_ids() {
        let rom = build_pci_rom(&make_pe(), 0x10EC, 0x8139, false);
        let pcir_off = get_pcir_off(&rom);
        let vendor = u16::from_le_bytes(rom[pcir_off + 0x04..pcir_off + 0x06].try_into().unwrap());
        let device = u16::from_le_bytes(rom[pcir_off + 0x06..pcir_off + 0x08].try_into().unwrap());
        assert_eq!(vendor, 0x10EC);
        assert_eq!(device, 0x8139);
    }

    #[test]
    fn test_large_pe_rounds_up() {
        let pe = vec![0u8; 5000];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        assert_eq!(rom.len() % 512, 0);
        let expected_blocks = ((TOTAL_HEADERS + 5000 + 511) / 512) as usize;
        assert_eq!(rom.len() / 512, expected_blocks);
    }

    #[test]
    fn test_size_blocks_match_header() {
        let pe = vec![0xABu8; 10000];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        let pcir_off = get_pcir_off(&rom);
        let runtime_blocks = u16::from_le_bytes(rom[pcir_off + 0x10..pcir_off + 0x12].try_into().unwrap());
        assert_eq!(runtime_blocks as usize, rom.len() / 512);
        let size_at_4 = u16::from_le_bytes(rom[0x04..0x06].try_into().unwrap());
        assert_eq!(size_at_4, runtime_blocks);
    }

    // ── UEFI-specific tests ──

    #[test]
    fn test_code_type_efi() {
        let rom = build_pci_rom(&make_pe(), 0x8086, 0x100E, false);
        let pcir_off = get_pcir_off(&rom);
        assert_eq!(rom[pcir_off + 0x0D], 0x03);
    }

    #[test]
    fn test_init_vector_zero_uefi() {
        let rom = build_pci_rom(&make_pe(), 0x8086, 0x100E, false);
        let init = u16::from_le_bytes(rom[0x02..0x04].try_into().unwrap());
        assert_eq!(init, 0x0000);
    }

    #[test]
    fn test_pe_data_preserved() {
        let pe = vec![b'M', b'Z', 0x90, 0x00, 0xAB, 0xCD];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        let pcir_off = get_pcir_off(&rom);
        let pcir_len = rom[pcir_off + 0x0A] as usize | (rom[pcir_off + 0x0B] as usize) << 8;
        let code_start = pcir_off + pcir_len;
        assert_eq!(&rom[code_start..code_start + pe.len()], pe);
    }

    // ── BIOS-specific tests ──

    #[test]
    fn test_code_type_bios() {
        let pe = vec![0x00u8; 100];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        let pcir_off = get_pcir_off(&rom);
        assert_eq!(rom[pcir_off + 0x0D], 0x00);
    }

    #[test]
    fn test_init_vector_nonzero_bios() {
        let pe = vec![0x00u8; 100];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        let init = u16::from_le_bytes(rom[0x02..0x04].try_into().unwrap());
        assert!(init != 0);
        assert_eq!(init as usize, TOTAL_HEADERS);
    }

    #[test]
    fn test_entry_routine_bios() {
        let pe = vec![0x00u8; 100];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        // Entry routine at offset TOTAL_HEADERS
        assert_eq!(rom[TOTAL_HEADERS], 0x33);     // xor ax, ax
        assert_eq!(rom[TOTAL_HEADERS + 1], 0xC0);
        assert_eq!(rom[TOTAL_HEADERS + 2], 0xCB); // retf
    }

    #[test]
    fn test_payload_after_entry_bios() {
        let pe = vec![0xDE, 0xAD, 0xBE, 0xEF];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        let payload_start = TOTAL_HEADERS + BIOS_ENTRY_SIZE;
        assert_eq!(&rom[payload_start..payload_start + 4], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn test_size_blocks_bios() {
        let pe = vec![0x00u8; 100];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        let pcir_off = get_pcir_off(&rom);
        let size_at_4 = u16::from_le_bytes(rom[0x04..0x06].try_into().unwrap());
        let pcir_blocks = u16::from_le_bytes(rom[pcir_off + 0x10..pcir_off + 0x12].try_into().unwrap());
        assert_eq!(size_at_4, pcir_blocks);
        assert_eq!(size_at_4 as usize, rom.len() / 512);
    }

    #[test]
    fn test_bios_rom_512_aligned() {
        let pe = vec![0x00u8; 2000];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        assert_eq!(rom.len() % 512, 0);
    }

    // ── Edge cases ──

    #[test]
    fn test_empty_payload_uefi() {
        let pe: Vec<u8> = vec![];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        assert_eq!(rom.len() % 512, 0);
        // Should have at least 1 block (headers are <512)
        assert!(rom.len() >= 512);
        // PCIR code type should be EFI
        let pcir_off = get_pcir_off(&rom);
        assert_eq!(rom[pcir_off + 0x0D], 0x03);
    }

    #[test]
    fn test_empty_payload_bios() {
        let pe: Vec<u8> = vec![];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        assert_eq!(rom.len() % 512, 0);
        // Should have at least 1 block
        assert!(rom.len() >= 512);
        let pcir_off = get_pcir_off(&rom);
        assert_eq!(rom[pcir_off + 0x0D], 0x00);
        // Entry routine should be present even with empty payload
        assert_eq!(rom[TOTAL_HEADERS], 0x33);
    }

    #[test]
    fn test_payload_exact_block_boundary() {
        // Payload that fills exactly to 512 boundary (minus headers)
        let header_size = TOTAL_HEADERS;
        let payload_size = 512 - header_size;
        let pe = vec![0xABu8; payload_size];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        // Should fit exactly in one block
        assert_eq!(rom.len(), 512);
        assert_eq!(rom.len() % 512, 0);
    }

    #[test]
    fn test_payload_one_over_block_boundary() {
        // Payload one byte larger than a single block
        let header_size = TOTAL_HEADERS;
        let payload_size = 512 - header_size + 1;
        let pe = vec![0xCDu8; payload_size];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        // Should need two blocks
        assert_eq!(rom.len(), 1024);
        let blocks = u16::from_le_bytes(rom[0x04..0x06].try_into().unwrap());
        assert_eq!(blocks, 2);
    }

    #[test]
    fn test_pcir_length_fields_match_uefi() {
        let pe = vec![0x00u8; 3000];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        let pcir_off = get_pcir_off(&rom);
        let runtime_blocks = u16::from_le_bytes(rom[pcir_off + 0x10..pcir_off + 0x12].try_into().unwrap());
        let storage_blocks = u16::from_le_bytes(rom[pcir_off + 0x12..pcir_off + 0x14].try_into().unwrap());
        assert_eq!(runtime_blocks, storage_blocks);
        assert_eq!(runtime_blocks as usize, rom.len() / 512);
    }

    #[test]
    fn test_pcir_length_fields_match_bios() {
        let pe = vec![0x00u8; 3000];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        let pcir_off = get_pcir_off(&rom);
        let runtime_blocks = u16::from_le_bytes(rom[pcir_off + 0x10..pcir_off + 0x12].try_into().unwrap());
        let storage_blocks = u16::from_le_bytes(rom[pcir_off + 0x12..pcir_off + 0x14].try_into().unwrap());
        assert_eq!(runtime_blocks, storage_blocks);
        assert_eq!(runtime_blocks as usize, rom.len() / 512);
    }

    #[test]
    fn test_pcir_header_size_correct() {
        let pe = make_pe();
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, false);
        let pcir_off = get_pcir_off(&rom);
        let pcir_len = rom[pcir_off + 0x0A] as usize | (rom[pcir_off + 0x0B] as usize) << 8;
        assert_eq!(pcir_len, PCIR_SIZE);
    }

    #[test]
    fn test_bios_payload_after_entry() {
        let pe = vec![0xFFu8; 100];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E, true);
        let payload_start = TOTAL_HEADERS + BIOS_ENTRY_SIZE;
        assert_eq!(&rom[payload_start..payload_start + 4], &[0xFF; 4]);
    }
}
