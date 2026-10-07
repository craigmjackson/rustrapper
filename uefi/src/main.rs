#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

#[cfg(test)]
extern crate std;

mod efi;
#[cfg(not(test))]
mod scan;
#[cfg(not(test))]
mod net;
#[cfg(not(test))]
mod mem;
#[cfg(not(test))]
mod loader;
#[cfg(not(test))]
mod fetch;

#[cfg(not(test))]
use core::panic::PanicInfo;

#[cfg(not(test))]
use common::menu::{show_menu, MenuAction};

#[cfg(not(test))]
use crate::efi::*;

#[cfg(not(test))]
pub static mut SYSTEM_TABLE: Option<&'static EFI_SYSTEM_TABLE> = None;

#[cfg(not(test))]
fn read_boot_svc_fn<T>(gbs: *const core::ffi::c_void, offset: usize) -> T {
    let ptr = (gbs as usize + offset) as *const *const core::ffi::c_void;
    unsafe { core::mem::transmute_copy(&*ptr) }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}

#[cfg(not(test))]
fn u16_puts(s: &str) {
    if let Some(st) = unsafe { SYSTEM_TABLE } {
        let con_out = unsafe { &*st.con_out };
        net::w16(con_out, s);
    }
}

#[cfg(not(test))]
pub fn u16_putc(c: u8) {
    let mut buf = [0u16; 2];
    buf[0] = c as u16;
    if let Some(st) = unsafe { SYSTEM_TABLE } {
        let con_out = unsafe { &*st.con_out };
        unsafe {
            (con_out.output_string)(con_out as *const _ as *mut _, buf.as_ptr());
        }
    }
}

// State for multi-byte escape sequences (ESC [ A etc.) that arrive across
// several ReadKeyStroke polls (serial-terminal input under -nographic).
// EscSeq maps them to the lua::repl key sentinels.
#[cfg(not(test))]
static mut ESC: lua::repl::EscSeq = lua::repl::EscSeq::new();

#[cfg(not(test))]
fn get_key() -> Option<u8> {
    loop {
        unsafe {
            let st = SYSTEM_TABLE?;
            let con_in = &*(st.con_in as *mut EFI_SIMPLE_TEXT_INPUT_PROTOCOL);
            let mut key = EFI_INPUT_KEY { scan_code: 0, unicode_char: 0 };
            let status = (con_in.read_key_stroke)(con_in as *const _ as *mut _, &mut key);
            if status != EFI_SUCCESS {
                return None;
            }
            let scan = key.scan_code;
            let unicode = key.unicode_char;
            let ch = unicode as u8;

            // Continue a partially-received escape sequence across polls.
            let esc = &mut *core::ptr::addr_of_mut!(ESC);
            if esc.in_progress() {
                return esc.feed(ch);
            }

            // EFI scan codes for keys without an ASCII encoding.
            let sentinel = match scan {
                0x01 => Some(lua::repl::KEY_UP),
                0x02 => Some(lua::repl::KEY_DOWN),
                0x03 => Some(lua::repl::KEY_RIGHT),
                0x04 => Some(lua::repl::KEY_LEFT),
                0x05 => Some(lua::repl::KEY_HOME),
                0x06 => Some(lua::repl::KEY_END),
                0x08 => Some(lua::repl::KEY_DELETE),
                _ => None,
            };
            if let Some(k) = sentinel {
                return Some(k);
            }

            // Backspace: unicode 0x08 / 0x7F (DEL).
            if ch == b'\x08' || ch == b'\x7F' {
                return Some(b'\x7F');
            }
            // Escape: unicode 0x1B or EDK2 SCAN_ESC (0x0017) starts a sequence.
            if ch == b'\x1B' || scan == 0x0017 {
                let esc = &mut *core::ptr::addr_of_mut!(ESC);
                let _ = esc.feed(0x1b);
                return None;
            }
            // Enter / line feeds must reach the REPL's enter handling.
            if ch == b'\r' || ch == b'\n' {
                return Some(ch);
            }
            // Control bytes the shell uses: Ctrl-C, Tab, Ctrl-L.
            if ch == b'\x03' || ch == b'\x09' || ch == b'\x0c' {
                return Some(ch);
            }
            if unicode > 0 && ch >= 0x20 && ch < 0x7F {
                return Some(ch);
            }
            return None;
        }
    }
}

#[cfg(not(test))]
#[export_name = "efi_main"]
pub extern "efiapi" fn efi_main(image_handle: EFI_HANDLE, system_table: &'static EFI_SYSTEM_TABLE) -> ! {
    unsafe { SYSTEM_TABLE = Some(system_table); }
    let con_out = unsafe { &*system_table.con_out };
    net::w16(con_out, "Rustrapper UEFI\r\n");

    // UEFI loaded images get a small firmware stack (the `LuaState` alone is
    // now ~100 KB: bytecode, per-thread stacks, and the coroutine pool). Run
    // the whole menu loop on a large custom stack allocated from Boot Services
    // on both architectures.
    {
        const BOOT_SVC_ALLOCATE_PAGES: usize = 0x28;
        const EFI_LOADER_DATA: u32 = 2;
        const STACK_PAGES: u64 = 256; // 1 MiB

        type AllocatePagesFn = unsafe extern "efiapi" fn(
            allocate_type: u32,
            memory_type: u32,
            pages: u64,
            memory: *mut u64,
        ) -> EFI_STATUS;

        let allocate_pages: AllocatePagesFn =
            read_boot_svc_fn(system_table.boot_services, BOOT_SVC_ALLOCATE_PAGES);
        let mut base: u64 = 0;
        let status = unsafe { allocate_pages(0, EFI_LOADER_DATA, STACK_PAGES, &mut base) };
        if status == EFI_SUCCESS && base != 0 {
            let top = (base as usize + STACK_PAGES as usize * 4096) & !15;
            #[cfg(target_arch = "aarch64")]
            unsafe {
                core::arch::asm!(
                    "mov sp, {top}",
                    "mov x0, {ih}",
                    "mov x1, {st}",
                    "b {func}",
                    top = in(reg) top,
                    ih = in(reg) image_handle as usize,
                    st = in(reg) system_table as *const _ as usize,
                    func = sym boot_loop,
                    options(noreturn),
                );
            }
            #[cfg(target_arch = "x86_64")]
            unsafe {
                // Windows x64 ABI (EFIAPI): args in rcx/rdx plus 32 bytes of
                // shadow space, 16-byte stack alignment.
                core::arch::asm!(
                    "mov rsp, {top}",
                    "sub rsp, 32",
                    "mov rcx, {ih}",
                    "mov rdx, {st}",
                    "jmp {func}",
                    top = in(reg) top,
                    ih = in(reg) image_handle as usize,
                    st = in(reg) system_table as *const _ as usize,
                    func = sym boot_loop,
                    options(noreturn),
                );
            }
        }
        boot_loop(image_handle, system_table);
    }
}

/// The main menu/action loop. Kept separate so `efi_main` can switch to a
/// large custom stack (ARM64 UEFI) before running it.
#[cfg(not(test))]
#[inline(never)]
fn boot_loop(image_handle: EFI_HANDLE, system_table: &'static EFI_SYSTEM_TABLE) -> ! {
    loop {
        match show_menu(u16_puts, u16_putc, get_key) {
            MenuAction::StorageScan => scan::scan_storage_devices(image_handle, system_table),
            MenuAction::NetworkBoot => net::scan_network_devices(image_handle, system_table),
            MenuAction::LuaShell => {
                let mut state = lua::LuaState::new();
                state.register_builtins(u16_putc);
                // Network is NOT set up automatically: the user runs the
                // `dhcp` command in the shell, which enables `fetch()`.
                state.set_fetch(None);
                state.set_dhcp(Some(dhcp_fn));
                state.set_load(Some(crate::fetch::load_file));
                state.set_dhcp_info(Some(crate::fetch::dhcp_info));
                state.set_dhcp_values(Some(crate::fetch::dhcp_values));
                lua::repl::repl_loop(&mut state, get_key, u16_putc, u16_puts);
            }
        }
    }
}

/// `dhcp` builtin: set up the network (e1000 + DHCP) and return the `fetch`
/// callback if a TFTP server is reachable.
#[cfg(not(test))]
fn dhcp_fn() -> Option<fn(source: &str, save_as: &str) -> Option<usize>> {
    if net::setup_fetch_context() {
        Some(crate::fetch::fetch_file)
    } else {
        None
    }
}
