#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]
#![cfg_attr(test, allow(dead_code))]

mod serial;
mod vga;
mod pci;
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
use common::print;
#[cfg(not(test))]
use common::scan;

#[cfg(not(test))]
fn dual_putc(c: u8) {
    serial::putc(c);
    vga::putc(c);
}

/// `dhcp` builtin: set up the network (e1000 + DHCP) and return the `fetch`
/// callback if a TFTP server is reachable.
#[cfg(not(test))]
fn dhcp_fn() -> Option<fn(&str) -> Option<usize>> {
    if net::setup_fetch_context() {
        Some(crate::fetch::fetch_file)
    } else {
        None
    }
}

#[cfg(not(test))]
#[no_mangle]
pub extern "C" fn _start(_boot_drive: u32) -> ! {
    print::init(dual_putc);
    print::puts("\nRustrapper BIOS Stage2 (Rust)\n");
    pci::pci_print_all();
    loop {
        match show_menu(common::print::puts, common::print::putc, serial::getc) {
            MenuAction::StorageScan => {
                print::puts("\nStorage devices:\n");
                scan::scan_devices(pci::detect_device);
            }
            MenuAction::NetworkBoot => {
                print::puts("\n");
                net::scan_network();
            }
            MenuAction::LuaShell => {
                let mut state = lua::LuaState::new();
                state.register_builtins(common::print::putc);
                // Network is NOT set up automatically: the user runs the
                // `dhcp` command in the shell, which enables `fetch()`.
                state.set_fetch(None);
                state.set_dhcp(Some(dhcp_fn));
                state.set_load(Some(crate::fetch::load_file));
                lua::repl::repl_loop(&mut state, serial::getc, common::print::putc, common::print::puts);
            }
        }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
