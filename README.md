# Rustrapper

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

A hybrid BIOS/UEFI bootloader that scans storage devices and network adapters —
written in Rust, with 16‑bit BIOS stages retained in C/NASM and an experimental
32‑bit Rust BIOS stage2.

Produces legacy BIOS (MBR+stage2) binaries, x86_64 UEFI and ARM64 EFI applications,
ARM64 bare-metal binaries, and PCI expansion ROMs.

## Features

- **BIOS** — 16‑bit MBR + 32-bit Rust stage2 (nightly): menu, PCI storage scan, e1000 MMIO DHCP
- **x86_64 UEFI** — Pure Rust PE/COFF: SNP protocol, DHCP client, storage scan
- **UEFI option ROM** — PCI expansion ROM with direct e1000 MMIO driver (no UEFI protocols needed during DXE)
- **ARM64 UEFI** — Same Rust code compiled for `aarch64-unknown-uefi`
- **ARM64 bare‑metal** — No firmware: PL011 UART, PCI ECAM walk, AHCI probe
- **BIOS option ROM** — Legacy PCI expansion ROM from `rust_payload.bin` via `romwrap --bios`
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
make run-x86_64-uefi              # x86_64 UEFI in QEMU (e1000 NIC, full DHCP)
make run-x86_64-uefi-rom          # x86_64 UEFI with custom option ROM + DHCP
```

## Build Targets

| Target | Binary | Description |
|--------|--------|-------------|
| `make i386-bios` | `bin/rust_payload.bin`, `bin/stage2_entry.bin` | 32‑bit BIOS stage2 (nightly only) |
| `make x86_64-uefi` | `bin/rustrapper.efi` | x86_64 UEFI application |
| `make aarch64-uefi` | `bin/rustrapper_arm64.efi` | ARM64 UEFI application |
| `make aarch64-bare` | `bin/rustrapper_arm64_bare.elf` | ARM64 bare‑metal |
| `make x86_64-uefi-rom` | `bin/rustrapper_efi.rom` | PCI expansion ROM (UEFI option ROM) |
| `make i386-bios-rom` | `bin/rustrapper_bios.rom` | PCI expansion ROM (BIOS option ROM) |

## Run in QEMU

```bash
make run-i386-bios                # Legacy BIOS stage2 (serial, Ctrl-A X to exit)
make run-i386-bios-rom            # Legacy BIOS with PCI expansion ROM (rust_payload.bin)
make run-x86_64-uefi              # x86_64 UEFI (e1000 NIC, SNP protocol, full DHCP)
make run-x86_64-uefi-rom          # x86_64 UEFI with custom option ROM (direct e1000 MMIO)
make run-aarch64-uefi             # ARM64 UEFI (virtio-net-pci NIC, DHCP OFFER)
make run-aarch64-bare             # ARM64 bare‑metal with AHCI drive
```

All BIOS targets use `-nographic` (Ctrl-A X to exit). The BIOS stage2 target (`run-i386-bios`) writes to both serial and VGA, so it also works with `-display curses` if you want a graphical view (Ctrl-A X won't work in curses mode — kill the process instead).

## Network Support

| Target | NIC | Method | DHCP |
|--------|-----|--------|------|
| BIOS Rust stage2 (disk) | e1000 | PCI I/O ports + MMIO | Full DHCP (DISCOVER→OFFER) |
| BIOS (option ROM) | e1000 | PCI option ROM with PCIR header | e1000 I/O BAR driver (real hardware only) |
| x86_64 UEFI (disk) | e1000 | SNP protocol | Full DHCP (DISCOVER/OFFER/REQUEST/ACK) |
| x86_64 UEFI (option ROM) | e1000 | Direct MMIO + I/O port PCI scan | Full DHCP (DISCOVER→OFFER) |
| ARM64 UEFI | virtio-net-pci | SNP protocol | Single-transmit DHCP (DISCOVER→OFFER) |

## Project Structure

```
├── bios/               # Minimal NASM for BIOS boot
│   ├── mbr.asm         # 512‑byte MBR stage‑1
│   └── stage2_entry.nasm # Entry stub for Rust stage2 (A20, protected mode, payload copy)
├── bios-rust/          # Rust 32-bit BIOS stage2 (nightly)
│   ├── Cargo.toml
│   ├── link.ld         # Link at 0x100000
│   ├── targets/i386-unknown-none.json
│   └── src/
│       ├── main.rs     # _start entry, menu dispatch
│       ├── serial.rs   # COM1 driver (putc, getc, flush)
│       ├── vga.rs      # VGA text-mode driver with scrolling
│       ├── pci.rs      # PCI scan via I/O ports 0xCF8/0xCFC
│       └── net.rs      # e1000 MMIO driver + DHCP (adapted from arm64-bare)
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
├── romwrap/            # CLI tool: wraps PE/COFF into PCI option ROM
├── Makefile            # Build orchestration
└── AGENTS.md           # Full development reference & gotchas
```

## Tests

All crates are host‑testable — platform‑specific code is guarded with `#[cfg(not(test))]`.

```bash
cargo test --workspace   # 107 tests across all crates
```

| Crate | Tests | What's Tested |
|-------|-------|---------------|
| `common` | 29 | Hex/decimal formatting, device info, scan loop with mocks |
| `uefi` | 24 | EFI type sizes, GUID values, SNP mode layout, constants, PCI IO protocol, DHCP frame parsing |
| `arm64-bare` | 21 | PCI offset encoding, storage subclass naming |
| `romwrap` | 33 | PCIR layout, BIOS/UEFI code types, entry routine, 512-byte alignment, edge cases |

## Requirements

- **Rust** with targets: `x86_64-unknown-uefi`, `aarch64-unknown-uefi`, `aarch64-unknown-none`
- **Rust nightly** for the `i386-bios` target (needs `-Zjson-target-spec` and `-Zbuild-std=core`)
- **BIOS**: `nasm`, `gcc`, `ld` (with `elf_i386` emulation), `objcopy`
- **Testing**: `qemu-system-x86_64` (with OVMF), `qemu-system-aarch64` (with `QEMU_EFI.fd`)
