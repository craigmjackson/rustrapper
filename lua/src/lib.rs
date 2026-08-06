//! Minimal Lua interpreter subset for rustrapper (`no_std`, no heap).
//!
//! Supported subset:
//! - Integer numbers, strings (single/double quotes with escapes), booleans, `nil`
//! - `local` and global variables
//! - Arithmetic `+ - * / %`, comparison `== ~= < <= > >=`
//! - `and` / `or` (short-circuit) / `not`, string concat `..`
//! - `if` / `elseif` / `else` / `end`, `while ... do ... end`
//! - Numeric `for i = a, b [, step] do ... end`
//! - Named functions `function name(a, b) ... end` and `return`
//! - Tables: array fields, `name =` fields, `[expr]` fields, `t.key`, `t[key]`
//! - `print(...)` builtin, `--` line comments
//!
//! Not supported: closures/upvalues, floats, `local function`, anonymous
//! function literals, string methods, `repeat`, `break`, multiple assignment.
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
    /// Builtin function; only `print` exists today (`Native(0)`).
    Native(u8),
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
    /// `target = value`.
    AssignStmt(u16, u16),
    /// Expression statement (a call).
    CallStmt(u16),
    /// `if cond then .. elseif .. else .. end`.
    IfStmt(u16, u16, u16),
    /// `while cond do .. end`.
    WhileStmt(u16, u16),
    /// `for i = start, limit [, step] do .. end`.
    ForStmt(u16, u16, u16, u16, u16),
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
        }
    }

    /// Parse and execute a Lua script. `source` must remain valid for the
    /// whole call (identifiers reference into it). Output from `print()`
    /// goes to `putc`.
    pub fn run(&mut self, source: &[u8], putc: fn(u8)) -> Result<(), &'static str> {
        self.reset();
        self.putc = putc;
        let print_name = self.intern(b"print")?;
        self.set_global(print_name, Value::Native(0));
        let first = {
            let mut p = parse::Parser::new(source, self);
            p.parse_script()?
        };
        eval::exec_script(self, first)
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

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::string::String;
    use std::thread_local;
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
    fn for_loop() {
        assert_eq!(exec("for i = 1, 3 do print(i) end").unwrap(), "1\n2\n3\n");
        assert_eq!(exec("for i = 3, 1, -1 do print(i) end").unwrap(), "3\n2\n1\n");
        assert_eq!(exec("for i = 1, 10, 3 do print(i) end").unwrap(), "1\n4\n7\n10\n");
        assert_eq!(exec("for i = 1, 0 do print(i) end\nprint(0)").unwrap(), "0\n");
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

    #[test]
    fn demo_script() {
        let src = std::fs::read_to_string("demo/test.lua").unwrap();
        assert_eq!(
            exec(&src).unwrap(),
            "Hello from Lua!\nfib(10) = 55\nrustrapper\tarm64\t2\nsum = 15\n"
        );
    }
}

