# Rustrapper

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A hybrid BIOS/UEFI bootloader that scans storage devices and prints media info — written in Rust, with 16‑bit BIOS stages retained in C/NASM.

Produces a single `bootloader.combined` disk image that boots under legacy BIOS and x86_64 UEFI, plus standalone ARM64 UEFI and ARM64 bare‑metal binaries.

## Features

- **BIOS** — 16‑bit MBR + stage2 (NASM + C `-m16`) scans drives via INT 13h
- **x86_64 UEFI** — Pure Rust PE/COFF enumerates Block IO handles, prints device paths and media info
- **ARM64 UEFI** — Same Rust code compiled for `aarch64-unknown-uefi`
- **ARM64 bare‑metal** — No firmware: PL011 UART, PCI ECAM walk, AHCI probe
- **Network (UEFI)** — SNP protocol, DHCP client, prints MAC/IP/subnet/gateway
- **Disk image builder** — Rust CLI tool assembles MBR + stage2 + FAT32 ESP into a hybrid image

## Quick Start

```bash
# Install Rust targets
rustup target add x86_64-unknown-uefi aarch64-unknown-uefi aarch64-unknown-none

# Install system dependencies
# Arch:  pacman -S nasm gcc mtools dosfstools qemu-system-x86 qemu-system-aarch64
# Debian: apt install nasm gcc mtools dosfstools qemu-system-x86 qemu-system-arm
# Fedora: dnf install nasm gcc mtools dosfstools qemu-system-x86 qemu-system-arm

make all                          # Build everything
make run-uefi                     # x86_64 UEFI in QEMU
```

## Build Targets

| Target | Binary | Description |
|--------|--------|-------------|
| `make bios` | `bin/bios.bin`, `bin/stage2.bin` | 16‑bit BIOS MBR + stage2 |
| `make uefi` | `bin/rustrapper.efi` | x86_64 UEFI application |
| `make arm64` | `bin/rustrapper_arm64.efi` | ARM64 UEFI application |
| `make bare-arm64` | `bin/rustrapper_arm64_bare.elf` | ARM64 bare‑metal |
| `make combined` | `bin/bootloader.combined` | Hybrid disk image (64 MB) |

## Run in QEMU

```bash
make run-bios           # Legacy BIOS boot
make run-uefi           # x86_64 UEFI (e1000 NIC, full DHCP)
make run-uefi-arm64     # ARM64 UEFI (virtio-net-pci NIC, DHCP OFFER info)
make run-bare-arm64     # ARM64 bare‑metal with AHCI drive
```

## Disk Image Layout

```
LBA 0:      MBR (512 bytes) with partition table at 0x1BE
LBA 1–7:    Stage-2 (up to 3584 bytes), loaded by MBR
LBA 8+:     FAT32 ESP containing EFI/BOOT/BOOTX64.EFI
```

## Project Structure

```
├── bios/               # C/NASM 16‑bit BIOS sources
├── common/             # no_std Rust library (print, scan)
├── uefi/               # Rust UEFI binary (x86_64 + ARM64)
│   └── src/
│       ├── efi.rs      # Hand-typed EFI types, GUIDs, function offsets
│       ├── scan.rs     # Storage device enumeration
│       ├── net.rs      # SNP protocol, DHCP client
│       └── main.rs     # efi_main entry point
├── arm64-bare/         # Rust ARM64 bare‑metal binary
│   └── src/
│       ├── pci.rs      # PCI ECAM walk, BAR sizing, AHCI probe
│       ├── uart.rs     # PL011 UART driver
│       └── main.rs     # global_asm! entry, UART/PCI init
├── disk-image/         # CLI tool: assembles combined disk image
├── Makefile            # Build orchestration
└── AGENTS.md           # Full development reference & gotchas
```

## Tests

All crates are host‑testable — platform‑specific code is guarded with `#[cfg(not(test))]`.

```bash
cargo test --workspace   # 69 tests across all crates
```

| Crate | Tests | What's Tested |
|-------|-------|---------------|
| `common` | 22 | Hex/decimal formatting, device info, scan loop with mocks |
| `uefi` | 18 | EFI type sizes, GUID values, SNP mode layout, constants |
| `arm64-bare` | 15 | PCI offset encoding, storage subclass naming |
| `disk-image` | 14 | CHS geometry, MBR partition entries, size invariants |

## Requirements

- **Rust** with targets: `x86_64-unknown-uefi`, `aarch64-unknown-uefi`, `aarch64-unknown-none`
- **BIOS**: `nasm`, `gcc`, `ld` (with `elf_i386` emulation), `objcopy`
- **Disk image**: `mkfs.fat`, `mmd`, `mcopy` (dosfstools + mtools)
- **Testing**: `qemu-system-x86_64` (with OVMF), `qemu-system-aarch64` (with QEMU_EFI.fd)
