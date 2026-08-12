//! Interactive character-at-a-time REPL driver, shared by all targets.
//!
//! The loop itself is architecture-agnostic: input and output are injected as
//! plain function pointers (`get_key`/`putc`/`puts`), exactly like
//! `common::menu::show_menu`. Each target supplies its own drivers:
//! - UEFI: `get_key` (ReadKeyStroke), `u16_putc`, `u16_puts`
//! - BIOS: `serial::getc`, `print::putc`, `print::puts`
//! - ARM64 bare-metal: `uart::getc`, `print::putc`, `print::puts`

use crate::{eval, LuaState};

/// Run the interactive Lua REPL until the user enters `exit`.
///
/// `state` must be freshly created with [`LuaState::register_builtins`] and
/// (optionally) [`LuaState::set_fetch`] by the caller. `get_key` returns a
/// single byte per key press (or `None` when nothing is pending); `putc` echoes
/// typed characters and carries `print()` output; `puts` emits whole strings
/// (banner, prompt, error messages).
pub fn repl_loop(
    state: &mut LuaState,
    get_key: fn() -> Option<u8>,
    putc: fn(u8),
    puts: fn(&str),
) {
    puts("\nLua Shell (type 'exit' to return)\n");
    let mut buf = [0u8; 256];
    let mut len = 0u32;
    let mut exited = false;
    while !exited {
        puts("> ");
        loop {
            match get_key() {
                Some(b'\r') | Some(b'\n') => {
                    if len > 0 {
                        putc(b'\n');
                        match eval::run_repl_once(state, &buf[..len as usize], putc) {
                            Ok(eval::ExecResult::Normal) => {}
                            Ok(eval::ExecResult::Exit) => exited = true,
                            Ok(eval::ExecResult::Shell) => {
                                puts("\n(nested shell not supported)\n\n");
                            }
                            Ok(eval::ExecResult::Ret(_)) => {}
                            Err(e) => {
                                puts("Lua error: ");
                                puts(e);
                                putc(b'\n');
                            }
                        }
                        len = 0;
                    }
                    break;
                }
                Some(b'\x7f') | Some(b'\x08') => {
                    if len > 0 {
                        len -= 1;
                        putc(b'\x08');
                        putc(b' ');
                        putc(b'\x08');
                    }
                }
                Some(ch) if ch >= 0x20 && ch < 0x7f && (len as usize) < buf.len() - 1 => {
                    buf[len as usize] = ch;
                    len += 1;
                    putc(ch);
                }
                _ => {}
            }
        }
    }
    puts("\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::string::String;
    use std::thread_local;
    use std::vec::Vec;

    thread_local! {
        static KEYS: RefCell<Vec<u8>> = RefCell::new(Vec::new());
        static OUT: RefCell<Vec<u8>> = RefCell::new(Vec::new());
    }

    fn get_key() -> Option<u8> {
        KEYS.with(|k| k.borrow_mut().pop())
    }

    fn putc(c: u8) {
        OUT.with(|o| o.borrow_mut().push(c));
    }

    fn puts(s: &str) {
        OUT.with(|o| o.borrow_mut().extend_from_slice(s.as_bytes()));
    }

    /// Queue `keys` so `get_key` delivers them first-to-last (the mock pops
    /// from the end, so keys are pushed in reverse).
    fn feed(keys: &[u8]) {
        KEYS.with(|k| {
            let mut q = k.borrow_mut();
            q.clear();
            for &b in keys.iter().rev() {
                q.push(b);
            }
        });
    }

    /// Run a full REPL session with the given key sequence, returning output.
    fn run_session(keys: &[u8]) -> String {
        feed(keys);
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = LuaState::new();
        state.register_builtins(putc);
        state.set_fetch(None);
        repl_loop(&mut state, get_key, putc, puts);
        OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap())
    }

    #[test]
    fn prompt_and_execute() {
        let out = run_session(b"1 + 2\rprint(1 + 2)\rexit\r");
        assert!(out.contains("> "));
        // Both a bare expression and a real print() call print the value.
        assert!(out.contains("3\n"));
    }

    #[test]
    fn echo_and_backspace() {
        // Type "12", backspace once -> line is "1"; bare expr prints 1.
        let out = run_session(b"12\x7f\rprint(1)\rexit\r");
        // The erase sequence (\x08 space \x08) is emitted for the backspace.
        assert!(out.contains("\x08 \x08"));
        // The edited line "1" executes as a bare expression and prints 1.
        assert!(out.contains("\n1\n"));
        // The erased '2' must not appear on its own line after the erase.
        assert!(!out.contains("> 12\n"));
    }

    #[test]
    fn empty_enter_reprompts() {
        let out = run_session(b"\rexit\r");
        // Two prompts back to back (empty line does not exit).
        assert!(out.contains("> > "));
    }

    #[test]
    fn exit_stops() {
        let out = run_session(b"exit\r");
        assert_eq!(out.matches("> ").count(), 1);
    }

    #[test]
    fn error_continues() {
        let out = run_session(b"undefined_var\rprint(1)\rexit\r");
        assert!(out.contains("Lua error:"));
        assert!(out.contains("1"));
    }

    #[test]
    fn shell_message() {
        let out = run_session(b"shell\rprint(1)\rexit\r");
        assert!(out.contains("(nested shell not supported)"));
        assert!(out.contains("1"));
    }

    #[test]
    fn state_persists_across_lines() {
        let out = run_session(b"x = 42\rprint(x)\rexit\r");
        assert!(out.contains("42"));
    }
}
