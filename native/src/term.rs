//! Raw-terminal I/O and process helpers for the native Linux targets.
//!
//! Input is character-at-a-time via a raw terminal (termios set up with inline
//! syscalls — the workspace has no external crate dependencies). Output goes
//! through `common::print`. Shared by the `native` menu binary and the `lua`
//! shell/script binary.

use common::print;

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
fn sys_fcntl(fd: u64, cmd: u64, arg: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 72u64 => ret,
            in("rdi") fd,
            in("rsi") cmd,
            in("rdx") arg,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

#[repr(C)]
struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[inline]
fn sys_nanosleep(req: *const Timespec, rem: *mut Timespec) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 35u64 => ret,
            in("rdi") req as u64,
            in("rsi") rem as u64,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

const F_GETFL: u64 = 3;
const F_SETFL: u64 = 4;
const O_NONBLOCK: u64 = 0x800;

#[inline]
pub fn exit_group(code: u64) -> ! {
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

/// Restore the terminal and exit (used on Ctrl-C at the menu, `exit` in the
/// shell, EOF, and fatal signals).
pub fn shutdown(code: u64) -> ! {
    restore_termios();
    exit_group(code);
}

// ── Signal safety net ──────────────────────────────────────────────────────
//
// If the process dies by a signal (eg. an external `kill`, or a crash), the
// terminal would otherwise be left in raw mode and the user must type `reset`.
// A handler restores termios before exiting for every common termination path,
// so Ctrl-C/SIGTERM/SIGSEGV etc. always return the terminal to the shell.

#[repr(C)]
struct SigAction {
    handler: u64,
    flags: u64,
    restorer: u64,
    mask: u64,
}

extern "C" fn sig_cleanup(sig: i32) {
    restore_termios();
    exit_group(128 + sig as u64);
}

/// Signal-frame return trampoline (`rt_sigreturn`). Never reached by
/// [`sig_cleanup`] (which exits), but the kernel needs a valid restorer
/// pointer when `SA_RESTORER` is set.
extern "C" fn sig_restore_rt() {
    unsafe {
        core::arch::asm!(
            "syscall",
            in("rax") 15u64,
            clobber_abi("system"),
            options(noreturn, nostack),
        );
    }
}

#[inline]
fn sys_rt_sigaction(sig: u64, act: *const SigAction, oldact: *mut SigAction, size: u64) -> i64 {
    let ret: i64;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") 13u64 => ret,
            in("rdi") sig,
            in("rsi") act as u64,
            in("rdx") oldact as u64,
            in("r10") size,
            out("rcx") _,
            out("r11") _,
            options(nostack),
        );
    }
    ret
}

/// Install `sig_cleanup` for the common fatal signals so the terminal is
/// restored even if the process is terminated by a signal rather than by the
/// normal `shutdown` path. Must be called after `set_raw_mode` (which saves the
/// original termios).
fn install_signal_handlers() {
// SIGINT, SIGQUIT, SIGABRT, SIGBUS, SIGFPE, SIGSEGV, SIGPIPE, SIGTERM.
    let handler: extern "C" fn(i32) = sig_cleanup;
    let restorer: extern "C" fn() = sig_restore_rt;
    const SA_RESTORER: u64 = 0x0400_0000;
    let action = SigAction {
        handler: handler as u64,
        flags: SA_RESTORER,
        restorer: restorer as u64,
        mask: 0,
    };
    for sig in [2u64, 3, 6, 7, 8, 11, 13, 15] {
        sys_rt_sigaction(sig, &action, core::ptr::null_mut(), 8);
    }
}

// ── I/O callbacks ───────────────────────────────────────────────────────────

/// Low-level byte output to stdout (flushed), wired into `common::print`.
/// `\n` is translated to `\r\n` (matching the firmware targets' putc drivers)
/// because raw mode has OPOST off, so the terminal would otherwise not return
/// the cursor to column 0.
pub fn stdout_putc(c: u8) {
    use std::io::Write;
    let mut out = std::io::stdout();
    let _ = if c == b'\n' {
        out.write_all(b"\r\n")
    } else {
        out.write_all(&[c])
    };
    let _ = out.flush();
}

/// Byte output to stdout without `\n` translation, for runs where the terminal
/// stays in cooked mode (e.g. `bin/lua script.lua`, where the kernel already
/// handles newline translation and output may be redirected to a file).
pub fn raw_putc(c: u8) {
    use std::io::Write;
    let _ = std::io::stdout().write_all(&[c]);
}

/// Decodes terminal escape sequences (arrow keys / home / end / delete) into the
/// [`lua::repl`] key sentinels. Bytes arrive as a burst, so after `ESC` the
/// driver polls stdin non-blockingly.
static mut ESC: lua::repl::EscSeq = lua::repl::EscSeq::new();

/// Blocking single-byte input from stdin (raw mode), decoding modifier-key
/// escape sequences. Ctrl-D exits the program; Ctrl-C is passed through so the
/// shell can cancel the current line.
pub fn get_key() -> Option<u8> {
    let mut b = [0u8; 1];
    let n = sys_read(0, b.as_mut_ptr(), 1);
    if n != 1 {
        if n == 0 {
            shutdown(0);
        }
        return None;
    }
    let first = b[0];
    if first == 0x04 {
        shutdown(0);
    }
    if first != 0x1b {
        return Some(first);
    }

    // Decode an escape sequence: read the following bytes non-blockingly with
    // short sleeps, then restore the original blocking mode.
    let f = sys_fcntl(0, F_GETFL, 0);
    if f < 0 {
        return None;
    }
    let flags = f as u64;
    sys_fcntl(0, F_SETFL, flags | O_NONBLOCK);
    let esc = unsafe { &mut *core::ptr::addr_of_mut!(ESC) };
    esc.reset();
    let _ = esc.feed(0x1b);
    let mut result = None;
    let ts = Timespec {
        tv_sec: 0,
        tv_nsec: 10_000_000, // 10 ms
    };
    for _ in 0..16 {
        let mut c = [0u8; 1];
        if sys_read(0, c.as_mut_ptr(), 1) == 1 {
            match esc.feed(c[0]) {
                Some(k) => {
                    result = Some(k);
                    break;
                }
                None if !esc.in_progress() => break,
                None => {}
            }
        } else {
            let mut rem = Timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            sys_nanosleep(&ts, &mut rem);
        }
    }
    sys_fcntl(0, F_SETFL, flags);
    result
}

/// Set up raw-terminal input plus the signal and panic hooks that restore the
/// terminal. Call once at startup before any interactive input.
pub fn init() {
    print::init(stdout_putc);
    set_raw_mode();
    install_signal_handlers();
    std::panic::set_hook(Box::new(|_| {
        restore_termios();
    }));
}
