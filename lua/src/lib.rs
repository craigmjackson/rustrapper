//! Minimal Lua interpreter subset for rustrapper (`no_std`, no heap).
//!
//! Supported subset:
//! - Integer numbers, strings (single/double quotes with escapes), booleans, `nil`
//! - `local` and `global` variable declarations (assignment to a plain name
//!   updates an existing local or creates a global; `global name = v` forces a
//!   write to the global table even when a local shadows it)
//! - Arithmetic `+ - * / %`, comparison `== ~= < <= > >=`
//! - `and` / `or` (short-circuit) / `not`, string concat `..`
//! - `if` / `elseif` / `else` / `end`, `while ... do ... end`, `repeat ... until
//!   cond` (runs the body at least once; locals from the body are visible in the
//!   condition), `break` (inside `while` / `repeat` / `for` / `for ... in` loops)
//! - `goto name` / `::name::` labels — jump to a label in the same block or an
//!   enclosing block (can jump out of a block but not into a nested one, and
//!   never across a function boundary)
//! - Numeric `for i = a, b [, step] do ... end`
//! - Generic `for k [, v] in table do ... end` (iterates a table's key/value
//!   pairs; the `in` keyword) — array fields iterate `1..n`, named fields by key
//! - Named functions `function name(a, b) ... end` and `return`
//! - Tables: array fields, `name =` fields, `[expr]` fields, `t.key`, `t[key]`
//! - `print(...)` builtin, `--` line comments
//! - `dhcp` / `dhcp()` builtin: runs the network setup (e1000 + DHCP) and
//!   enables the `fetch()` builtin. The REPL starts with networking disabled
//!   until the user runs `dhcp`.
//! - `fetch("file")` builtin: downloads a file from the TFTP server (DHCP
//!   `next_server`) into host memory and returns its byte count as a number,
//!   or `nil` if the download fails. Requires a host callback, so scripts
//!   using it must run through [`LuaState::run_with_fetch`] / [`run_with_fetch`].
//! - `dofile("file")` builtin: loads a Lua source chunk by name (e.g. via
//!   TFTP), executes it in the current interpreter state, and returns the
//!   chunk's return value (`nil` if it doesn't return). Errors inside the
//!   chunk propagate to the caller. Requires a host loader callback, so
//!   scripts using it must run through [`LuaState::run_with_fetch_load`] /
//!   [`run_with_fetch_load`].
//!
//! Not supported: closures/upvalues, floats, `local function`, anonymous
//! function literals, string methods, multiple assignment.
//!
//! All interpreter state lives in a fixed-size [`LuaState`] with no dynamic
//! allocation. `LuaState` is passed by `&mut` everywhere (no global mutable
//! state), so the interpreter is safe to call from multiple threads and can be
//! exercised by the host test harness in parallel.

#![no_std]

#[cfg(test)]
extern crate std;

pub mod eval;
pub mod lex;
pub mod parse;
pub mod repl;

// ── Sizing constants (all memory is fixed static buffers) ──────────────────

/// Maximum number of AST nodes (each node is 16 bytes).
pub const MAX_NODES: usize = 1024;
/// String arena capacity in bytes (all strings are interned here).
pub const STR_CAP: usize = 4096;
/// Maximum number of distinct interned strings.
pub const MAX_STRINGS: usize = 256;
/// Expression/argument value stack capacity.
pub const STACK_CAP: usize = 128;
/// Maximum call/block frame depth.
pub const MAX_FRAMES: usize = 16;
/// Maximum number of locals per frame.
pub const MAX_LOCALS: usize = 16;
/// Maximum number of defined functions.
pub const MAX_FUNCS: usize = 32;
/// Maximum number of global variables.
pub const MAX_GLOBALS: usize = 64;
/// Number of key/value slots per table (tables have their own fixed slots,
/// so nested table literals can't interleave and corrupt each other).
pub const TABLE_SLOTS: usize = 8;
/// Maximum number of tables alive at once.
pub const MAX_TABLES: usize = 16;
/// Safety cap on total statements executed per script run.
pub const MAX_STEPS: u64 = 5_000_000;
/// Scratch buffer used by `dofile()` to hold a loaded Lua chunk while parsing.
pub const DOFILE_CAP: usize = 4096;

/// Sentinel for "no node" / end-of-chain. Node indices are well below this.
pub const NO_NODE: u16 = u16::MAX;

/// Reference into the string arena, packed as `(offset << 16) | len`.
pub type StrRef = u32;

/// Pack an (offset, length) pair into a [`StrRef`].
#[inline]
pub fn strref(off: u16, len: u16) -> StrRef {
    ((off as u32) << 16) | (len as u32)
}

/// A runtime value. Numbers are 64-bit integers (no floats).
#[derive(Clone, Copy, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Num(i64),
    Str(StrRef),
    Table(u16),
    Func(u16),
    /// Builtin function: `print` is `Native(0)`, `fetch` is `Native(1)`,
    /// `dofile` is `Native(2)`.
    Native(u8),
    /// Builtin: `shell()` — enters the interactive Lua REPL.
    Shell,
    /// Builtin: `dhcp` / `dhcp()` — runs the network setup so `fetch()` works.
    Dhcp,
    /// Builtin: `exit()` — exits the REPL (only meaningful inside a shell).
    Exit,
}

/// Binary and unary operators. `And`/`Or` are handled with short-circuiting.
#[derive(Clone, Copy, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Concat,
    And,
    Or,
    Not,
    Neg,
}

/// AST node. Expressions evaluate to a [`Value`]; statement nodes drive
/// control flow and are chained via the parallel `next[]` array in [`LuaState`].
#[derive(Clone, Copy)]
pub enum Node {
    Empty,
    Nil,
    True,
    False,
    Num(i64),
    Str(StrRef),
    /// Variable reference by name.
    Var(StrRef),
    Bin(Op, u16, u16),
    Un(Op, u16),
    /// `base[key]` — base and key are expression node indices.
    Index(u16, u16),
    /// `func(first_arg)` — args are a chain of [`Node::Arg`] nodes linked via
    /// `next[]`, starting at `first_arg` (`NO_NODE` for no args).
    Call(u16, u16),
    /// One argument expression, chained via `next[]`.
    Arg(u16),
    /// Reference to a function defined in `funcs[]`.
    FuncLit(u16),
    /// `{ ... }` — fields are a chain of [`Node::Field`] nodes linked via
    /// `next[]`, starting at `first_field`.
    TableLit(u16),
    /// One `(key, value)` field, chained via `next[]`.
    Field(u16, u16),
    /// `local name = value`.
    LocalDecl(u16, u16),
    /// `global name [= value]` — force a write to the global table, ignoring
    /// any local that shadows `name`.
    GlobalDecl(u16, u16),
    /// `target = value`.
    AssignStmt(u16, u16),
    /// Expression statement (a call).
    CallStmt(u16),
    /// Evaluate an expression and print its result.
    ExprStmt(u16),
    /// `if cond then .. elseif .. else .. end`.
    IfStmt(u16, u16, u16),
    /// `while cond do .. end`.
    WhileStmt(u16, u16),
    /// `for i = start, limit [, step] do .. end`.
    ForStmt(u16, u16, u16, u16, u16),
    /// `for k [, v] in table do .. end` — iterate a table's key/value pairs.
    /// Fields are (key_var, value_var or `NO_NODE`, table_expr, body).
    ForInStmt(u16, u16, u16, u16),
    /// `break` — terminate the innermost loop. Only valid inside a loop.
    BreakStmt,
    /// `repeat body until cond` — run `body` at least once, then repeat until
    /// `cond` is true. `cond` is evaluated in the body's scope.
    RepeatStmt(u16, u16),
    /// `goto name` — jump to the `::name::` label. The payload is the resolved
    /// label node index (filled in by the parser after block resolution).
    Goto(u16),
    /// `::name::` — a label; a no-op jump target for `goto`.
    Label(StrRef),
    /// `return value` (value node index 0 means `return`).
    ReturnStmt(u16),
}

/// A defined function: parameters are `nparams` contiguous [`Node::Var`]
/// nodes starting at `params`; `body` is the first statement of its block.
#[derive(Clone, Copy)]
pub struct FuncDef {
    pub params: u16,
    pub nparams: u8,
    pub body: u16,
}

/// A local variable slot within a frame.
#[derive(Clone, Copy)]
pub struct Local {
    pub name: StrRef,
    pub value: Value,
}

/// One call/block scope: a fixed array of locals.
#[derive(Clone, Copy)]
pub struct Frame {
    pub locals: [Local; MAX_LOCALS],
    pub count: u8,
}

/// A global variable slot.
#[derive(Clone, Copy)]
pub struct Global {
    pub name: StrRef,
    pub value: Value,
}

/// A table's fixed array of key/value slots.
#[derive(Clone, Copy)]
pub struct TableRec {
    pub slots: [TableSlot; TABLE_SLOTS],
    pub len: u8,
}

/// One key/value pair in a table.
#[derive(Clone, Copy)]
pub struct TableSlot {
    pub key: Value,
    pub value: Value,
}

/// Location of an interned string within the arena.
#[derive(Clone, Copy)]
pub struct StrReg {
    pub off: u16,
    pub len: u16,
}

/// All interpreter memory. Created on the caller's stack (typically ~37 KB).
pub struct LuaState {
    pub nodes: [Node; MAX_NODES],
    /// Chain links: for statement nodes, the next statement in the same
    /// block; for [`Node::Arg`] nodes, the next argument; for [`Node::Field`]
    /// nodes, the next field. `NO_NODE` terminates every chain.
    pub next: [u16; MAX_NODES],
    pub node_used: u32,
    pub strings: [u8; STR_CAP],
    pub strregs: [StrReg; MAX_STRINGS],
    pub nstrings: u32,
    pub str_next: u32,
    pub vstack: [Value; STACK_CAP],
    pub vsp: u32,
    pub frames: [Frame; MAX_FRAMES],
    pub fsp: u32,
    pub funcs: [FuncDef; MAX_FUNCS],
    pub funcs_used: u32,
    pub globals: [Global; MAX_GLOBALS],
    pub nglobals: u32,
    pub tbls: [TableRec; MAX_TABLES],
    pub ntables: u32,
    pub steps: u64,
    /// Character output callback used by `print()`.
    pub putc: fn(u8),
    /// Host callback for the `fetch()` builtin: downloads `name` from the
    /// TFTP server and returns its byte count, or `None` on failure. Set by
    /// [`LuaState::run_with_fetch`] or by a successful `dhcp`; `None` means
    /// `fetch()` errors out.
    pub fetch: Option<fn(&str) -> Option<usize>>,
    /// Host callback for the `dhcp` builtin: runs the network setup (e1000 +
    /// DHCP) and returns the `fetch` callback if a TFTP server is reachable.
    /// Set by the host before entering the REPL; `None` means `dhcp` errors.
    pub dhcp: Option<fn() -> Option<fn(&str) -> Option<usize>>>,
    /// Host callback for the `dofile()` builtin: loads the Lua source for
    /// `name` (e.g. via TFTP) into `buf` and returns its length, or `None` if
    /// the file can't be loaded. The interpreter owns `buf` (`DOFILE_CAP`
    /// bytes); the callback must not write past it. `None` means `dofile()`
    /// errors out.
    pub load: Option<fn(&str, &mut [u8]) -> Option<usize>>,
}

fn noop(_c: u8) {}

impl LuaState {
    /// Create a fresh, empty interpreter state.
    pub fn new() -> Self {
        LuaState {
            nodes: [Node::Empty; MAX_NODES],
            next: [NO_NODE; MAX_NODES],
            node_used: 0,
            strings: [0u8; STR_CAP],
            strregs: [StrReg { off: 0, len: 0 }; MAX_STRINGS],
            nstrings: 0,
            str_next: 0,
            vstack: [Value::Nil; STACK_CAP],
            vsp: 0,
            frames: [Frame {
                locals: [Local {
                    name: 0,
                    value: Value::Nil,
                }; MAX_LOCALS],
                count: 0,
            }; MAX_FRAMES],
            fsp: 0,
            funcs: [FuncDef {
                params: 0,
                nparams: 0,
                body: 0,
            }; MAX_FUNCS],
            funcs_used: 0,
            globals: [Global {
                name: 0,
                value: Value::Nil,
            }; MAX_GLOBALS],
            nglobals: 0,
            tbls: [TableRec {
                slots: [TableSlot {
                    key: Value::Nil,
                    value: Value::Nil,
                }; TABLE_SLOTS],
                len: 0,
            }; MAX_TABLES],
            ntables: 0,
            steps: 0,
            putc: noop,
            fetch: None,
            dhcp: None,
            load: None,
        }
    }

    /// Register built-in globals (`print`, `fetch`, `shell`, `dhcp`, `exit`).
    /// Call this once after creating a fresh `LuaState` before entering the REPL.
    pub fn register_builtins(&mut self, putc: fn(u8)) {
        self.putc = putc;
        let _ = self.intern(b"print");
        let print_name = self.intern(b"print").unwrap();
        self.set_global(print_name, Value::Native(0));
        let _ = self.intern(b"fetch");
        let fetch_name = self.intern(b"fetch").unwrap();
        self.set_global(fetch_name, Value::Native(1));
        let _ = self.intern(b"shell");
        let shell_name = self.intern(b"shell").unwrap();
        self.set_global(shell_name, Value::Shell);
        let _ = self.intern(b"dhcp");
        let dhcp_name = self.intern(b"dhcp").unwrap();
        self.set_global(dhcp_name, Value::Dhcp);
        let _ = self.intern(b"dofile");
        let dofile_name = self.intern(b"dofile").unwrap();
        self.set_global(dofile_name, Value::Native(2));
        let _ = self.intern(b"exit");
        let exit_name = self.intern(b"exit").unwrap();
        self.set_global(exit_name, Value::Exit);
    }

    /// Parse and execute a Lua script. `source` must remain valid for the
    /// whole call (identifiers reference into it). Output from `print()`
    /// goes to `putc`.
    pub fn run(&mut self, source: &[u8], putc: fn(u8)) -> Result<(), &'static str> {
        self.reset();
        self.putc = putc;
        self.register_builtins(putc);
        let first = {
            let mut p = parse::Parser::new(source, self);
            p.parse_script()?
        };
        eval::exec_script(self, first)
    }

    /// Install a host `fetch()` callback. Call `set_fetch(None)` to explicitly
    /// disable fetch (e.g. when no TFTP server is reachable). The REPL starts
    /// with `fetch` disabled and enables it only after a successful `dhcp`.
    pub fn set_fetch(&mut self, fetch: Option<fn(&str) -> Option<usize>>) {
        self.fetch = fetch;
    }

    /// Install a host `dhcp` callback: runs the network setup (e1000 + DHCP)
    /// and returns the `fetch` callback when a TFTP server is reachable.
    /// Call `set_dhcp(None)` to disable the `dhcp` builtin.
    pub fn set_dhcp(&mut self, dhcp: Option<fn() -> Option<fn(&str) -> Option<usize>>>) {
        self.dhcp = dhcp;
    }

    /// Install a host `dofile()` callback: loads the Lua source for a filename
    /// (e.g. via TFTP) into a caller-provided buffer and returns its length.
    /// Call `set_load(None)` to disable the `dofile` builtin.
    pub fn set_load(&mut self, load: Option<fn(&str, &mut [u8]) -> Option<usize>>) {
        self.load = load;
    }

    /// Run the `dhcp` builtin: establish network connectivity and enable the
    /// `fetch` callback. Returns `true` on success, `false` if the network
    /// setup failed, or an error if no host callback is registered.
    pub fn run_dhcp(&mut self) -> Result<bool, &'static str> {
        match self.dhcp {
            Some(f) => match f() {
                Some(fetch_cb) => {
                    self.fetch = Some(fetch_cb);
                    Ok(true)
                }
                None => Ok(false),
            },
            None => Err("dhcp not available"),
        }
    }

    /// Run a script with a host `fetch()` callback installed, so the script
    /// can call `fetch("file")` to download files from the TFTP server.
    pub fn run_with_fetch(
        &mut self,
        source: &[u8],
        putc: fn(u8),
        fetch: fn(&str) -> Option<usize>,
    ) -> Result<(), &'static str> {
        self.fetch = Some(fetch);
        self.run(source, putc)
    }

    /// Run a script with both a host `fetch()` callback and a `dofile()`
    /// loader callback installed, so the script can download files and run
    /// `.lua` chunks loaded by name.
    pub fn run_with_fetch_load(
        &mut self,
        source: &[u8],
        putc: fn(u8),
        fetch: fn(&str) -> Option<usize>,
        load: fn(&str, &mut [u8]) -> Option<usize>,
    ) -> Result<(), &'static str> {
        self.fetch = Some(fetch);
        self.load = Some(load);
        self.run(source, putc)
    }

    fn reset(&mut self) {
        self.node_used = 0;
        self.next = [NO_NODE; MAX_NODES];
        self.nstrings = 0;
        self.str_next = 0;
        self.strings = [0u8; STR_CAP];
        self.strregs = [StrReg { off: 0, len: 0 }; MAX_STRINGS];
        self.vsp = 0;
        self.fsp = 0;
        self.funcs_used = 0;
        self.nglobals = 0;
        self.ntables = 0;
        self.steps = 0;
    }

    // ── String arena ────────────────────────────────────────────────────────

    /// Return the bytes of a string reference.
    pub fn str_bytes(&self, r: StrRef) -> &[u8] {
        let off = (r >> 16) as usize;
        let len = (r & 0xFFFF) as usize;
        &self.strings[off..off + len]
    }

    /// Intern `bytes` into the arena, deduplicating so equal strings share a
    /// [`StrRef`] (which makes string equality a pointer comparison).
    pub fn intern(&mut self, bytes: &[u8]) -> Result<StrRef, &'static str> {
        for i in 0..self.nstrings as usize {
            let reg = self.strregs[i];
            if reg.len as usize == bytes.len() {
                let off = reg.off as usize;
                let same = bytes
                    .iter()
                    .enumerate()
                    .all(|(j, b)| self.strings[off + j] == *b);
                if same {
                    return Ok(strref(reg.off, reg.len));
                }
            }
        }
        if self.nstrings as usize >= MAX_STRINGS {
            return Err("too many strings");
        }
        let end = self.str_next as usize + bytes.len();
        if end > STR_CAP {
            return Err("string overflow");
        }
        let off = self.str_next as usize;
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.strings.as_mut_ptr().add(off), bytes.len());
        }
        self.str_next = end as u32;
        self.strregs[self.nstrings as usize] = StrReg {
            off: off as u16,
            len: bytes.len() as u16,
        };
        self.nstrings += 1;
        Ok(strref(off as u16, bytes.len() as u16))
    }

    // ── Node arena ──────────────────────────────────────────────────────────

    /// Allocate one AST node, returning its index.
    pub fn alloc_node(&mut self, n: Node) -> Result<u16, &'static str> {
        if self.node_used as usize >= MAX_NODES {
            return Err("script too complex");
        }
        let idx = self.node_used;
        self.nodes[idx as usize] = n;
        self.node_used += 1;
        Ok(idx as u16)
    }

    /// Define a function (parameters are `nparams` contiguous [`Node::Var`]
    /// nodes starting at `params`), returning its function index.
    pub fn alloc_func(&mut self, params: u16, nparams: u8, body: u16) -> Result<u16, &'static str> {
        if self.funcs_used as usize >= MAX_FUNCS {
            return Err("too many functions");
        }
        let idx = self.funcs_used;
        self.funcs[idx as usize] = FuncDef {
            params,
            nparams,
            body,
        };
        self.funcs_used += 1;
        Ok(idx as u16)
    }

    // ── Value stack (call arguments) ────────────────────────────────────────

    pub fn push_val(&mut self, v: Value) -> Result<(), &'static str> {
        if self.vsp as usize >= STACK_CAP {
            return Err("stack overflow");
        }
        self.vstack[self.vsp as usize] = v;
        self.vsp += 1;
        Ok(())
    }

    // ── Frames / locals / globals ───────────────────────────────────────────

    pub fn push_frame(&mut self) -> Result<(), &'static str> {
        if self.fsp as usize >= MAX_FRAMES {
            return Err("call stack overflow");
        }
        self.frames[self.fsp as usize] = Frame {
            locals: [Local {
                name: 0,
                value: Value::Nil,
            }; MAX_LOCALS],
            count: 0,
        };
        self.fsp += 1;
        Ok(())
    }

    pub fn pop_frame(&mut self) {
        if self.fsp > 0 {
            self.fsp -= 1;
        }
    }

    /// Declare a new local in the innermost frame.
    pub fn declare_local(&mut self, name: StrRef, value: Value) -> Result<(), &'static str> {
        if self.fsp == 0 {
            return Err("no active scope");
        }
        let f = (self.fsp - 1) as usize;
        if self.frames[f].count as usize >= MAX_LOCALS {
            return Err("too many locals");
        }
        let i = self.frames[f].count as usize;
        self.frames[f].locals[i] = Local { name, value };
        self.frames[f].count += 1;
        Ok(())
    }

    /// Set (or declare, if missing) a local in the innermost frame. Used for
    /// the numeric `for` loop variable.
    pub fn set_local_top(&mut self, name: StrRef, value: Value) -> Result<(), &'static str> {
        if self.fsp == 0 {
            return Err("no active scope");
        }
        let f = (self.fsp - 1) as usize;
        for i in 0..self.frames[f].count as usize {
            if self.frames[f].locals[i].name == name {
                self.frames[f].locals[i].value = value;
                return Ok(());
            }
        }
        self.declare_local(name, value)
    }

    /// Set a local in the innermost scope that has it. Returns whether found.
    pub fn assign_local(&mut self, name: StrRef, value: Value) -> bool {
        for f in (0..self.fsp as usize).rev() {
            for i in 0..self.frames[f].count as usize {
                if self.frames[f].locals[i].name == name {
                    self.frames[f].locals[i].value = value;
                    return true;
                }
            }
        }
        false
    }

    /// Look up a variable: innermost frame outwards, then globals.
    pub fn lookup(&self, name: StrRef) -> Option<Value> {
        for f in (0..self.fsp as usize).rev() {
            for i in 0..self.frames[f].count as usize {
                if self.frames[f].locals[i].name == name {
                    return Some(self.frames[f].locals[i].value);
                }
            }
        }
        for i in 0..self.nglobals as usize {
            if self.globals[i].name == name {
                return Some(self.globals[i].value);
            }
        }
        None
    }

    /// Create or update a global variable.
    pub fn set_global(&mut self, name: StrRef, value: Value) {
        for i in 0..self.nglobals as usize {
            if self.globals[i].name == name {
                self.globals[i].value = value;
                return;
            }
        }
        if (self.nglobals as usize) < MAX_GLOBALS {
            self.globals[self.nglobals as usize] = Global { name, value };
            self.nglobals += 1;
        }
    }
}

/// Convenience wrapper: run a script with a fresh [`LuaState`].
pub fn run(source: &[u8], putc: fn(u8)) -> Result<(), &'static str> {
    let mut state = LuaState::new();
    state.run(source, putc)
}

/// Convenience wrapper: run a script with a fresh [`LuaState`] and a host
/// `fetch()` callback installed.
pub fn run_with_fetch(
    source: &[u8],
    putc: fn(u8),
    fetch: fn(&str) -> Option<usize>,
) -> Result<(), &'static str> {
    let mut state = LuaState::new();
    state.run_with_fetch(source, putc, fetch)
}

/// Convenience wrapper: run a script with a fresh [`LuaState`] and both host
/// `fetch()` and `dofile()` callbacks installed.
pub fn run_with_fetch_load(
    source: &[u8],
    putc: fn(u8),
    fetch: fn(&str) -> Option<usize>,
    load: fn(&str, &mut [u8]) -> Option<usize>,
) -> Result<(), &'static str> {
    let mut state = LuaState::new();
    state.run_with_fetch_load(source, putc, fetch, load)
}

/// Run the interactive Lua REPL. `read_line` should write a line into the
/// provided buffer and return its length, or `None` when input is exhausted
/// (exits the REPL). Output from `print()` and the prompt go to `putc`.
pub fn run_repl(
    mut read_line: impl FnMut(&mut [u8]) -> Option<usize>,
    putc: fn(u8),
) -> Result<(), &'static str> {
    let mut state = LuaState::new();
    // Register builtins (print, fetch, shell, dhcp, exit) by running an empty script.
    state.run(&[], putc)?;
    let mut buf = [0u8; 256];
    loop {
        putc(b'>');
        putc(b' ');
        let len = match read_line(&mut buf) {
            Some(l) if l > 0 => l,
            _ => break, // EOF or empty input
        };
        match eval::run_repl_once(&mut state, &buf[..len], putc) {
            Ok(eval::ExecResult::Normal) => {}
            Ok(eval::ExecResult::Break) => {}
            Ok(eval::ExecResult::Goto(_)) => {}
            Ok(eval::ExecResult::Ret(_)) => {}
            Ok(eval::ExecResult::Exit) => break,
            Ok(eval::ExecResult::Shell) => {
                // Nested shell — not supported in this simple REPL.
                putc(b'\n');
                putc(b'\n');
            }
            Err(e) => {
                putc(b'\n');
                for &b in e.as_bytes() {
                    putc(b);
                }
                putc(b'\n');
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::string::String;
    use std::thread_local;
    use std::vec;
    use std::string::ToString;
    use std::vec::Vec;

    thread_local! {
        static OUT: RefCell<Vec<u8>> = RefCell::new(Vec::new());
    }

    fn putc_test(c: u8) {
        OUT.with(|o| o.borrow_mut().push(c));
    }

    fn exec(src: &str) -> Result<String, &'static str> {
        OUT.with(|o| o.borrow_mut().clear());
        super::run(src.as_bytes(), putc_test)?;
        Ok(OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap()))
    }

    #[test]
    fn arithmetic_and_precedence() {
        assert_eq!(exec("print(1 + 2 * 3)").unwrap(), "7\n");
        assert_eq!(exec("print((1 + 2) * 3)").unwrap(), "9\n");
        assert_eq!(exec("print(10 / 3)").unwrap(), "3\n");
        assert_eq!(exec("print(10 % 3)").unwrap(), "1\n");
        assert_eq!(exec("print(-7 + 2)").unwrap(), "-5\n");
        assert_eq!(exec("print(7 - 10)").unwrap(), "-3\n");
    }

    #[test]
    fn variables_and_assignment() {
        assert_eq!(exec("x = 5\nprint(x)").unwrap(), "5\n");
        assert_eq!(exec("x = 5\nx = x + 2\nprint(x)").unwrap(), "7\n");
        assert_eq!(exec("local x = 10\nprint(x)").unwrap(), "10\n");
        assert_eq!(exec("local x = 10\nx = 3\nprint(x)").unwrap(), "3\n");
    }

    #[test]
    fn global_keyword() {
        // Basic declaration and init.
        assert_eq!(exec("global g = 7\nprint(g)").unwrap(), "7\n");
        // No value -> nil.
        assert_eq!(exec("global g\nprint(g)").unwrap(), "nil\n");
        // Global set from inside a function scope.
        assert_eq!(
            exec("function set()\nglobal g = 42\nend\nset()\nprint(g)").unwrap(),
            "42\n"
        );
        // `global` bypasses a shadowing local (plain `x = 3` would write the local).
        assert_eq!(
            exec("x = 1\nfunction f()\nlocal x = 2\nglobal x = 3\nend\nf()\nprint(x)").unwrap(),
            "3\n"
        );
        // A local still shadows READS, but `global` writes the global table so
        // the value is visible once the local goes out of scope.
        assert_eq!(
            exec("global g = 1\nlocal g = 2\nprint(g)\nglobal g = 9\nprint(g)").unwrap(),
            "2\n2\n"
        );
        assert_eq!(
            exec("function f()\nlocal g = 2\nglobal g = 9\nprint(g)\nend\nf()\nprint(g)").unwrap(),
            "2\n9\n"
        );
        // Global can be updated incrementally.
        assert_eq!(exec("global c = 1\nglobal c = c + 1\nprint(c)").unwrap(), "2\n");
    }

    #[test]
    fn global_syntax_errors() {
        assert!(exec("global").is_err());
        assert!(exec("global 5").is_err());
        assert!(exec("global x + 1").is_err());
    }

    #[test]
    fn comparison_and_logic() {
        assert_eq!(exec("print(5 == 5)").unwrap(), "true\n");
        assert_eq!(exec("print(5 ~= 6)").unwrap(), "true\n");
        assert_eq!(exec("print(5 < 6)").unwrap(), "true\n");
        assert_eq!(exec("print(6 <= 6)").unwrap(), "true\n");
        assert_eq!(exec("print(7 > 6)").unwrap(), "true\n");
        assert_eq!(exec("print(7 >= 8)").unwrap(), "false\n");
        assert_eq!(exec("print(1 == 1 and 2 == 2)").unwrap(), "true\n");
        assert_eq!(exec("print(1 == 2 or 3 == 3)").unwrap(), "true\n");
        assert_eq!(exec("print(not true)").unwrap(), "false\n");
        assert_eq!(exec("print(0)").unwrap(), "0\n");
    }

    #[test]
    fn short_circuit() {
        assert_eq!(
            exec("x = 0\nfunction set() x = 1 end\nfalse and set()\nprint(x)").unwrap(),
            "0\n"
        );
        assert_eq!(
            exec("x = 0\nfunction set() x = 1 end\ntrue or set()\nprint(x)").unwrap(),
            "0\n"
        );
    }

    #[test]
    fn if_elseif_else() {
        assert_eq!(exec("if 1 < 2 then print(1) else print(2) end").unwrap(), "1\n");
        assert_eq!(exec("if 1 > 2 then print(1) else print(2) end").unwrap(), "2\n");
        assert_eq!(
            exec("x = 2\nif x == 1 then print(1) elseif x == 2 then print(2) else print(3) end").unwrap(),
            "2\n"
        );
        assert_eq!(
            exec("x = 9\nif x == 1 then print(1) elseif x == 2 then print(2) else print(3) end").unwrap(),
            "3\n"
        );
    }

    #[test]
    fn while_loop() {
        assert_eq!(exec("i = 0\nwhile i < 5 do print(i) i = i + 1 end").unwrap(), "0\n1\n2\n3\n4\n");
        assert_eq!(exec("while false do print(1) end\nprint(2)").unwrap(), "2\n");
    }

    #[test]
    fn break_keyword() {
        // break in a while loop.
        assert_eq!(
            exec("i = 0\nwhile true do i = i + 1 if i == 3 then break end end\nprint(i)").unwrap(),
            "3\n"
        );
        // break in a numeric for loop.
        assert_eq!(
            exec("for i = 1, 10 do if i == 3 then break end print(i) end").unwrap(),
            "1\n2\n"
        );
        // break in a generic for-in loop.
        assert_eq!(
            exec("t = {10, 20, 30}\nfor k, v in t do if k == 2 then break end print(v) end").unwrap(),
            "10\n"
        );
        // break with a trailing semicolon.
        assert_eq!(exec("for i = 1, 5 do break; end\nprint(0)").unwrap(), "0\n");
        // break only exits the innermost loop.
        assert_eq!(
            exec("for i = 1, 3 do for j = 1, 3 do if j == 2 then break end print(i, j) end end").unwrap(),
            "1\t1\n2\t1\n3\t1\n"
        );
        // statements after the loop still run.
        assert_eq!(exec("i = 0\nwhile true do break end\nprint(i)").unwrap(), "0\n");
    }

    #[test]
    fn break_outside_loop_errors() {
        assert!(exec("break").is_err());
        assert!(exec("if true then break end").is_err());
        // A break in a function body is not inside a loop, even when the
        // function is called from within a loop.
        assert!(exec("function f() break end\nfor i = 1, 3 do f() end").is_err());
    }

    #[test]
    fn repeat_until_loop() {
        // Classic repeat loop.
        assert_eq!(
            exec("i = 0\nrepeat i = i + 1 until i == 3\nprint(i)").unwrap(),
            "3\n"
        );
        // Body runs at least once even if the condition starts true.
        assert_eq!(
            exec("i = 0\nrepeat i = i + 1 until i > 0\nprint(i)").unwrap(),
            "1\n"
        );
        // print() inside the body.
        assert_eq!(
            exec("i = 0\nrepeat print(i) i = i + 1 until i == 3").unwrap(),
            "0\n1\n2\n"
        );
        // break inside repeat.
        assert_eq!(
            exec("i = 0\nrepeat i = i + 1 if i == 2 then break end until false\nprint(i)").unwrap(),
            "2\n"
        );
        // Locals from the body are visible in the until condition (Lua).
        assert_eq!(exec("repeat local x = 1 until x == 1\nprint(1)").unwrap(), "1\n");
        // Empty body with a true condition runs zero extra iterations.
        assert_eq!(exec("repeat until true\nprint(1)").unwrap(), "1\n");
        // Step limit catches an infinite repeat.
        assert!(exec("repeat until false").is_err());
    }

    #[test]
    fn repeat_until_syntax_errors() {
        // Missing until.
        assert!(exec("repeat print(1)").is_err());
        // Missing condition.
        assert!(exec("repeat break until").is_err());
        // until without repeat.
        assert!(exec("until true").is_err());
    }

    #[test]
    fn goto_label() {
        // Forward jump skips statements.
        assert_eq!(exec("goto skip\nprint(1)\n::skip::\nprint(2)").unwrap(), "2\n");
        // Backward jump loops.
        assert_eq!(
            exec("i = 0\n::top::\ni = i + 1\nprint(i)\nif i < 3 then goto top end").unwrap(),
            "1\n2\n3\n"
        );
        // goto inside an if branch jumps to a label in the enclosing block.
        assert_eq!(
            exec("x = 1\nif x == 1 then goto done end\nprint(1)\n::done::\nprint(2)").unwrap(),
            "2\n"
        );
        // continue-style: goto to a label at the end of a loop body.
        assert_eq!(
            exec("for i = 1, 3 do\nif i == 2 then goto continue end\nprint(i)\n::continue::\nend").unwrap(),
            "1\n3\n"
        );
        // goto jumping out of a loop entirely.
        assert_eq!(
            exec("for i = 1, 10 do\nif i == 2 then goto done end\nend\n::done::\nprint(0)").unwrap(),
            "0\n"
        );
        // goto is scoped within a function.
        assert_eq!(
            exec("function f() goto done\n::done::\nreturn 5 end\nprint(f())").unwrap(),
            "5\n"
        );
        // goto inside a while body.
        assert_eq!(
            exec("i = 0\nwhile true do\ni = i + 1\nif i == 3 then goto out end\nend\n::out::\nprint(i)").unwrap(),
            "3\n"
        );
        // label at the end of a block.
        assert_eq!(exec("goto l\n::l::").unwrap(), "");
        // infinite goto loop is caught by the step limit.
        assert!(exec("::top::\ngoto top").is_err());
    }

    #[test]
    fn goto_syntax_errors() {
        // goto to an unknown label.
        assert!(exec("goto nope").is_err());
        // goto with no label name.
        assert!(exec("goto").is_err());
        // label with no name.
        assert!(exec(":: ::").is_err());
        // a goto in a function cannot reference a caller's label.
        assert!(exec("::l::\nfunction f() goto l end").is_err());
        assert!(exec("function f() goto out end\n::out::\nf()").is_err());
        // a goto cannot jump into a nested block (label not visible after block).
        assert!(exec("if true then ::l:: end\ngoto l").is_err());
    }

    #[test]
    fn for_loop() {
        assert_eq!(exec("for i = 1, 3 do print(i) end").unwrap(), "1\n2\n3\n");
        assert_eq!(exec("for i = 3, 1, -1 do print(i) end").unwrap(), "3\n2\n1\n");
        assert_eq!(exec("for i = 1, 10, 3 do print(i) end").unwrap(), "1\n4\n7\n10\n");
        assert_eq!(exec("for i = 1, 0 do print(i) end\nprint(0)").unwrap(), "0\n");
    }

    #[test]
    fn for_in_loop() {
        // Keys only.
        assert_eq!(
            exec("t = {10, 20, 30}\nfor k in t do print(k) end").unwrap(),
            "1\n2\n3\n"
        );
        // Key/value pairs over array fields.
        assert_eq!(
            exec("t = {\"a\", \"b\"}\nfor k, v in t do print(k, v) end").unwrap(),
            "1\ta\n2\tb\n"
        );
        // Named fields iterate by key.
        assert_eq!(
            exec("t = {name = \"bob\", age = 30}\nfor k, v in t do print(k, v) end").unwrap(),
            "name\tbob\nage\t30\n"
        );
        // Iterating a non-table errors.
        assert!(exec("for k in 5 do end").is_err());
        assert!(exec("for k in nil do end").is_err());
        // Empty table -> no iterations.
        assert_eq!(exec("t = {}\nfor k, v in t do print(1) end\nprint(0)").unwrap(), "0\n");
    }

    #[test]
    fn for_in_syntax_errors() {
        // Missing `in`.
        assert!(exec("for k in t do print(k) end\nt = {1, 2}").is_err());
        assert!(exec("for k t do end\nt = {}").is_err());
        // Missing `do` / `end`.
        assert!(exec("for k in {} print(k) end").is_err());
        assert!(exec("for k in {} do print(k)").is_err());
        // Value variable requires a name.
        assert!(exec("for k, in {} do end").is_err());
    }

    #[test]
    fn functions() {
        assert_eq!(exec("function f() print(42) end\nf()").unwrap(), "42\n");
        assert_eq!(exec("function add(a, b) return a + b end\nprint(add(3, 4))").unwrap(), "7\n");
        assert_eq!(exec("function f(a) return a end\nprint(f())").unwrap(), "nil\n");
        assert_eq!(exec("function fact(n) if n <= 1 then return 1 end return n * fact(n - 1) end\nprint(fact(6))").unwrap(), "720\n");
        assert_eq!(
            exec("function is_even(n) if n % 2 == 0 then return true end return false end\nprint(is_even(4), is_even(5))").unwrap(),
            "true\tfalse\n"
        );
    }

    #[test]
    fn tables() {
        assert_eq!(exec("t = {10, 20, 30}\nprint(t[1], t[2], t[3])").unwrap(), "10\t20\t30\n");
        assert_eq!(exec("t = {10, 20, 30}\nt[2] = 99\nprint(t[2])").unwrap(), "99\n");
        assert_eq!(exec("t = {}\nt.x = 5\nprint(t.x)").unwrap(), "5\n");
        assert_eq!(exec("t = {name = \"bob\", age = 30}\nprint(t.name, t.age)").unwrap(), "bob\t30\n");
        assert_eq!(exec("t = {}\nt[\"k\"] = 7\nprint(t[\"k\"])").unwrap(), "7\n");
        assert_eq!(exec("t = {x = 1}\nprint(t.missing)").unwrap(), "nil\n");
        assert_eq!(
            exec("a = {2, 3}\nb = {10, 20, 30}\nprint(b[a[1]])").unwrap(),
            "20\n"
        );
    }

    #[test]
    fn string_concat() {
        assert_eq!(exec("print(\"foo\" .. \"bar\")").unwrap(), "foobar\n");
        assert_eq!(exec("print(\"n=\" .. 5)").unwrap(), "n=5\n");
        assert_eq!(exec("print(\"a\" .. \"b\" .. \"c\")").unwrap(), "abc\n");
    }

    #[test]
    fn string_escapes() {
        assert_eq!(exec("print(\"a\\tb\\n\")").unwrap(), "a\tb\n\n");
        assert_eq!(exec("print('it\\'s')").unwrap(), "it's\n");
    }

    #[test]
    fn comments() {
        assert_eq!(exec("-- hello\nprint(1) -- trailing").unwrap(), "1\n");
        assert_eq!(exec("-- all comment").unwrap(), "");
    }

    #[test]
    fn print_multi_args() {
        assert_eq!(exec("print()").unwrap(), "\n");
        assert_eq!(exec("print(1, 2, 3)").unwrap(), "1\t2\t3\n");
        assert_eq!(exec("print(true, false, nil)").unwrap(), "true\tfalse\tnil\n");
    }

    #[test]
    fn syntax_errors() {
        assert!(exec("print(1 +").is_err());
        assert!(exec("if 1 then").is_err());
        assert!(exec("x =").is_err());
        assert!(exec("function f() print(1)").is_err());
        assert!(exec("print(\"unterminated)").is_err());
        assert!(exec("local = 5").is_err());
        assert!(exec("x = 1 + ").is_err());
    }

    #[test]
    fn runtime_errors() {
        assert!(exec("print(undefined_var)").is_err());
        assert!(exec("print(1 / 0)").is_err());
        assert!(exec("print(1 + \"a\")").is_err());
        assert!(exec("print(5 < \"a\")").is_err());
        assert!(exec("x = 1\nx()").is_err());
        assert!(exec("print(nil.x)").is_err());
    }

    #[test]
    fn step_limit() {
        assert!(exec("while true do end").is_err());
    }

    /// Mock `fetch()` host callback matching the two files the demo fetches.
    fn fetch_demo(name: &str) -> Option<usize> {
        match name {
            "test.txt" => Some(21),
            "rust_payload.bin" => Some(7400),
            _ => None,
        }
    }

    #[test]
    fn demo_script() {
        let src = std::fs::read_to_string("demo/test.lua").unwrap();
        OUT.with(|o| o.borrow_mut().clear());
        super::run_with_fetch(src.as_bytes(), putc_test, fetch_demo).unwrap();
        let out = OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap());
        assert_eq!(
            out,
            "Hello from Lua!\nfib(10) = 55\nrustrapper\tarm64\t2\nsum = 15\n\
             fetch test.txt: 21 bytes\nfetch rust_payload.bin: 7400 bytes\n"
        );
    }

    /// Mock `fetch()` host callback: returns a size for known names, `None`
    /// for anything else (simulating a TFTP download failure).
    fn fetch_count(name: &str) -> Option<usize> {
        match name {
            "a.txt" => Some(5),
            "b.txt" => Some(12),
            _ => None,
        }
    }

    fn exec_fetch(src: &str) -> Result<String, &'static str> {
        OUT.with(|o| o.borrow_mut().clear());
        super::run_with_fetch(src.as_bytes(), putc_test, fetch_count)?;
        Ok(OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap()))
    }

    #[test]
    fn fetch_builtin() {
        assert_eq!(exec_fetch("print(fetch(\"a.txt\"))").unwrap(), "5\n");
        assert_eq!(exec_fetch("s = fetch(\"b.txt\")\nprint(s)").unwrap(), "12\n");
        // Missing file (download failure) -> nil
        assert_eq!(exec_fetch("print(fetch(\"missing.txt\"))").unwrap(), "nil\n");
        // Filename can come from a variable
        assert_eq!(exec_fetch("f = \"a.txt\"\nprint(fetch(f))").unwrap(), "5\n");
        // Multiple downloads in one script
        assert_eq!(
            exec_fetch("a = fetch(\"a.txt\")\nb = fetch(\"b.txt\")\nprint(a + b)").unwrap(),
            "17\n"
        );
    }

    #[test]
    fn fetch_errors() {
        // Wrong arity or argument type
        assert!(exec_fetch("fetch()").is_err());
        assert!(exec_fetch("fetch(5)").is_err());
        assert!(exec_fetch("fetch(true)").is_err());
        // No host callback installed (plain `run`) -> clear error
        assert!(exec("print(fetch(\"a.txt\"))").is_err());
    }

    /// Mock `dofile()` loader callback: returns the source for known chunk
    /// names, `None` for anything else (simulating a failed load).
    fn load_demo(name: &str, buf: &mut [u8]) -> Option<usize> {
        let src: &[u8] = match name {
            "fortytwo.lua" => b"return 42",
            "greet.lua" => b"print(\"hello from dofile\")\nreturn 7",
            "addone.lua" => b"g = g + 1\nreturn g",
            "defn.lua" => b"function fromfile() return 3 end",
            "chunk.lua" => b"x = 99",
            "a.lua" => b"return dofile(\"b.lua\")",
            "b.lua" => b"return 9",
            "bad.lua" => b"print(1 / 0)",
            _ => return None,
        };
        if src.len() > buf.len() {
            return None;
        }
        buf[..src.len()].copy_from_slice(src);
        Some(src.len())
    }

    fn exec_dofile(src: &str) -> Result<String, &'static str> {
        OUT.with(|o| o.borrow_mut().clear());
        super::run_with_fetch_load(src.as_bytes(), putc_test, fetch_count, load_demo)?;
        Ok(OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap()))
    }

    #[test]
    fn dofile_builtin() {
        // Returns the chunk's return value.
        assert_eq!(exec_dofile("print(dofile(\"fortytwo.lua\"))").unwrap(), "42\n");
        // Chunk output plus its return value.
        assert_eq!(
            exec_dofile("print(dofile(\"greet.lua\"))").unwrap(),
            "hello from dofile\n7\n"
        );
        // The chunk runs in the same global environment.
        assert_eq!(
            exec_dofile("g = 1\nprint(dofile(\"addone.lua\"))\nprint(g)").unwrap(),
            "2\n2\n"
        );
        // A chunk can define functions usable by the caller afterwards.
        assert_eq!(exec_dofile("dofile(\"defn.lua\")\nprint(fromfile())").unwrap(), "3\n");
        // A chunk that returns nothing -> nil.
        assert_eq!(exec_dofile("print(dofile(\"chunk.lua\"))").unwrap(), "nil\n");
        // Nested dofile: a chunk can call dofile itself.
        assert_eq!(exec_dofile("print(dofile(\"a.lua\"))").unwrap(), "9\n");
        // The filename can come from a variable.
        assert_eq!(
            exec_dofile("f = \"fortytwo.lua\"\nprint(dofile(f))").unwrap(),
            "42\n"
        );
    }

    #[test]
    fn dofile_errors() {
        // Missing file -> error propagates.
        assert!(exec_dofile("dofile(\"missing.lua\")").is_err());
        // A runtime error inside the chunk propagates to the caller.
        assert!(exec_dofile("print(dofile(\"bad.lua\"))").is_err());
        // Wrong arity or argument type.
        assert!(exec_dofile("dofile()").is_err());
        assert!(exec_dofile("dofile(5)").is_err());
        assert!(exec_dofile("dofile(true)").is_err());
        // No loader callback installed (plain `run`) -> clear error.
        assert!(exec("dofile(\"fortytwo.lua\")").is_err());
    }

    /// Mock `dhcp()` host callback: network setup succeeds and enables `fetch`.
    fn dhcp_ok() -> Option<fn(&str) -> Option<usize>> {
        Some(fetch_count)
    }

    /// Mock `dhcp()` host callback: network setup fails.
    fn dhcp_fail() -> Option<fn(&str) -> Option<usize>> {
        None
    }

    /// Run a script with a mock `dhcp` callback installed (fetch disabled).
    fn exec_dhcp(src: &str, dhcp: fn() -> Option<fn(&str) -> Option<usize>>) -> Result<String, &'static str> {
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = super::LuaState::new();
        state.register_builtins(putc_test);
        state.set_fetch(None);
        state.set_dhcp(Some(dhcp));
        let first = {
            let mut p = super::parse::Parser::new(src.as_bytes(), &mut state);
            p.parse_script()?
        };
        super::eval::exec_script(&mut state, first)?;
        Ok(OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap()))
    }

    #[test]
    fn dhcp_builtin() {
        // dhcp() returns true when the setup succeeds, then fetch works.
        assert_eq!(exec_dhcp("print(dhcp())", dhcp_ok).unwrap(), "true\n");
        assert_eq!(exec_dhcp("dhcp()\nprint(fetch(\"a.txt\"))", dhcp_ok).unwrap(), "5\n");
        // Bare `dhcp` as a statement works too.
        assert_eq!(exec_dhcp("dhcp\nprint(fetch(\"b.txt\"))", dhcp_ok).unwrap(), "12\n");
        // dhcp() returns false when the setup fails; fetch stays disabled.
        assert_eq!(exec_dhcp("print(dhcp())", dhcp_fail).unwrap(), "false\n");
        assert!(exec_dhcp("dhcp()\nprint(fetch(\"a.txt\"))", dhcp_fail).is_err());
    }

    #[test]
    fn dhcp_errors() {
        // No host callback installed (plain `run`) -> clear error.
        assert!(exec("dhcp()").is_err());
        assert!(exec("dhcp").is_err());
        // Wrong arity.
        assert!(exec_dhcp("dhcp(1)", dhcp_ok).is_err());
    }

    #[test]
    fn dhcp_prints_type() {
        assert_eq!(exec("print(dhcp)").unwrap(), "dhcp\n");
    }

    #[test]
    fn shell_builtin_exists() {
        // shell() returns ExecResult::Shell which propagates up — we test
        // that the builtin is callable by checking it doesn't error out
        // during parsing / evaluation before the shell result is returned.
        assert_eq!(exec("shell").unwrap(), "");
    }

    #[test]
    fn exit_builtin_exists() {
        assert_eq!(exec("exit").unwrap(), "");
    }

    #[test]
    fn repl_builtin_registration() {
        // Verify builtins are registered when using register_builtins + run_repl_once.
        let mut state = super::LuaState::new();
        state.register_builtins(putc_test);
        state.set_fetch(None);
        let mut lines = vec!["exit".to_string(), "print(exit)".to_string(), "print(shell)".to_string()];
        super::run_repl(
            |buf: &mut [u8]| -> Option<usize> {
                if let Some(line) = lines.pop() {
                    let bytes = line.as_bytes();
                    buf[..bytes.len()].copy_from_slice(bytes);
                    Some(bytes.len())
                } else {
                    None
                }
            },
            |c| {
                OUT.with(|o| o.borrow_mut().push(c));
            },
        )
        .unwrap();
        OUT.with(|o| {
            let out = String::from_utf8(o.borrow().clone()).unwrap();
            assert!(out.contains("shell"));
            assert!(out.contains("exit"));
        });
    }

    #[test]
    fn shell_wrong_args() {
        assert!(exec("shell(1)").is_err());
    }

    #[test]
    fn exit_wrong_args() {
        assert!(exec("exit(1)").is_err());
    }

    #[test]
    fn shell_prints_type() {
        assert_eq!(exec("print(shell)").unwrap(), "shell\n");
    }

    #[test]
    fn exit_prints_type() {
        assert_eq!(exec("print(exit)").unwrap(), "exit\n");
    }

    #[test]
    fn repl_single_line() {
        let mut lines = vec!["print(1 + 2)".to_string()];
        let result = super::run_repl(
            |buf: &mut [u8]| -> Option<usize> {
                if let Some(line) = lines.pop() {
                    let bytes = line.as_bytes();
                    buf[..bytes.len()].copy_from_slice(bytes);
                    Some(bytes.len())
                } else {
                    None
                }
            },
            |c| {
                OUT.with(|o| o.borrow_mut().push(c));
            },
        );
        assert!(result.is_ok());
        OUT.with(|o| {
            let out = String::from_utf8(o.borrow().clone()).unwrap();
            assert!(out.contains("> "));
            assert!(out.contains("3"));
        });
    }

    #[test]
    fn repl_exit() {
        let mut lines = vec!["1 + 1".to_string(), "exit".to_string()];
        let result = super::run_repl(
            |buf: &mut [u8]| -> Option<usize> {
                if let Some(line) = lines.pop() {
                    let bytes = line.as_bytes();
                    buf[..bytes.len()].copy_from_slice(bytes);
                    Some(bytes.len())
                } else {
                    None
                }
            },
            |c| {
                OUT.with(|o| o.borrow_mut().push(c));
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn repl_error_handling() {
        let mut lines = vec!["print(undefined_var)".to_string(), "exit".to_string()];
        let result = super::run_repl(
            |buf: &mut [u8]| -> Option<usize> {
                if let Some(line) = lines.pop() {
                    let bytes = line.as_bytes();
                    buf[..bytes.len()].copy_from_slice(bytes);
                    Some(bytes.len())
                } else {
                    None
                }
            },
            |c| {
                OUT.with(|o| o.borrow_mut().push(c));
            },
        );
        assert!(result.is_ok());
    }

    #[test]
    fn repl_preserves_state() {
        let mut lines = vec!["exit".to_string(), "print(x)".to_string(), "x = 42".to_string()];
        let result = super::run_repl(
            |buf: &mut [u8]| -> Option<usize> {
                if let Some(line) = lines.pop() {
                    let bytes = line.as_bytes();
                    buf[..bytes.len()].copy_from_slice(bytes);
                    Some(bytes.len())
                } else {
                    None
                }
            },
            |c| {
                OUT.with(|o| o.borrow_mut().push(c));
            },
        );
        assert!(result.is_ok());
        OUT.with(|o| {
            let out = String::from_utf8(o.borrow().clone()).unwrap();
            assert!(out.contains("42"));
        });
    }

    #[test]
    fn repl_bare_expression_prints() {
        // Bare expressions at the REPL prompt should evaluate and print.
        let mut state = super::LuaState::new();
        state.register_builtins(putc_test);
        state.set_fetch(None);
        let mut lines = vec!["exit".to_string(), "x".to_string(), "x = 42".to_string(), "1 + 2".to_string()];
        super::run_repl(
            |buf: &mut [u8]| -> Option<usize> {
                if let Some(line) = lines.pop() {
                    let bytes = line.as_bytes();
                    buf[..bytes.len()].copy_from_slice(bytes);
                    Some(bytes.len())
                } else {
                    None
                }
            },
            |c| {
                OUT.with(|o| o.borrow_mut().push(c));
            },
        )
        .unwrap();
        OUT.with(|o| {
            let out = String::from_utf8(o.borrow().clone()).unwrap();
            assert!(out.contains("3"));
            assert!(out.contains("42"));
        });
    }
}

