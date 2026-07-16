# Rustrapper - Rust Bootloader
# Targets: all, x86_64-uefi, aarch64-uefi, aarch64-bare, i386-bios
# All code is Rust. The only non-Rust pieces are:
#   - bios/src/mbr.asm          (512-byte MBR, loads sector 1+ and jumps to it)
#   - bios/src/stage2_entry.nasm (enables A20, switches to PM, copies Rust payload
#     from 0x1200 to 0x100000, zeros BSS, calls Rust _start)

CARGO    := cargo
NASM     := nasm
BIN      := bin
BIOS_SRC := bios/src

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
        i386-bios run-i386-bios check-deps

all: x86_64-uefi aarch64-uefi aarch64-bare i386-bios x86_64-uefi-rom i386-bios-rom

# Create output directory
$(BIN):
	mkdir -p $(BIN)

# ── BIOS MBR (stage-1, 512 bytes, NASM) ──────────────────────────
# Loads 16 sectors (LBA 1-16, 8192 bytes) to 0x1000 and jumps there.
# The 512-byte stub at 0x1000 (stage2_entry.nasm) then takes over.
$(BIN)/bios.bin: TARGET := i386-bios
$(BIN)/bios.bin: $(BIOS_SRC)/mbr.asm | $(BIN) check-deps
	$(NASM) -f bin -o $@ $<

# ── BIOS Rust payload (32-bit protected mode, loaded at 1 MB) ────
# Requires nightly for -Zjson-target-spec and -Zbuild-std=core
BIOS_RUST_TARGET := $(CURDIR)/targets/i386-unknown-none.json
CARGO_NIGHTLY    := cargo +nightly

$(BIN)/rust_payload.bin: TARGET := i386-bios
$(BIN)/rust_payload.bin: $(shell find bios common -name '*.rs') \
                         bios/link.ld Cargo.toml $(BIOS_RUST_TARGET) | $(BIN) check-deps
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-arg=-T$(CURDIR)/bios/link.ld -C link-arg=-N" \
		$(CARGO_NIGHTLY) build -Zjson-target-spec -Zbuild-std=core \
		--target $(BIOS_RUST_TARGET) --package bios --release
	objcopy -O binary target/i386-unknown-none/release/bios $@

# Combined stage2 = entry stub + Rust payload (assembled by NASM, which
# incbins the payload so it can compute copy offsets at assembly time)
$(BIN)/stage2_entry.bin: $(BIN)/rust_payload.bin | $(BIN)
	cd $(BIOS_SRC) && $(NASM) -f bin -o ../../$@ stage2_entry.nasm

i386-bios: $(BIN)/bios.bin $(BIN)/stage2_entry.bin

# ── x86_64 UEFI ──────────────────────────────────────────────────
# The common crate does not need a separate -I path; Cargo handles deps
$(BIN)/rustrapper.efi: TARGET := x86_64-uefi
$(BIN)/rustrapper.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN) check-deps
	CARGO_TARGET_DIR=target RUSTFLAGS="-C link-args=/NODEFAULTLIB" \
		$(CARGO) build --target $(UEFI_TARGET) --package uefi --release
	cp target/$(UEFI_TARGET)/release/uefi.efi $@

# ── ARM64 UEFI ───────────────────────────────────────────────────
$(BIN)/rustrapper_arm64.efi: TARGET := aarch64-uefi
$(BIN)/rustrapper_arm64.efi: $(shell find uefi common -name '*.rs') Cargo.toml | $(BIN) check-deps
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
$(BIN)/rustrapper_arm64_bare.elf: TARGET := aarch64-bare
$(BIN)/rustrapper_arm64_bare.elf: $(shell find arm64-bare common -name '*.rs') \
                                arm64-bare/link.ld Cargo.toml | $(BIN) check-deps
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

run-i386-bios: TARGET := run-i386-bios
run-i386-bios: $(BIN)/bios.img check-deps
	qemu-system-x86_64 -drive file=$(BIN)/bios.img,format=raw -nic user,model=e1000 -nographic

run-x86_64-uefi: TARGET := run-x86_64-uefi
run-x86_64-uefi: $(BIN)/rustrapper.efi check-deps
	mkdir -p EFI/BOOT
	cp $(BIN)/rustrapper.efi EFI/BOOT/BOOTX64.EFI
	qemu-system-x86_64 -bios /usr/share/edk2/x64/OVMF.4m.fd \
		-drive file=fat:rw:.,format=raw -nic user,model=e1000 -nographic

run-x86_64-uefi-rom: TARGET := run-x86_64-uefi-rom
run-x86_64-uefi-rom: $(BIN)/rustrapper.efi $(BIN)/rustrapper_efi.rom check-deps
	mkdir -p EFI/BOOT
	cp $(BIN)/rustrapper.efi EFI/BOOT/BOOTX64.EFI
	qemu-system-x86_64 -bios /usr/share/edk2/x64/OVMF.4m.fd \
		-drive file=fat:rw:.,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_efi.rom,netdev=net0 \
		-nographic

run-i386-bios-rom: TARGET := run-i386-bios-rom
run-i386-bios-rom: $(BIN)/bios.img $(BIN)/rustrapper_bios.rom check-deps
	qemu-system-x86_64 -drive file=$(BIN)/bios.img,format=raw \
		-netdev user,id=net0 \
		-device e1000,romfile=$(BIN)/rustrapper_bios.rom,netdev=net0 \
		-nographic

run-aarch64-uefi: TARGET := run-aarch64-uefi
run-aarch64-uefi: $(BIN)/rustrapper_arm64.efi check-deps
	mkdir -p EFI/BOOT
	cp $< EFI/BOOT/BOOTAA64.EFI
	qemu-system-aarch64 -machine virt -cpu max \
		-bios /usr/share/edk2/aarch64/QEMU_EFI.fd \
		-drive file=fat:rw:.,format=raw -nic user,model=virtio-net-pci -nographic

test.img:
	qemu-img create -f raw $@ 64M 2>/dev/null || dd if=/dev/zero bs=1M count=64 of=$@ 2>/dev/null

run-aarch64-bare: TARGET := run-aarch64-bare
run-aarch64-bare: $(BIN)/rustrapper_arm64_bare.elf test.img check-deps
	qemu-system-aarch64 -M virt -cpu max -kernel $< \
		-drive file=test.img,format=raw,if=none,id=drive0 \
		-device ahci,id=ahci \
		-device ide-hd,bus=ahci.0,drive=drive0 \
		-nic user,model=e1000 -nographic

# ── Clean ────────────────────────────────────────────────────────
clean:
	rm -rf $(BIN)/* build/
	rm -rf target/ EFI/

# ── Dependency check ─────────────────────────────────────────────
# Usage:
#   make check-deps                    # check all build + run deps
#   make check-deps TARGET=i386-bios   # check deps for a specific target
check-deps:
	@TARGET="$(TARGET)" ; \
	MISSING="" ; \
	MISSING_RUST="" ; \
	ARCH_PKG="" ; DEB_PKG="" ; FED_PKG="" ; \
	NEED_NIGHTLY=0 ; \
	case "$$TARGET" in \
		i386-bios|i386-bios-rom|run-i386-bios|run-i386-bios-rom|"") \
			NEED_NIGHTLY=1 ;; \
		x86_64-uefi|x86_64-uefi-rom|run-x86_64-uefi|run-x86_64-uefi-rom) ;; \
		aarch64-uefi|run-aarch64-uefi) ;; \
		aarch64-bare|run-aarch64-bare) ;; \
		*) echo "Unknown target: $$TARGET"; exit 1 ;; \
	esac ; \
	if ! command -v cargo >/dev/null 2>&1; then \
		MISSING="$$MISSING cargo" ; \
	fi ; \
	if ! command -v objcopy >/dev/null 2>&1; then \
		MISSING="$$MISSING objcopy" ; \
		ARCH_PKG="$$ARCH_PKG binutils" ; DEB_PKG="$$DEB_PKG binutils" ; FED_PKG="$$FED_PKG binutils" ; \
	fi ; \
	if [ "$$NEED_NIGHTLY" = "1" ]; then \
		if ! command -v nasm >/dev/null 2>&1; then \
			MISSING="$$MISSING nasm" ; \
			ARCH_PKG="$$ARCH_PKG nasm" ; DEB_PKG="$$DEB_PKG nasm" ; FED_PKG="$$FED_PKG nasm" ; \
		fi ; \
		if ! cargo +nightly --version >/dev/null 2>&1; then \
			MISSING="$$MISSING rust-nightly" ; \
		fi ; \
	fi ; \
	case "$$TARGET" in \
		run-i386-bios|run-i386-bios-rom|run-x86_64-uefi|run-x86_64-uefi-rom|"") \
			if ! command -v qemu-system-x86_64 >/dev/null 2>&1; then \
				MISSING="$$MISSING qemu-system-x86_64" ; \
				ARCH_PKG="$$ARCH_PKG qemu-system-x86" ; DEB_PKG="$$DEB_PKG qemu-system-x86" ; FED_PKG="$$FED_PKG qemu-system-x86" ; \
			fi ;; \
	esac ; \
	case "$$TARGET" in \
		run-aarch64-uefi|run-aarch64-bare|"") \
			if ! command -v qemu-system-aarch64 >/dev/null 2>&1; then \
				MISSING="$$MISSING qemu-system-aarch64" ; \
				ARCH_PKG="$$ARCH_PKG qemu-system-aarch64" ; DEB_PKG="$$DEB_PKG qemu-system-arm" ; FED_PKG="$$FED_PKG qemu-system-arm" ; \
			fi ;; \
	esac ; \
	case "$$TARGET" in \
		run-x86_64-uefi|run-x86_64-uefi-rom|run-i386-bios|run-i386-bios-rom|"") \
			if [ ! -f /usr/share/edk2/x64/OVMF.4m.fd ]; then \
				MISSING="$$MISSING edk2-ovmf-x86" ; \
				ARCH_PKG="$$ARCH_PKG edk2-ovmf" ; DEB_PKG="$$DEB_PKG ovmf" ; FED_PKG="$$FED_PKG edk2-ovmf" ; \
			fi ;; \
	esac ; \
	case "$$TARGET" in \
		run-aarch64-uefi|"") \
			if [ ! -f /usr/share/edk2/aarch64/QEMU_EFI.fd ]; then \
				MISSING="$$MISSING edk2-ovmf-aarch64" ; \
				ARCH_PKG="$$ARCH_PKG edk2-aarch64" ; DEB_PKG="$$DEB_PKG qemu-efi-aarch64" ; FED_PKG="$$FED_PKG edk2-aarch64" ; \
			fi ;; \
	esac ; \
	if command -v rustup >/dev/null 2>&1; then \
		INSTALLED=$$(rustup target list --installed 2>/dev/null) ; \
		case "$$TARGET" in \
			x86_64-uefi|x86_64-uefi-rom|run-x86_64-uefi|run-x86_64-uefi-rom|"") \
				echo "$$INSTALLED" | grep -q x86_64-unknown-uefi || MISSING_RUST="$$MISSING_RUST x86_64-unknown-uefi" ;; \
		esac ; \
		case "$$TARGET" in \
			aarch64-uefi|run-aarch64-uefi|"") \
				echo "$$INSTALLED" | grep -q aarch64-unknown-uefi || MISSING_RUST="$$MISSING_RUST aarch64-unknown-uefi" ;; \
		esac ; \
		case "$$TARGET" in \
			aarch64-bare|run-aarch64-bare|"") \
				echo "$$INSTALLED" | grep -q aarch64-unknown-none || MISSING_RUST="$$MISSING_RUST aarch64-unknown-none" ;; \
		esac ; \
	fi ; \
	if [ -z "$$MISSING" ] && [ -z "$$MISSING_RUST" ]; then \
		echo "All dependencies satisfied." ; \
		exit 0 ; \
	fi ; \
	echo "Missing dependencies:" ; \
	echo ; \
	if [ -n "$$MISSING" ]; then \
		echo "  Tools:$$MISSING" ; \
		echo ; \
	fi ; \
	if [ -n "$$MISSING_RUST" ]; then \
		echo "  Rust targets:$$MISSING_RUST" ; \
		echo ; \
	fi ; \
	echo "Install instructions:" ; \
	echo ; \
	if [ -n "$$ARCH_PKG" ]; then \
		echo "  Arch Linux:" ; \
		echo "    pacman -S$$ARCH_PKG" ; \
		echo ; \
	fi ; \
	if [ -n "$$DEB_PKG" ]; then \
		echo "  Debian/Ubuntu:" ; \
		echo "    apt install$$DEB_PKG" ; \
		echo ; \
	fi ; \
	if [ -n "$$FED_PKG" ]; then \
		echo "  Fedora:" ; \
		echo "    dnf install$$FED_PKG" ; \
		echo ; \
	fi ; \
	echo "  Rust (all distros):" ; \
	if ! command -v cargo >/dev/null 2>&1; then \
		echo "    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" ; \
	fi ; \
	if [ -n "$$MISSING_RUST" ]; then \
		echo "    rustup target add$$MISSING_RUST" ; \
	fi ; \
	if [ "$$NEED_NIGHTLY" = "1" ]; then \
		echo "    rustup toolchain install nightly" ; \
	fi ; \
	exit 1
