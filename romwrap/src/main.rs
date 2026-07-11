use std::env;
use std::fs;
use std::process;

const ROM_HEADER_SIZE: usize = 0x1C;
const PCIR_SIZE: usize = 0x18;
const TOTAL_HEADERS: usize = ROM_HEADER_SIZE + PCIR_SIZE;

fn build_pci_rom(pe_data: &[u8], vendor_id: u16, device_id: u16) -> Vec<u8> {
    let pcir_offset: u32 = ROM_HEADER_SIZE as u32;

    let raw_total = TOTAL_HEADERS + pe_data.len();
    let total_blocks = ((raw_total + 511) / 512) as u16;
    let padded_len = (total_blocks as usize) * 512;

    let mut rom = Vec::with_capacity(padded_len);

    rom.extend_from_slice(&0xAA55u16.to_le_bytes());
    rom.extend_from_slice(&0x0000u16.to_le_bytes());
    rom.extend_from_slice(&[0u8; 20]);
    rom.extend_from_slice(&pcir_offset.to_le_bytes());

    rom.extend_from_slice(b"PCIR");
    rom.extend_from_slice(&vendor_id.to_le_bytes());
    rom.extend_from_slice(&device_id.to_le_bytes());
    rom.extend_from_slice(&0x0000u16.to_le_bytes());
    rom.extend_from_slice(&(PCIR_SIZE as u16).to_le_bytes());
    rom.push(0x00);
    rom.push(0x03);
    rom.push(0x80);
    rom.push(0x00);
    rom.extend_from_slice(&total_blocks.to_le_bytes());
    rom.extend_from_slice(&total_blocks.to_le_bytes());
    rom.extend_from_slice(&[0u8; 4]);

    rom.extend_from_slice(pe_data);

    rom.resize(padded_len, 0);
    rom
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        eprintln!("Usage: {} <input.efi> <output.rom> [--vendor=VENDOR] [--device=DEVICE]", args[0]);
        process::exit(1);
    }

    let input_path = &args[1];
    let output_path = &args[2];

    let mut vendor_id: u16 = 0x8086;
    let mut device_id: u16 = 0x100E;

    for arg in &args[3..] {
        if let Some(v) = arg.strip_prefix("--vendor=") {
            vendor_id = u16::from_str_radix(v.trim_start_matches("0x"), 16)
                .expect("Invalid vendor ID");
        } else if let Some(d) = arg.strip_prefix("--device=") {
            device_id = u16::from_str_radix(d.trim_start_matches("0x"), 16)
                .expect("Invalid device ID");
        }
    }

    let pe_data = fs::read(input_path).expect("Failed to read input EFI file");

    if pe_data.len() < 2 || &pe_data[0..2] != b"MZ" {
        eprintln!("Error: Input file does not start with MZ (not a valid PE/COFF)");
        process::exit(1);
    }

    let rom = build_pci_rom(&pe_data, vendor_id, device_id);

    fs::write(output_path, &rom).expect("Failed to write output ROM file");

    println!("Wrote {} bytes to {}", rom.len(), output_path);
    println!("  Vendor: 0x{:04X}, Device: 0x{:04X}", vendor_id, device_id);
    println!("  PE/COFF size: {} bytes", pe_data.len());
    println!("  ROM size: {} bytes ({} blocks)", rom.len(), rom.len() / 512);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rom_starts_with_aa55() {
        let pe = &[b'M', b'Z', 0, 0];
        let rom = build_pci_rom(pe, 0x8086, 0x100E);
        assert_eq!(rom[0], 0x55);
        assert_eq!(rom[1], 0xAA);
    }

    #[test]
    fn test_pcir_signature() {
        let pe = &[b'M', b'Z', 0, 0];
        let rom = build_pci_rom(pe, 0x8086, 0x100E);
        let pcir_off = u32::from_le_bytes(rom[0x18..0x1C].try_into().unwrap()) as usize;
        assert_eq!(&rom[pcir_off..pcir_off + 4], b"PCIR");
    }

    #[test]
    fn test_code_type_efi() {
        let pe = &[b'M', b'Z', 0, 0];
        let rom = build_pci_rom(pe, 0x8086, 0x100E);
        let pcir_off = u32::from_le_bytes(rom[0x18..0x1C].try_into().unwrap()) as usize;
        assert_eq!(rom[pcir_off + 0x0D], 0x03);
    }

    #[test]
    fn test_last_image_indicator() {
        let pe = &[b'M', b'Z', 0, 0];
        let rom = build_pci_rom(pe, 0x8086, 0x100E);
        let pcir_off = u32::from_le_bytes(rom[0x18..0x1C].try_into().unwrap()) as usize;
        assert_eq!(rom[pcir_off + 0x0E], 0x80);
    }

    #[test]
    fn test_init_vector_zero() {
        let pe = &[b'M', b'Z', 0, 0];
        let rom = build_pci_rom(pe, 0x8086, 0x100E);
        let init = u16::from_le_bytes(rom[0x02..0x04].try_into().unwrap());
        assert_eq!(init, 0x0000);
    }

    #[test]
    fn test_size_512_aligned() {
        let pe = &[b'M', b'Z', 0, 0];
        let rom = build_pci_rom(pe, 0x8086, 0x100E);
        assert_eq!(rom.len() % 512, 0);
    }

    #[test]
    fn test_pe_data_preserved() {
        let pe = &[b'M', b'Z', 0x90, 0x00, 0xAB, 0xCD];
        let rom = build_pci_rom(pe, 0x8086, 0x100E);
        let pcir_off = u32::from_le_bytes(rom[0x18..0x1C].try_into().unwrap()) as usize;
        let pcir_len = rom[pcir_off + 0x0A] as usize | (rom[pcir_off + 0x0B] as usize) << 8;
        let code_start = pcir_off + pcir_len;
        assert_eq!(&rom[code_start..code_start + pe.len()], pe);
    }

    #[test]
    fn test_vendor_device_ids() {
        let pe = &[b'M', b'Z', 0, 0];
        let rom = build_pci_rom(pe, 0x10EC, 0x8139);
        let pcir_off = u32::from_le_bytes(rom[0x18..0x1C].try_into().unwrap()) as usize;
        let vendor = u16::from_le_bytes(rom[pcir_off + 0x04..pcir_off + 0x06].try_into().unwrap());
        let device = u16::from_le_bytes(rom[pcir_off + 0x06..pcir_off + 0x08].try_into().unwrap());
        assert_eq!(vendor, 0x10EC);
        assert_eq!(device, 0x8139);
    }

    #[test]
    fn test_large_pe_rounds_up() {
        let pe = vec![0u8; 5000];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E);
        assert_eq!(rom.len() % 512, 0);
        let expected_blocks = ((TOTAL_HEADERS + 5000 + 511) / 512) as usize;
        assert_eq!(rom.len() / 512, expected_blocks);
    }

    #[test]
    fn test_size_blocks_match_header() {
        let pe = vec![0xABu8; 10000];
        let rom = build_pci_rom(&pe, 0x8086, 0x100E);
        let pcir_off = u32::from_le_bytes(rom[0x18..0x1C].try_into().unwrap()) as usize;
        let runtime_blocks = u16::from_le_bytes(rom[pcir_off + 0x10..pcir_off + 0x12].try_into().unwrap());
        assert_eq!(runtime_blocks as usize, rom.len() / 512);
    }
}
