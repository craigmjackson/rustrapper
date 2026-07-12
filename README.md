# Rustrapper

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A hybrid BIOS/UEFI bootloader that scans storage devices and network adapters —
written in Rust, with 16‑bit BIOS stages retained in C/NASM.

Produces a single `bootloader.combined` disk image that boots under legacy BIOS
and x86_64 UEFI, plus standalone ARM64 UEFI and ARM64 bare‑metal binaries. Also
produces a PCI expansion ROM (`rustrapper_efi.rom`) for use as a UEFI option ROM.

## Features

- **BIOS** — 16‑bit MBR + stage2 (NASM + C `-m16`) scans drives via INT 13h
- **BIOS network** — Direct e1000 I/O BAR driver (real hardware) or PXE/UNDI fallback via iPXE ROM
- **x86_64 UEFI** — Pure Rust PE/COFF: SNP protocol, DHCP client, storage scan
- **UEFI option ROM** — PCI expansion ROM with direct e1000 MMIO driver (no UEFI protocols needed during DXE)
- **ARM64 UEFI** — Same Rust code compiled for `aarch64-unknown-uefi`
- **ARM64 bare‑metal** — No firmware: PL011 UART, PCI ECAM walk, AHCI probe
- **Disk image builder** — Rust CLI tool assembles MBR + stage2 + FAT32 ESP
- **ROM wrapper** — Rust CLI tool wraps PE/COFF into UEFI PCI option ROM (`--bios` for BIOS option ROM)

## Quick Start

```bash
# Install Rust targets
rustup target add x86_64-unknown-uefi aarch64-unknown-uefi aarch64-unknown-none

# Install system dependencies
# Arch:  pacman -S nasm gcc mtools dosfstools qemu-system-x86 qemu-system-aarch64
# Debian: apt install nasm gcc mtools dosfstools qemu-system-x86 qemu-system-arm
# Fedora: dnf install nasm gcc mtools dosfstools qemu-system-x86 qemu-system-arm

make all                          # Build everything
make run-uefi                     # x86_64 UEFI in QEMU (e1000 NIC, full DHCP)
make run-rom-uefi                 # x86_64 UEFI with custom option ROM + DHCP
```

## Build Targets

| Target | Binary | Description |
|--------|--------|-------------|
| `make bios` | `bin/bios.bin`, `bin/stage2.bin` | 16‑bit BIOS MBR + stage2 |
| `make uefi` | `bin/rustrapper.efi` | x86_64 UEFI application |
| `make arm64` | `bin/rustrapper_arm64.efi` | ARM64 UEFI application |
| `make bare-arm64` | `bin/rustrapper_arm64_bare.elf` | ARM64 bare‑metal |
| `make combined` | `bin/bootloader.combined` | Hybrid disk image (64 MB) |
| `make romwrap-uefi` | `bin/rustrapper_efi.rom` | PCI expansion ROM (UEFI option ROM) |
| `make romwrap-bios` | `bin/rustrapper_bios.rom` | PCI expansion ROM (BIOS option ROM) |
| `make seabios` | `build/seabios/out/bios.bin` | Custom SeaBIOS (auto-cloned) |
| `make seabios-clean` | — | Remove SeaBIOS checkout |

## Run in QEMU

```bash
make run-bios                    # Legacy BIOS boot (no PXE, e1000 I/O stub)
make run-bios-pxe                # Legacy BIOS + PXE (custom SeaBIOS + iPXE ROM)
make run-uefi                    # x86_64 UEFI (e1000 NIC, SNP protocol, full DHCP)
make run-rom-uefi                # x86_64 UEFI with custom option ROM (direct e1000 MMIO)
make run-bios-rom                # Legacy BIOS with custom option ROM (SeaBIOS)
make run-bios-rom-pxe            # Legacy BIOS option ROM with PXE fallback (second NIC)
make run-uefi-arm64              # ARM64 UEFI (virtio-net-pci NIC, DHCP OFFER)
make run-bare-arm64              # ARM64 bare‑metal with AHCI drive
```

## Network Support

| Target | NIC | Method | DHCP |
|--------|-----|--------|------|
| BIOS (direct e1000) | e1000 | PCI I/O BAR | Raw Ethernet/IP/UDP (real hardware) |
| BIOS (PXE fallback) | Any with PXE ROM | INT 1A PXE/UNDI | Single-transmit DHCP via iPXE ROM |
| BIOS (option ROM, direct) | e1000 | PCI option ROM with PCIR header | e1000 I/O BAR driver (real hardware only) |
| BIOS (option ROM + PXE) | Two NICs | Option ROM + iPXE on second NIC | PXE fallback via INT 1A |
| x86_64 UEFI (disk) | e1000 | SNP protocol | Full DHCP (DISCOVER/OFFER/REQUEST/ACK) |
| x86_64 UEFI (option ROM) | e1000 | Direct MMIO + I/O port PCI scan | Full DHCP (DISCOVER→OFFER) |
| ARM64 UEFI | virtio-net-pci | SNP protocol | Single-transmit DHCP (DISCOVER→OFFER) |

## Disk Image Layout

```
LBA 0:      MBR (512 bytes) with partition table at 0x1BE
LBA 1–14:   Stage-2 (up to 7168 bytes), loaded by MBR to 0x1000
LBA 15+:    FAT32 ESP containing EFI/BOOT/BOOTX64.EFI
```

## Project Structure

```
├── bios/               # C/NASM 16‑bit BIOS sources
│   ├── mbr.asm         # 512‑byte MBR stage‑1
│   ├── stage2.c        # Stage‑2: drive + network scan
│   ├── pxe.c           # e1000 driver + PXE/UNDI fallback
│   ├── div64.c         # 64‑bit division helpers
│   ├── print.c/.h      # Shared formatting (putc, hex, dec)
│   └── scan.c/.h       # Shared device‑scan loop
├── common/             # no_std Rust library (print, scan, menu)
├── uefi/               # Rust UEFI binary (x86_64 + ARM64)
│   └── src/
│       ├── efi.rs      # Hand-typed EFI types, GUIDs, function offsets
│       ├── scan.rs     # Storage device enumeration
│       ├── net.rs      # SNP + direct e1000 DHCP client
│       └── main.rs     # efi_main entry point
├── arm64-bare/         # Rust ARM64 bare‑metal binary
│   └── src/
│       ├── pci.rs      # PCI ECAM walk, BAR sizing, AHCI probe
│       ├── uart.rs     # PL011 UART driver
│       ├── net.rs      # e1000 MMIO driver + DHCP client (no firmware)
│       └── main.rs     # global_asm! entry, UART/PCI init
├── disk-image/         # CLI tool: assembles combined disk image
├── romwrap/            # CLI tool: wraps PE/COFF into PCI option ROM
├── Makefile            # Build orchestration
└── AGENTS.md           # Full development reference & gotchas
```

## Tests

All crates are host‑testable — platform‑specific code is guarded with `#[cfg(not(test))]`.

```bash
cargo test --workspace   # 121 tests across all crates
```

| Crate | Tests | What's Tested |
|-------|-------|---------------|
| `common` | 29 | Hex/decimal formatting, device info, scan loop with mocks |
| `uefi` | 24 | EFI type sizes, GUID values, SNP mode layout, constants, PCI IO protocol, DHCP frame parsing |
| `arm64-bare` | 21 | PCI offset encoding, storage subclass naming |
| `disk-image` | 14 | CHS geometry, MBR partition entries, size invariants |
| `romwrap` | 33 | PCIR layout, BIOS/UEFI code types, entry routine, 512-byte alignment, edge cases |

## Requirements

- **Rust** with targets: `x86_64-unknown-uefi`, `aarch64-unknown-uefi`, `aarch64-unknown-none`
- **BIOS**: `nasm`, `gcc`, `ld` (with `elf_i386` emulation), `objcopy`
- **Disk image**: `mkfs.fat`, `mmd`, `mcopy` (dosfstools + mtools)
- **Testing**: `qemu-system-x86_64` (with OVMF), `qemu-system-aarch64` (with `QEMU_EFI.fd`)
