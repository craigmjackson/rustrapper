# Rustrapper Hybrid BIOS/UEFI Bootloader

## Overview

Produces legacy BIOS (MBR+stage2) binaries, x86_64 UEFI and ARM64 EFI applications, ARM64 bare-metal binaries, and PCI expansion ROMs. On startup, all variants present a menu to choose between scanning storage devices or booting from network (DHCP).

## Directory Structure

```
.
├── Cargo.toml              # Workspace root (6 member crates)
├── Makefile                # Build orchestration (make all, make run-*)
├── AGENTS.md               # This file
├── bios/                   # Retained C/NASM sources
│   ├── mbr.asm             # 16-bit MBR stage-1
│   ├── stage2.c            # 16-bit stage-2 scanner
│   ├── stage2_entry.nasm   # Entry stub for Rust stage2 (A20, protected mode, copy to 1 MB)
│   ├── div64.c             # 64-bit division helpers for -m16
│   ├── print.c / print.h   # Shared formatting (putc callback, hex, dec)
│   └── scan.c / scan.h     # Shared scan loop (calls arch detect_device)
├── bios-rust/              # Rust 32-bit BIOS stage2 (experimental, nightly)
│   ├── Cargo.toml
│   ├── link.ld             # Link at 0x100000
│   └── src/
│       └── main.rs         # _start entry, VGA text-mode driver
├── common/                 # Rust no_std library
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── menu.rs         # Shared [1]/[2] menu logic
│       ├── print.rs        # Callback-based print (putc/puts/print_hex/print_dec)
│       └── scan.rs         # Generic device-scan loop
├── uefi/                   # Rust UEFI binary (x86_64 + ARM64)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # efi_main entry point
│       ├── efi.rs          # Hand-typed EFI types, GUIDs, Boot Services offsets
│       ├── scan.rs         # UEFI device scan (LocateHandleBuffer + OpenProtocol)
│       └── net.rs          # DHCP, SNP send/receive, IP/ARP parsing
├── arm64-bare/             # Rust ARM64 bare-metal binary (no firmware)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # Entry (global_asm! start), UART init, PCI scan
│       ├── pci.rs          # PCI ECAM walk, BAR sizing/enable, AHCI probe
│       ├── net.rs          # e1000 MMIO driver + DHCP client (no firmware)
│       └── uart.rs         # PL011 UART driver
├── romwrap/                # Rust CLI tool (PCI expansion ROM wrapper)
    ├── Cargo.toml
    └── src/
        └── main.rs         # Wraps PE/COFF → PCI option ROM with PCIR header
```

## Files

### `uefi/src/efi.rs` — EFI type definitions

- All types hand-written (no `uefi` crate dependency).
- `extern "efiapi"` calling convention is the EFI ABI on both x86_64 and ARM64.
- Function pointers that dereference raw pointers (e.g. `output_string`) must be `unsafe extern "efiapi" fn(...)`, NOT `extern "efiapi" fn(...)`. Using safe `extern "efiapi" fn` causes "unnecessary `unsafe` block" warnings when wrapping calls.
- **Boot Services function offsets** (hardcoded, verified on OVMF/EDK2):
  - `0x48` — `FreePool`
  - `0x118` — `OpenProtocol`
  - `0x138` — `LocateHandleBuffer`
- GUIDs must be `static` (not `const`) to guarantee stable address semantics when taking references.
- `UINTN` = `u64` on both x86_64 and ARM64 (LP64).

### `uefi/src/main.rs` — UEFI entry point

- `efi_main` is `extern "efiapi" fn(image_handle, system_table) -> !`.
- Stores a global reference to the system table for console input.
- Presents the `[1]/[2]` menu via `common::menu::show_menu`, using `EFI_SIMPLE_TEXT_INPUT_PROTOCOL.ReadKeyStroke` for non-blocking input.
- Dispatches to `scan_storage_devices` or `net::scan_network_devices` based on the choice.

### `uefi/src/scan.rs` — UEFI storage scan

- Function pointers read from Boot Services table via `read_boot_svc_fn::<T>(gbs, offset)` which casts an arbitrary offset to the target function type.
- `w16(con_out, s)` — converts `&str` to a null-terminated `[u16; 256]` and calls `OutputString`. The null terminator must be written explicitly at `buf[len]`; the buffer is NOT null-initialized because we only zero the first `len+1` entries via the write loop.
- `put_dec(con_out, val)` — converts `u64` to decimal digits using two temporary `[u16; 24]` arrays. Digits are written reversed into `rev[]`, then copied forward into `buf[]` starting at index 0 with an explicit null terminator `buf[j] = 0`. **Must null-terminate**: passing a mid-array pointer without a trailing null causes `OutputString` to read past the end of the buffer into stack garbage.
- `LocateHandleBuffer` with search type `2` (`ByProtocol`) enumerates all handles supporting `EFI_BLOCK_IO_PROTOCOL`.
- Each handle is opened with `OpenProtocol` for both Block IO and Device Path protocols using `EFI_OPEN_PROTOCOL_BY_HANDLE_PROTOCOL` (`0x00000001`).
- `EFI_DEVICE_PATH_PROTOCOL` type/subtype gives a hint about the device; the first node's type/subtype is read, not the full path.
- The handle buffer is freed with `FreePool` after enumeration.

### `uefi/src/net.rs` — UEFI network scan and DHCP

- `print_network_info` was removed (dead code). All NIC detection uses `scan_e1000_devices` with a 4-tier fallback chain.
- `pci_read_config32` is cross-platform: x86_64 uses I/O ports (`outl`/`inl` at `0xCF8`/`0xCFC`); ARM64 uses ECAM MMIO at `0x4010_0000_0000`.
- `e1000_init_and_dhcp` initializes e1000 and runs DHCP via `DirectMmioE1000` (previously a stub that only printed and returned None).
- `DirectMmioE1000` is the only e1000 driver used; `PciIoHandle` and `scan_root_bridge` were removed (dead code).
- SNP driver start: accepts both `EFI_SUCCESS`, `EFI_ALREADY_STARTED`, and the ARM64 firmware's error-bit-encoded variant (`0x8000000000000014`). ARM64 virtio SNP returns the latter.
- SNP initialize: failure is non-fatal. ARM64 virtio SNP returns `EFI_UNSUPPORTED` (with error bit).
- `dhcp_run` sends a single DHCPDISCOVER and waits for an OFFER — the ARM64 virtio SNP only supports one transmit per session (no buffer recycling without `Initialize`). x86_64 e1000 works normally with full DISCOVER→OFFER→REQUEST→ACK.
- `send_udp_dhcp` builds Ethernet/IP/UDP/DHCP frame from scratch and calls `SNP.Transmit`.
- `try_receive` calls `SNP.Receive` with non-null `SrcAddr`, `DestAddr`, and `Protocol` output pointers; some firmware requires them.
- `parse_dhcp_response` validates IP/UDP headers, DHCP magic cookie, transaction ID, MAC match, and extracts IP/subnet/gateway from DHCP options.

### `arm64-bare/src/main.rs` — ARM64 bare-metal entry

- Entry point via `global_asm!`: sets stack pointer, enables FP/SIMD access via `CPACR_EL1`, zeroes BSS, calls `main`.
- **FP/SIMD must be enabled before any Rust code runs**: `CPACR_EL1.FPEN` bits (20-21) default to trapping. rustc's compiler-generated `memset`/`memcpy` intrinsics use NEON instructions for buffers over a few hundred bytes; without `orr x9, x9, #(3 << 20)` / `msr cpacr_el1, x9` / `isb` in the boot stub, any such call takes a synchronous exception. Since no exception vector table is installed (`VBAR_EL1` defaults to 0), the CPU silently loops forever fetching garbage at address `0x200` (the "current EL, SPx, synchronous" vector offset) — this looks exactly like a hang with no error output.
- UART output via MMIO PL011 at `0x09000000` (QEMU virt machine).
- PCI ECAM at `0x4010_0000_00` (QEMU virt v11+).
- Load address: `0x4020_0000` (above DTB at `0x4000_0000`).
- Presents the `[1]/[2]` menu via `common::menu::show_menu`, reading from the PL011 UART (`uart::getc`), and dispatches to `scan::scan_devices` or `net::scan_network` based on the choice.

### `arm64-bare/src/net.rs` — ARM64 bare-metal e1000 NIC + DHCP

- Direct MMIO driver for the Intel 82540EM (`e1000`) emulated NIC — no firmware/UEFI involved, unlike the UEFI DHCP path in `uefi/src/net.rs`.
- Descriptor rings (`RxDescs`/`TxDescs`) are `#[repr(align(16))]` wrapper structs around `#[repr(C, packed)]` descriptor arrays; QEMU's e1000 requires 16-byte-aligned ring base addresses for `RDBAL`/`TDBAL`.
- **`RDLEN`/`TDLEN` minimum is 128 bytes** on QEMU's e1000 model — rings smaller than 8 descriptors (16 bytes each) are silently rejected (register reads back as the last valid value, not what was written).
- **`RDT` must be set to `NUM_RX_DESC - 1` after init**, not `0`. `RDH == RDT` means zero descriptors are owned by hardware (empty ring) — a very easy off-by-one that results in silently receiving nothing.
- All RX descriptors point at a single shared `RX_BUF` (2048 bytes); fine for the single in-flight DHCP transaction this driver performs, but would corrupt data under concurrent multi-packet traffic.
- DHCP RX polling must be RAM-only status checks (`RX_DESCS.0[idx].status & RX_STATUS_DD`), not MMIO register polling (`reg_read32(base, REG_RDH)`) in a tight loop — repeated MMIO reads under QEMU TCG are drastically slower than cached RAM reads and can make a poll loop take far longer than intended for the same iteration count.
- The RX poll loop needs roughly 100 million iterations (~1 second of wall time) for QEMU's slirp (`-nic user`) DHCP server to respond; smaller counts (e.g. 2 million, ~0.1s) reliably miss the OFFER even though the packet is delivered on the wire (verified with `-object filter-dump,netdev=net0,file=...` producing a valid pcap showing both DISCOVER and OFFER).
- Uses `-nic user,model=e1000` in `run-aarch64-bare` (not `-net none`); QEMU's e1000 emulation works normally with full ARP/DHCP over user-mode (slirp) networking on both x86_64 and aarch64 hosts.

### `arm64-bare/src/pci.rs` — PCI/AHCI probe

- Walks PCI bus 0, enumerates all devices.
- For each mass-storage class device, calls `enable_bars` (BAR sizing via all-1s write, MMIO from `0x3E00_0000`) and `probe_ahci`.
- `probe_ahci` enables HBA (`GHC.AE`, bit 31 of ABAR+0x04), reads CAP/PI/SSTS to detect attached drives.
- `pci_read_config` / `pci_write_config`: MMIO ECAM access via pointer dereference.

### `common/src/menu.rs` — Shared menu logic

- `MenuAction` enum: `StorageScan` / `NetworkBoot`.
- `show_menu(puts, putc, get_key)` prints `[1]/[2]` and polls `get_key` until the user presses a valid choice.
- `puts`/`putc` are function pointers, so the same function can be used with ASCII (UART/BIOS) and UEFI (wrapped to u16 `OutputString`) output.

### `bios/stage2.c` — BIOS stage-2 menu and scan

- Presents `[1]/[2]` menu and reads the choice from the serial port (COM1, 0x3F8), because the BIOS stage-2 uses serial output. This matches the `-serial stdio` setup used by `run-i386-bios` and `run-i386-bios-ipxe`.
- Dispatches to `scan_devices()` (INT 13h) or `pxe_scan()` (e1000 direct I/O BAR + PXE/UNDI fallback).

### `bios/stage2_entry.nasm` — BIOS entry stub for Rust stage2

- Loaded by MBR at physical `0x1000` (part of the 16-sector load). First 512 bytes are the stub itself; the Rust payload (`incbin`'d) follows at offset 512.
- Enables A20 gate (fast method via port `0x92`).
- Loads a minimal GDT (32-bit code/data segments, flat at 0-4G).
- Enters protected mode via `mov cr0, eax; jmp 0x08:pmode_start`.
- Copies Rust payload from `0x1200` → `0x100000` using `rep movsb`.
- Zeros 0x2000 bytes of BSS immediately after the payload with `rep stosb`.
- **Stack must NOT be in BIOS ROM area** (`0xF0000`–`0xFFFFF`). The BIOS ROM is read-only; pushes silently drop, corrupting return addresses, causing triple faults. Stack is at `0x00070000` (low RAM, well below BIOS area).
- **`cli` before calling Rust**: no IDT is set up in protected mode. A hardware timer interrupt with no IDT causes a triple fault. `cli` disables interrupts for the duration of the Rust code.
- Pushes `boot_drive` on the stack, calls `_start(0x100000)` via `call eax`.
- Total size = 512 B stub + Rust payload (currently ~7.4 KB = ~15 sectors total, under 16-sector limit).

### `bios-rust/src/main.rs` — Rust BIOS stage2 entry

- `#![no_std]`, `#![no_main]`, `extern "C" fn _start(boot_drive: u32) -> !` as entry point.
- **Dual output**: VGA text-mode driver (`vga.rs`) writing directly to `0xB8000` via `write_volatile` (COL/ROW statics in BSS, with scrolling when row ≥ 25), and serial output (`serial.rs`) via COM1 (`0x3F8`) using inline `asm!` for `in al, dx` / `out dx, al`.
- Both VGA and serial `putc` auto-translate `\n` → `\r\n` (matching `arm64-bare/src/uart.rs`).
- Serial `putc` polls LSR (0x3FD) bit 5 (THRE) before writing; `flush` waits for TEMT (bit 6); `getc` polls LSR bit 0 (Data Ready) for non-blocking input.
- Uses `common::print::init(dual_putc)` to wire the global print callback, then `common::menu::show_menu` for the `[1]/[2]` menu, and `common::scan::scan_devices` for storage scan — same pattern as `arm64-bare/src/main.rs`.
- Presents the `[1]/[2]` menu via `common::menu::show_menu`, reading from serial COM1 (`serial::getc`), and dispatches to `scan::scan_devices(pci::detect_device)` or `net::scan_network()`.
- `#[cfg(not(test))]` guards on `_start` and `#[panic_handler]` so the crate can be compiled in the host test harness.
- Custom `link.ld` links at `0x100000` (where the entry stub copies the payload).

### `bios-rust/src/pci.rs` — BIOS Rust PCI scan (x86 I/O ports)

- PCI config space via x86 I/O ports `0xCF8`/`0xCFC` (NOT ECAM like `arm64-bare/src/pci.rs`).
- `pci_read32`/`pci_write32` use `out dx, eax` / `in eax, dx` inline `asm!` with the 32-bit address format `0x80000000 | (bus << 16) | (dev << 11) | (func << 8) | (offset & 0xFC)`.
- `pci_enable_bars` always enables I/O + Memory + Bus Master (command bits 0-2). Does NOT re-assign BARs if firmware already assigned them (checks if BAR0 is non-zero). BAR sizing only runs for unassigned BARs, allocating from `MMIO_NEXT` starting at `0x1000_0000`.
- `detect_device` callback for `common::scan::scan_devices` — walks PCI bus 0 for mass-storage class (0x01) devices, enables BARs, probes AHCI for SATA controllers.
- `storage_name` maps subclass codes to names (same as `arm64-bare/src/pci.rs`).
- `pci_print_all` prints all PCI devices with vendor/device IDs and class descriptions.

### `bios-rust/src/net.rs` — BIOS Rust e1000 NIC + DHCP

- Direct MMIO driver for the Intel 82540EM (`e1000`) emulated NIC — same logic as `arm64-bare/src/net.rs` but using `u32` addresses instead of `u64` (32-bit target).
- Uses `core::ptr::read_volatile`/`write_volatile` with `&raw const`/`&raw mut` for packed struct descriptor field access (avoiding "reference to field of packed struct is unaligned" errors).
- `e1000_init` resets NIC, reads MAC, sets up RX/TX descriptor rings, enables RCTL/TCTL. Explicitly writes `REG_RDBAH`/`REG_TDBAH` to 0 (high 32 bits of ring base address).
- `e1000_send` copies frame to TX_BUF, fills TX descriptor with EOP|IFCS|RS, rings TDT, polls status DD bit via `read_volatile`.
- `dhcp_run` sends a single DHCPDISCOVER and polls for OFFER (100M iterations, ~1 second).
- `scan_network` walks PCI bus 0 for network class (0x02) devices, enables BARs, inits e1000, runs DHCP, prints IP/subnet/gateway.

### `bios-rust/src/serial.rs` — BIOS Rust serial driver

- `putc` auto-translates `\n` → `\r\n` (writes `\r` first).
- Polls LSR (0x3FD) bit 5 (THRE) before writing to 0x3F8.
- `getc` polls LSR bit 0 (Data Ready), reads from 0x3F8 if ready, returns `Option<u8>`.
- `flush` waits for TEMT (LSR bit 6).
- Uses separate `asm!` blocks for reads and writes; poll block must NOT use `options(pure)`.

### `bios-rust/src/vga.rs` — BIOS Rust VGA text-mode driver

- Writes directly to `0xB8000` (VGA text buffer) via `read_volatile`/`write_volatile`.
- COL/ROW statics in BSS track cursor position.
- `putc` auto-translates `\n` → `\r\n` (writes `\r` first, then the char).
- 80x25 text mode, attribute byte `0x07` (white on black).
- **Scrolling**: when ROW ≥ 25, shifts all lines up by one row via `read_volatile`/`write_volatile` byte copies and clears the last row. Without scrolling, output wrapping to row 0 would overwrite earlier lines (the DHCP results would be invisible).

### `common/src/print.rs` — Shared formatting

- Callback-based: `print_init(putc_fn)` sets the output function.
- `print_hex(val, nibbles)` — prints hex, no leading zeros except for `0`.
- `print_dec(val)` — converts `u64` to decimal using a right-to-left buffer.
- All functions call the `putc` callback for each character.

### `common/src/scan.rs` — Shared device scan loop

- Generic loop: for `index` in `0..MAX_DEVICES`, calls an arch-specific `detect_device` callback, prints results.
- Architecture-specific detection is provided by `detect_device` via a function pointer or trait (not yet implemented — the current C stage2 and Rust UEFI/ARM64 targets each handle scanning independently).

### `romwrap/src/main.rs` — PCI expansion ROM wrapper

- Wraps a binary into a PCI expansion ROM container usable by UEFI firmware (OVMF) or legacy BIOS (SeaBIOS).
- ROM header at offset 0: `0xAA55` signature, init vector (0x0000 for UEFI, non-zero for BIOS), size in 512-byte blocks at offset 0x04, PCIR offset at `0x18`.
- **PCI Data Structure (PCIR)**: `"PCIR"` signature, vendor/device IDs (default `0x8086`/`0x100E` for Intel e1000), code type `0x03` (EFI) or `0x00` (PC-AT/BIOS), indicator `0x80` (last image), revision `0x00`.
- Total header size: 52 bytes (28 B ROM header + 24 B PCIR structure).
- For BIOS: a 3-byte entry routine (`xor ax,ax; retf` = `0x33 0xC0 0xCB`) at offset `0x34` sets CF=0 for success and returns. Init vector points to offset `0x34`. Payload (stage2.bin) follows at offset `0x37`.
- For UEFI: PE/COFF binary follows headers at offset `0x34`. Init vector is 0 (not used by UEFI firmware).
- Output is padded to the next 512-byte boundary (file size is always a multiple of 512).
- Supports `--vendor=` and `--device=` flags for custom PCI IDs.
- Supports `--bios` flag for legacy BIOS ROMs (accepts any binary, not just PE/COFF).

### `Makefile` — Build orchestration

- Top-level targets: `all`, `i386-bios`, `x86_64-uefi`, `aarch64-uefi`, `aarch64-bare`, `x86_64-uefi-rom`, `i386-bios-rom`, `i386-bios-rust-rom`, `run-*`, `clean`.
- Uses `CARGO_TARGET_DIR=target` and per-target `RUSTFLAGS` for UEFI (needs `/NODEFAULTLIB` on x86_64).
- BIOS targets compile from `bios/` using the same GCC+NASM flags as the original project.
- `i386-bios-rust` target requires nightly (`-Zjson-target-spec -Zbuild-std=core`). Produces `bin/stage2_entry.bin` (entry stub + incbinned Rust payload) and `bin/rust_payload.bin` (raw binary).
- `run-i386-bios-rust` uses `-nographic` (matching all other BIOS targets) so Ctrl-A X exits cleanly. The Rust stage2 also writes to VGA text mode, so it works with `-display curses` too (but Ctrl-A X won't work in curses mode — kill the process instead).

## Key Gotchas

### Rust-specific

1. **EFI function pointer safety**: UEFI protocol function pointers that dereference raw pointers must be typed as `unsafe extern "efiapi" fn(...)`. Marking them safe triggers "unnecessary `unsafe` block" when wrapping the call.
2. **Null-terminate u16 arrays for OutputString**: `SIMPLE_TEXT_OUTPUT_PROTOCOL.OutputString` reads until a `0x0000` word. Always write an explicit `buf[j] = 0` after the last character. Passing a pointer into the middle of an array without a trailing null reads past the buffer into stack garbage.
3. **GUIDs must be `static`**, not `const`. Taking a reference to a `const` generates a new temporary each time, producing non-identical addresses. EFI runtime code compares GUID pointer addresses.
4. **`extern "efiapi"` is portable**: Both x86_64 and ARM64 UEFI use the same `extern "efiapi"` calling convention in Rust. No `ms_abi` distinction needed (unlike C where `__attribute__((ms_abi))` is required on x86_64).
5. **`/NODEFAULTLIB` on x86_64-unknown-uefi**: Without `RUSTFLAGS="-C link-args=/NODEFAULTLIB"`, the linker pulls in CRT startup which conflicts with the UEFI environment. ARM64 doesn't need this.
6. **`panic = "abort"`**: UEFI and bare-metal targets must set `panic = "abort"` in `[profile.*]`. Define profiles in the workspace root `Cargo.toml` (not per-crate) to avoid "profiles for the non root package will be ignored" warnings.
7. **`aarch64-unknown-none` requires `global_asm!` entry**: There's no CRT; use `core::arch::global_asm!` to define the `_start` symbol that sets SP and clears BSS before calling Rust code.
8. **Enable `CPACR_EL1` FP/SIMD access before any Rust code runs on `aarch64-unknown-none`**: rustc's `memset`/`memcpy` compiler intrinsics use NEON registers for larger buffers (e.g. a 1514-byte Ethernet frame array). Without setting `CPACR_EL1.FPEN` (bits 20-21) to `0b11` in the boot stub, the first such call traps. With no exception vector table installed (`VBAR_EL1 == 0` at reset), the trap vectors to address `0x200` and the CPU spins forever on undefined instructions there — indistinguishable from a plain hang, with zero diagnostic output.

9. **`i386-unknown-none` requires `i128:128` in data-layout**: The custom target JSON (`targets/i386-unknown-none.json`) must include `i128:128` in the `data-layout` field. LLVM's 32-bit x86 default data layout has `i128:64` on some versions; omitting `i128:128` produces linker errors about missing `__multi3`.
10. **Rust BIOS payload compiles as a 32-bit ELF, not a flat binary**: `cargo build` produces a relocatable ELF; `objcopy -O binary` strips it to a flat binary suitable for the entry stub's `incbin` and copy to 1 MB.
11. **NASM incbin path relative to source, not CWD**: The `incbin "../bin/rust_payload.bin"` in `stage2_entry.nasm` resolves relative to the `bios/` directory. The Makefile runs NASM with `cd bios` to make this work.
12. **Stack must be in low RAM, NOT BIOS ROM area for Rust BIOS stage2**: The BIOS ROM at `0xF0000`–`0xFFFFF` is read-only. Setting `esp` to `0x000FFFF0` (top of 1 MB) causes pushes to silently drop, corrupting return addresses and causing triple faults. Use `0x00070000` (low RAM) instead.
13. **`cli` before Rust code in protected mode**: No IDT is set up after the protected mode switch. A hardware timer interrupt with no IDT causes a triple fault. Always `cli` before calling Rust code from the entry stub.
14. **Inline `asm!` for serial I/O**: Use separate `asm!` blocks for the LSR poll (`in al, dx`) and data output (`out dx, al`). The poll block must NOT use `options(pure)` (I/O port reads are NOT idempotent — `pure` causes the compiler to optimize the poll loop into `jmp $`). Pass the output byte via `in("al") c` — do NOT let the compiler choose the register, as `in al, dx` clobbers AL.
15. **BSS_ZERO_SIZE must cover actual BSS**: The entry stub zeros `BSS_ZERO_SIZE` bytes after the payload. If BSS grows (e1000 descriptor rings + packet buffers ~4.3 KB), increase `BSS_ZERO_SIZE` from `0x1000` to `0x2000`. Check with `readelf -S target/i386-unknown-none/release/bios-rust | grep .bss`.
16. **MBR sector count must match payload**: The MBR loads 16 sectors (8192 bytes) for the Rust stage2 (was 14 for the C stage2). The Rust payload + 512 B stub is ~7.8 KB = 15 sectors. Increase the `dap` sector count in `mbr.asm` if the payload grows further.
17. **PCI Bus Master must be enabled for e1000 DMA**: SeaBIOS enables Memory Space (bit 1) but may NOT enable Bus Master (bit 2) in the PCI Command register. Without Bus Master, the e1000 can read descriptors (MMIO works) but TX descriptor write-back (DMA) silently fails — TDH advances but the status DD bit is never set. `pci_enable_bars` must always set command bits 0-2 (I/O + Memory + Bus Master), even if some are already set. Do NOT re-assign BARs if firmware already assigned them (check if BAR0 is non-zero before sizing).

### Original project (C/NASM) still relevant

9. **"MZ" corrupts DL**: The MBR's first 2 bytes are `0x4D 0x5A`, which execute as `dec bp; pop dx`. The `pop dx` overwrites the boot-drive value in DL. Always set DL explicitly before INT 13h calls.
10. **512-byte MBR limit**: The MBR is strictly 512 bytes (including `0xAA55` at `0x1FE`). All real logic goes in `stage2.bin`.
11. **ARM64 UEFI SNP single-transmit limit**: The ARM64 virtio SNP driver only supports one `Transmit` per session because `Initialize` returns `EFI_UNSUPPORTED` (no buffer recycling). Send DHCPDISCOVER and accept the OFFER as final — do not attempt REQUEST/ACK. x86_64 e1000 supports the full handshake.
12. **ARM64 EDK2 error bit**: ARM64 firmware encodes EFI errors with bit 63 set (`0x8000000000000014` for `EFI_ALREADY_STARTED`). Always check both the plain constant and the error-bit variant when comparing status codes on ARM64.
13. **`virtio-net-pci` is the only ARM64 SNP NIC**: QEMU `qemu-system-aarch64` EDK2 firmware only detects SNP via `virtio-net-pci`. All other models (e1000, e1000e, rtl8139, etc.) return "No network adapters found".
14. **`EFI_OPEN_PROTOCOL_GET_PROTOCOL`**: Use `0x00000002` instead of `BY_HANDLE_PROTOCOL` (`0x00000001`) for SNP on ARM64 to avoid agent handle tracking issues.
15. **Non-null SNP.Receive pointers**: Some firmware (ARM64) requires `SrcAddr`, `DestAddr`, and `Protocol` to be non-null output pointers. Passing `null` causes `EFI_INVALID_PARAMETER`.
16. **`.comment` section causes OVMF Load Error**: Removed from stage2 with `objcopy -R .comment`. When producing PE/COFF from Rust UEFI targets, the linker may still add metadata sections; ensure they're stripped.
17. **QEMU virt ECAM moved to 64-bit address**: PCIe ECAM is at `0x4010_0000_0000` (QEMU virt v11+), not `0x3F00_0000`. Hardcoded in `arm64-bare/src/pci.rs`.
18. **PCI BARs unassigned without firmware**: On bare-metal, BARs read as 0. Must implement BAR sizing (write-all-1s) and resource allocation from PCI MMIO window (`0x1000_0000–0x3EFF_FFFF` on virt).
19. **AHCI requires HBA enable before port access**: Set GHC.AE (bit 31 of ABAR+0x04) before reading CAP, PI, SSTS.
20. **QEMU DTB at 0x4000_0000**: Load bare-metal binaries at `0x4020_0000` or higher to avoid overlap. Set in linker script / start.S.
21. **`llvm-objcopy` for ARM64 ELF→binary**: Host `objcopy` cannot handle ARM64 ELF. Use `llvm-objcopy -O binary`.
22. **`lld` required for ARM64 linking**: Host `ld` lacks `aarch64linux` emulation. Use `-fuse-ld=lld` with clang or `lld-link` for UEFI.

### BIOS stage2 (C, retained as-is)

26. **INT 13h register preservation**: `INT 13h` can clobber registers. Use C local variables instead of relying on register values across calls.
27. **DPB buffer size**: Must be set to `30` (`0x001E`) in the first word before AH=48h.
28. **Global data in `.data` for `--oformat=binary`**: Accessed globals must be in `.data` (not `.bss`) because `ld --oformat=binary` skips `.bss`. Both `dpb_buf` and `g_putc` use `__attribute__((section(".data")))`.
29. **`"di"` clobber for INT 13h AH=48h**: The call clobbers `%edi`. List `"di"` as a clobber in inline asm, otherwise GCC may keep the info pointer in `%edi` across the asm, corrupting memory.
30. **`__umoddi3` inline asm indexing**: With two outputs (`"=a", "=d"`) before three inputs, the divisor is `%4`, not `%3`. Using `%3` causes `divl %edx` (divide by EDX), which hangs when EDX=0.

31. **Stage2 14-sector limit (7168 bytes)**: The MBR loads 14 sectors (LBA 1-14, 7168 bytes) at physical 0x1000 for the C stage2. Keep `stage2.bin` under this limit. Check with `stat -c%s bin/stage2.bin`. The Rust stage2 MBR loads 16 sectors (8192 bytes) to accommodate the larger Rust payload.

32. **PXE/UNDI via INT 1Ah**: PXE entry is discovered via INT 1Ah AX=0x5650. Functions called via INT 1Ah AX=0, BX=function, ES:DI=parameter block. Function numbers: 0x0001=UNDI_STARTUP, 0x0003=UNDI_INITIALIZE, 0x0005=UNDI_OPEN, 0x000B=UNDI_GET_INFO, 0x0030=UDP_OPEN, 0x0034=UDP_TRANSMIT, 0x0036=UDP_RECEIVE.

33. **PXE inline asm clobbers**: For `int 0x1A` PXE calls, only AX and flags are preserved. Use `push`/`pop` for BX and ES inside the asm. List `"cc"` and `"memory"` as clobbers.

34. **PXE buffer in `.data`**: The 64-byte `pxe_buf` must be in `.data` (not `.bss`) because `ld --oformat=binary` skips `.bss`. Must be zeroed before each call via `pxe_clear()`.

35. **Frame buffers at fixed offset 0x1500**: DHCP and receive buffers at offset 0x1500 from DS=0x0100 (physical 0x2500) — past the max stage2 size of 0x1400 bytes. Hardcoded pointers avoid bloat from 1514+ byte buffers.

36. **UDP port fields in network byte order**: PXE UDP parameter structures use big-endian port numbers (`htons()`). `BufferLen` is in host byte order.

37. **e1000 is the BIOS test NIC**: Use `-nic user,model=e1000` for SeaBIOS PXE support in QEMU. virtio-net-pci also works. Other models may not have UNDI support.

38. **Single-transmit DHCP for BIOS**: Same pattern as ARM64 UEFI — send one DISCOVER via UDP_TRANSMIT, poll UDP_RECEIVE up to 100k iterations, accept OFFER as final. No REQUEST/ACK.

39. **Custom SeaBIOS for PXE testing**: The `run-i386-bios-ipxe` target builds a custom SeaBIOS (`make x86_64-seabios`, cloned from GitHub) and uses it via `-bios`. This ensures the PXE/UNDI INT 1Ah handler stays resident after boot (the default SeaBIOS that ships with QEMU tears down the PXE stack after it boots from a non-PXE device). The build uses `git clone --depth=1` into `build/seabios` and runs `make defconfig` to produce a QEMU-optimized `bios.bin`. On real hardware workstations, the PXE stack remains resident in UMA regardless of boot source and no custom SeaBIOS is needed.

40. **QEMU e1000 I/O BAR is a stub**: QEMU's classic e1000 PCI I/O handler (`e1000_io_read`/`e1000_io_write`) is an empty stub — it returns 0 for all reads and ignores all writes. This means the direct e1000 driver in `pxe.c` (which uses the PCI I/O BAR for register access) can NOT work on QEMU. On real hardware, the I/O BAR software access protocol provides full register access. For QEMU testing, use `make run-x86_64-uefi` (UEFI path works in QEMU) or `make run-i386-bios-ipxe` (BIOS PXE with custom SeaBIOS) or `make run-i386-bios-rom-pxe` (BIOS option ROM with PXE fallback via second NIC).
41. **UEFI keyboard input is non-blocking**: `EFI_SIMPLE_TEXT_INPUT_PROTOCOL.ReadKeyStroke` returns `EFI_NOT_READY` when no key is pressed. The menu polls it in a tight loop until a valid choice is received. Store the system table pointer globally so the polling function can reach `con_in` without threading it through every helper.
42. **BIOS menu reads from serial port**: The BIOS stage-2 uses serial output, so the menu reads from COM1 (0x3F8) rather than the keyboard. This matches the `-serial stdio` setup used by `run-i386-bios` and `run-i386-bios-ipxe`. Real hardware with a serial console will work the same way; a VGA+keyboard setup would require a keyboard input path instead.

### UEFI Option ROM & Direct e1000 (added during option ROM work)

43. **PCI device path node packs device/function into one byte**: UEFI PCI device path nodes are 6 bytes total — 4-byte header (`type_`, `sub_type`, `length=6`) + 1-byte `bus` + 1-byte `dev_func`. The `dev_func` byte packs device number in the upper 5 bits (`>> 3`) and function in the lower 3 bits (`& 0x07`). Do NOT use 3 separate bytes for bus/device/function. See `uefi/src/net.rs PciNode`.

44. **Device path PCI type check must be 0x01**: Use `type_ == 0x01 && sub_type == 0x01` to detect PCI hardware device path nodes. Type `0x02` is ACPI (not PCI). The first node of most device paths is ACPI Expanded, which has a completely different data layout. Wrong type check (=0x02) reads ACPI HID bytes as if they were PCI bus/dev, producing garbage.

45. **Direct x86 I/O ports for PCI config space**: On x86, `outl(0xCF8, addr)` + `inl(0xCFC)` reads PCI config space at any time — during DXE phase, while option ROMs run, in real mode, etc. This bypasses all UEFI protocols. Use when `EFI_PCI_IO_PROTOCOL.Pci.Read` returns `EFI_UNSUPPORTED` (as it does during option ROM entry on OVMF). On ARM64, `pci_read_config32` uses ECAM MMIO at `0x4010_0000_0000` instead.

46. **Raw MMIO e1000 driver for option ROM context**: `DirectMmioE1000` in `uefi/src/net.rs` duplicates the e1000 init/send/receive logic from `PciIoHandle` but uses `core::ptr::read_volatile`/`write_volatile` on the BAR0 address instead of UEFI PCI IO protocol MMIO access. Shares the same `static mut` descriptor ring and buffer structures. Called from tier 4 of the fallback chain.

47. **`scan_e1000_devices` fallback chain**: 4 tiers, each tried in order until one succeeds: (1) DevicePath on image handle — fails for option ROM (DP not installed), (2) PCI IO protocol handles — `OpenProtocol` works but `PciRead`/`GetLocation` return `EFI_UNSUPPORTED`, (3) Loaded Image protocol — device handle lacks PCI IO, (4) Direct PCI scan via I/O ports + raw MMIO e1000 — works in all phases.

48. **Option ROM runs `efi_main` twice with `run-x86_64-uefi-rom`**: The custom option ROM is executed once during DXE (driver entry) and again from the disk (BDS menu). With tier 4 fallback, the DXE instance now succeeds too. The x86_64 non-ROM path (`run-x86_64-uefi`) only runs from the disk and has full SNP support.

49. **QEMU e1000 MMIO BAR0 is valid during DXE**: Even though option ROM runs early, QEMU's firmware assigns PCI resources before dispatching option ROMs. BAR0 reads back a valid MMIO address (e.g. `0x80F80000`). On real hardware this is also true — the BIOS/firmware configures PCI devices before option ROM entry.

50. **`pci_read_config32` is cross-platform**: In `uefi/src/net.rs`, `pci_read_config32` uses x86 I/O ports on `#[cfg(target_arch = "x86_64")]` and ECAM MMIO at `0x4010_0000_0000` on ARM64. Both return `u32` with the same signature, so call sites need no cfg guards. The ARM64 ECAM address matches QEMU virt v11+ (same as `arm64-bare/src/pci.rs`).

## Makefile Targets

```bash
make all                         # Build everything
make i386-bios                   # Build MBR + stage2 (C/NASM)
make i386-bios-rust              # Build Rust BIOS stage2 (nightly only)
make x86_64-uefi                 # Build x86_64 UEFI binary
make aarch64-uefi                # Build ARM64 UEFI binary
make aarch64-bare                # Build Rust ARM64 bare-metal
make x86_64-uefi-rom             # Build UEFI PCI expansion ROM from rustrapper.efi
make i386-bios-rom               # Build BIOS PCI expansion ROM from stage2.bin
make i386-bios-rust-rom          # Build BIOS PCI expansion ROM from rust_payload.bin
make x86_64-seabios              # Build custom SeaBIOS (auto-cloned from GitHub)
make run-i386-bios               # BIOS mode via SeaBIOS (serial output; e1000 I/O not avail)
make run-i386-bios-rust          # BIOS with Rust stage2 (serial, Ctrl-A X to exit)
make run-i386-bios-ipxe          # BIOS PXE with custom iPXE ROM + custom SeaBIOS
make run-i386-bios-rom           # Legacy BIOS with C PCI expansion ROM (SeaBIOS)
make run-i386-bios-rust-rom      # Legacy BIOS with Rust PCI expansion ROM (rust_payload.bin)
make run-i386-bios-rom-pxe       # Legacy BIOS option ROM + PXE (second NIC with iPXE ROM)
make run-x86_64-uefi             # x86_64 UEFI via FAT dir (e1000, full DHCP)
make run-x86_64-uefi-rom         # x86_64 UEFI with custom PCI expansion ROM
make run-aarch64-uefi            # ARM64 UEFI from FAT directory
make run-aarch64-bare            # ARM64 bare-metal + AHCI drive
make clean                       # Clean all artifacts (keeps SeaBIOS checkout)
make x86_64-seabios-clean        # Remove SeaBIOS checkout
```

### `i386-bios-rust-rom` — Rust BIOS option ROM

- Wraps `bin/rust_payload.bin` (raw 32-bit PM binary) as a legacy BIOS PCI expansion ROM via `romwrap --bios`. Produces `bin/rustrapper_bios_rust.rom` (~15 blocks / ~7.7 KB for the current payload).
- Like the C BIOS option ROM, the 3-byte init routine at 0x34 is a no-op (`xor ax,ax; retf`). SeaBIOS acknowledges the ROM and continues to boot from the disk image.
- The Rust payload at 0x37+ is just data — the actual Rust execution comes from the MBR → stage2 entry stub → Rust code at 0x100000 on the disk, not from the ROM.
- The `run-i386-bios-rust-rom` target boots `bin/bios-rust.img` with the ROM loaded on an e1000 NIC via `-device e1000,romfile=...`. Useful for testing that the option ROM is correctly detected by SeaBIOS.
- Default PCI IDs are `0x8086:0x100E` (Intel e1000). Use `romwrap --vendor=... --device=...` for custom IDs.

## Targets

| Target                          | Arch   | Firmware          | Binary                           |
| ------------------------------- | ------ | ----------------- | -------------------------------- |
| `x86_64-unknown-uefi`           | x86_64 | UEFI              | `bin/rustrapper.efi`               |
| `aarch64-unknown-uefi`          | ARM64  | UEFI              | `bin/rustrapper_arm64.efi`         |
| `aarch64-unknown-none`          | ARM64  | None (bare-metal) | `bin/rustrapper_arm64_bare.elf`    |
| `x86_64-linux-gnu` (romwrap)    | Host   | —                 | `target/debug/romwrap`           |
| `i386-none-elf` (BIOS)          | x86-16 | BIOS              | `bin/bios.bin`, `bin/stage2.bin` |
| `i386-unknown-none` (Rust BIOS) | i386   | BIOS (32-bit PM)  | `bin/rust_payload.bin`, `bin/stage2_entry.bin` |
| PCI Option ROM (UEFI)           | x86_64 | UEFI (ROM)        | `bin/rustrapper_efi.rom`         |
| PCI Option ROM (BIOS, C)        | x86-16 | BIOS (ROM)        | `bin/rustrapper_bios.rom`        |
| PCI Option ROM (BIOS, Rust)     | x86-32 | BIOS (ROM)        | `bin/rustrapper_bios_rust.rom`   |

## Tests

Run the full host-testable suite:

```bash
cargo test --workspace        # All 107 tests across all crates
cargo test --package common   # 29 tests (formatting, scan loop)
cargo test --package uefi     # 24 tests (type sizes, GUID values, constants, SNP mode layout, DHCP frame parsing)
cargo test --package arm64-bare  # 21 tests (pci_off, storage_name)
cargo test --package romwrap  # 33 tests (PCIR layout, BIOS/UEFI code types, entry routine, 512-byte alignment, edge cases)
```

| Crate | Tests | What's tested |
|-------|-------|---------------|
| `common` | 29 | `format_hex` edge cases (zero, leading-zero suppression, all nibbles, 64-bit, truncation), `format_dec` (zero, round numbers, max u64, powers of 10/2, boundaries), `DeviceInfo` construction, `scan_devices` with mock detectors (0/1/multiple devices, non-present skipping) |
| `uefi`/`efi` | 24 | Struct sizes under `repr(C)`, GUID byte values (all 5 GUIDs), `EFI_SUCCESS=0`, type consistency (UINTN=8 bytes, EFI_HANDLE=pointer size), GUID uniqueness, SNP mode layout, SNP transmit/receive function offsets, `EFI_SIMPLE_TEXT_INPUT_PROTOCOL` / `EFI_INPUT_KEY` sizes, PCI IO protocol struct sizes, `EFI_PCI_IO_PROTOCOL_WIDTH` enum values, boot service offset consistency |
| `arm64-bare`/`pci` | 21 | `pci_off` bit-field encoding (bus/dev/func/offset combinations, max values), `storage_name` mapping (all 12 subclass codes including unassigned, unknown fallback) |
| `romwrap` | 33 | PCIR signature/offset/fields, ROM header `0xAA55`/init vector, code type `0x03`/`0x00`, indicator `0x80`, 512-byte alignment, vendor/device ID passthrough, PE/COFF preservation, BIOS entry routine, block count consistency, PCIR length field matching, empty/boundary-aligned/overlapping payload sizes |

**Test architecture**: `common` tests run directly on the host. `uefi` tests run on the host by guarding platform-specific code with `#[cfg(not(test))]` and using `#[cfg_attr(not(test), no_std)]` / `#[cfg_attr(not(test), no_main)]` so the standard test harness can drive them. `arm64-bare` tests similarly guard the ARM64 `global_asm!` entry point and the `extern "C" fn main` behind `#[cfg(not(test))]`, exposing only pure functions (`pci_off`, `storage_name`) for host testing.

The `print` module separates formatting from I/O: `format_hex`/`format_dec` write to caller-provided byte buffers and return `&str`, while `print_hex`/`print_dec` call `puts` through the global `PUTC_FN` callback. Tests exercise the pure formatting functions directly, avoiding the `static mut` callback.

## Tools Required

- **Rust**: `rustc`, `cargo` with targets `x86_64-unknown-uefi`, `aarch64-unknown-uefi`, `aarch64-unknown-none`
- **Rust nightly**: needed for `i386-bios-rust` target (`-Zjson-target-spec -Zbuild-std=core`)
- **BIOS (C/NASM)**: `gcc`, `ld` (`-m elf_i386`), `nasm`, `objcopy`
- **Testing**: `qemu-system-x86_64` (with OVMF), `qemu-system-aarch64` (with `/usr/share/edk2/aarch64/QEMU_EFI.fd`)
