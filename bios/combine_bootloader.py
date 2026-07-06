#!/usr/bin/env python3
"""
Build a hybrid disk image bootable under both legacy BIOS and UEFI.

Layout:
  LBA 0 (0x000):     MBR (bios.bin, 512 bytes) with partition table
  LBA 1-7 (0x200):   Stage-2 (stage2.bin) + zero padding
  LBA 8+ (0x1000):   FAT32 partition containing EFI/BOOT/BOOTX64.EFI
"""

import struct, sys, subprocess, os, tempfile

PARTITION_LBA = 8  # FAT32 partition starts at LBA 8
DISK_SIZE_MB = 64  # Total disk image size


def chs_from_lba(lba, heads=16, sectors=63):
    c = lba // (heads * sectors)
    h = (lba // sectors) % heads
    s = (lba % sectors) + 1
    return (c, h, s)


def build_mbr_partition(bootable, part_type, start_lba, total_sectors, heads=16, sectors=63):
    if bootable:
        boot_flag = 0x80
    else:
        boot_flag = 0x00

    c_start, h_start, s_start = chs_from_lba(start_lba, heads, sectors)
    end_lba = start_lba + total_sectors - 1
    c_end, h_end, s_end = chs_from_lba(end_lba, heads, sectors)

    CHS_MAX = (1023, 254, 63)
    if c_start > CHS_MAX[0]:
        c_start, h_start, s_start = CHS_MAX[0], CHS_MAX[1], CHS_MAX[2]
    if c_end > CHS_MAX[0]:
        c_end, h_end, s_end = CHS_MAX[0], CHS_MAX[1], CHS_MAX[2]

    return struct.pack('<BBBBBBBBII',
                       boot_flag,
                       h_start, s_start | ((c_start >> 2) & 0xC0), c_start & 0xFF,
                       part_type,
                       h_end, s_end | ((c_end >> 2) & 0xC0), c_end & 0xFF,
                       start_lba,
                       total_sectors)


def build_fat_partition(pe_data, size_bytes):
    """
    Create a FAT32 filesystem image containing EFI/BOOT/BOOTX64.EFI.

    Uses mkfs.fat + mtools (mmd, mcopy) via subprocess.
    """
    with tempfile.NamedTemporaryFile(delete=False) as f:
        fat_path = f.name

    try:
        # Create blank image for mkfs.fat
        with open(fat_path, 'wb') as f:
            f.write(b'\x00' * size_bytes)

        result = subprocess.run(['mkfs.fat', '-F32', fat_path],
                                capture_output=True, text=True)
        if result.returncode != 0:
            print(f"mkfs.fat error: {result.stderr}")
            sys.exit(1)

        result = subprocess.run(['mmd', '-i', fat_path, 'EFI', 'EFI/BOOT'],
                                capture_output=True, text=True)
        if result.returncode != 0:
            print(f"mmd error: {result.stderr}")
            sys.exit(1)

        # Write strapper.efi as EFI/BOOT/BOOTX64.EFI
        with tempfile.NamedTemporaryFile(delete=False) as f:
            efi_tmp = f.name
            f.write(pe_data)

        result = subprocess.run(['mcopy', '-i', fat_path, efi_tmp, '::EFI/BOOT/BOOTX64.EFI'],
                                capture_output=True, text=True)
        os.unlink(efi_tmp)
        if result.returncode != 0:
            print(f"mcopy error: {result.stderr}")
            sys.exit(1)

        with open(fat_path, 'rb') as f:
            fat_data = f.read()
    finally:
        os.unlink(fat_path)

    return fat_data


def combine(bios_path, stage2_path, pe_path, output_path):
    with open(bios_path, 'rb') as f:
        bios_data = f.read()
    if len(bios_data) != 512:
        print(f"Error: BIOS file must be exactly 512 bytes, got {len(bios_data)}")
        sys.exit(1)

    with open(stage2_path, 'rb') as f:
        stage2_data = f.read()

    with open(pe_path, 'rb') as f:
        pe_data = f.read()

    if pe_data[:2] != b'MZ':
        print("Error: PE file does not start with MZ")
        sys.exit(1)

    # Calculate partition size
    total_disk_bytes = DISK_SIZE_MB * 1024 * 1024
    mbr_stage2_size = PARTITION_LBA * 512
    partition_bytes = total_disk_bytes - mbr_stage2_size
    partition_sectors = partition_bytes // 512
    print(f"Disk size: {DISK_SIZE_MB} MB ({total_disk_bytes} bytes)")
    print(f"Partition: LBA {PARTITION_LBA}, {partition_sectors} sectors ({partition_bytes} bytes)")

    # Build FAT32 partition
    print("Building FAT32 partition with EFI/BOOT/BOOTX64.EFI...")
    fat_data = build_fat_partition(pe_data, partition_bytes)
    print(f"  FAT partition: {len(fat_data)} bytes")

    # Build MBR with partition table entry
    combined = bytearray(bios_data)
    part_entry = build_mbr_partition(
        bootable=True,
        part_type=0x0C,  # FAT32 LBA
        start_lba=PARTITION_LBA,
        total_sectors=partition_sectors,
    )
    # Write partition entry at offset 0x1BE (first of 4 slots)
    combined[0x1BE:0x1BE + 16] = part_entry
    # Clear the other 3 partition slots
    for slot in range(1, 4):
        off = 0x1BE + slot * 16
        combined[off:off + 16] = b'\x00' * 16

    # Verify boot signature is intact
    sig = struct.unpack_from('<H', combined, 0x1FE)[0]
    if sig != 0xAA55:
        print("Warning: Boot signature at 0x1FE is not 0xAA55")
    print(f"Partition table written at offset 0x1BE")
    print(f"  Bootable: yes, Type: 0x0C (FAT32 LBA)")
    print(f"  Start LBA: {PARTITION_LBA}, Sectors: {partition_sectors}")

    # Add stage-2
    combined.extend(stage2_data)

    # Pad to partition start (LBA 8 = offset 0x1000)
    pad_size = PARTITION_LBA * 512 - len(combined)
    if pad_size > 0:
        combined.extend(b'\x00' * pad_size)
    elif pad_size < 0:
        print(f"Error: Stage-2 too large ({len(stage2_data)} bytes), exceeds LBA {PARTITION_LBA}")
        sys.exit(1)

    # Concatenate FAT partition
    combined.extend(fat_data)

    # Write output
    with open(output_path, 'wb') as f:
        f.write(combined)

    print(f"\nWritten {len(combined)} bytes to {output_path}")
    print("Done.")


if __name__ == '__main__':
    if len(sys.argv) != 5:
        print(f"Usage: {sys.argv[0]} <bios.bin> <stage2.bin> <pe.efi> <output>")
        sys.exit(1)
    combine(sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4])
