use std::fs;
use std::io::{Read, Write};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const PARTITION_LBA: u64 = 8;
const DISK_SIZE_MB: u64 = 64;
const DISK_SIZE: u64 = DISK_SIZE_MB * 1024 * 1024;

struct Chs {
    c: u64,
    h: u64,
    s: u64,
}

fn chs_from_lba(lba: u64, heads: u64, sectors: u64) -> Chs {
    let c = lba / (heads * sectors);
    let h = (lba / sectors) % heads;
    let s = (lba % sectors) + 1;
    Chs { c, h, s }
}

fn build_mbr_partition(
    bootable: bool,
    part_type: u8,
    start_lba: u64,
    total_sectors: u64,
) -> [u8; 16] {
    let heads = 16u64;
    let sectors = 63u64;
    let chs_max = (1023u64, 254u64, 63u64);

    let boot_flag: u8 = if bootable { 0x80 } else { 0x00 };

    let mut cs = chs_from_lba(start_lba, heads, sectors);
    let end_lba = start_lba + total_sectors - 1;
    let mut ce = chs_from_lba(end_lba, heads, sectors);

    if cs.c > chs_max.0 {
        cs.c = chs_max.0;
        cs.h = chs_max.1;
        cs.s = chs_max.2;
    }
    if ce.c > chs_max.0 {
        ce.c = chs_max.0;
        ce.h = chs_max.1;
        ce.s = chs_max.2;
    }

    let mut buf = [0u8; 16];
    buf[0] = boot_flag;
    buf[1] = cs.h as u8;
    buf[2] = (cs.s as u8) | (((cs.c >> 2) as u8) & 0xC0);
    buf[3] = cs.c as u8;
    buf[4] = part_type;
    buf[5] = ce.h as u8;
    buf[6] = (ce.s as u8) | (((ce.c >> 2) as u8) & 0xC0);
    buf[7] = ce.c as u8;
    buf[8..12].copy_from_slice(&(start_lba as u32).to_le_bytes());
    buf[12..16].copy_from_slice(&(total_sectors as u32).to_le_bytes());
    buf
}

fn build_fat_partition(pe_data: &[u8], size_bytes: u64) -> Vec<u8> {
    let tmp_dir = std::env::temp_dir();
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fat_path = tmp_dir.join(format!("fat_{}.img", timestamp));
    let efi_path = tmp_dir.join(format!("efi_{}.efi", timestamp));

    let fat_path_str = fat_path.to_str().unwrap();
    let efi_path_str = efi_path.to_str().unwrap();

    {
        let mut f = fs::File::create(&fat_path).unwrap();
        f.write_all(&vec![0u8; size_bytes as usize]).unwrap();
    }

    let result = Command::new("mkfs.fat")
        .args(&["-F32", fat_path_str])
        .output()
        .expect("mkfs.fat not found");
    if !result.status.success() {
        eprintln!(
            "mkfs.fat error: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::process::exit(1);
    }

    for dir in &["EFI", "EFI/BOOT"] {
        let result = Command::new("mmd")
            .args(&["-i", fat_path_str, dir])
            .output()
            .expect("mmd not found");
        if !result.status.success() {
            eprintln!("mmd error for {}: {}", dir, String::from_utf8_lossy(&result.stderr));
            std::process::exit(1);
        }
    }

    {
        let mut f = fs::File::create(&efi_path).unwrap();
        f.write_all(pe_data).unwrap();
    }

    let result = Command::new("mcopy")
        .args(&[
            "-i",
            fat_path_str,
            efi_path_str,
            "::EFI/BOOT/BOOTX64.EFI",
        ])
        .output()
        .expect("mcopy not found");
    if !result.status.success() {
        eprintln!(
            "mcopy error: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        std::process::exit(1);
    }

    let mut fat_data = Vec::new();
    fs::File::open(&fat_path)
        .unwrap()
        .read_to_end(&mut fat_data)
        .unwrap();

    let _ = fs::remove_file(&fat_path);
    let _ = fs::remove_file(&efi_path);

    fat_data
}

fn combine(
    bios_path: &str,
    stage2_path: &str,
    pe_path: &str,
    output_path: &str,
) {
    let mut bios_data = Vec::new();
    fs::File::open(bios_path)
        .unwrap_or_else(|_| panic!("Cannot open BIOS file: {}", bios_path))
        .read_to_end(&mut bios_data)
        .unwrap();

    if bios_data.len() != 512 {
        eprintln!(
            "Error: BIOS file must be exactly 512 bytes, got {}",
            bios_data.len()
        );
        std::process::exit(1);
    }

    let mut stage2_data = Vec::new();
    fs::File::open(stage2_path)
        .unwrap_or_else(|_| panic!("Cannot open stage2 file: {}", stage2_path))
        .read_to_end(&mut stage2_data)
        .unwrap();

    let mut pe_data = Vec::new();
    fs::File::open(pe_path)
        .unwrap_or_else(|_| panic!("Cannot open PE file: {}", pe_path))
        .read_to_end(&mut pe_data)
        .unwrap();

    if pe_data.len() < 2 || &pe_data[0..2] != b"MZ" {
        eprintln!("Error: PE file does not start with MZ");
        std::process::exit(1);
    }

    let mbr_stage2_size = PARTITION_LBA * 512;
    let partition_bytes = DISK_SIZE - mbr_stage2_size;
    let partition_sectors = partition_bytes / 512;

    println!("Disk size: {} MB ({} bytes)", DISK_SIZE_MB, DISK_SIZE);
    println!(
        "Partition: LBA {}, {} sectors ({} bytes)",
        PARTITION_LBA, partition_sectors, partition_bytes
    );

    println!("Building FAT32 partition with EFI/BOOT/BOOTX64.EFI...");
    let fat_data = build_fat_partition(&pe_data, partition_bytes);
    println!("  FAT partition: {} bytes", fat_data.len());

    let mut combined: Vec<u8> = bios_data.clone();

    let part_entry = build_mbr_partition(true, 0x0C, PARTITION_LBA, partition_sectors);
    combined[0x1BE..0x1CE].copy_from_slice(&part_entry);

    for slot in 1..4 {
        let off = 0x1BE + slot * 16;
        combined[off..off + 16].fill(0);
    }

    let sig = u16::from_le_bytes([combined[0x1FE], combined[0x1FF]]);
    if sig != 0xAA55 {
        eprintln!("Warning: Boot signature at 0x1FE is not 0xAA55");
    }

    println!("Partition table written at offset 0x1BE");
    println!("  Bootable: yes, Type: 0x0C (FAT32 LBA)");
    println!("  Start LBA: {}, Sectors: {}", PARTITION_LBA, partition_sectors);

    combined.extend_from_slice(&stage2_data);

    let pad_size = (PARTITION_LBA * 512) as i64 - combined.len() as i64;
    if pad_size > 0 {
        combined.extend(std::iter::repeat(0).take(pad_size as usize));
    } else if pad_size < 0 {
        eprintln!(
            "Error: Stage-2 too large ({} bytes), exceeds LBA {}",
            stage2_data.len(),
            PARTITION_LBA
        );
        std::process::exit(1);
    }

    combined.extend_from_slice(&fat_data);

    let mut output = fs::File::create(output_path)
        .unwrap_or_else(|_| panic!("Cannot create output file: {}", output_path));
    output.write_all(&combined).unwrap();

    println!("\nWritten {} bytes to {}", combined.len(), output_path);
    println!("Done.");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 5 {
        eprintln!("Usage: {} <bios.bin> <stage2.bin> <pe.efi> <output>", args[0]);
        std::process::exit(1);
    }
    combine(&args[1], &args[2], &args[3], &args[4]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chs_lba_zero() {
        let chs = chs_from_lba(0, 16, 63);
        assert_eq!(chs.c, 0);
        assert_eq!(chs.h, 0);
        assert_eq!(chs.s, 1);
    }

    #[test]
    fn test_chs_lba_first_sector() {
        let chs = chs_from_lba(0, 16, 63);
        assert_eq!(chs.s, 1);
    }

    #[test]
    fn test_chs_lba_second_sector() {
        let chs = chs_from_lba(1, 16, 63);
        assert_eq!(chs.c, 0);
        assert_eq!(chs.h, 0);
        assert_eq!(chs.s, 2);
    }

    #[test]
    fn test_chs_lba_head_boundary() {
        // After 63 sectors, next head
        let chs = chs_from_lba(63, 16, 63);
        assert_eq!(chs.c, 0);
        assert_eq!(chs.h, 1);
        assert_eq!(chs.s, 1);
    }

    #[test]
    fn test_chs_lba_cylinder_boundary() {
        // After 16*63 = 1008 sectors, next cylinder
        let chs = chs_from_lba(1008, 16, 63);
        assert_eq!(chs.c, 1);
        assert_eq!(chs.h, 0);
        assert_eq!(chs.s, 1);
    }

    #[test]
    fn test_chs_lba_arbitrary() {
        let chs = chs_from_lba(123456, 16, 63);
        // c = 123456 / (16*63) = 123456 / 1008 = 122 rem 480
        // h = 480 / 63 = 7 rem 39
        // s = 39 + 1 = 40
        assert_eq!(chs.c, 122);
        assert_eq!(chs.h, 7);
        assert_eq!(chs.s, 40);
    }

    #[test]
    fn test_chs_cylinder_boundary_exact() {
        let lba = 1023 * 16 * 63;
        let chs = chs_from_lba(lba, 16, 63);
        assert_eq!(chs.c, 1023);
        assert_eq!(chs.h, 0);
        assert_eq!(chs.s, 1);
    }

    #[test]
    fn test_chs_max_within_geometry() {
        // Max addressable within 16 heads, 63 sectors/track
        let lba = 1023 * 16 * 63 + 15 * 63 + 62;
        let chs = chs_from_lba(lba, 16, 63);
        assert_eq!(chs.c, 1023);
        assert_eq!(chs.h, 15);
        assert_eq!(chs.s, 63);
    }

    #[test]
    fn test_build_mbr_partition_bootable() {
        let entry = build_mbr_partition(true, 0x0C, 8, 131064);
        assert_eq!(entry[0], 0x80); // bootable
        assert_eq!(entry[4], 0x0C); // FAT32 LBA
        let start_lba = u32::from_le_bytes(entry[8..12].try_into().unwrap());
        assert_eq!(start_lba, 8);
        let sectors = u32::from_le_bytes(entry[12..16].try_into().unwrap());
        assert_eq!(sectors, 131064);
    }

    #[test]
    fn test_build_mbr_partition_non_bootable() {
        let entry = build_mbr_partition(false, 0x0C, 8, 131064);
        assert_eq!(entry[0], 0x00); // non-bootable
    }

    #[test]
    fn test_build_mbr_partition_zero_start_nonzero_sectors() {
        let entry = build_mbr_partition(false, 0x00, 0, 1);
        assert_eq!(entry[0], 0x00);
        let start_lba = u32::from_le_bytes(entry[8..12].try_into().unwrap());
        assert_eq!(start_lba, 0);
        let sectors = u32::from_le_bytes(entry[12..16].try_into().unwrap());
        assert_eq!(sectors, 1);
    }

    #[test]
    fn test_build_mbr_partition_type() {
        let entry = build_mbr_partition(false, 0x83, 2048, 4096);
        assert_eq!(entry[4], 0x83); // Linux ext4
    }

    #[test]
    fn test_chs_clamping() {
        // Very large LBA should clamp CHS to max values
        let entry = build_mbr_partition(true, 0x0C, 1_000_000_000, 100_000);
        // CHS should be clamped to 1023/254/63
        let ce_h = entry[5];
        let ce_s = entry[6] & 0x3F;
        let ce_c = ((entry[6] as u16 & 0xC0) << 2) | entry[7] as u16;
        assert_eq!(ce_c, 1023);
        assert_eq!(ce_h, 254);
        assert_eq!(ce_s, 63);
    }

    #[test]
    fn test_build_fat_partition_size() {
        // Test that we can compute partition size correctly
        let mbr_stage2_size = PARTITION_LBA * 512;
        let partition_bytes = DISK_SIZE - mbr_stage2_size;
        assert_eq!(DISK_SIZE_MB, 64);
        assert_eq!(DISK_SIZE, 64 * 1024 * 1024);
        assert_eq!(PARTITION_LBA, 8);
        assert_eq!(mbr_stage2_size, 4096);
        assert_eq!(partition_bytes, 67104768);
        assert_eq!(partition_bytes % 512, 0);
        assert_eq!(partition_bytes / 512, 131064);
    }
}
