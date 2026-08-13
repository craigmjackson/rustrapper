//! Rustrapper native Linux x86_64 target.
//!
//! Runs the same menu and Lua interpreter as the firmware targets, but the
//! kernel handles networking (std UDP sockets, `/proc/net/route`) instead of
//! the direct e1000 driver. Useful for fast iteration on the Lua interpreter.
//!
//! Input is character-at-a-time via a raw terminal (termios set up with inline
//! syscalls — the workspace has no external crate dependencies).

use common::menu::{show_menu, MenuAction};
use common::print;

mod fetch;
mod net;

// ── Raw-terminal helpers (Linux x86_64 syscalls via inline asm) ────────────

const TCGETS: u64 = 0x5401;
const TCSETS: u64 = 0x5402;
// c_cc indices within termios (after c_line at byte 16)
const CC_VTIME: usize = 16 + 5;
const CC_VMIN: usize = 16 + 6;
// c_lflag bits
const ISIG: u32 = 0x0001;
const ICANON: u32 = 0x0002;
const ECHO: u32 = 0x0008;
const IEXTEN: u32 = 0x8000;
// c_oflag bits
const OPOST: u32 = 0x0001;

static mut SAVED_TERMIOS: [u8; 64] = [0; 64];

#[inline]
fn sys_read(fd: u64, buf: *mut u8, len: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 0u64 => ret,
            in("rdi") fd,
            in("rsi") buf as u64,
            in("rdx") len,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[inline]
fn sys_ioctl(fd: u64, req: u64, arg: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 16u64 => ret,
            in("rdi") fd,
            in("rsi") req,
            in("rdx") arg,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[inline]
fn exit_group(code: u64) -> ! {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 231u64,
            in("rdi") code,
            options(noreturn, nostack),
        );
    }
}

/// Switch stdin to raw mode (no echo, no line buffering, byte-at-a-time) and
/// save the original termios so it can be restored. ISIG is cleared so Ctrl-C
/// is delivered as a literal 0x03 byte that `get_key` handles by exiting.
fn set_raw_mode() {
    unsafe {
        let mut t = [0u8; 64];
        if sys_ioctl(0, TCGETS, t.as_mut_ptr() as u64) < 0 {
            return;
        }
        SAVED_TERMIOS = t;
        // c_lflag at offset 12: clear ISIG, ICANON, ECHO, IEXTEN.
        let lflag = u32::from_ne_bytes([t[12], t[13], t[14], t[15]]);
        t[12..16].copy_from_slice(&(lflag & !(ISIG | ICANON | ECHO | IEXTEN)).to_ne_bytes());
        // c_oflag at offset 4: clear OPOST (no \n -> \r\n translation).
        let oflag = u32::from_ne_bytes([t[4], t[5], t[6], t[7]]);
        t[4..8].copy_from_slice(&(oflag & !OPOST).to_ne_bytes());
        // Blocking byte-at-a-time reads.
        t[CC_VTIME] = 0;
        t[CC_VMIN] = 1;
        sys_ioctl(0, TCSETS, t.as_mut_ptr() as u64);
    }
}

fn restore_termios() {
    let saved = core::ptr::addr_of_mut!(SAVED_TERMIOS) as *mut u8 as u64;
    sys_ioctl(0, TCSETS, saved);
}

/// Restore the terminal and exit (used on Ctrl-C and on EOF).
fn shutdown(code: u64) -> ! {
    restore_termios();
    exit_group(code);
}

// ── I/O callbacks ───────────────────────────────────────────────────────────

/// Low-level byte output to stdout (flushed), wired into `common::print`.
/// `\n` is translated to `\r\n` (matching the firmware targets' putc drivers)
/// because raw mode has OPOST off, so the terminal would otherwise not return
/// the cursor to column 0.
fn stdout_putc(c: u8) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = if c == b'\n' {
        out.write_all(b"\r\n")
    } else {
        out.write_all(&[c])
    };
    let _ = out.flush();
}

/// Blocking single-byte input from stdin (raw mode). Exits on Ctrl-C / Ctrl-D.
fn get_key() -> Option<u8> {
    let mut b = [0u8; 1];
    let n = sys_read(0, b.as_mut_ptr(), 1);
    if n == 1 {
        if b[0] == 0x03 {
            // Ctrl-C (ISIG off, delivered as a literal byte)
            shutdown(130);
        }
        if b[0] == 0x04 {
            // Ctrl-D (raw mode delivers it as a literal byte)
            shutdown(0);
        }
        Some(b[0])
    } else if n == 0 {
        shutdown(0);
    } else {
        None
    }
}

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

fn main() {
    print::init(stdout_putc);
    set_raw_mode();
    std::panic::set_hook(Box::new(|_| {
        restore_termios();
    }));
    print::puts("\nRustrapper Native (Linux x86_64)\n");

    loop {
        match show_menu(print::puts, print::putc, get_key) {
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
                lua::repl::repl_loop(&mut state, get_key, print::putc, print::puts);
            }
        }
    }
}
