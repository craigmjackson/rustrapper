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
                        let line = &buf[..len as usize];
                        if !handle_help(line, puts) {
                            match eval::run_repl_once(state, line, putc) {
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

/// One REPL command described in the `help` output.
struct HelpEntry {
    name: &'static str,
    short: &'static str,
    detail: &'static str,
}

const HELP_COMMANDS: &[HelpEntry] = &[
    HelpEntry {
        name: "help",
        short: "Show this help; 'help <cmd>' for details",
        detail: "help [command]\n\
                  No argument lists all REPL commands.\n\
                  'help <cmd>' shows detailed help for one command.\n",
    },
    HelpEntry {
        name: "exit",
        short: "Exit the Lua shell and return to the menu",
        detail: "exit\n\
                  Leaves the Lua shell and returns to the main menu.\n\
                  May also be written exit() in a script.\n",
    },
    HelpEntry {
        name: "print",
        short: "Print one or more values",
        detail: "print(v1, v2, ...)\n\
                  Prints values separated by tabs, followed by a newline.\n\
                  Example: print(1 + 2) -> 3\n",
    },
    HelpEntry {
        name: "fetch",
        short: "Download a file from the TFTP server",
        detail: "fetch(\"file\")\n\
                  Downloads 'file' from the TFTP server (DHCP next_server) and\n\
                  returns its byte count, or nil on failure.\n\
                  Requires a TFTP server: run 'dhcp' first to set up the\n\
                  network (or it works automatically in PXE scripts).\n",
    },
    HelpEntry {
        name: "dhcp",
        short: "Set up the network (e1000 + DHCP) so fetch() works",
        detail: "dhcp\n\
                  Runs the network setup: scans PCI for the e1000, initializes\n\
                  it, and runs DHCP. Returns true on success, false on failure.\n\
                  After it succeeds, fetch(\"file\") can download files from the\n\
                  TFTP server. May also be written dhcp().\n",
    },
    HelpEntry {
        name: "shell",
        short: "Enter a nested Lua shell",
        detail: "shell()\n\
                  Tries to enter a nested interactive shell.\n\
                  Not supported inside the REPL.\n",
    },
    HelpEntry {
        name: "lua",
        short: "Run Lua expressions and statements",
        detail: "lua\n\
                  Every line is evaluated as Lua: bare expressions print their\n\
                  result; statements (assignment, if/while/for, functions,\n\
                  tables) run normally.\n",
    },
];

/// Trim leading/trailing spaces and tabs from a byte slice.
fn trim(b: &[u8]) -> &[u8] {
    let mut s = 0;
    let mut e = b.len();
    while s < e && (b[s] == b' ' || b[s] == b'\t') {
        s += 1;
    }
    while e > s && (b[e - 1] == b' ' || b[e - 1] == b'\t') {
        e -= 1;
    }
    &b[s..e]
}

fn print_general_help(puts: fn(&str)) {
    puts("Commands:\n");
    for e in HELP_COMMANDS {
        puts("  ");
        puts(e.name);
        for _ in e.name.len()..8 {
            puts(" ");
        }
        puts(e.short);
        puts("\n");
    }
    puts("\nType 'help <cmd>' for details on a command.\n");
}

/// Intercept `help` / `help <cmd>` lines before they reach the Lua parser.
/// Returns `true` if the line was a help command (already printed), `false`
/// if it should be passed to the interpreter as normal input.
fn handle_help(line: &[u8], puts: fn(&str)) -> bool {
    let target: Option<&[u8]> = if line == &b"help"[..] {
        Some(&[])
    } else if line.starts_with(&b"help "[..]) {
        Some(trim(&line[5..]))
    } else {
        None
    };
    match target {
        None => return false,
        Some(sub) if sub.is_empty() => print_general_help(puts),
        Some(sub) => {
            let mut found = false;
            for e in HELP_COMMANDS {
                if e.name.as_bytes() == sub {
                    puts(e.detail);
                    found = true;
                }
            }
            if !found {
                puts("Unknown command: ");
                puts(core::str::from_utf8(sub).unwrap_or("?"));
                puts("\n");
            }
        }
    }
    true
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

    /// Mock `fetch()` host callback: returns a size for known names, `None`
    /// for anything else (simulating a TFTP download failure).
    fn mock_fetch(name: &str) -> Option<usize> {
        match name {
            "a.txt" => Some(5),
            "b.txt" => Some(12),
            _ => None,
        }
    }

    /// Run a full REPL session with a mock `fetch()` callback installed.
    fn run_session_with_fetch(keys: &[u8]) -> String {
        feed(keys);
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = LuaState::new();
        state.register_builtins(putc);
        state.set_fetch(Some(mock_fetch));
        repl_loop(&mut state, get_key, putc, puts);
        OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap())
    }

    /// Mock `dhcp()` host callback: network setup succeeds and enables `fetch`.
    fn mock_dhcp() -> Option<fn(&str) -> Option<usize>> {
        Some(mock_fetch)
    }

    /// Run a full REPL session starting with networking disabled but a `dhcp`
    /// callback installed (mirrors the on-device Lua shell entry).
    fn run_session_with_dhcp(keys: &[u8]) -> String {
        feed(keys);
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = LuaState::new();
        state.register_builtins(putc);
        state.set_fetch(None);
        state.set_dhcp(Some(mock_dhcp));
        repl_loop(&mut state, get_key, putc, puts);
        OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap())
    }

    #[test]
    fn dhcp_enables_fetch_in_repl() {
        // Before `dhcp`, fetch is unavailable.
        let out = run_session_with_dhcp(b"print(fetch(\"a.txt\"))\rexit\r");
        assert!(out.contains("Lua error: fetch not available"));
        // After `dhcp` (bare command), fetch works and stays enabled.
        let out = run_session_with_dhcp(b"dhcp\rprint(fetch(\"a.txt\"))\rexit\r");
        assert!(out.contains("true\n"));
        assert!(out.contains("5\n"));
        // The call form works too.
        let out = run_session_with_dhcp(b"print(dhcp())\rexit\r");
        assert!(out.contains("true\n"));
    }

    #[test]
    fn fetch_works_in_repl() {
        let out = run_session_with_fetch(b"print(fetch(\"a.txt\"))\rexit\r");
        assert!(out.contains("5\n"));
    }

    #[test]
    fn fetch_failure_in_repl() {
        // Download failure -> nil, does not kill the REPL.
        let out = run_session_with_fetch(b"print(fetch(\"missing.txt\"))\rexit\r");
        assert!(out.contains("nil\n"));
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

    #[test]
    fn global_works_in_repl() {
        // `global` is a keyword, so the line is parsed as a statement (not a
        // bare expression), and the global persists across REPL lines.
        let out = run_session(b"global g = 42\rprint(g)\rexit\r");
        assert!(out.contains("42"));
    }

    #[test]
    fn help_lists_commands() {
        let out = run_session(b"help\rexit\r");
        for cmd in ["help", "exit", "print", "fetch", "shell", "dhcp"] {
            assert!(out.contains(cmd), "missing '{}' in:\n{}", cmd, out);
        }
        assert!(out.contains("Type 'help <cmd>'"));
    }

    #[test]
    fn help_detail_for_command() {
        let out = run_session(b"help exit\rexit\r");
        assert!(out.contains("Leaves the Lua shell"));
    }

    #[test]
    fn help_unknown_command() {
        let out = run_session(b"help bogus\rexit\r");
        assert!(out.contains("Unknown command: bogus"));
    }

    #[test]
    fn help_does_not_break_lua() {
        let out = run_session(b"help\rprint(1 + 2)\rexit\r");
        assert!(out.contains("3\n"));
    }
}
