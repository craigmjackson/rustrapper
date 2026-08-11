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
                print::puts("\nLua Shell (type 'exit' to return)\n\n");
                let mut state = lua::LuaState::new();
                state.register_builtins(common::print::putc);
                state.set_fetch(None);
                let mut buf = [0u8; 256];
                let mut len = 0u32;
                loop {
                    let c = serial::getc();
                    match c {
                        Some(b'\r') | Some(b'\n') => {
                            if len > 0 {
                                print::putc(b'\n');
                                let line = &buf[..len as usize];
                                match lua::eval::run_repl_once(&mut state, line, common::print::putc) {
                                    Ok(lua::eval::ExecResult::Normal) => {}
                                    Ok(lua::eval::ExecResult::Exit) => break,
                                    Ok(lua::eval::ExecResult::Shell) => {
                                        print::puts("\n(nested shell not supported)\n\n");
                                    }
                                    Ok(lua::eval::ExecResult::Ret(_)) => {}
                                    Err(e) => {
                                        print::puts("Lua error: ");
                                        print::puts(e);
                                        print::putc(b'\n');
                                    }
                                }
                                len = 0;
                            }
                        }
                        Some(b'\x7f') | Some(b'\x08') => {
                            if len > 0 {
                                len -= 1;
                                print::putc(b'\x08');
                                print::putc(b' ');
                                print::putc(b'\x08');
                            }
                        }
                        Some(ch) if ch >= 0x20 && ch < 0x7f && len < buf.len() as u32 - 1 => {
                            buf[len as usize] = ch;
                            len += 1;
                            print::putc(ch);
                        }
                        _ => {}
                    }
                }
                print::puts("\n");
            }
        }
    }
}

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
