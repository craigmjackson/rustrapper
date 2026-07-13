# Rustrapper Hybrid BIOS/UEFI Bootloader

## Overview

Produces legacy BIOS (MBR+stage2) binaries, x86_64 UEFI and ARM64 EFI applications, ARM64 bare-metal binaries, and PCI expansion ROMs. On startup, all variants present a menu to choose between scanning storage devices or booting from network (DHCP).

## Directory Structure

```
.
├── Cargo.toml              # Workspace root (5 member crates)
├── Makefile                # Build orchestration (make all, make run-*)
├── AGENTS.md               # This file
├── bios/                   # Minimal NASM for BIOS boot
│   ├── mbr.asm             # 16-bit MBR stage-1 (loads sector 1+ and jumps to it)
│   └── stage2_entry.nasm   # Entry stub for Rust stage2 (A20, PM, copy to 1 MB)
├── bios-rust/              # Rust 32-bit BIOS stage2 (nightly only)
│   ├── Cargo.toml
│   ├── link.ld             # Link at 0x100000
│   ├── targets/i386-unknown-none.json  # Bare-metal i386 target spec
│   └── src/
│       ├── main.rs         # _start entry, menu dispatch
│       ├── serial.rs       # COM1 driver (putc, getc, flush)
│       ├── vga.rs          # VGA text-mode driver with scrolling
│       ├── pci.rs          # PCI scan via I/O ports 0xCF8/0xCFC
│       └── net.rs          # PCI + e1000 scan, DHCP (thin wrapper over common)
├── common/                 # Rust no_std library (shared by all targets)
│   ├── Cargo.toml
│   └── src/
│       ├── lib.rs
│       ├── menu.rs         # Shared [1]/[2] menu logic
│       ├── print.rs        # Callback-based print (putc/puts/print_hex/print_dec)
│       ├── scan.rs         # Generic device-scan loop
│       ├── e1000.rs        # Direct MMIO e1000 driver (init/send/recv) — used by all 3 targets
│       └── dhcp.rs         # DHCP frame build/parse, IP checksum, DhcpConfig
├── uefi/                   # Rust UEFI binary (x86_64 + ARM64)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # efi_main entry point
│       ├── efi.rs          # Hand-typed EFI types, GUIDs, Boot Services offsets
│       ├── scan.rs         # UEFI device scan (LocateHandleBuffer + OpenProtocol)
│       └── net.rs          # SNP + 4-tier e1000 DHCP (uses common::e1000 + common::dhcp)
├── arm64-bare/             # Rust ARM64 bare-metal binary (no firmware)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs         # Entry (global_asm! start), UART init, PCI scan
│       ├── pci.rs          # PCI ECAM walk, BAR sizing/enable, AHCI probe
│       ├── net.rs          # PCI + e1000 scan, DHCP (thin wrapper over common)
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

- DHCP frame build/parse and the direct MMIO e1000 driver live in `common::e1000` and `common::dhcp` (shared with BIOS and ARM64 bare-metal). This file contains only the UEFI-specific glue.
- `print_network_info` was removed (dead code). All NIC detection uses `scan_e1000_devices` with a 4-tier fallback chain.
- `pci_read_config32` is cross-platform: x86_64 uses I/O ports (`outl`/`inl` at `0xCF8`/`0xCFC`); ARM64 uses ECAM MMIO at `0x4010_0000_0000`.
- `DirectMmioE1000` is a thin wrapper around `common::e1000` (~30 lines): `init()` calls `common::e1000::init`, `send()` calls `common::e1000::send`, `dhcp_run()` builds the DISCOVER via `common::dhcp::build_discover`, sends, receives via `common::e1000::try_receive`, and parses via `common::dhcp::parse_response`. The output adapter for `con_out` lives in `print_mac`/`print_ip`/`w16`/`put_dec` helpers.
- SNP driver start: accepts both `EFI_SUCCESS`, `EFI_ALREADY_STARTED`, and the ARM64 firmware's error-bit-encoded variant (`0x8000000000000014`). ARM64 virtio SNP returns the latter.
- SNP initialize: failure is non-fatal. ARM64 virtio SNP returns `EFI_UNSUPPORTED` (with error bit).
- `send_udp_dhcp` builds the Ethernet/IP/UDP/DHCP frame via `common::dhcp::build_eth_ip_udp` and calls `SNP.Transmit`.
- `try_receive` calls `SNP.Receive` with non-null `SrcAddr`, `DestAddr`, and `Protocol` output pointers; some firmware requires them.
- The ARM64 virtio SNP only supports one transmit per session (no buffer recycling without `Initialize`), so `send_udp_dhcp` sends one DHCPDISCOVER and accepts the OFFER as final. x86_64 e1000 works normally with full DISCOVER→OFFER→REQUEST→ACK.

### `arm64-bare/src/main.rs` — ARM64 bare-metal entry

- Entry point via `global_asm!`: sets stack pointer, enables FP/SIMD access via `CPACR_EL1`, zeroes BSS, calls `main`.
- **FP/SIMD must be enabled before any Rust code runs**: `CPACR_EL1.FPEN` bits (20-21) default to trapping. rustc's compiler-generated `memset`/`memcpy` intrinsics use NEON instructions for buffers over a few hundred bytes; without `orr x9, x9, #(3 << 20)` / `msr cpacr_el1, x9` / `isb` in the boot stub, any such call takes a synchronous exception. Since no exception vector table is installed (`VBAR_EL1` defaults to 0), the CPU silently loops forever fetching garbage at address `0x200` (the "current EL, SPx, synchronous" vector offset) — this looks exactly like a hang with no error output.
- UART output via MMIO PL011 at `0x09000000` (QEMU virt machine).
- PCI ECAM at `0x4010_0000_00` (QEMU virt v11+).
- Load address: `0x4020_0000` (above DTB at `0x4000_0000`).
- Presents the `[1]/[2]` menu via `common::menu::show_menu`, reading from the PL011 UART (`uart::getc`), and dispatches to `scan::scan_devices` or `net::scan_network` based on the choice.

### `arm64-bare/src/net.rs` — ARM64 bare-metal e1000 NIC + DHCP

- Thin wrapper around `common::e1000` and `common::dhcp`. All e1000 register access, descriptor ring setup, and DHCP frame build/parse are in `common/`; this file only handles the PCI scan, output, and glue.
- `scan_network` walks PCI bus 0 for network class (0x02) devices, enables BARs, calls `e1000_common::init(bar0 as u64)`, runs DHCP, prints IP/subnet/gateway to the PL011 UART.
- `dhcp_run` builds a DISCOVER via `dhcp::build_discover`, sends via `e1000_common::send`, polls via `e1000_common::try_receive` (100M iterations, ~1 second), and parses via `dhcp::parse_response`.
- `print_mac` and `print_ip` are tiny local helpers that format MAC/IP to the global print sink.
- Uses `-nic user,model=e1000` in `run-aarch64-bare` (not `-net none`); QEMU's e1000 emulation works normally with full ARP/DHCP over user-mode (slirp) networking on both x86_64 and aarch64 hosts.

### e1000 / DHCP details (shared `common::e1000` and `common::dhcp`)

The following details apply to the common e1000 driver used by all three targets (BIOS, ARM64, UEFI DirectMmioE1000):

- Descriptor rings (`RxDescs`/`TxDescs`) are `#[repr(align(16))]` wrapper structs around `#[repr(C, packed)]` descriptor arrays; QEMU's e1000 requires 16-byte-aligned ring base addresses for `RDBAL`/`TDBAL`.
- **`RDLEN`/`TDLEN` minimum is 128 bytes** on QEMU's e1000 model — rings smaller than 8 descriptors (16 bytes each) are silently rejected (register reads back as the last valid value, not what was written).
- **`RDT` must be set to `NUM_RX_DESC - 1` after init**, not `0`. `RDH == RDT` means zero descriptors are owned by hardware (empty ring) — a very easy off-by-one that results in silently receiving nothing.
- All RX descriptors point at a single shared `RX_BUF` (2048 bytes); fine for the single in-flight DHCP transaction this driver performs, but would corrupt data under concurrent multi-packet traffic.
- DHCP RX polling must be RAM-only status checks (`RX_DESCS.0[idx].status & RX_STATUS_DD`), not MMIO register polling (`reg_read32(base, REG_RDH)`) in a tight loop — repeated MMIO reads under QEMU TCG are drastically slower than cached RAM reads and can make a poll loop take far longer than intended for the same iteration count.
- The RX poll loop needs roughly 100 million iterations (~1 second of wall time) for QEMU's slirp (`-nic user`) DHCP server to respond; smaller counts (e.g. 2 million, ~0.1s) reliably miss the OFFER even though the packet is delivered on the wire (verified with `-object filter-dump,netdev=net0,file=...` producing a valid pcap showing both DISCOVER and OFFER).

### `arm64-bare/src/pci.rs` — PCI/AHCI probe

- Walks PCI bus 0, enumerates all devices.
- For each mass-storage class device, calls `enable_bars` (BAR sizing via all-1s write, MMIO from `0x3E00_0000`) and `probe_ahci`.
- `probe_ahci` enables HBA (`GHC.AE`, bit 31 of ABAR+0x04), reads CAP/PI/SSTS to detect attached drives.
- `pci_read_config` / `pci_write_config`: MMIO ECAM access via pointer dereference.

### `common/src/menu.rs` — Shared menu logic

- `MenuAction` enum: `StorageScan` / `NetworkBoot`.
- `show_menu(puts, putc, get_key)` prints `[1]/[2]` and polls `get_key` until the user presses a valid choice.
- `puts`/`putc` are function pointers, so the same function can be used with ASCII (UART/BIOS) and UEFI (wrapped to u16 `OutputString`) output.

### `bios/mbr.asm` — 16-bit MBR stage-1

- 512-byte MBR that loads 16 sectors (LBA 1-16, 8192 bytes) to physical `0x1000` and jumps to `0x0100:0x0000`.
- The DAP (Disk Address Packet) at line 69 sets the sector count to 16. Increase this if the entry stub + payload grows past 16 sectors.
- "MZ" at offset 0 executes as `dec bp; pop dx`, which overwrites DL (the boot-drive value). Always set DL explicitly before `int 0x13`.

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

- Thin wrapper around `common::e1000` and `common::dhcp`. All e1000 register access, descriptor ring setup, and DHCP frame build/parse are in `common/`; this file only handles the PCI scan, output, and glue.
- `scan_network` walks PCI bus 0 for network class (0x02) devices, enables BARs, calls `e1000_common::init(bar0 as u64)`, runs DHCP, prints IP/subnet/gateway to the global print sink (serial + VGA).
- `dhcp_run` builds a DISCOVER via `dhcp::build_discover`, sends via `e1000_common::send`, polls via `e1000_common::try_receive` (100M iterations, ~1 second), and parses via `dhcp::parse_response`.
- `print_mac` and `print_ip` are tiny local helpers (12 and 9 lines) that format MAC/IP to the global print sink.

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
- Architecture-specific detection is provided by `detect_device` via a function pointer or trait.

### `common/src/e1000.rs` — Direct MMIO e1000 driver (shared by all targets)

- Register offsets as `u64` constants (`REG_CTRL`, `REG_STATUS`, `REG_RDBAL`, etc.) — works on both 32-bit and 64-bit targets (32-bit targets cast the BAR0 `u32` to `u64` at the call site).
- `reg_read32(base, reg)`, `reg_write32(base, reg, val)` — raw MMIO via `read_volatile`/`write_volatile` on `(base + reg) as *const u32`.
- `read_mac(base) -> [u8; 6]` — reads the MAC from the Receive Address registers.
- `init(base) -> Option<[u8; 6]>` — reset, wait for link, set MAC, clear multicast, set up RX/TX descriptor rings and RCTL/TCTL, return MAC on success or `None` on failure.
- `send(base, data: &[u8]) -> bool` — copy frame to `TX_BUF`, fill descriptor with `EOP|IFCS|RS`, ring TDT, poll status DD bit.
- `try_receive(base, buf: &mut [u8; 1514], timeout_iters: u64) -> Option<usize>` — poll RX descriptors for DD, copy to `buf`, re-arm, return length.
- Descriptor rings (`RxDescs`/`TxDescs`) are `#[repr(align(16))]` wrapper structs around `#[repr(C, packed)]` descriptor arrays; field access uses `&raw const/&raw mut` + `addr_of!` to avoid "reference to field of packed struct is unaligned" errors on all targets.

### `common/src/dhcp.rs` — DHCP frame build/parse (shared by all targets)

- `DhcpConfig` struct: `yiaddr`, `subnet`, `gateway` — the result of a successful DHCP exchange.
- `ip_checksum(buf) -> u16` — standard Internet checksum (one's complement of one's complement sum of 16-bit words).
- `build_discover(xid, mac) -> [u8; 300]` — DHCPDISCOVER payload with magic cookie 0x63825363, option 53=1 (DISCOVER), option 55=param request list (1/3/6), option 255=end.
- `build_eth_ip_udp(mac, dhcp_payload, dhcp_len, frame) -> usize` — wraps the DHCP payload in Ethernet + IPv4 + UDP headers (including IP checksum), returns total frame length.
- `parse_response(buf, len, xid, mac) -> Option<DhcpConfig>` — validates EtherType, IP protocol, UDP offset, magic cookie, transaction ID, MAC match, then walks options for type 53 (OFFER or ACK), type 1 (subnet), type 3 (gateway).

### `romwrap/src/main.rs` — PCI expansion ROM wrapper

- Wraps a binary into a PCI expansion ROM container usable by UEFI firmware (OVMF) or legacy BIOS (SeaBIOS).
- ROM header at offset 0: `0xAA55` signature, init vector (0x0000 for UEFI, non-zero for BIOS), size in 512-byte blocks at offset 0x04, PCIR offset at `0x18`.
- **PCI Data Structure (PCIR)**: `"PCIR"` signature, vendor/device IDs (default `0x8086`/`0x100E` for Intel e1000), code type `0x03` (EFI) or `0x00` (PC-AT/BIOS), indicator `0x80` (last image), revision `0x00`.
- Total header size: 52 bytes (28 B ROM header + 24 B PCIR structure).
- For BIOS: a 3-byte entry routine (`xor ax,ax; retf` = `0x33 0xC0 0xCB`) at offset `0x34` sets CF=0 for success and returns. Init vector points to offset `0x34`. Payload follows at offset `0x37`.
- For UEFI: PE/COFF binary follows headers at offset `0x34`. Init vector is 0 (not used by UEFI firmware).
- Output is padded to the next 512-byte boundary (file size is always a multiple of 512).
- Supports `--vendor=` and `--device=` flags for custom PCI IDs.
- Supports `--bios` flag for legacy BIOS ROMs (accepts any binary, not just PE/COFF).

### `Makefile` — Build orchestration

- Top-level targets: `all`, `x86_64-uefi`, `aarch64-uefi`, `aarch64-bare`, `x86_64-uefi-rom`, `i386-bios-rom`, `run-*`, `clean`.
- Uses `CARGO_TARGET_DIR=target` and per-target `RUSTFLAGS` for UEFI (needs `/NODEFAULTLIB` on x86_64).
- `i386-bios` target requires nightly (`-Zjson-target-spec -Zbuild-std=core`). Produces `bin/stage2_entry.bin` (entry stub + incbinned Rust payload) and `bin/rust_payload.bin` (raw binary).
- `run-i386-bios` uses `-nographic` (matching all other BIOS targets) so Ctrl-A X exits cleanly. The Rust stage2 also writes to VGA text mode, so it works with `-display curses` too (but Ctrl-A X won't work in curses mode — kill the process instead).

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
16. **MBR sector count must match payload**: The MBR loads 16 sectors (8192 bytes). The entry stub + Rust payload is ~7.8 KB = 15 sectors. Increase the `dap` sector count in `mbr.asm` if the payload grows further.
17. **PCI Bus Master must be enabled for e1000 DMA**: SeaBIOS enables Memory Space (bit 1) but may NOT enable Bus Master (bit 2) in the PCI Command register. Without Bus Master, the e1000 can read descriptors (MMIO works) but TX descriptor write-back (DMA) silently fails — TDH advances but the status DD bit is never set. `pci_enable_bars` must always set command bits 0-2 (I/O + Memory + Bus Master), even if some are already set. Do NOT re-assign BARs if firmware already assigned them (check if BAR0 is non-zero before sizing).

### MBR

9. **"MZ" corrupts DL**: The MBR's first 2 bytes are `0x4D 0x5A`, which execute as `dec bp; pop dx`. The `pop dx` overwrites the boot-drive value in DL. Always set DL explicitly before INT 13h calls.
10. **512-byte MBR limit**: The MBR is strictly 512 bytes (including `0xAA55` at `0x1FE`). All real logic goes in the stage2 entry stub.

### ARM64 / UEFI

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

### QEMU e1000

23. **QEMU e1000 I/O BAR is a stub**: QEMU's classic e1000 PCI I/O handler (`e1000_io_read`/`e1000_io_write`) is an empty stub — it returns 0 for all reads and ignores all writes. This means the direct e1000 driver in any code that uses the PCI I/O BAR (not the MMIO BAR) for register access will NOT work on QEMU. On real hardware, the I/O BAR software access protocol provides full register access. For QEMU testing, use `make run-x86_64-uefi` (UEFI path works in QEMU) or `make run-i386-bios` (BIOS uses MMIO + PCI I/O port config).

### UEFI keyboard / menu

41. **UEFI keyboard input is non-blocking**: `EFI_SIMPLE_TEXT_INPUT_PROTOCOL.ReadKeyStroke` returns `EFI_NOT_READY` when no key is pressed. The menu polls it in a tight loop until a valid choice is received. Store the system table pointer globally so the polling function can reach `con_in` without threading it through every helper.

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
make i386-bios                   # Build BIOS stage2 (nightly only) — MBR + entry stub + Rust payload
make x86_64-uefi                 # Build x86_64 UEFI binary
make aarch64-uefi                # Build ARM64 UEFI binary
make aarch64-bare                # Build Rust ARM64 bare-metal
make x86_64-uefi-rom             # Build UEFI PCI expansion ROM from rustrapper.efi
make i386-bios-rom               # Build BIOS PCI expansion ROM from rust_payload.bin
make run-i386-bios                # BIOS stage2 (serial, Ctrl-A X to exit)
make run-i386-bios-rom            # Legacy BIOS with PCI expansion ROM (rust_payload.bin)
make run-x86_64-uefi             # x86_64 UEFI via FAT dir (e1000, full DHCP)
make run-x86_64-uefi-rom         # x86_64 UEFI with custom PCI expansion ROM
make run-aarch64-uefi            # ARM64 UEFI from FAT directory
make run-aarch64-bare            # ARM64 bare-metal + AHCI drive
make clean                       # Clean all artifacts (keeps SeaBIOS checkout)
make x86_64-seabios-clean        # Remove SeaBIOS checkout
```

### `i386-bios-rom` — BIOS option ROM

- Wraps `bin/rust_payload.bin` (raw 32-bit PM binary) as a legacy BIOS PCI expansion ROM via `romwrap --bios`. Produces `bin/rustrapper_bios.rom` (~15 blocks / ~7.7 KB for the current payload).
- Like all BIOS option ROMs, the 3-byte init routine at 0x34 is a no-op (`xor ax,ax; retf`). SeaBIOS acknowledges the ROM and continues to boot from the disk image.
- The Rust payload at 0x37+ is just data — the actual Rust execution comes from the MBR → stage2 entry stub → Rust code at 0x100000 on the disk, not from the ROM.
- The `run-i386-bios-rom` target boots `bin/bios.img` with the ROM loaded on an e1000 NIC via `-device e1000,romfile=...`. Useful for testing that the option ROM is correctly detected by SeaBIOS.
- Default PCI IDs are `0x8086:0x100E` (Intel e1000). Use `romwrap --vendor=... --device=...` for custom IDs.

## Targets

| Target                          | Arch   | Firmware          | Binary                           |
| ------------------------------- | ------ | ----------------- | -------------------------------- |
| `x86_64-unknown-uefi`           | x86_64 | UEFI              | `bin/rustrapper.efi`               |
| `aarch64-unknown-uefi`          | ARM64  | UEFI              | `bin/rustrapper_arm64.efi`         |
| `aarch64-unknown-none`          | ARM64  | None (bare-metal) | `bin/rustrapper_arm64_bare.elf`    |
| `x86_64-linux-gnu` (romwrap)    | Host   | —                 | `target/debug/romwrap`           |
| `i386-unknown-none` (BIOS)      | i386   | BIOS (32-bit PM)  | `bin/rust_payload.bin`, `bin/stage2_entry.bin` |
| `i386-unknown-none` (Rust BIOS) | i386   | BIOS (32-bit PM)  | `bin/rust_payload.bin`, `bin/stage2_entry.bin` |
| PCI Option ROM (UEFI)           | x86_64 | UEFI (ROM)        | `bin/rustrapper_efi.rom`         |
| PCI Option ROM (BIOS, C)        | x86-16 | BIOS (ROM)        | `bin/rustrapper_bios.rom`        |
| PCI Option ROM (BIOS)           | x86-32 | BIOS (ROM)        | `bin/rustrapper_bios.rom`        |

## Tests

Run the full host-testable suite:

```bash
cargo test --workspace        # All 122 tests across all crates
cargo test --package common   # 44 tests (formatting, scan loop, DHCP build/parse)
cargo test --package uefi     # 24 tests (type sizes, GUID values, constants, SNP mode layout)
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
- **Rust nightly**: needed for `i386-bios` target (`-Zjson-target-spec -Zbuild-std=core`)
- **BIOS**: `nasm` (assembles the MBR and entry stub), `objcopy` (strips the Rust ELF to a flat binary)
- **Testing**: `qemu-system-x86_64` (with OVMF), `qemu-system-aarch64` (with `/usr/share/edk2/aarch64/QEMU_EFI.fd`)
