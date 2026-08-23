//! Interactive character-at-a-time REPL driver, shared by all targets.
//!
//! The loop itself is architecture-agnostic: input and output are injected as
//! plain function pointers (`get_key`/`putc`/`puts`), exactly like
//! `common::menu::show_menu`. Each target supplies its own drivers:
//! - UEFI: `get_key` (ReadKeyStroke), `u16_putc`, `u16_puts`
//! - BIOS: `serial::getc`, `print::putc`, `print::puts`
//! - ARM64 bare-metal: `uart::getc`, `print::putc`, `print::puts`
//!
//! The drivers decode arrow/function keys into the [`KEY_UP`]…[`KEY_DELETE`]
//! sentinel bytes (values above printable ASCII). The shell also understands
//! the control bytes 0x03 (Ctrl-C = cancel the current chunk), 0x09 (Tab =
//! complete a name), and 0x0c (Ctrl-L = clear the screen). Features: full line
//! editing with a cursor, a fixed command history navigated with Up/Down,
//! tab completion of builtin/global names, and multiline continuation
//! (`>> ` prompt) for incomplete Lua input.

use crate::lex::{Lexer, Tok};
use crate::{eval, LuaState};

// ── Key sentinels ───────────────────────────────────────────────────────────

/// Up-arrow / previous history (`ESC [ A`). All sentinels are > 0x7E so they
/// can never collide with printable input.
pub const KEY_UP: u8 = 0x80;
/// Down-arrow / next history (`ESC [ B`).
pub const KEY_DOWN: u8 = 0x81;
/// Right-arrow / move cursor right (`ESC [ C`).
pub const KEY_RIGHT: u8 = 0x82;
/// Left-arrow / move cursor left (`ESC [ D`).
pub const KEY_LEFT: u8 = 0x83;
/// Home (`ESC [ H` / `ESC [ 1 ~` / `ESC [ 7 ~`).
pub const KEY_HOME: u8 = 0x84;
/// End (`ESC [ F` / `ESC [ 4 ~` / `ESC [ 8 ~`).
pub const KEY_END: u8 = 0x85;
/// Forward-delete (`ESC [ 3 ~`); 0x7f / 0x08 remain backspace.
pub const KEY_DELETE: u8 = 0x86;

/// Incremental ANSI escape-sequence parser used by the per-target input
/// drivers. Feed raw bytes one at a time; `feed` returns the decoded key (a
/// passthrough byte when no sequence is pending, or a [`KEY_*`] sentinel once
/// a sequence completes) or `None` while a sequence is being assembled or after
/// a lone/malformed `ESC` is aborted.
pub struct EscSeq {
    pub buf: [u8; 8],
    pub n: usize,
}

impl EscSeq {
    pub const fn new() -> Self {
        EscSeq { buf: [0; 8], n: 0 }
    }

    /// True while an escape sequence is partially received.
    pub fn in_progress(&self) -> bool {
        self.n > 0
    }

    pub fn reset(&mut self) {
        self.n = 0;
    }

    /// Feed one raw byte into the sequence machine.
    pub fn feed(&mut self, b: u8) -> Option<u8> {
        if self.n == 0 {
            if b == 0x1b {
                self.buf[0] = b;
                self.n = 1;
                return None;
            }
            return Some(b);
        }
        if self.n >= self.buf.len() {
            self.n = 0;
            return None;
        }
        self.buf[self.n] = b;
        self.n += 1;
        let seq = &self.buf[..self.n];
        if let Some(k) = map_escape(seq) {
            self.n = 0;
            return Some(k);
        }
        if is_escape_prefix(seq) {
            return None;
        }
        self.n = 0;
        None
    }
}

/// Map a complete (or candidate) modifier-key escape sequence to a key
/// sentinel. `seq` must start with `ESC` (0x1b).
pub fn map_escape(seq: &[u8]) -> Option<u8> {
    if seq.len() < 2 || seq[0] != 0x1b {
        return None;
    }
    match seq[1] {
        b'O' if seq.len() == 3 => match seq[2] {
            b'A' => Some(KEY_UP),
            b'B' => Some(KEY_DOWN),
            b'C' => Some(KEY_RIGHT),
            b'D' => Some(KEY_LEFT),
            _ => None,
        },
        b'[' if seq.len() >= 3 => {
            let c = seq[2];
            if seq.len() == 3 {
                match c {
                    b'A' => Some(KEY_UP),
                    b'B' => Some(KEY_DOWN),
                    b'C' => Some(KEY_RIGHT),
                    b'D' => Some(KEY_LEFT),
                    b'H' => Some(KEY_HOME),
                    b'F' => Some(KEY_END),
                    // `[N...` variants need more bytes.
                    b'1' | b'3' | b'4' | b'7' | b'8' => None,
                    _ => None,
                }
            } else if seq.len() == 4 {
                match (c, seq[3]) {
                    (b'1', b'~') => Some(KEY_HOME),
                    (b'7', b'~') => Some(KEY_HOME),
                    (b'4', b'~') => Some(KEY_END),
                    (b'8', b'~') => Some(KEY_END),
                    (b'3', b'~') => Some(KEY_DELETE),
                    _ => None,
                }
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Whether `seq` could still grow into a valid escape sequence.
fn is_escape_prefix(seq: &[u8]) -> bool {
    if seq.is_empty() || seq[0] != 0x1b {
        return false;
    }
    if seq.len() == 1 {
        return true;
    }
    if seq[1] != b'[' && seq[1] != b'O' {
        return false;
    }
    if seq.len() >= 3 {
        let last = seq[seq.len() - 1];
        if last == b'~' || last.is_ascii_alphabetic() {
            return false;
        }
    }
    true
}

// ── Buffer sizes ────────────────────────────────────────────────────────────

const LINE_CAP: usize = 256;
const CHUNK_CAP: usize = 512;
const HIST_MAX: usize = 12;
const HIST_CAP: usize = 512;

/// Run the interactive Lua REPL until the user enters `exit`.
///
/// `state` must be freshly created with [`LuaState::register_builtins`] and
/// (optionally) [`LuaState::set_fetch`] / set_dhcp etc. by the caller.
/// `get_key` returns a decoded byte per key press (printable byte, the
/// [`KEY_*`] sentinels, or one of the control bytes described above), or `None`
/// when nothing is pending.
pub fn repl_loop(
    state: &mut LuaState,
    get_key: fn() -> Option<u8>,
    putc: fn(u8),
    puts: fn(&str),
) {
    puts("\nLua Shell (type 'exit' to return)\n");

    let mut acc = [0u8; CHUNK_CAP];
    let mut acc_len = 0usize;
    let mut line = [0u8; LINE_CAP];
    let mut len = 0usize;
    let mut pos = 0usize;
    // Number of line characters currently displayed (used to erase stale text
    // when a redraw makes the line shorter — see `redraw_line`).
    let mut shown = 0usize;

    let mut hist = [[0u8; HIST_CAP]; HIST_MAX];
    let mut hist_n = 0usize;
    let mut hist_pos = 0usize;
    let mut draft_acc = [0u8; CHUNK_CAP];
    let mut draft_len = 0usize;
    let mut draft_line = [0u8; LINE_CAP];
    let mut draft_line_len = 0usize;

    let mut exited = false;
    while !exited {
        puts(if acc_len == 0 { "> " } else { ">> " });
        'keys: loop {
            match get_key() {
                Some(b'\r') | Some(b'\n') => {
                    // Bare Enter on the first, empty line just reprompts.
                    if acc_len == 0 && len == 0 {
                        break 'keys;
                    }
                    putc(b'\n');
                    // Append the current line to the accumulated chunk.
                    for &b in &line[..len] {
                        if acc_len < CHUNK_CAP {
                            acc[acc_len] = b;
                            acc_len += 1;
                        }
                    }
                    if acc_len < CHUNK_CAP {
                        acc[acc_len] = b'\n';
                        acc_len += 1;
                    }
                    len = 0;
                    pos = 0;
                    shown = 0;
                    if is_complete(&acc[..acc_len]) {
                        let mut clen = acc_len;
                        while clen > 0 && acc[clen - 1] == b'\n' {
                            clen -= 1;
                        }
                        let content = &acc[..clen];
                        if !content.is_empty() {
                            if !handle_repl_cmd(content, putc, puts) {
                                match eval::run_repl_once(state, content, putc) {
                                    Ok(eval::ExecResult::Normal) => {}
                                    Ok(eval::ExecResult::Break) => {}
                                    Ok(eval::ExecResult::Goto(_)) => {}
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
                            record_history(&mut hist, &mut hist_n, content);
                        }
                        acc_len = 0;
                        hist_pos = hist_n;
                        draft_len = 0;
                        draft_line_len = 0;
                        break 'keys;
                    }
                    puts(">> ");
                }

                Some(KEY_UP) => {
                    if hist_n == 0 {
                        continue;
                    }
                    if hist_pos == hist_n {
                        // First navigation: save the chunk being edited.
                        draft_len = 0;
                        for &b in &acc[..acc_len] {
                            draft_acc[draft_len] = b;
                            draft_len += 1;
                        }
                        draft_line_len = 0;
                        for &b in &line[..len] {
                            draft_line[draft_line_len] = b;
                            draft_line_len += 1;
                        }
                    }
                    if hist_pos > 0 {
                        hist_pos -= 1;
                        load_entry(&mut acc, &mut acc_len, &mut line, &mut len, &hist[hist_pos]);
                        pos = len;
                        render_recalled(putc, puts, &acc[..acc_len], &line[..len], &mut shown);
                    }
                }
                Some(KEY_DOWN) => {
                    if hist_pos < hist_n {
                        hist_pos += 1;
                        if hist_pos == hist_n {
                            // Back to the bottom: restore the saved draft.
                            acc_len = draft_len;
                            acc[..acc_len].copy_from_slice(&draft_acc[..acc_len]);
                            len = draft_line_len;
                            line[..len].copy_from_slice(&draft_line[..len]);
                            pos = len;
                        } else {
                            load_entry(&mut acc, &mut acc_len, &mut line, &mut len, &hist[hist_pos]);
                            pos = len;
                        }
                        render_recalled(putc, puts, &acc[..acc_len], &line[..len], &mut shown);
                    }
                }

                Some(KEY_LEFT) => {
                    if pos > 0 {
                        pos -= 1;
                        putc(b'\x08');
                    }
                }
                Some(KEY_RIGHT) => {
                    if pos < len {
                        putc(line[pos]);
                        pos += 1;
                    }
                }
                Some(KEY_HOME) => {
                    pos = 0;
                    redraw_line(putc, puts, prompt(acc_len), &line, len, pos, &mut shown);
                }
                Some(KEY_END) => {
                    pos = len;
                    redraw_line(putc, puts, prompt(acc_len), &line, len, pos, &mut shown);
                }
                Some(KEY_DELETE) => {
                    if pos < len {
                        for i in pos..len - 1 {
                            line[i] = line[i + 1];
                        }
                        len -= 1;
                        redraw_line(putc, puts, prompt(acc_len), &line, len, pos, &mut shown);
                    }
                }
                Some(b'\x7f') | Some(b'\x08') => {
                    if pos > 0 {
                        pos -= 1;
                        for i in pos..len - 1 {
                            line[i] = line[i + 1];
                        }
                        len -= 1;
                        redraw_line(putc, puts, prompt(acc_len), &line, len, pos, &mut shown);
                    }
                }

                Some(0x03) => {
                    // Ctrl-C: cancel the current chunk and reprompt.
                    if acc_len > 0 || len > 0 {
                        puts("^C\n");
                        acc_len = 0;
                        len = 0;
                        pos = 0;
                        shown = 0;
                        draft_len = 0;
                        draft_line_len = 0;
                        hist_pos = hist_n;
                    }
                    break 'keys;
                }
                Some(0x0c) => {
                    // Ctrl-L: clear the screen (form feed).
                    putc(b'\x0c');
                    redraw_line(putc, puts, prompt(acc_len), &line, len, pos, &mut shown);
                }
                Some(0x09) => {
                    complete(
                        state,
                        &mut line,
                        &mut len,
                        &mut pos,
                        putc,
                        puts,
                        prompt(acc_len),
                        &mut shown,
                    );
                }

                Some(ch) if ch >= 0x20 && ch < 0x7f && len < LINE_CAP => {
                    for i in (pos..len).rev() {
                        line[i + 1] = line[i];
                    }
                    line[pos] = ch;
                    len += 1;
                    putc(ch);
                    for &b in &line[pos + 1..len] {
                        putc(b);
                    }
                    for _ in 0..(len - pos - 1) {
                        putc(b'\x08');
                    }
                    pos += 1;
                    shown = len;
                }
                _ => {}
            }
        }
    }
    puts("\n");
}

fn prompt(acc_len: usize) -> &'static str {
    if acc_len == 0 {
        "> "
    } else {
        ">> "
    }
}

/// Redraw the current line in place: carriage return, erase the previously
/// displayed content (which may be longer than the new line after an edit),
/// then reprint the prompt and the line. `shown` tracks the most recent
/// displayed line length so erasing always covers the old text.
fn redraw_line(
    putc: fn(u8),
    puts: fn(&str),
    pr: &str,
    line: &[u8],
    len: usize,
    pos: usize,
    shown: &mut usize,
) {
    putc(b'\r');
    for _ in 0..(*shown).max(len) + pr.len() {
        putc(b' ');
    }
    putc(b'\r');
    puts(pr);
    for &b in &line[..len] {
        putc(b);
    }
    for _ in 0..len - pos {
        putc(b'\x08');
    }
    *shown = len;
}

/// After an in-place edit at `pos`, print the (shorter) tail and settle the
/// cursor back at `pos`.
#[allow(dead_code)]
fn redraw_tail(putc: fn(u8), line: &[u8], len: usize, pos: usize) {
    for &b in &line[pos..len] {
        putc(b);
    }
    putc(b' ');
    putc(b'\x08');
    for _ in 0..(len + 1 - pos) {
        putc(b'\x08');
    }
}

// ── History ─────────────────────────────────────────────────────────────────

fn record_history(hist: &mut [[u8; HIST_CAP]; HIST_MAX], hist_n: &mut usize, content: &[u8]) {
    if content.is_empty() || content.len() > HIST_CAP {
        return;
    }
    if *hist_n > 0 {
        let last = &hist[*hist_n - 1];
        let last_len = last.iter().position(|&b| b == 0).unwrap_or(HIST_CAP);
        if &last[..last_len] == content {
            return;
        }
    }
    if *hist_n >= HIST_MAX {
        for i in 1..HIST_MAX {
            hist[i - 1] = hist[i];
        }
        *hist_n = HIST_MAX - 1;
    }
    let entry = &mut hist[*hist_n];
    entry.fill(0);
    entry[..content.len()].copy_from_slice(content);
    *hist_n += 1;
}

/// Load a history entry into the chunk (`acc`) + active line (`line`). A
/// single-line entry lands in `line` with an empty `acc`; a multi-line entry
/// splits so all but the last line land in `acc` (each followed by `\n`).
fn load_entry(
    acc: &mut [u8; CHUNK_CAP],
    acc_len: &mut usize,
    line: &mut [u8; LINE_CAP],
    len: &mut usize,
    entry: &[u8; HIST_CAP],
) {
    *acc_len = 0;
    let entry_len = entry.iter().position(|&b| b == 0).unwrap_or(HIST_CAP);
    let bytes = &entry[..entry_len];
    match bytes.iter().rposition(|&b| b == b'\n') {
        None => {
            let n = entry_len.min(LINE_CAP);
            line[..n].copy_from_slice(&bytes[..n]);
            *len = n;
        }
        Some(last_nl) => {
            for &b in &bytes[..=last_nl] {
                if *acc_len < CHUNK_CAP {
                    acc[*acc_len] = b;
                    *acc_len += 1;
                }
            }
            let n = (entry_len - last_nl - 1).min(LINE_CAP);
            line[..n].copy_from_slice(&bytes[last_nl + 1..last_nl + 1 + n]);
            *len = n;
        }
    }
}

/// Render a recalled history entry: in place for a single-line entry, otherwise
/// on fresh lines under `> ` / `>> ` prompts.
fn render_recalled(
    putc: fn(u8),
    puts: fn(&str),
    acc: &[u8],
    line: &[u8],
    shown: &mut usize,
) {
    if acc.is_empty() {
        redraw_line(putc, puts, "> ", line, line.len(), line.len(), shown);
        return;
    }
    putc(b'\n');
    let mut i = 0;
    let mut first = true;
    while i < acc.len() {
        let seg_end = match acc[i..].iter().position(|&b| b == b'\n') {
            Some(p) => i + p,
            None => acc.len(),
        };
        puts(if first { "> " } else { ">> " });
        for &b in &acc[i..seg_end] {
            putc(b);
        }
        putc(b'\n');
        first = false;
        i = seg_end + 1;
    }
    puts(">> ");
    for &b in line {
        putc(b);
    }
    *shown = line.len();
}

// ── Multiline completeness ──────────────────────────────────────────────────

/// Whether an accumulated chunk is complete Lua (vs. needing continuation).
fn is_complete(src: &[u8]) -> bool {
    let mut lex = Lexer::new(src);
    let mut depth: i32 = 0;
    let mut delims: i32 = 0;
    let mut last: Option<Tok> = None;
    loop {
        match lex.next_token() {
            Ok(Tok::Eof) => break,
            Ok(t) => {
                match t {
                    Tok::If | Tok::While | Tok::For | Tok::Function | Tok::Repeat => depth += 1,
                    Tok::End | Tok::Until => depth -= 1,
                    Tok::LParen | Tok::LBracket | Tok::LBrace => delims += 1,
                    Tok::RParen | Tok::RBracket | Tok::RBrace => delims -= 1,
                    _ => {}
                }
                last = Some(t);
            }
            // Unterminated string / bad token: still building.
            Err(_) => return false,
        }
    }
    if depth > 0 || delims > 0 {
        return false;
    }
    if depth < 0 || delims < 0 {
        return true;
    }
    !matches!(
        last,
        Some(Tok::Plus)
            | Some(Tok::Minus)
            | Some(Tok::Star)
            | Some(Tok::Slash)
            | Some(Tok::Percent)
            | Some(Tok::Equals)
            | Some(Tok::Comma)
            | Some(Tok::DotDot)
            | Some(Tok::Semi)
            | Some(Tok::And)
            | Some(Tok::Or)
            | Some(Tok::Not)
            | Some(Tok::Lt)
            | Some(Tok::Le)
            | Some(Tok::Gt)
            | Some(Tok::Ge)
            | Some(Tok::EqEq)
            | Some(Tok::Neq)
    )
}

// ── Tab completion ──────────────────────────────────────────────────────────

fn is_ident(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Tab-complete the word before the cursor against the global names (builtins
/// are registered as globals, plus the `help` command).
fn complete(
    state: &LuaState,
    line: &mut [u8; LINE_CAP],
    len: &mut usize,
    pos: &mut usize,
    putc: fn(u8),
    puts: fn(&str),
    pr: &str,
    shown: &mut usize,
) {
    let mut start = *pos;
    while start > 0 && is_ident(line[start - 1]) {
        start -= 1;
    }
    let word = &line[start..*pos];

    // Only a handful of names can match; gather up to 64.
    let mut indices = [0usize; 64];
    let mut n = 0usize;
    for g in 0..state.nglobals as usize {
        let name = state.str_bytes(state.globals[g].name);
        if name.starts_with(word) && n < indices.len() {
            indices[n] = g;
            n += 1;
        }
    }
    // The `help` command is intercepted, not a Lua global.
    let help_matches = b"help".starts_with(word);

    if n == 0 && !help_matches {
        return;
    }

    // Single unambiguous match -> insert the remainder and redraw.
    let candidate: Option<&[u8]> = if n == 1 && !help_matches {
        let idx = indices[0];
        Some(state.str_bytes(state.globals[idx].name))
    } else if n == 0 && help_matches {
        Some(b"help")
    } else {
        None
    };
    if let Some(name_bytes) = candidate {
        let suffix = &name_bytes[word.len()..];
        if !suffix.is_empty() && *len + suffix.len() <= LINE_CAP {
            let old_len = *len;
            let old_pos = *pos;
            for j in (old_pos..old_len).rev() {
                line[j + suffix.len()] = line[j];
            }
            for (k, &sb) in suffix.iter().enumerate() {
                line[old_pos + k] = sb;
            }
            *len = old_len + suffix.len();
            *pos = old_pos + suffix.len();
            // Print the inserted chars plus the shifted tail, then walk the
            // cursor back to the new position.
            for &b in suffix {
                putc(b);
            }
            for i in (old_pos + suffix.len())..*len {
                putc(line[i]);
            }
            for _ in 0..(old_len - old_pos) {
                putc(b'\x08');
            }
            *shown = *len;
        }
        return;
    }

    // Multiple matches: list them and reprompt.
    putc(b'\n');
    puts("Completions:\n");
    for &idx in &indices[..n] {
        puts("  ");
        puts(core::str::from_utf8(state.str_bytes(state.globals[idx].name)).unwrap_or("?"));
        putc(b'\n');
    }
    if help_matches {
        puts("  help\n");
    }
    redraw_line(putc, puts, pr, line, *len, *pos, shown);
}

// ── Repl commands (help, clear) ─────────────────────────────────────────────

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
        name: "clear",
        short: "Clear the screen",
        detail: "clear\n\
                  Clears the display (same as Ctrl-L).\n",
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
        name: "dofile",
        short: "Run a Lua chunk loaded by name (TFTP) and return its value",
        detail: "dofile(\"file.lua\")\n\
                  Loads a Lua source chunk from the TFTP server, executes it,\n\
                  and returns the chunk's return value (nil if it doesn't\n\
                  return). The chunk shares globals but has its own locals.\n\
                  Requires a TFTP server: run 'dhcp' first.\n",
    },
    HelpEntry {
        name: "ls",
        short: "List the files downloaded with fetch()",
        detail: "ls\n\
                  Lists every file successfully downloaded with fetch(\"file\"),\n\
                  one per line as 'name (N bytes)'. Prints nothing if none yet.\n",
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
    puts("Editing: Up/Down history, Left/Right/Home/End cursor, Tab completes,\n");
    puts("Ctrl-C cancels the line, Ctrl-L clears, `>> ` continues multi-line input.\n");
}

/// Intercept built-in shell commands (`help`, `clear`) before they reach the
/// Lua parser. Returns `true` if the line was a shell command (already handled).
fn handle_repl_cmd(line: &[u8], putc: fn(u8), puts: fn(&str)) -> bool {
    if trim(line) == &b"clear"[..] {
        putc(b'\x0c');
        return true;
    }
    let target: Option<&[u8]> = if line == &b"help"[..] {
        Some(&[])
    } else if line.starts_with(&b"help "[..]) {
        Some(trim(&line[5..]))
    } else {
        None
    };
    match target {
        None => false,
        Some(sub) if sub.is_empty() => {
            print_general_help(puts);
            true
        }
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
            true
        }
    }
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

    /// Mock `dhcp_info` host callback: formats the negotiated network details.
    fn mock_dhcp_info(buf: &mut [u8]) -> usize {
        let s = b"MAC: aa:bb:cc:dd:ee:ff\nIP: 10.0.0.15\n";
        let n = s.len().min(buf.len());
        buf[..n].copy_from_slice(&s[..n]);
        n
    }

    /// REPL session with `dhcp` and `dhcp_info` callbacks installed.
    fn run_session_with_dhcp_info(keys: &[u8]) -> String {
        feed(keys);
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = LuaState::new();
        state.register_builtins(putc);
        state.set_fetch(None);
        state.set_dhcp(Some(mock_dhcp));
        state.set_dhcp_info(Some(mock_dhcp_info));
        repl_loop(&mut state, get_key, putc, puts);
        OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap())
    }

    #[test]
    fn dhcp_prints_info_in_repl() {
        // Bare `dhcp` prints the negotiated details, then the success marker.
        let out = run_session_with_dhcp_info(b"dhcp\rexit\r");
        assert!(out.contains("MAC: aa:bb:cc:dd:ee:ff"));
        assert!(out.contains("IP: 10.0.0.15"));
        assert!(out.contains("true"));
    }

    /// Mock `dhcp_values` host callback: fills the structured DHCP result.
    fn mock_dhcp_values(v: &mut crate::DhcpValues) {
        v.mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        v.ip = [10, 0, 0, 15];
        v.subnet = [255, 255, 255, 0];
        v.gateway = [10, 0, 0, 1];
        v.server = [10, 0, 0, 1];
        let bf: &[u8] = b"test.lua";
        v.bootfile[..bf.len()].copy_from_slice(bf);
    }

    /// REPL session with `dhcp` and `dhcp_values` callbacks installed.
    fn run_session_with_dhcp_values(keys: &[u8]) -> String {
        feed(keys);
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = LuaState::new();
        state.register_builtins(putc);
        state.set_fetch(None);
        state.set_dhcp(Some(mock_dhcp));
        state.set_dhcp_values(Some(mock_dhcp_values));
        repl_loop(&mut state, get_key, putc, puts);
        OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap())
    }

    #[test]
    fn dhcp_variable_globals_in_repl() {
        // After `dhcp`, the MAC/IP/etc. globals persist across REPL lines.
        let out = run_session_with_dhcp_values(b"dhcp\rprint(mac)\rprint(ip)\rprint(server)\rprint(bootfile)\rprint(tftp_port)\rexit\r");
        assert!(out.contains("52:54:00:12:34:56\n"));
        assert!(out.contains("10.0.0.15\n"));
        assert!(out.contains("10.0.0.1\n"));
        assert!(out.contains("test.lua\n"));
        assert!(out.contains("69\n"));
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
    fn ls_in_repl() {
        // Before fetch, ls prints nothing.
        let out = run_session_with_fetch(b"ls\rexit\r");
        assert!(!out.contains("bytes)"));
        // After fetch, bare `ls` lists the downloaded files.
        let out = run_session_with_fetch(b"fetch(\"a.txt\")\rls\rexit\r");
        assert!(out.contains("a.txt (5 bytes)"));
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
        // The erase sequence (\r + spaces + \r full-line redraw) is emitted.
        assert!(out.contains("\r"));
        // The edited line "1" executes as a bare expression and prints 1.
        assert!(out.contains("\n1\n"));
        // The erased '2' must not appear on its own line after the erase.
        assert!(!out.contains("> 12\n"));
    }

    #[test]
    fn backspace_erases_previous_width() {
        // After shrinking "123" -> "12", the redraw must erase the whole old
        // line (5 columns including the prompt), not just the new 2-char line,
        // so no stale character is left on screen.
        let out = run_session(b"123\x7f\rprint(1)\rexit\r");
        assert!(out.contains("\r     \r> 12"), "redraw should erase 5 cols:\n{}", out);
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
        for cmd in ["help", "exit", "clear", "print", "fetch", "dofile", "ls", "shell", "dhcp"] {
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

    // ── New shell features ──────────────────────────────────────────────────

    #[test]
    fn history_up_down() {
        // Execute two commands, then Up recalls the last one; Enter re-runs it.
        let out = run_session(b"x = 1\rx = x + 1\r\x80\rprint(x)\rexit\r");
        // x = 1, then x = 2; Up recalls "x = x + 1"; Enter -> x = 3.
        assert!(out.contains("3\n"));
    }

    #[test]
    fn history_draft_restore() {
        // Type a draft, navigate up and back down, Enter executes the draft.
        let out = run_session(b"x = 5\x80\x81\rprint(x)\rexit\r");
        assert!(out.contains("5"));
    }

    #[test]
    fn line_editing_insert_middle() {
        // Type "12", Left, insert '0' -> "102".
        let out = run_session(b"12\x830\rprint(1)\rexit\r");
        assert!(out.contains("102"));
    }

    #[test]
    fn ctrl_c_cancels_line() {
        let out = run_session(b"abc\x03print(9)\rexit\r");
        // The cancelled line must not execute (would be an undefined var).
        assert!(out.contains("^C\n"));
        assert!(!out.contains("Lua error"));
        assert!(out.contains("9"));
    }

    #[test]
    fn tab_completes_name() {
        // "pri" + Tab -> "print"; then "(1)" executes and prints 1.
        let out = run_session(b"pri\t(1)\rprint(1 + 1)\rexit\r");
        assert!(out.contains("1\n"));
    }

    #[test]
    fn tab_lists_multiple_matches() {
        // "d" + Tab matches dhcp and dofile, so both are listed.
        let out = run_session(b"d\t\rexit\r");
        assert!(out.contains("Completions:"));
        assert!(out.contains("dhcp"));
        assert!(out.contains("dofile"));
    }

    #[test]
    fn ctrl_l_clears() {
        let out = run_session(b"\x0cprint(1)\rexit\r");
        assert!(out.contains("\x0c"));
        assert!(out.contains("1"));
    }

    #[test]
    fn clear_command() {
        let out = run_session(b"clear\rprint(1)\rexit\r");
        assert!(out.contains("\x0c"));
        assert!(!out.contains("Lua error"));
    }

    #[test]
    fn multiline_continuation() {
        // An incomplete `if` continues with `>> ` and executes once complete.
        let out = run_session(b"if 1 then\rprint(7)\rend\rprint(1)\rexit\r");
        assert!(out.contains(">> "));
        assert!(out.contains("7\n"));
        assert!(out.contains("1\n"));
    }

    #[test]
    fn multiline_incomplete_then_cancelled() {
        // Start an incomplete chunk, then Ctrl-C abandons it.
        let out = run_session(b"if 1 then\r\x03print(5)\rexit\r");
        assert!(out.contains("^C"));
        assert!(out.contains("5\n"));
    }

    // ── Escape mapping ──────────────────────────────────────────────────────

    #[test]
    fn map_escape_arrows() {
        assert_eq!(map_escape(b"\x1b[A"), Some(KEY_UP));
        assert_eq!(map_escape(b"\x1b[B"), Some(KEY_DOWN));
        assert_eq!(map_escape(b"\x1b[C"), Some(KEY_RIGHT));
        assert_eq!(map_escape(b"\x1b[D"), Some(KEY_LEFT));
        assert_eq!(map_escape(b"\x1b[H"), Some(KEY_HOME));
        assert_eq!(map_escape(b"\x1b[F"), Some(KEY_END));
        assert_eq!(map_escape(b"\x1b[1~"), Some(KEY_HOME));
        assert_eq!(map_escape(b"\x1b[4~"), Some(KEY_END));
        assert_eq!(map_escape(b"\x1b[3~"), Some(KEY_DELETE));
        assert_eq!(map_escape(b"\x1bOA"), Some(KEY_UP));
        assert_eq!(map_escape(b"\x1b[5~"), None);
    }

    #[test]
    fn esc_seq_incremental() {
        let mut e = EscSeq::new();
        assert_eq!(e.feed(0x1b), None);
        assert!(e.in_progress());
        assert_eq!(e.feed(b'['), None);
        assert_eq!(e.feed(b'A'), Some(KEY_UP));
        assert!(!e.in_progress());
        // A lone ESC followed by a normal char aborts (byte is lost).
        assert_eq!(e.feed(0x1b), None);
        assert_eq!(e.feed(b'x'), None);
        assert!(!e.in_progress());
        // Non-ESC bytes pass through.
        assert_eq!(e.feed(b'z'), Some(b'z'));
    }

    #[test]
    fn is_complete_checks() {
        assert!(is_complete(b"print(1)"));
        assert!(is_complete(b"x = 1 +\n2\n"));
        assert!(is_complete(b"repeat x = x + 1 until x > 3\n"));
        assert!(!is_complete(b"if x then\n"));
        assert!(!is_complete(b"for i = 1, 3 do\n"));
        assert!(!is_complete(b"x = 1 +\n"));
        assert!(!is_complete(b"x = (\n"));
        assert!(!is_complete(b"s = \"abc\n"));
        assert!(is_complete(b""));
    }
}