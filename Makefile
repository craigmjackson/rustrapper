# Rustrapper - Rust Bootloader
# Targets: all, x86_64-uefi, aarch64-uefi, aarch64-bare, i386-bios
# All code is Rust. The only non-Rust pieces are:
#   - bios/mbr.asm          (512-byte MBR, loads sector 1+ and jumps to it)
#   - bios/stage2_entry.nasm (enables A20, switches to PM, copies Rust payload
#     from 0x1200 to 0x100000, zeros BSS, calls Rust _start)

CARGO    := cargo
NASM     := nasm
BIN      := bin
BIOS_SRC := bios

# x86_64 UEFI target
UEFI_TARGET   := x86_64-unknown-uefi
# ARM64 UEFI target (requires `rustup target add aarch64-unknown-uefi`)
UEFI_ARM64_TARGET := aarch64-unknown-uefi
# ARM64 bare-metal target
BARE_ARM64_TARGET := aarch64-unknown-none

.PHONY: all x86_64-uefi aarch64-uefi aarch64-bare clean \
        x86_64-uefi-rom i386-bios-rom \
        run-x86_64-uefi run-aarch64-uefi run-aarch64-bare \
        run-x86_64-uefi-rom run-i386-bios-rom \
        i386-bios run-i386-bios

all: x86_64-uefi aarch64-uefi aarch64-bare

# Create output directory
$(BIN):
	mkdir -p $(BIN)

# ── BIOS MBR (stage-1, 512 bytes, NASM) ──────────────────────────
# Loads 16 sectors (LBA 1-16, 8192 bytes) to 0x1000 and jumps there.
# The 512-byte stub at 0x1000 (stage2_entry.nasm) then takes over.
$(BIN)/bios.bin: $(BIOS_SRC)/mbr.asm | $(BIN)
	$(NASM) -f bin -o $@ $<

# ── BIOS Rust payload (32-bit protected mode, loaded at 1 MB) ────
# Requires nightly for -Zjson-target-spec -Zbuild-std=core
BIOS_RUST_TARGET := $(CURDIR)/targets/i386-unknown-none.json
CARGO_NIGHTLY    := cargo +nightly

$(BIN)/rust_payload.bin: $(shell find bios common -name '*.rs') \
                         bios/link.ld Cargo.toml $(BIOS_RUST_TARGET) | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-arg=-T$(CURDIR)/bios/link.ld -C link-arg=-N" \
		$(CARGO_NIGHTLY) build -Zjson-target-spec -Zbuild-std=core \
		--target $(BIOS_RUST_TARGET) --package bios --release
	objcopy -O binary target/i386-unknown-none/release/bios $@

# Combined stage2 = entry stub + Rust payload (assembled by NASM, which
# incbins the payload so it can compute copy offsets at assembly time)
$(BIN)/stage2_entry.bin: $(BIN)/rust_payload.bin | $(BIN)
	cd $(BIOS_SRC) && $(NASM) -f bin -o ../$@ stage2_entry.nasm

i386-bios: $(BIN)/bios.bin $(BIN)/stage2_entry.bin

# ── x86_64 UEFI ──────────────────────────────────────────────────
# The common crate does not need a separate -I path; Cargo handles deps
$(BIN)/rustrapper.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-args=/NODEFAULTLIB" \
		$(CARGO) build --target $(UEFI_TARGET) --package uefi --release
	cp target/$(UEFI_TARGET)/release/uefi.efi $@

# ── ARM64 UEFI ───────────────────────────────────────────────────
$(BIN)/rustrapper_arm64.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target \
		$(CARGO) build --target $(UEFI_ARM64_TARGET) --package uefi --release
	cp target/$(UEFI_ARM64_TARGET)/release/uefi.efi $@

x86_64-uefi: $(BIN)/rustrapper.efi
aarch64-uefi: $(BIN)/rustrapper_arm64.efi

# ── UEFI PCI Expansion ROM ──────────────────────────────────────
ROMWRAP_BIN := target/debug/romwrap

$(ROMWRAP_BIN): $(shell find romwrap -name '*.rs') Cargo.toml
	$(CARGO) build --package romwrap

$(BIN)/rustrapper_efi.rom: $(BIN)/rustrapper.efi $(ROMWRAP_BIN) | $(BIN)
	$(CARGO) run --package romwrap -- $(BIN)/rustrapper.efi $@

x86_64-uefi-rom: $(BIN)/rustrapper_efi.rom

# ── BIOS PCI Expansion ROM (from Rust payload) ─────────────────
# Wraps the raw 32-bit PM binary as a legacy BIOS option ROM.
# Like other BIOS option ROMs, the 3-byte init routine at 0x34 is a
# no-op (xor ax,ax; retf); the payload is just data. The Rust code
# actually runs from disk via the stage2 entry stub loaded by the MBR.
$(BIN)/rustrapper_bios.rom: $(BIN)/rust_payload.bin $(ROMWRAP_BIN) | $(BIN)
	$(CARGO) run --package romwrap -- $(BIN)/rust_payload.bin $@ --bios

i386-bios-rom: $(BIN)/rustrapper_bios.rom

# ── ARM64 Bare-metal ──────────────────────────────────────────────
$(BIN)/rustrapper_arm64_bare.elf: $(shell find arm64-bare common -name '*.rs') \
                                arm64-bare/link.ld Cargo.toml | $(BIN)
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-arg=-T$(CURDIR)/arm64-bare/link.ld -C link-arg=-N" \
		$(CARGO) build --target $(BARE_ARM64_TARGET) --package arm64-bare --release
	cp target/$(BARE_ARM64_TARGET)/release/arm64-bare $@

aarch64-bare: $(BIN)/rustrapper_arm64_bare.elf

# ── BIOS disk image (MBR + Rust stage2 entry + payload) ──────────
$(BIN)/bios.img: $(BIN)/bios.bin $(BIN)/stage2_entry.bin | $(BIN)
	dd if=/dev/zero bs=1M count=64 of=$@ 2>/dev/null
	dd if=$(BIN)/bios.bin of=$@ conv=notrunc 2>/dev/null
	dd if=$(BIN)/stage2_entry.bin of=$@ bs=512 seek=1 conv=notrunc 2>/dev/null

# ── Run targets ──────────────────────────────────────────────────
# All targets use -nographic (Ctrl-A X to exit). QEMU's e1000 I/O BAR is a
# stub (see AGENTS.md gotcha), so the Rust stage2's direct e1000 driver uses
# MMIO (PCI config via I/O ports 0xCF8/0xCFC + MMIO register access).

run-i386-bios: $(BIN)/bios.img
	qemu-system-x86_64 -drive file=$(BIN)/bios.img,format=raw -nic user,model=e1000 -nographic

run-x86_64-uefi: $(BIN)/rustrapper.efi
	mkdir -p EFI/BOOT
	cp $(BIN)/rustrapper.efi EFI/BOOT/BOOTX64.EFI
	qemu-system-x86_64 -bios /usr/share/edk2/x64/OVMF.4m.fd \
		-drive file=fat:rw:.,format=raw -nic user,model=e1000 -nographic

run-x86_64-uefi-rom: $(BIN)/rustrapper.efi $(BIN)/rustrapper_efi.rom
	mkdir -p EFI/BOOT
	cp $(BIN)/rustrapper.efi EFI/BOOT/BOOTX64.EFI
	qemu-system-x86_64 -bios /usr/share/edk2/x64/OVMF.4m.fd \
		-drive file=fat:rw:.,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_efi.rom,netdev=net0 \
		-nographic

run-i386-bios-rom: $(BIN)/bios.img $(BIN)/rustrapper_bios.rom
	qemu-system-x86_64 -drive file=$(BIN)/bios.img,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_bios.rom,netdev=net0 \
		-nographic

run-aarch64-uefi: $(BIN)/rustrapper_arm64.efi
	mkdir -p EFI/BOOT
	cp $< EFI/BOOT/BOOTAA64.EFI
	qemu-system-aarch64 -machine virt -cpu max \
		-bios /usr/share/edk2/aarch64/QEMU_EFI.fd \
		-drive file=fat:rw:.,format=raw -nic user,model=virtio-net-pci -nographic

test.img:
	qemu-img create -f raw $@ 64M 2>/dev/null || dd if=/dev/zero bs=1M count=64 of=$@ 2>/dev/null

run-aarch64-bare: $(BIN)/rustrapper_arm64_bare.elf test.img
	qemu-system-aarch64 -M virt -cpu max -kernel $< \
		-drive file=test.img,format=raw,if=none,id=drive0 \
		-device ahci,id=ahci \
		-device ide-hd,bus=ahci.0,drive=drive0 \
		-nic user,model=e1000 -nographic

# ── Clean ────────────────────────────────────────────────────────
clean:
	rm -rf $(BIN)/* build/
	rm -rf target/ EFI/
