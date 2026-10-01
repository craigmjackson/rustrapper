//! Rustrapper native Linux x86_64 target.
//!
//! Runs the same menu and Lua interpreter as the firmware targets, but the
//! kernel handles networking (std UDP sockets, `/proc/net/route`) instead of
//! the direct e1000 driver. Useful for fast iteration on the Lua interpreter.
//!
//! The raw-terminal, networking, and `fetch` glue lives in the `native`
//! library (`native/src/lib.rs`), shared with the standalone `bin/lua` shell.

use common::menu::{show_menu, MenuAction};
use common::print;
use native::{fetch, net, term};

// ── Storage scan ────────────────────────────────────────────────────────────

static mut DESC_BUF: [u8; 64] = [0; 64];

/// Detect block devices by enumerating `/sys/block`.
fn native_detect_device(index: usize, info: &mut common::scan::DeviceInfo) -> bool {
    let mut names: Vec<String> = Vec::new();
    if let Ok(rd) = std::fs::read_dir("/sys/block") {
        for e in rd.flatten() {
            names.push(e.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    let name = match names.get(index) {
        Some(n) => n,
        None => return false,
    };
    let sys = format!("/sys/block/{}", name);
    info.index = index as u8;
    info.present = true;
    info.removable = std::fs::read_to_string(format!("{}/removable", sys))
        .map(|s| s.trim() == "1")
        .unwrap_or(false);
    info.block_size = std::fs::read_to_string(format!("{}/queue/logical_block_size", sys))
        .ok()
        .and_then(|s| s.trim().parse::<u16>().ok())
        .unwrap_or(512);
    info.block_count = std::fs::read_to_string(format!("{}/size", sys))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    unsafe {
        let desc_buf = &mut *core::ptr::addr_of_mut!(DESC_BUF);
        let bytes = name.as_bytes();
        let n = bytes.len().min(desc_buf.len() - 1);
        desc_buf[..n].copy_from_slice(&bytes[..n]);
        info.description = Some(core::str::from_utf8(&desc_buf[..n]).unwrap_or("?"));
    }
    true
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// `get_key` for the main menu: unlike the Lua shell (where Ctrl-C cancels the
/// current line), Ctrl-C at the menu quits the program.
fn menu_get_key() -> Option<u8> {
    match term::get_key() {
        Some(0x03) => term::shutdown(130),
        k => k,
    }
}

fn main() {
    term::init();
    print::puts("\nRustrapper Native (Linux x86_64)\n");

    loop {
        match show_menu(print::puts, print::putc, menu_get_key) {
            MenuAction::StorageScan => {
                print::puts("\nStorage devices:\n");
                common::scan::scan_devices(native_detect_device);
            }
            MenuAction::NetworkBoot => {
                print::puts("\n");
                net::network_boot();
            }
            MenuAction::LuaShell => {
                let mut state = lua::LuaState::new();
                state.register_builtins(print::putc);
                state.set_fetch(None);
                state.set_dhcp(Some(net::dhcp_fn));
                state.set_load(Some(fetch::load_file));
                state.set_dhcp_info(Some(fetch::dhcp_info));
                state.set_dhcp_values(Some(fetch::dhcp_values));
                lua::repl::repl_loop(&mut state, term::get_key, print::putc, print::puts);
            }
        }
    }
}
