//! Minimal Lua interpreter subset for rustrapper (`no_std`, no heap).
//!
//! Supported subset:
//! - Integer and floating-point numbers (IEEE-754 doubles), strings
//!   (single/double quotes with escapes), booleans, `nil`
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
//! - Functions: named (`function name(a, b) ... end`), `local function
//!   name(...)`, and anonymous (`function(...) ... end`) literals, plus
//!   `return`. Functions capture enclosing locals as shared upvalues
//!   (closures): two closures over the same local see each other's writes, and
//!   each `for` iteration (and each execution of a block declaring a captured
//!   local) captures a fresh variable.
//! - Tables: array fields, `name =` fields, `[expr] =` fields, `t.key`, `t[key]`
//! - `print(...)` builtin, `--` line comments
//! - `next(t [, k])` builtin: returns the next key/value pair of a table (or
//!   `nil` at the end / for an empty table). Calls can yield multiple values,
//!   which `local a, b = f()`, `a, b = f()`, `return f()` and a final call
//!   argument (`print(f())`) consume (`next`/`pairs` yield two, `select` any
//!   number). `t[k] = nil` removes the field.
//! - `select(index, ...)` builtin: with a number index, returns the arguments
//!   after that position (-1 is the last); with `"#"`, returns the number of
//!   extra arguments.
//! - `setmetatable(table, mt|nil)` builtin: stores/removes a table's
//!   metatable (returning the table). Dispatched metamethods: `__index`,
//!   `__newindex`, `__eq`, `__lt`, `__le`, `__concat`, `__add`, `__sub`,
//!   `__mul`, `__div`, `__mod`, `__unm`, `__call`, `__tostring`, `__pairs`.
//!   A non-nil `__metatable` field protects the metatable (changing it
//!   errors).
//! - `tonumber(e [, base])` builtin: converts a number or a decimal numeric
//!   string to an integer/float (or `nil`); with a `base` (2..36) the first
//!   argument must be a string and the result is an integer in that base.
//! - `tostring(v)` builtin: the human-readable string form of any value (the
//!   same rendering `print` uses). A table's `__tostring` metamethod is
//!   dispatched (it must return a string).
//! - `pairs(t)` builtin: returns the `next` function and the table, so
//!   `for k, v in pairs(t) do ... end` iterates every key/value pair. A
//!   `__pairs` metamethod is dispatched when present.
//! - `rawequal(v1, v2)` builtin: primitive equality, never `__eq`.
//! - `rawget(table, index)` builtin: the real `table[index]`, never `__index`.
//! - `rawlen(v)` builtin: length of a table or string without `__len`. A
//!   table's length is the run of consecutive integer keys starting at 1.
//! - `rawset(table, index, value)` builtin: the real `table[index] = value`
//!   without `__newindex`, returning the table. The index may not be nil or
//!   NaN (plain assignment enforces the same rule).
//! - `type(v)` builtin: the Lua type name of a value (native builtins count
//!   as functions). `_VERSION` is the string `"Lua 5.5"`.
//! - `warn(msg, ...)` builtin: concatenates its string/number arguments and
//!   emits `Lua warning: <msg>`; the control messages `"@on"`/`"@off"` toggle
//!   warnings.
//! - `pcall(f, ...)` builtin: protected call — `true` plus the results on
//!   success, or `false` plus the error object on failure. A coroutine may
//!   `yield` through `pcall` (the protected frame is kept across the yield).
//! - `error(v [, level])` builtin: raises `v` as an error object (`level` is
//!   validated but ignored; there are no source positions).
//! - `dhcp` / `dhcp()` builtin: runs the network setup (e1000 + DHCP) and
//!   enables the `fetch()` builtin. The REPL starts with networking disabled
//!   until the user runs `dhcp`.
//! - `fetch("file")` / `fetch("file", "dest")` builtin: downloads a file from
//!   the TFTP server (DHCP `next_server`) into host memory, saving it under the
//!   optional local `dest` name (defaults to the source name; shown by `ls`),
//!   and returns its byte count as a number, or `nil` if the download fails.
//!   Requires a host callback, so scripts using it must run through
//!   [`LuaState::run_with_fetch`] / [`run_with_fetch`].
//! - `dofile("file")` builtin: loads a Lua source chunk by name (e.g. via
//!   TFTP), executes it in the current interpreter state, and returns the
//!   chunk's return value (`nil` if it doesn't return). Errors inside the
//!   chunk propagate to the caller. Requires a host loader callback, so
//!   scripts using it must run through [`LuaState::run_with_fetch_load`] /
//!   [`run_with_fetch_load`].
//! - `ls` / `ls()` builtin: lists the files downloaded with `fetch()` this
//!   run/session, one per line as `name (N bytes)` (nothing if none yet).
//! - `dhcp` / `dhcp()` builtin: runs the network setup (e1000 + DHCP) and, on
//!   success, prints the negotiated details — MAC, IP, subnet, gateway, TFTP
//!   server, and bootfile — from the host `dhcp_info` callback, and exposes
//!   them as the `mac` / `ip` / `subnet` / `gateway` / `server` / `bootfile`
//!   globals (strings) plus a numeric `tftp_port` (default 69) from the host
//!   `dhcp_values` callback.
//! - `coroutine.create/resume/yield/status/wrap/isyieldable/running/close`:
//!   real coroutines (a pool of [`MAX_COS`] plus the main thread). Each
//!   coroutine owns a thread state swapped in on resume, so `yield` works at
//!   any call depth; `type(co)` is `"thread"`.
//!
//! The parser produces an AST which [`vm`] compiles to bytecode and runs on a
//! stack machine (no execution state on the Rust stack). Builtins and runtime
//! helpers live in [`eval`].
//!
//! Not supported: string methods, right-hand-side expression lists
//! (`a, b = 1, 2`), yielding across a metamethod or `dofile`, and generic
//! `for` iterators that are not the `next`/table pair (`__pairs` must return a
//! table or `pairs(t)`).
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
pub mod vm;

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
/// Maximum number of distinct files recorded by successful `fetch()` calls
/// (for the `ls` builtin).
pub const MAX_FETCHED: usize = 16;
/// Maximum number of live coroutines (plus the main thread).
pub const MAX_COS: usize = 3;
/// Thread slots: the main thread plus [`MAX_COS`] coroutines.
pub const MAX_THREADS: usize = MAX_COS + 1;

/// Number of dispatched metamethod names (see `MM_NAMES` in [`crate::eval`]).
pub const MM_COUNT: usize = 15;
/// Maximum upvalues per closure.
pub const MAX_UPVALS: usize = 8;
/// Maximum captured-variable cells (no GC: each capture allocates one).
pub const MAX_CELLS: usize = 64;
/// Maximum live closures.
pub const MAX_CLOSURES: usize = 32;
/// Sentinel: a local holds its value directly (not in a cell).
pub const NO_CELL: u16 = u16::MAX;
/// Sentinel: a frame is not a closure (no captured upvalues).
pub const NO_CLOSURE: u16 = u16::MAX;

/// Coroutine status values (stored in [`CoState::status`]).
pub const CO_DEAD: u8 = 0;
pub const CO_SUSPENDED: u8 = 1;
pub const CO_RUNNING: u8 = 2;
pub const CO_NORMAL: u8 = 3;
/// Scratch buffer used by `dhcp()` to hold the formatted network details
/// (MAC / IP / subnet / gateway / TFTP server / bootfile) from the host.
pub const DHCP_INFO_CAP: usize = 384;
/// Capacity of the compiled bytecode instruction array.
pub const MAX_CODE: usize = 4096;
/// "All results" sentinel for `Call`/`Return` instruction result counts.
pub const WANT_ALL: u16 = u16::MAX;

/// Sentinel for "no node" / end-of-chain. Node indices are well below this.
pub const NO_NODE: u16 = u16::MAX;

/// Reference into the string arena, packed as `(offset << 16) | len`.
pub type StrRef = u32;

/// Pack an (offset, length) pair into a [`StrRef`].
#[inline]
pub fn strref(off: u16, len: u16) -> StrRef {
    ((off as u32) << 16) | (len as u32)
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

fn fmt_dec(v: u32, buf: &mut [u8], n: &mut usize) {
    let mut tmp = [0u8; 10];
    let mut i = tmp.len();
    let mut x = v;
    loop {
        i -= 1;
        tmp[i] = b'0' + (x % 10) as u8;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    while i < tmp.len() {
        buf[*n] = tmp[i];
        *n += 1;
        i += 1;
    }
}

/// Format an IPv4 address as `A.B.C.D`, returning the byte length.
fn fmt_dotted(ip: &[u8; 4], buf: &mut [u8]) -> usize {
    let mut n = 0usize;
    for i in 0..4 {
        if i > 0 {
            buf[n] = b'.';
            n += 1;
        }
        fmt_dec(ip[i] as u32, buf, &mut n);
    }
    n
}

/// Format a MAC address as `XX:XX:XX:XX:XX:XX`, returning the byte length.
fn fmt_mac(mac: &[u8; 6], buf: &mut [u8]) -> usize {
    let mut n = 0usize;
    for i in 0..6 {
        if i > 0 {
            buf[n] = b':';
            n += 1;
        }
        buf[n] = HEX[(mac[i] >> 4) as usize];
        n += 1;
        buf[n] = HEX[(mac[i] & 0x0F) as usize];
        n += 1;
    }
    n
}

/// A runtime value. Numbers are 64-bit integers or IEEE-754 doubles.
#[derive(Clone, Copy, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Num(i64),
    /// Floating-point number (IEEE-754 double).
    Float(f64),
    Str(StrRef),
    Table(u16),
    Func(u16),
    /// Builtin function: `print` is `Native(0)`, `fetch` is `Native(1)`,
    /// `dofile` is `Native(2)`, `next` is `Native(3)`, `pairs` is `Native(4)`,
    /// `rawequal` is `Native(5)`, `rawget` is `Native(6)`, `rawlen` is
    /// `Native(7)`, `rawset` is `Native(8)`, `select` is `Native(9)`,
    /// `setmetatable` is `Native(10)`, `tonumber` is `Native(11)`,
    /// `tostring` is `Native(12)`, `type` is `Native(13)`, `warn` is
    /// `Native(14)`, `pcall` is `Native(15)`, `error` is `Native(16)`, and
    /// 17..=24 are the `coroutine` library (create, resume, yield, status,
    /// wrap, isyieldable, running, close).
    Native(u8),
    /// Builtin: `shell()` — enters the interactive Lua REPL.
    Shell,
    /// Builtin: `dhcp` / `dhcp()` — runs the network setup so `fetch()` works.
    Dhcp,
    /// Builtin: `exit()` — exits the REPL (only meaningful inside a shell).
    Exit,
    /// Builtin: `ls` / `ls()` — lists the files downloaded with `fetch()`.
    Ls,
    /// A coroutine thread value (index into `LuaState::cos`); `type()` is
    /// "thread".
    Co(u8),
    /// A wrapped coroutine (`coroutine.wrap`) — callable, resumes the
    /// coroutine and propagates errors; `type()` is "function".
    Wrapped(u8),
    /// A closure (function plus captured upvalue cells), index into
    /// [`LuaState::closures`].
    Closure(u16),
}

/// A runtime error. Internal errors carry a static message; the `error()`
/// builtin can raise any Lua value ([`LuaError::Obj`]), which `pcall` returns
/// unchanged. The `?` operator converts `&'static str` errors automatically.
#[derive(Clone, Copy)]
pub enum LuaError {
    /// An internal/runtime error with a static message.
    Msg(&'static str),
    /// An error object raised by `error(v)` — any Lua value.
    Obj(Value),
}

impl From<&'static str> for LuaError {
    fn from(msg: &'static str) -> Self {
        LuaError::Msg(msg)
    }
}

impl core::fmt::Debug for LuaError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            LuaError::Msg(m) => f.write_str(m),
            LuaError::Obj(Value::Str(_)) => f.write_str("error object is a string value"),
            LuaError::Obj(Value::Nil) => f.write_str("error object is a nil value"),
            LuaError::Obj(Value::Bool(_)) => f.write_str("error object is a boolean value"),
            LuaError::Obj(Value::Num(_)) | LuaError::Obj(Value::Float(_)) => {
                f.write_str("error object is a number value")
            }
            LuaError::Obj(Value::Table(_)) => f.write_str("error object is a table value"),
            LuaError::Obj(_) => f.write_str("error object is a function value"),
        }
    }
}

/// Structured DHCP result, filled by the host `dhcp_values` callback after a
/// successful `dhcp` and exposed to scripts as the `mac` / `ip` / `subnet` /
/// `gateway` / `server` / `bootfile` / `tftp_port` globals.
#[derive(Clone, Copy)]
pub struct DhcpValues {
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub subnet: [u8; 4],
    pub gateway: [u8; 4],
    pub server: [u8; 4],
    /// TFTP bootfile name, null-terminated.
    pub bootfile: [u8; 128],
    /// TFTP server port (defaults to 69).
    pub tftp_port: u16,
}

impl Default for DhcpValues {
    fn default() -> Self {
        DhcpValues {
            mac: [0; 6],
            ip: [0; 4],
            subnet: [0; 4],
            gateway: [0; 4],
            server: [0; 4],
            bootfile: [0; 128],
            tftp_port: 69,
        }
    }
}

/// Binary and unary operators. `And`/`Or` are handled with short-circuiting.
#[derive(Clone, Copy, PartialEq)]
#[repr(u8)]
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
    /// Floating-point literal (IEEE-754 double).
    Float(f64),
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
    /// `local function name(...) ... end` — `name` (a `Var` node) plus the
    /// function index; the local is declared before the closure is assigned so
    /// the body can recurse.
    LocalFunc(u16, u16),
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
    /// Numeric `for` whose control variable is captured by a closure: each
    /// iteration gets a fresh cell.
    ForStmtCell(u16, u16, u16, u16, u16),
    /// `for k [, v] in table do .. end` — iterate a table's key/value pairs.
    /// Fields are (key_var, value_var or `NO_NODE`, table_expr, body).
    ForInStmt(u16, u16, u16, u16),
    /// Generic `for` whose variables are captured by a closure: each iteration
    /// gets fresh cells.
    ForInStmtCell(u16, u16, u16, u16),
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
/// nodes starting at `params`; `body` is the first statement of its block and
/// `entry` is the bytecode offset the compiler assigned to the body.
#[derive(Clone, Copy)]
pub struct FuncDef {
    pub params: u16,
    pub nparams: u8,
    pub body: u16,
    pub entry: u16,
    /// Names of the upvalues this function captures (resolved by the parser).
    pub up_names: [StrRef; MAX_UPVALS],
    pub nup: u8,
}

/// One captured upvalue: the name and the cell holding its value
/// ([`NO_CELL`] means the name resolves to a global at call time).
#[derive(Clone, Copy)]
pub struct UpEntry {
    pub name: StrRef,
    pub cell: u16,
}

/// A closure: a function plus the upvalue cells captured when it was created.
#[derive(Clone, Copy)]
pub struct ClosureDef {
    pub func: u16,
    pub ups: [UpEntry; MAX_UPVALS],
    pub nup: u8,
}

/// One bytecode instruction. `a`/`b` are operands whose meaning depends on the
/// opcode (node indices, jump targets, counts).
#[derive(Clone, Copy)]
pub struct Instr {
    pub op: InstrOp,
    pub a: u16,
    pub b: u16,
}

/// Bytecode opcodes for the stack-machine interpreter in [`crate::vm`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum InstrOp {
    /// No operation / safety terminator (used to initialize the code array).
    Halt,
    /// Push the constant stored in `nodes[a]` (Num/Float/Str/Nil/True/False).
    PushConst,
    /// Push `nil`.
    PushNil,
    /// Push the small integer `a` (sign-extended); used for `for` step defaults.
    PushInt,
    /// Push the value of the variable named by `Var` node `a`.
    GetVar,
    /// Pop a value and assign it (local if it exists, else global).
    SetVar,
    /// Pop a value and assign it to the global named by `Var` node `a`.
    SetGlobal,
    /// Pop a value and set/declare the local named by `Var` node `a`.
    SetLocalTop,
    /// Like `SetLocalTop`, but into a fresh cell (per-iteration capture).
    SetLocalTopCell,
    /// Declare the names chained from node `a` (b values on the stack).
    DeclareLocal,
    /// Pop key then base, push `base[key]`.
    GetIndex,
    /// Pop value, key, base; set `base[key] = value`.
    SetIndex,
    /// Assign a target list: `a` = first target node, `b` = number of values.
    /// Index targets' base/key pairs are on the stack below the values.
    AssignMulti,
    /// Push a fresh empty table.
    NewTable,
    /// Duplicate the top of the stack.
    Dup,
    /// Pop and discard the top of the stack.
    Pop,
    /// Binary operator (`a` = AST op); pops right then left, pushes result.
    BinOp,
    /// Unary operator (`a` = AST op); pops, pushes result.
    UnOp,
    /// Jump to `a`.
    Jump,
    /// Pop `b` frames, then jump to `a` (break/goto out of scopes).
    JumpScopes,
    /// Pop a value; jump to `a` when it is falsy.
    JumpIfFalse,
    /// Peek: jump to `a` (keeping the value) when falsy, else pop.
    JumpIfFalseKeep,
    /// Peek: jump to `a` (keeping the value) when truthy, else pop.
    JumpIfTrueKeep,
    /// Push `Value::Func(a)`.
    LoadFunc,
    /// Push a closure for function `a`, capturing the upvalue cells its
    /// `FuncDef` lists.
    MakeClosure,
    /// Record the current operand-stack position for a dynamic call.
    Mark,
    /// Call: `a` arguments plus callee on the stack; leave `b` results
    /// (`WANT_ALL` = every result). `a == WANT_ALL` means the argument count is
    /// dynamic (a final call argument expanded): it is the values between the
    /// last `Mark` and the top.
    Call,
    /// Return: `a` results on the stack (`WANT_ALL` = all values above the
    /// function's stack base).
    Return,
    /// Pop a value and print it (bare expression statement).
    Print,
    /// Pop a value; handle the `exit`/`shell`/`dhcp`/`ls` statement forms.
    StmtCheck,
    /// Push a fresh local scope.
    PushScope,
    /// Pop the innermost local scope.
    PopScope,
    /// Numeric `for` setup: pops start/limit/step, sets the loop control in
    /// the current frame, declares the loop variable (`b` = Var node), and
    /// jumps to `a` when the loop body must be skipped.
    ForPrep,
    /// Numeric `for` step: increments the control, sets the loop variable
    /// (`b`), and jumps to the body at `a` when it is still in range.
    ForLoop,
    /// Like `ForPrep`, but the captured loop variable gets a fresh cell.
    ForPrepCell,
    /// Like `ForLoop`, but the captured loop variable gets a fresh cell.
    ForLoopCell,
    /// Generic `for` setup: pops the two `pairs(t)`/table values and records
    /// the table in the current frame.
    ForInPrep,
    /// Generic `for` next: jumps to `a` when the iteration is finished, else
    /// pushes the current key and value.
    ForInNext,
    /// Generic `for` advance: moves to the next slot (removal-aware) and jumps
    /// back to the `ForInNext` at `a`.
    ForInAdvance,
    /// Suspend the current coroutine, yielding `a` values (reserved for the
    /// coroutine implementation).
    Yield,
}

/// One execution context (the main script or a coroutine). The running
/// thread's fields live in [`LuaState`] directly; suspended threads are kept
/// in [`LuaState::threads`] and swapped in/out on resume/yield.
#[derive(Clone, Copy)]
pub struct Thread {
    pub vstack: [Value; STACK_CAP],
    pub vsp: u32,
    pub frames: [Frame; MAX_FRAMES],
    pub fsp: u32,
    pub call_marks: [u32; MAX_FRAMES],
    pub mark_sp: u8,
    pub steps: u64,
}

impl Thread {
    pub(crate) fn empty() -> Self {
        Thread {
            vstack: [Value::Nil; STACK_CAP],
            vsp: 0,
            frames: [Frame {
                locals: [Local {
                    name: 0,
                    value: Value::Nil,
                    cell: NO_CELL,
                }; MAX_LOCALS],
                count: 0,
                ret_ip: 0,
                want: 0,
                base: 0,
                ctrl: [Value::Nil; 4],
                closure: NO_CLOSURE,
                protected: false,
                mark_sp: 0,
            }; MAX_FRAMES],
            fsp: 0,
            call_marks: [0; MAX_FRAMES],
            mark_sp: 0,
            steps: 0,
        }
    }
}

/// A local variable slot within a frame. When `cell != NO_CELL` the value
/// lives in [`LuaState::cells`] (the local was captured by a closure, so all
/// accesses must go through the shared cell).
#[derive(Clone, Copy)]
pub struct Local {
    pub name: StrRef,
    pub value: Value,
    pub cell: u16,
}

/// One call/block scope: a fixed array of locals, plus call bookkeeping
/// (`ret_ip`/`want`/`base`) and the loop control slots used by `for`.
#[derive(Clone, Copy)]
pub struct Frame {
    pub locals: [Local; MAX_LOCALS],
    pub count: u8,
    /// Return instruction pointer for a function frame.
    pub ret_ip: u16,
    /// Number of results the caller wants (`WANT_ALL` for all).
    pub want: u16,
    /// Operand-stack position where this function's results belong.
    pub base: u32,
    /// Loop control slots (numeric/generic `for`).
    pub ctrl: [Value; 4],
    /// Closure running in this frame ([`NO_CLOSURE`] for plain functions and
    /// block scopes); its upvalues are visible to name lookup.
    pub closure: u16,
    /// This frame is a VM-level `pcall` boundary: errors raised inside it are
    /// caught and turned into `false, error` (and a coroutine may yield
    /// through it).
    pub protected: bool,
    /// Operand-stack marks in effect when the protected frame was pushed.
    pub mark_sp: u8,
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
    /// Metatable, set by `setmetatable`; dispatched by `tget`/`tset`/`binop`
    /// (`__index`, `__newindex`, `__eq`, `__lt`, `__le`, `__concat`,
    /// arithmetic, `__call`, `__tostring`, `__pairs`).
    pub mt: Option<u16>,
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
    /// Compiled bytecode for the chunks parsed so far.
    pub code: [Instr; MAX_CODE],
    pub ncode: u16,
    pub strings: [u8; STR_CAP],
    pub strregs: [StrReg; MAX_STRINGS],
    pub nstrings: u32,
    pub str_next: u32,
    pub vstack: [Value; STACK_CAP],
    pub vsp: u32,
    /// Operand-stack marks for dynamic calls (a final call argument that
    /// expands to several values).
    pub call_marks: [u32; MAX_FRAMES],
    pub mark_sp: u8,
    pub frames: [Frame; MAX_FRAMES],
    pub fsp: u32,
    pub funcs: [FuncDef; MAX_FUNCS],
    pub funcs_used: u32,
    pub globals: [Global; MAX_GLOBALS],
    pub nglobals: u32,
    pub tbls: [TableRec; MAX_TABLES],
    pub ntables: u32,
    pub steps: u64,
    /// Files successfully downloaded by `fetch()`, for the `ls` builtin
    /// (names are interned; lengths are the byte counts fetch returned).
    pub fetched_names: [StrRef; MAX_FETCHED],
    pub fetched_lens: [u64; MAX_FETCHED],
    pub fetched_n: u8,
    /// Character output callback used by `print()`.
    pub putc: fn(u8),
    /// Host callback for the `fetch()` builtin: downloads `source` from the
    /// TFTP server, saving it under the local name `save_as` (equal to
    /// `source` when `fetch` is called with one argument), and returns its byte
    /// count, or `None` on failure. Set by [`LuaState::run_with_fetch`] or by a
    /// successful `dhcp`; `None` means `fetch()` errors out.
    pub fetch: Option<fn(source: &str, save_as: &str) -> Option<usize>>,
    /// Host callback for the `dhcp` builtin: runs the network setup (e1000 +
    /// DHCP) and returns the `fetch` callback if a TFTP server is reachable.
    /// Set by the host before entering the REPL; `None` means `dhcp` errors.
    pub dhcp: Option<fn() -> Option<fn(source: &str, save_as: &str) -> Option<usize>>>,
    /// Host callback for the `dofile()` builtin: loads the Lua source for
    /// `name` (e.g. via TFTP) into `buf` and returns its length, or `None` if
    /// the file can't be loaded. The interpreter owns `buf` (`DOFILE_CAP`
    /// bytes); the callback must not write past it. `None` means `dofile()`
    /// errors out.
    pub load: Option<fn(&str, &mut [u8]) -> Option<usize>>,
    /// Host callback for the `dhcp` builtin result: formats the negotiated
    /// network details (MAC, IP, subnet, gateway, TFTP server, bootfile) into
    /// `buf`, returning the number of bytes written. The interpreter prints
    /// the bytes after a successful `dhcp`. `None` means nothing is printed.
    pub dhcp_info: Option<fn(&mut [u8]) -> usize>,
    /// Host callback for the `dhcp` builtin result: fills the structured
    /// [`DhcpValues`] (mac, ip, subnet, gateway, server, bootfile), which the
    /// interpreter exposes as Lua globals after a successful `dhcp`.
    pub dhcp_values: Option<fn(&mut DhcpValues)>,
    /// Whether `warn()` emits messages; toggled by the `"@on"`/`"@off"`
    /// control messages. Persists across `run()` calls in the same state.
    pub warn_on: bool,
    /// Saved execution contexts: index 0 is the main thread, 1.. are
    /// coroutines.
    pub threads: [Thread; MAX_THREADS],
    /// Index of the currently running thread.
    pub current: u8,
    /// Coroutine records (one per thread slot 1..=MAX_COS).
    pub cos: [CoState; MAX_COS],
    /// Continuation saved by the VM when a coroutine yields (the instruction
    /// after the `yield` call, and the number of results it expects).
    pub yield_ip: u16,
    pub yield_want: u16,
    /// Captured-variable cells (bump-allocated, no GC).
    pub cells: [Value; MAX_CELLS],
    pub ncells: u16,
    /// Live closures.
    pub closures: [ClosureDef; MAX_CLOSURES],
    pub nclosures: u16,
    /// Interned metamethod names (`__index`, `__add`, ...), filled lazily.
    pub mm_refs: [StrRef; MM_COUNT],
    pub mm_ready: bool,
    /// Nesting depth of metamethod dispatch, to catch `__index`/`__call`
    /// loops.
    pub mm_depth: u16,
}

/// A coroutine record. `func` is the body; `ip`/`want` are the resume point
/// saved when the coroutine yielded.
#[derive(Clone, Copy)]
pub struct CoState {
    pub status: u8,
    pub func: Value,
    pub ip: u16,
    /// Number of results the suspended `yield` call expects on resume.
    pub want: u16,
    /// Thread index that resumed this coroutine.
    pub parent: u8,
    pub started: bool,
}

fn noop(_c: u8) {}

impl LuaState {
    /// Create a fresh, empty interpreter state.
    pub fn new() -> Self {
        LuaState {
            nodes: [Node::Empty; MAX_NODES],
            next: [NO_NODE; MAX_NODES],
            node_used: 0,
            code: [Instr {
                op: InstrOp::Halt,
                a: 0,
                b: 0,
            }; MAX_CODE],
            ncode: 0,
            strings: [0u8; STR_CAP],
            strregs: [StrReg { off: 0, len: 0 }; MAX_STRINGS],
            nstrings: 0,
            str_next: 0,
            vstack: [Value::Nil; STACK_CAP],
            vsp: 0,
            call_marks: [0; MAX_FRAMES],
            mark_sp: 0,
            frames: [Frame {
                locals: [Local {
                    name: 0,
                    value: Value::Nil,
                    cell: NO_CELL,
                }; MAX_LOCALS],
                count: 0,
                ret_ip: 0,
                want: 0,
                base: 0,
                ctrl: [Value::Nil; 4],
                closure: NO_CLOSURE,
                protected: false,
                mark_sp: 0,
            }; MAX_FRAMES],
            fsp: 0,
            funcs: [FuncDef {
                params: 0,
                nparams: 0,
                body: 0,
                entry: 0,
                up_names: [0; MAX_UPVALS],
                nup: 0,
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
                mt: None,
            }; MAX_TABLES],
            ntables: 0,
            steps: 0,
            fetched_names: [0; MAX_FETCHED],
            fetched_lens: [0; MAX_FETCHED],
            fetched_n: 0,
            putc: noop,
            fetch: None,
            dhcp: None,
            load: None,
            dhcp_info: None,
            dhcp_values: None,
            warn_on: true,
            threads: [Thread::empty(); MAX_THREADS],
            current: 0,
            cos: [CoState {
                status: CO_DEAD,
                func: Value::Nil,
                ip: 0,
                want: 0,
                parent: 0,
                started: false,
            }; MAX_COS],
            yield_ip: 0,
            yield_want: 0,
            cells: [Value::Nil; MAX_CELLS],
            ncells: 0,
            closures: [ClosureDef {
                func: 0,
                ups: [UpEntry {
                    name: 0,
                    cell: NO_CELL,
                }; MAX_UPVALS],
                nup: 0,
            }; MAX_CLOSURES],
            nclosures: 0,
            mm_refs: [0; MM_COUNT],
            mm_ready: false,
            mm_depth: 0,
        }
    }

    /// Switch the flat execution fields from thread `from` to thread `to`,
    /// saving `from`'s state and loading `to`'s.
    pub fn swap_threads(&mut self, from: u8, to: u8) {
        if from == to {
            return;
        }
        {
            let t = &mut self.threads[to as usize];
            core::mem::swap(&mut self.vstack, &mut t.vstack);
            core::mem::swap(&mut self.vsp, &mut t.vsp);
            core::mem::swap(&mut self.frames, &mut t.frames);
            core::mem::swap(&mut self.fsp, &mut t.fsp);
            core::mem::swap(&mut self.call_marks, &mut t.call_marks);
            core::mem::swap(&mut self.mark_sp, &mut t.mark_sp);
            core::mem::swap(&mut self.steps, &mut t.steps);
        }
        let tmp = self.threads[to as usize];
        self.threads[to as usize] = self.threads[from as usize];
        self.threads[from as usize] = tmp;
        self.current = to;
    }

    /// Register built-in globals (`print`, `fetch`, `shell`, `dhcp`, `dofile`,
    /// `exit`, `ls`, the standard library functions, and `_VERSION`). Call
    /// this once after creating a fresh `LuaState` before entering the REPL.
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
        let _ = self.intern(b"ls");
        let ls_name = self.intern(b"ls").unwrap();
        self.set_global(ls_name, Value::Ls);
        let _ = self.intern(b"next");
        let next_name = self.intern(b"next").unwrap();
        self.set_global(next_name, Value::Native(3));
        let _ = self.intern(b"pairs");
        let pairs_name = self.intern(b"pairs").unwrap();
        self.set_global(pairs_name, Value::Native(4));
        let _ = self.intern(b"rawequal");
        let rawequal_name = self.intern(b"rawequal").unwrap();
        self.set_global(rawequal_name, Value::Native(5));
        let _ = self.intern(b"rawget");
        let rawget_name = self.intern(b"rawget").unwrap();
        self.set_global(rawget_name, Value::Native(6));
        let _ = self.intern(b"rawlen");
        let rawlen_name = self.intern(b"rawlen").unwrap();
        self.set_global(rawlen_name, Value::Native(7));
        let _ = self.intern(b"rawset");
        let rawset_name = self.intern(b"rawset").unwrap();
        self.set_global(rawset_name, Value::Native(8));
        let _ = self.intern(b"select");
        let select_name = self.intern(b"select").unwrap();
        self.set_global(select_name, Value::Native(9));
        let _ = self.intern(b"setmetatable");
        let setmetatable_name = self.intern(b"setmetatable").unwrap();
        self.set_global(setmetatable_name, Value::Native(10));
        let _ = self.intern(b"tonumber");
        let tonumber_name = self.intern(b"tonumber").unwrap();
        self.set_global(tonumber_name, Value::Native(11));
        let _ = self.intern(b"tostring");
        let tostring_name = self.intern(b"tostring").unwrap();
        self.set_global(tostring_name, Value::Native(12));
        let _ = self.intern(b"type");
        let type_name = self.intern(b"type").unwrap();
        self.set_global(type_name, Value::Native(13));
        let _ = self.intern(b"Lua 5.5");
        let version_value = self.intern(b"Lua 5.5").unwrap();
        let _ = self.intern(b"_VERSION");
        let version_name = self.intern(b"_VERSION").unwrap();
        self.set_global(version_name, Value::Str(version_value));
        let _ = self.intern(b"warn");
        let warn_name = self.intern(b"warn").unwrap();
        self.set_global(warn_name, Value::Native(14));
        let _ = self.intern(b"pcall");
        let pcall_name = self.intern(b"pcall").unwrap();
        self.set_global(pcall_name, Value::Native(15));
        let _ = self.intern(b"error");
        let error_name = self.intern(b"error").unwrap();
        self.set_global(error_name, Value::Native(16));
        // The `coroutine` library is a table of builtins.
        let coroutine_name = self.intern(b"coroutine").unwrap();
        let tid = crate::eval::new_table(self).unwrap();
        let fields: [(&[u8], u8); 8] = [
            (b"create", 17),
            (b"resume", 18),
            (b"yield", 19),
            (b"status", 20),
            (b"wrap", 21),
            (b"isyieldable", 22),
            (b"running", 23),
            (b"close", 24),
        ];
        for &(name, id) in fields.iter() {
            let n = self.intern(name).unwrap();
            crate::eval::tset(self, Value::Table(tid), Value::Str(n), Value::Native(id)).unwrap();
        }
        self.set_global(coroutine_name, Value::Table(tid));
    }

    /// Parse and execute a Lua script. `source` must remain valid for the
    /// whole call (identifiers reference into it). Output from `print()`
    /// goes to `putc`.
    pub fn run(&mut self, source: &[u8], putc: fn(u8)) -> Result<(), LuaError> {
        self.reset();
        self.putc = putc;
        self.register_builtins(putc);
        let first = {
            let mut p = parse::Parser::new(source, self);
            p.parse_script()?
        };
        let entry = vm::compile(self, first)?;
        vm::exec_script(self, entry)
    }

    /// Install a host `fetch()` callback. Call `set_fetch(None)` to explicitly
    /// disable fetch (e.g. when no TFTP server is reachable). The REPL starts
    /// with `fetch` disabled and enables it only after a successful `dhcp`.
    pub fn set_fetch(&mut self, fetch: Option<fn(source: &str, save_as: &str) -> Option<usize>>) {
        self.fetch = fetch;
    }

    /// Install a host `dhcp` callback: runs the network setup (e1000 + DHCP)
    /// and returns the `fetch` callback when a TFTP server is reachable.
    /// Call `set_dhcp(None)` to disable the `dhcp` builtin.
    pub fn set_dhcp(
        &mut self,
        dhcp: Option<fn() -> Option<fn(source: &str, save_as: &str) -> Option<usize>>>,
    ) {
        self.dhcp = dhcp;
    }

    /// Install a host callback that formats the negotiated network details
    /// (MAC / IP / subnet / gateway / TFTP server / bootfile) into a buffer
    /// for the `dhcp` builtin to print. Call `set_dhcp_info(None)` to print
    /// nothing after `dhcp`.
    pub fn set_dhcp_info(&mut self, dhcp_info: Option<fn(&mut [u8]) -> usize>) {
        self.dhcp_info = dhcp_info;
    }

    /// Install a host callback that fills the structured [`DhcpValues`] so the
    /// `dhcp` builtin can expose `mac` / `ip` / `subnet` / `gateway` / `server`
    /// / `bootfile` as Lua globals. Call `set_dhcp_values(None)` to set nothing.
    pub fn set_dhcp_values(&mut self, dhcp_values: Option<fn(&mut DhcpValues)>) {
        self.dhcp_values = dhcp_values;
    }

    /// Populate the `mac` / `ip` / `subnet` / `gateway` / `server` / `bootfile`
    /// globals (as interned strings) and the numeric `tftp_port` global from
    /// the host [`DhcpValues`]. Called after a successful `dhcp`. Strings are
    /// interned, so comparisons like `ip == "10.0.0.15"` work by identity.
    pub fn set_dhcp_globals(&mut self) -> Result<(), &'static str> {
        let cb = self.dhcp_values;
        if let Some(f) = cb {
            let mut v = DhcpValues::default();
            f(&mut v);
            let mac_name = self.intern(b"mac")?;
            let ip_name = self.intern(b"ip")?;
            let subnet_name = self.intern(b"subnet")?;
            let gateway_name = self.intern(b"gateway")?;
            let server_name = self.intern(b"server")?;
            let bootfile_name = self.intern(b"bootfile")?;
            let tftp_port_name = self.intern(b"tftp_port")?;
            let mut mac_buf = [0u8; 17];
            let mac_len = fmt_mac(&v.mac, &mut mac_buf);
            let mac_val = self.intern(&mac_buf[..mac_len])?;
            let mut ip_buf = [0u8; 15];
            let ip_len = fmt_dotted(&v.ip, &mut ip_buf);
            let ip_val = self.intern(&ip_buf[..ip_len])?;
            let mut sub_buf = [0u8; 15];
            let subnet_len = fmt_dotted(&v.subnet, &mut sub_buf);
            let subnet_val = self.intern(&sub_buf[..subnet_len])?;
            let mut gw_buf = [0u8; 15];
            let gateway_len = fmt_dotted(&v.gateway, &mut gw_buf);
            let gateway_val = self.intern(&gw_buf[..gateway_len])?;
            let mut srv_buf = [0u8; 15];
            let server_len = fmt_dotted(&v.server, &mut srv_buf);
            let server_val = self.intern(&srv_buf[..server_len])?;
            let bf_len = v
                .bootfile
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(v.bootfile.len());
            let bootfile_val = self.intern(&v.bootfile[..bf_len])?;
            self.set_global(mac_name, Value::Str(mac_val));
            self.set_global(ip_name, Value::Str(ip_val));
            self.set_global(subnet_name, Value::Str(subnet_val));
            self.set_global(gateway_name, Value::Str(gateway_val));
            self.set_global(server_name, Value::Str(server_val));
            self.set_global(bootfile_name, Value::Str(bootfile_val));
            self.set_global(tftp_port_name, Value::Num(v.tftp_port as i64));
        }
        Ok(())
    }

    /// Emit the negotiated DHCP details via `putc` after a successful `dhcp`.
    pub fn emit_dhcp_info(&mut self) {
        if let Some(f) = self.dhcp_info {
            let mut buf = [0u8; DHCP_INFO_CAP];
            let n = f(&mut buf).min(buf.len());
            for &b in &buf[..n] {
                (self.putc)(b);
            }
        }
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
                    self.set_dhcp_globals()?;
                    Ok(true)
                }
                None => Ok(false),
            },
            None => Err("dhcp not available"),
        }
    }

    /// Record a successful `fetch()` download for the `ls` builtin. `name` is
    /// the (already-interned) filename of the fetched file, `len` the byte
    /// count the fetch returned. Re-fetching a recorded name updates its
    /// length instead of adding a duplicate entry.
    pub fn record_fetch_ref(&mut self, name: StrRef, len: u64) -> Result<(), &'static str> {
        for i in 0..self.fetched_n as usize {
            if self.fetched_names[i] == name {
                self.fetched_lens[i] = len;
                return Ok(());
            }
        }
        if self.fetched_n as usize >= MAX_FETCHED {
            return Err("too many fetched files");
        }
        let i = self.fetched_n as usize;
        self.fetched_names[i] = name;
        self.fetched_lens[i] = len;
        self.fetched_n += 1;
        Ok(())
    }

    /// Run a script with a host `fetch()` callback installed, so the script
    /// can call `fetch("file")` to download files from the TFTP server.
    pub fn run_with_fetch(
        &mut self,
        source: &[u8],
        putc: fn(u8),
        fetch: fn(source: &str, save_as: &str) -> Option<usize>,
    ) -> Result<(), LuaError> {
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
        fetch: fn(source: &str, save_as: &str) -> Option<usize>,
        load: fn(&str, &mut [u8]) -> Option<usize>,
    ) -> Result<(), LuaError> {
        self.fetch = Some(fetch);
        self.load = Some(load);
        self.run(source, putc)
    }

    fn reset(&mut self) {
        self.node_used = 0;
        self.next = [NO_NODE; MAX_NODES];
        self.ncode = 0;
        self.nstrings = 0;
        self.str_next = 0;
        self.strings = [0u8; STR_CAP];
        self.strregs = [StrReg { off: 0, len: 0 }; MAX_STRINGS];
        self.vsp = 0;
        self.mark_sp = 0;
        self.fsp = 0;
        self.funcs_used = 0;
        self.nglobals = 0;
        self.ntables = 0;
        self.steps = 0;
        self.fetched_n = 0;
        self.current = 0;
        self.mm_ready = false;
        self.mm_depth = 0;
        self.ncells = 0;
        self.nclosures = 0;
        for c in self.cos.iter_mut() {
            c.status = CO_DEAD;
        }
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
    pub fn alloc_func(
        &mut self,
        params: u16,
        nparams: u8,
        body: u16,
        up_names: &[StrRef],
    ) -> Result<u16, &'static str> {
        if self.funcs_used as usize >= MAX_FUNCS {
            return Err("too many functions");
        }
        if up_names.len() > MAX_UPVALS {
            return Err("too many upvalues");
        }
        let mut ups = [0; MAX_UPVALS];
        ups[..up_names.len()].copy_from_slice(up_names);
        let idx = self.funcs_used;
        self.funcs[idx as usize] = FuncDef {
            params,
            nparams,
            body,
            entry: 0,
            up_names: ups,
            nup: up_names.len() as u8,
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
                cell: NO_CELL,
            }; MAX_LOCALS],
            count: 0,
            ret_ip: 0,
            want: 0,
            base: 0,
            ctrl: [Value::Nil; 4],
            closure: NO_CLOSURE,
            protected: false,
            mark_sp: 0,
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
        self.frames[f].locals[i] = Local {
            name,
            value,
            cell: NO_CELL,
        };
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
                let cell = self.frames[f].locals[i].cell;
                if cell == NO_CELL {
                    self.frames[f].locals[i].value = value;
                } else {
                    self.cells[cell as usize] = value;
                }
                return Ok(());
            }
        }
        self.declare_local(name, value)
    }

    /// Set a captured loop variable to a *fresh* cell (each `for` iteration
    /// gets its own cell, so closures created in the body capture that
    /// iteration's value). Declares the local on the first iteration.
    pub fn set_local_top_cell(&mut self, name: StrRef, value: Value) -> Result<(), &'static str> {
        if self.fsp == 0 {
            return Err("no active scope");
        }
        let cell = self.alloc_cell(value)?;
        let f = (self.fsp - 1) as usize;
        for i in 0..self.frames[f].count as usize {
            if self.frames[f].locals[i].name == name {
                self.frames[f].locals[i].cell = cell;
                return Ok(());
            }
        }
        if self.frames[f].count as usize >= MAX_LOCALS {
            return Err("too many locals");
        }
        let i = self.frames[f].count as usize;
        self.frames[f].locals[i] = Local {
            name,
            value: Value::Nil,
            cell,
        };
        self.frames[f].count += 1;
        Ok(())
    }

    /// The current function's frames: block scopes (ret_ip == 0) above the
    /// call frame. Returns the frame index of the function frame, if any.
    fn function_frame(&self) -> Option<usize> {
        let mut f = self.fsp as usize;
        while f > 0 {
            f -= 1;
            if self.frames[f].ret_ip != 0 {
                return Some(f);
            }
        }
        None
    }

    /// Set a local in the current function's scopes (innermost outwards), or
    /// an upvalue cell of the running closure. Returns whether found.
    pub fn assign_local(&mut self, name: StrRef, value: Value) -> bool {
        let mut f = self.fsp as usize;
        while f > 0 {
            f -= 1;
            let mut found = None;
            for i in 0..self.frames[f].count as usize {
                if self.frames[f].locals[i].name == name {
                    found = Some(i);
                    break;
                }
            }
            if let Some(i) = found {
                let cell = self.frames[f].locals[i].cell;
                if cell == NO_CELL {
                    self.frames[f].locals[i].value = value;
                } else {
                    self.cells[cell as usize] = value;
                }
                return true;
            }
            if self.frames[f].ret_ip != 0 {
                let ci = self.frames[f].closure;
                if ci != NO_CLOSURE {
                    for i in 0..self.closures[ci as usize].nup as usize {
                        let up = self.closures[ci as usize].ups[i];
                        if up.name == name {
                            if up.cell != NO_CELL {
                                self.cells[up.cell as usize] = value;
                            }
                            return true;
                        }
                    }
                }
                break;
            }
        }
        false
    }

    /// Look up a variable: the current function's locals, then the running
    /// closure's upvalues, then globals (lexical scoping).
    pub fn lookup(&self, name: StrRef) -> Option<Value> {
        let mut f = self.fsp as usize;
        while f > 0 {
            f -= 1;
            for i in 0..self.frames[f].count as usize {
                let l = self.frames[f].locals[i];
                if l.name == name {
                    return Some(if l.cell == NO_CELL {
                        l.value
                    } else {
                        self.cells[l.cell as usize]
                    });
                }
            }
            if self.frames[f].ret_ip != 0 {
                let ci = self.frames[f].closure;
                if ci != NO_CLOSURE {
                    let c = &self.closures[ci as usize];
                    for i in 0..c.nup as usize {
                        if c.ups[i].name == name {
                            let cell = c.ups[i].cell;
                            return Some(if cell == NO_CELL {
                                Value::Nil
                            } else {
                                self.cells[cell as usize]
                            });
                        }
                    }
                }
                break;
            }
        }
        for i in 0..self.nglobals as usize {
            if self.globals[i].name == name {
                return Some(self.globals[i].value);
            }
        }
        None
    }

    /// Allocate a captured-variable cell holding `value`.
    pub fn alloc_cell(&mut self, value: Value) -> Result<u16, &'static str> {
        if self.ncells as usize >= MAX_CELLS {
            return Err("too many captured variables");
        }
        let i = self.ncells;
        self.cells[i as usize] = value;
        self.ncells += 1;
        Ok(i)
    }

    /// Allocate a closure slot for `func`.
    pub fn alloc_closure(&mut self, func: u16) -> Result<u16, &'static str> {
        if self.nclosures as usize >= MAX_CLOSURES {
            return Err("too many closures");
        }
        let i = self.nclosures;
        self.closures[i as usize] = ClosureDef {
            func,
            ups: [UpEntry {
                name: 0,
                cell: NO_CELL,
            }; MAX_UPVALS],
            nup: 0,
        };
        self.nclosures += 1;
        Ok(i)
    }

    /// Find a local with `name` in the current function's scopes and return
    /// its cell, boxing it (allocating a cell) if it is still direct.
    pub fn find_or_box_local_cell(&mut self, name: StrRef) -> Result<Option<u16>, &'static str> {
        let mut f = self.fsp as usize;
        while f > 0 {
            f -= 1;
            let mut found = None;
            for i in 0..self.frames[f].count as usize {
                if self.frames[f].locals[i].name == name {
                    found = Some(i);
                    break;
                }
            }
            if let Some(i) = found {
                let cell = self.frames[f].locals[i].cell;
                if cell != NO_CELL {
                    return Ok(Some(cell));
                }
                let value = self.frames[f].locals[i].value;
                let cell = self.alloc_cell(value)?;
                self.frames[f].locals[i].cell = cell;
                return Ok(Some(cell));
            }
            if self.frames[f].ret_ip != 0 {
                break;
            }
        }
        Ok(None)
    }

    /// The cell of an upvalue of the running closure, by name.
    pub fn closure_up_cell(&self, name: StrRef) -> Option<u16> {
        let f = self.function_frame()?;
        let ci = self.frames[f].closure;
        if ci == NO_CLOSURE {
            return None;
        }
        let c = &self.closures[ci as usize];
        for i in 0..c.nup as usize {
            if c.ups[i].name == name {
                return Some(c.ups[i].cell);
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
pub fn run(source: &[u8], putc: fn(u8)) -> Result<(), LuaError> {
    let mut state = LuaState::new();
    state.run(source, putc)
}

/// Convenience wrapper: run a script with a fresh [`LuaState`] and a host
/// `fetch()` callback installed.
pub fn run_with_fetch(
    source: &[u8],
    putc: fn(u8),
    fetch: fn(source: &str, save_as: &str) -> Option<usize>,
) -> Result<(), LuaError> {
    let mut state = LuaState::new();
    state.run_with_fetch(source, putc, fetch)
}

/// Convenience wrapper: run a script with a fresh [`LuaState`] and both host
/// `fetch()` and `dofile()` callbacks installed.
pub fn run_with_fetch_load(
    source: &[u8],
    putc: fn(u8),
    fetch: fn(source: &str, save_as: &str) -> Option<usize>,
    load: fn(&str, &mut [u8]) -> Option<usize>,
) -> Result<(), LuaError> {
    let mut state = LuaState::new();
    state.run_with_fetch_load(source, putc, fetch, load)
}

/// Run the interactive Lua REPL. `read_line` should write a line into the
/// provided buffer and return its length, or `None` when input is exhausted
/// (exits the REPL). Output from `print()` and the prompt go to `putc`.
pub fn run_repl(
    mut read_line: impl FnMut(&mut [u8]) -> Option<usize>,
    putc: fn(u8),
) -> Result<(), LuaError> {
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
        match vm::run_repl_once(&mut state, &buf[..len]) {
            Ok(eval::ExecResult::Normal) => {}
            Ok(eval::ExecResult::Break) => {}
            Ok(eval::ExecResult::Goto(_)) => {}
            Ok(eval::ExecResult::Ret(_)) => {}
            Ok(eval::ExecResult::Ret2(..)) => {}
            Ok(eval::ExecResult::RetN(_)) => {}
            Ok(eval::ExecResult::Exit) => break,
            Ok(eval::ExecResult::Yield(_)) => {}
            Ok(eval::ExecResult::Shell) => {
                // Nested shell — not supported in this simple REPL.
                putc(b'\n');
                putc(b'\n');
            }
            Err(e) => {
                putc(b'\n');
                eval::emit_error(&state, e, putc);
                putc(b'\n');
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::LuaError;
    use std::cell::RefCell;
    use std::format;
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

    fn exec(src: &str) -> Result<String, LuaError> {
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
    fn floats() {
        // Literals (including `.5`, `5.` and exponents).
        assert_eq!(exec("print(5.5)").unwrap(), "5.5\n");
        assert_eq!(exec("print(5.0)").unwrap(), "5.0\n");
        assert_eq!(exec("print(.5 + 5.)").unwrap(), "5.5\n");
        assert_eq!(exec("print(1e3)").unwrap(), "1000.0\n");
        assert_eq!(exec("print(2.5e-2)").unwrap(), "0.025\n");
        // Mixed int/float arithmetic promotes to float.
        assert_eq!(exec("print(5.0 + 5)").unwrap(), "10.0\n");
        assert_eq!(exec("print(5.5 * 2)").unwrap(), "11.0\n");
        assert_eq!(exec("print(10.0 / 4)").unwrap(), "2.5\n");
        assert_eq!(exec("print(-5.5)").unwrap(), "-5.5\n");
        // Integer arithmetic is unchanged.
        assert_eq!(exec("print(10 / 3)").unwrap(), "3\n");
        // Float division and floor-based modulo (Lua semantics).
        assert_eq!(exec("print(5.0 / 3)").unwrap(), "1.6666666666667\n");
        assert_eq!(exec("print(7.0 % 3)").unwrap(), "1.0\n");
        assert_eq!(exec("print(-7.0 % 3)").unwrap(), "2.0\n");
        assert_eq!(exec("print(2.5 % 1)").unwrap(), "0.5\n");
        // Comparisons and equality mix ints and floats.
        assert_eq!(exec("print(1 == 1.0)").unwrap(), "true\n");
        assert_eq!(exec("print(1 ~= 2.0)").unwrap(), "true\n");
        assert_eq!(exec("print(1 < 1.5)").unwrap(), "true\n");
        assert_eq!(exec("print(2.0 <= 2)").unwrap(), "true\n");
        assert_eq!(exec("print(3.5 >= 4)").unwrap(), "false\n");
        // Concat coerces floats to strings.
        assert_eq!(exec("print(\"x=\" .. 1.5)").unwrap(), "x=1.5\n");
        // Numeric for loops iterate in floating point when a bound is a float.
        assert_eq!(
            exec("for i = 1, 2, 0.5 do print(i) end").unwrap(),
            "1.0\n1.5\n2.0\n"
        );
        // Float table keys are the same key as equal integers.
        assert_eq!(
            exec("t = {}\nt[2.0] = \"two\"\nprint(t[2])").unwrap(),
            "two\n"
        );
        // Non-numbers still error.
        assert!(exec("print(\"a\" + 1)").is_err());
    }

    #[test]
    fn float_formatting() {
        assert_eq!(exec("print(0.1 + 0.2)").unwrap(), "0.3\n");
        assert_eq!(exec("print(1.5e-5)").unwrap(), "1.5e-05\n");
        assert_eq!(exec("print(1e14)").unwrap(), "1e+14\n");
        assert_eq!(exec("print(1.0e300 * 10)").unwrap(), "1e+301\n");
        assert_eq!(exec("print(12345678901234.0)").unwrap(), "12345678901234.0\n");
        assert_eq!(exec("print(-0.0)").unwrap(), "-0.0\n");
        // Float division by zero is inf/nan, unlike the integer error.
        assert_eq!(exec("print(1.0 / 0)").unwrap(), "inf\n");
        assert_eq!(exec("print(-1.0 / 0)").unwrap(), "-inf\n");
        assert_eq!(exec("print(0.0 / 0)").unwrap(), "nan\n");
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
        // Removing the current field during traversal neither skips nor
        // re-visits entries.
        assert_eq!(
            exec("t = {a = 1, b = 2, c = 3}\nfor k, v in t do\nif k == \"b\" then t[k] = nil end\nprint(k, v)\nend").unwrap(),
            "a\t1\nb\t2\nc\t3\n"
        );
    }

    #[test]
    fn next_builtin() {
        // Empty table -> nil (the empty-table test).
        assert_eq!(exec("print(next({}))").unwrap(), "nil\n");
        assert_eq!(exec("print(next({}) == nil, next({1}) == nil)").unwrap(), "true\tfalse\n");
        // next(t) returns the first index and value, in slot order.
        assert_eq!(exec("print(next({10, 20}))").unwrap(), "1\t10\n");
        // Full traversal with multiple assignment.
        assert_eq!(
            exec("t = {10, 20, x = 30}\nk, v = next(t)\nprint(k, v)\nk, v = next(t, k)\nprint(k, v)\nk, v = next(t, k)\nprint(k, v)\nprint(next(t, k))").unwrap(),
            "1\t10\n2\t20\nx\t30\nnil\n"
        );
        // Multiple locals take the two results.
        assert_eq!(
            exec("local k, v = next({a = 7})\nprint(k, v)").unwrap(),
            "a\t7\n"
        );
        // Missing values default to nil.
        assert_eq!(
            exec("local a, b, c = next({z = 1})\nprint(a, b, c)").unwrap(),
            "z\t1\tnil\n"
        );
        // A function can forward both results with `return next(t)`.
        assert_eq!(
            exec("function f() return next({q = 7}) end\nprint(f())").unwrap(),
            "q\t7\n"
        );
        // A final call argument expands; a non-final one is truncated.
        assert_eq!(exec("function g(a, b) print(a, b) end\ng(next({5}))").unwrap(), "1\t5\n");
        assert_eq!(exec("print(next({5}), \"x\")").unwrap(), "1\tx\n");
        // Assigning nil to a visited field removes it from the traversal.
        assert_eq!(
            exec("t = {a = 1, b = 2}\nk, v = next(t)\nt[k] = nil\nprint(next(t))").unwrap(),
            "b\t2\n"
        );
        // Invalid key / non-table / wrong arity error.
        assert!(exec("next({1}, 5)").is_err());
        assert!(exec("next(5)").is_err());
        assert!(exec("next()").is_err());
        assert!(exec("next({}, 1, 2)").is_err());
    }

    #[test]
    fn pairs_builtin() {
        // `for ... in pairs(t)` iterates every key/value pair.
        assert_eq!(
            exec("t = {\"a\", \"b\"}\nfor k, v in pairs(t) do print(k, v) end").unwrap(),
            "1\ta\n2\tb\n"
        );
        // Keys only.
        assert_eq!(
            exec("t = {x = 1, y = 2}\nfor k in pairs(t) do print(k) end").unwrap(),
            "x\ny\n"
        );
        // Empty table -> no iterations.
        assert_eq!(
            exec("for k, v in pairs({}) do print(1) end\nprint(0)").unwrap(),
            "0\n"
        );
        // pairs(t) yields the next function and the table as its first two
        // results, so it can also be captured and used manually.
        assert_eq!(exec("local f, s = pairs({7})\nprint(f(s))").unwrap(), "1\t7\n");
        // The (implicit) third result is nil, like Lua's control variable.
        assert_eq!(exec("local f, s, c = pairs({7})\nprint(c)").unwrap(), "nil\n");
        // A call yielding a table also works (`p(t)` -> t).
        assert_eq!(
            exec("function p(t) return t end\nfor k, v in p({9}) do print(k, v) end").unwrap(),
            "1\t9\n"
        );
        // Errors: non-table argument, wrong arity.
        assert!(exec("pairs(5)").is_err());
        assert!(exec("pairs()").is_err());
        assert!(exec("pairs({}, {})").is_err());
        assert!(exec("for k in pairs(5) do end").is_err());
    }

    #[test]
    fn rawequal_builtin() {
        assert_eq!(exec("print(rawequal(1, 1), rawequal(1, 2))").unwrap(), "true\tfalse\n");
        // Numbers compare across int/float subtypes.
        assert_eq!(exec("print(rawequal(1, 1.0))").unwrap(), "true\n");
        assert_eq!(exec("print(rawequal(nil, nil), rawequal(nil, false))").unwrap(), "true\tfalse\n");
        assert_eq!(exec("print(rawequal(true, true), rawequal(true, false))").unwrap(), "true\tfalse\n");
        assert_eq!(exec("print(rawequal(\"a\", \"a\"), rawequal(\"a\", \"b\"))").unwrap(), "true\tfalse\n");
        // Tables/functions compare by identity.
        assert_eq!(
            exec("t = {}\nprint(rawequal(t, t), rawequal({}, {}))").unwrap(),
            "true\tfalse\n"
        );
        assert_eq!(
            exec("print(rawequal(print, print), rawequal(print, next))").unwrap(),
            "true\tfalse\n"
        );
        assert_eq!(exec("print(rawequal(dhcp, dhcp), rawequal(dhcp, ls))").unwrap(), "true\tfalse\n");
        assert_eq!(
            exec("print((1 == 1.0) == rawequal(1, 1.0))").unwrap(),
            "true\n"
        );
        assert!(exec("rawequal(1)").is_err());
        assert!(exec("rawequal(1, 2, 3)").is_err());
    }

    #[test]
    fn rawget_builtin() {
        assert_eq!(
            exec("t = {10, x = 20}\nprint(rawget(t, 1), rawget(t, \"x\"))").unwrap(),
            "10\t20\n"
        );
        // A missing key yields nil.
        assert_eq!(
            exec("print(rawget({}, 1), rawget({}, \"missing\"))").unwrap(),
            "nil\tnil\n"
        );
        // Any index value is accepted.
        assert_eq!(exec("t = {}\nprint(rawget(t, nil))").unwrap(), "nil\n");
        assert_eq!(exec("print(rawget({7}, 1.0))").unwrap(), "7\n");
        assert_eq!(exec("t = {5}\nprint(rawget(t, 1) == t[1])").unwrap(), "true\n");
        // Values compare by identity (a table-valued field).
        assert_eq!(
            exec("u = {}\nt = {u}\nprint(rawget(t, 1) == u)").unwrap(),
            "true\n"
        );
        // Errors: non-table / arity.
        assert!(exec("rawget(1, 1)").is_err());
        assert!(exec("rawget()").is_err());
        assert!(exec("rawget({})").is_err());
        assert!(exec("rawget({}, 1, 2)").is_err());
    }

    #[test]
    fn rawlen_builtin() {
        // Strings: byte length.
        assert_eq!(exec("print(rawlen(\"hello\"), rawlen(\"\"))").unwrap(), "5\t0\n");
        assert_eq!(exec("print(rawlen(\"a\\tb\"))").unwrap(), "3\n");
        // Tables: run of consecutive integer keys from 1.
        assert_eq!(exec("print(rawlen({1, 2, 3}), rawlen({}))").unwrap(), "3\t0\n");
        assert_eq!(exec("print(rawlen({1, 2, name = \"x\"}))").unwrap(), "2\n");
        assert_eq!(exec("print(rawlen({[2] = 5}))").unwrap(), "0\n");
        assert_eq!(exec("print(rawlen({[1] = 5, [3] = 7}))").unwrap(), "1\n");
        // Removing an array field shrinks the table.
        assert_eq!(exec("t = {1, 2, 3}\nt[3] = nil\nprint(rawlen(t))").unwrap(), "2\n");
        // Errors: non-table/string, arity.
        assert!(exec("rawlen(5)").is_err());
        assert!(exec("rawlen(nil)").is_err());
        assert!(exec("rawlen(true)").is_err());
        assert!(exec("rawlen()").is_err());
        assert!(exec("rawlen(\"a\", \"b\")").is_err());
    }

    #[test]
    fn rawset_builtin() {
        // Sets the field and returns the table (so results can be indexed).
        assert_eq!(exec("t = {}\nprint(rawset(t, 1, 10)[1])").unwrap(), "10\n");
        assert_eq!(exec("t = {}\nprint(rawset(t, \"x\", 5).x)").unwrap(), "5\n");
        // Returns the same table it was given.
        assert_eq!(
            exec("t = {}\nprint(rawequal(rawset(t, 1, 1), t))").unwrap(),
            "true\n"
        );
        // Any index value except nil/NaN; float keys match int keys.
        assert_eq!(exec("t = {}\nrawset(t, 1.0, 7)\nprint(t[1])").unwrap(), "7\n");
        assert_eq!(exec("t = {}\nrawset(t, true, 1)\nprint(t[true])").unwrap(), "1\n");
        // Setting nil removes the field.
        assert_eq!(
            exec("t = {1, 2}\nrawset(t, 1, nil)\nprint(t[1], rawlen(t))").unwrap(),
            "nil\t0\n"
        );
        // Errors: non-table, nil/NaN index, arity.
        assert!(exec("rawset(1, \"k\", 1)").is_err());
        assert!(exec("rawset({}, nil, 1)").is_err());
        assert!(exec("rawset({}, 0.0/0.0, 1)").is_err());
        assert!(exec("rawset({}, 1)").is_err());
        assert!(exec("rawset({}, 1, 2, 3)").is_err());
        // Plain assignment rejects nil/NaN keys too (Lua semantics), while
        // reading them is still fine.
        assert!(exec("t = {}\nt[nil] = 1").is_err());
        assert!(exec("t = {}\nt[0.0/0.0] = 1").is_err());
        assert_eq!(exec("t = {}\nprint(t[nil], t[0.0/0.0])").unwrap(), "nil\tnil\n");
        assert!(exec("t = {[0.0/0.0] = 1}").is_err());
    }

    #[test]
    fn select_builtin() {
        // "#" returns the number of extra arguments.
        assert_eq!(exec("print(select(\"#\", 1, 2, 3))").unwrap(), "3\n");
        assert_eq!(exec("print(select(\"#\"))").unwrap(), "0\n");
        // A number index returns the arguments after it.
        assert_eq!(exec("print(select(1, \"a\", \"b\", \"c\"))").unwrap(), "a\tb\tc\n");
        assert_eq!(exec("print(select(2, \"a\", \"b\", \"c\"))").unwrap(), "b\tc\n");
        assert_eq!(exec("print(select(3, \"a\", \"b\", \"c\"))").unwrap(), "c\n");
        // Negative indexes count from the end (-1 is the last argument).
        assert_eq!(exec("print(select(-1, \"a\", \"b\", \"c\"))").unwrap(), "c\n");
        assert_eq!(exec("print(select(-2, \"a\", \"b\", \"c\"))").unwrap(), "b\tc\n");
        // An index past the end yields no values.
        assert_eq!(exec("print(select(4, \"a\"))").unwrap(), "\n");
        // Any number of results can be consumed.
        assert_eq!(
            exec("local a, b, c = select(1, 10, 20, 30)\nprint(a, b, c)").unwrap(),
            "10\t20\t30\n"
        );
        assert_eq!(
            exec("local a, b, c = select(1)\nprint(a, b, c)").unwrap(),
            "nil\tnil\tnil\n"
        );
        // A non-final call argument is truncated to one value.
        assert_eq!(exec("print(select(1, \"a\", \"b\"), \"z\")").unwrap(), "a\tz\n");
        // Forwarding every result through a function.
        assert_eq!(
            exec("function f() return select(2, \"x\", \"y\", \"z\") end\nprint(f())").unwrap(),
            "y\tz\n"
        );
        // Integral float indexes are accepted.
        assert_eq!(exec("print(select(1.0, \"f\"))").unwrap(), "f\n");
        // Errors: index out of range, non-integer/strange index, arity.
        assert!(exec("select(0, 1)").is_err());
        assert!(exec("select(-4, 1, 2)").is_err());
        assert!(exec("select(\"x\", 1)").is_err());
        assert!(exec("select(1.5, 1)").is_err());
        assert!(exec("select()").is_err());
    }

    #[test]
    fn yield_across_pcall() {
        // Yielding through a protected call suspends and resumes correctly.
        assert_eq!(
            exec("function body()\nlocal x, y = coroutine.yield(1, 2)\nreturn x .. y\nend\nco = coroutine.create(function()\nlocal ok, v = pcall(body)\nprint(\"pcall:\", ok, v)\nend)\nprint(coroutine.resume(co))\nprint(coroutine.resume(co, \"a\", \"b\"))\nprint(coroutine.status(co))").unwrap(),
            "true\t1\t2\npcall:\ttrue\tab\ntrue\ndead\n"
        );
        // An error after the yield is caught by the protected call.
        assert_eq!(
            exec("function body()\ncoroutine.yield(\"a\")\nerror(\"late\")\nend\nco = coroutine.create(function()\nprint(\"inner:\", pcall(body))\nend)\nprint(coroutine.resume(co))\nprint(coroutine.resume(co))").unwrap(),
            "true\ta\ninner:\tfalse\tlate\ntrue\n"
        );
        // Nested protected calls with a yield in the middle.
        assert_eq!(
            exec("function deep()\nlocal a = coroutine.yield(1)\nreturn a + 1\nend\nco = coroutine.create(function()\nlocal ok1, ok2, v = pcall(function() return pcall(deep) end)\nprint(\"nested:\", ok1, ok2, v)\nend)\nprint(coroutine.resume(co))\nprint(coroutine.resume(co, 41))").unwrap(),
            "true\t1\nnested:\ttrue\ttrue\t42\ntrue\n"
        );
        // A closure body can yield through pcall and keeps its upvalues.
        assert_eq!(
            exec("function make()\nlocal n = 0\nreturn function()\nn = n + 1\ncoroutine.yield(n)\nreturn n\nend\nend\nco = coroutine.create(function()\nlocal ok, a = pcall(make())\nprint(\"done:\", ok, a)\nend)\nprint(coroutine.resume(co))\nprint(coroutine.resume(co))\nprint(coroutine.resume(co))").unwrap(),
            "true\t1\ndone:\ttrue\t1\ntrue\nfalse\tcannot resume dead coroutine\n"
        );
        // Plain errors are still caught, including error objects.
        assert_eq!(exec("print(pcall(function() error(\"boom\") end))").unwrap(), "false\tboom\n");
        assert_eq!(
            exec("print(pcall(function() error({code = 5}) end))").unwrap(),
            "false\ttable\n"
        );
        // pcall as a first-class value and callable tables.
        assert_eq!(exec("p = pcall\nprint(p(function() return 5 end))").unwrap(), "true\t5\n");
        assert_eq!(
            exec("print(pcall(setmetatable({}, {__call = function(self) return 9 end})))").unwrap(),
            "true\t9\n"
        );
        // Errors raised in metamethods called inside pcall are caught.
        assert_eq!(
            exec("function f() return setmetatable({}, {__index = function() error(\"mm\") end}).x end\nprint(pcall(f))").unwrap(),
            "false\tmm\n"
        );
    }

    #[test]
    fn closures() {
        // A closure captures a local by reference.
        assert_eq!(
            exec("function counter()\nlocal n = 0\nreturn function() n = n + 1 return n end\nend\nc = counter()\nprint(c(), c(), c())").unwrap(),
            "1\t2\t3\n"
        );
        // Independent invocations have independent upvalues.
        assert_eq!(
            exec("function counter()\nlocal n = 0\nreturn function() n = n + 1 return n end\nend\nc = counter()\nc()\nprint(counter()())").unwrap(),
            "1\n"
        );
        // Two closures share the same upvalue.
        assert_eq!(
            exec("function make()\nlocal x = 0\nt = {inc = function() x = x + 1 end, get = function() return x end}\nend\nmake()\nt.inc()\nt.inc()\nprint(t.get())").unwrap(),
            "2\n"
        );
        // The defining scope sees mutations made through the closure.
        assert_eq!(
            exec("function f()\nlocal x = 1\nlocal get = function() return x end\nx = 2\nreturn get()\nend\nprint(f())").unwrap(),
            "2\n"
        );
        // `local function` supports recursion.
        assert_eq!(
            exec("local function fact(n)\nif n <= 1 then return 1 end\nreturn n * fact(n - 1)\nend\nprint(fact(5))").unwrap(),
            "120\n"
        );
        // Parameters are captured.
        assert_eq!(
            exec("function adder(n)\nreturn function(x) return x + n end\nend\nadd5 = adder(5)\nprint(add5(10))").unwrap(),
            "15\n"
        );
        // Each numeric-for iteration captures its own variable.
        assert_eq!(
            exec("fns = {}\nfor i = 1, 3 do fns[i] = function() return i end end\nprint(fns[1](), fns[2](), fns[3]())").unwrap(),
            "1\t2\t3\n"
        );
        // Captured `local`s in a loop body are per-iteration.
        assert_eq!(
            exec("fns = {}\nfor i = 1, 2 do\nlocal v = i * 10\nfns[i] = function() return v end\nend\nprint(fns[1](), fns[2]())").unwrap(),
            "10\t20\n"
        );
        // Generic-for variables are captured per iteration.
        assert_eq!(
            exec("fns = {}\nfor k, v in {10, 20} do fns[k] = function() return v end end\nprint(fns[1](), fns[2]())").unwrap(),
            "10\t20\n"
        );
        // Transitive capture through nested closures.
        assert_eq!(
            exec("function outer()\nlocal a = 7\nreturn function()\nreturn function() return a end\nend\nend\nprint(outer()()())").unwrap(),
            "7\n"
        );
        // type()/tostring of a closure.
        assert_eq!(
            exec("function f()\nlocal x = 1\nreturn function() return x end\nend\nc = f()\nprint(type(c), tostring(c))").unwrap(),
            "function\tfunction\n"
        );
        // Closures as coroutine bodies keep their upvalues across yields.
        assert_eq!(
            exec("function f()\nlocal n = 0\nreturn function() while true do n = n + 1 coroutine.yield(n) end end\nend\nco = coroutine.create(f())\nprint(coroutine.resume(co))\nprint(coroutine.resume(co))").unwrap(),
            "true\t1\ntrue\t2\n"
        );
        // A `local function` is not visible outside its block.
        assert!(exec("if true then local function f() end end\nprint(f)").is_err());
        // Closures work through pcall and metamethods.
        assert_eq!(
            exec("function f()\nlocal x = 41\nreturn function() return x + 1 end\nend\nprint(pcall(f()))").unwrap(),
            "true\t42\n"
        );
        assert_eq!(
            exec("function f()\nlocal x = 5\nreturn function() return x end\nend\nt = setmetatable({}, {__index = f()})\nprint(t.anything)").unwrap(),
            "5\n"
        );
    }

    #[test]
    fn metamethods() {
        // __index: table chain and function form.
        assert_eq!(
            exec("base = {x = 1}\nt = setmetatable({y = 2}, {__index = base})\nprint(t.x, t.y, t.z)").unwrap(),
            "1\t2\tnil\n"
        );
        assert_eq!(
            exec("t = setmetatable({}, {__index = function(t, k) return k .. \"!\" end})\nprint(t.hi, t.ho)").unwrap(),
            "hi!\tho!\n"
        );
        // __index chains through multiple metatables.
        assert_eq!(
            exec("a = {x = 1}\nb = setmetatable({}, {__index = a})\nc = setmetatable({}, {__index = b})\nprint(c.x)").unwrap(),
            "1\n"
        );
        // __index loops are caught.
        assert!(exec("a = {}\nb = {}\nsetmetatable(a, {__index = b})\nsetmetatable(b, {__index = a})\nprint(a.x)").is_err());
        // rawget ignores __index.
        assert_eq!(
            exec("t = setmetatable({}, {__index = {x = 1}})\nprint(rawget(t, \"x\"), t.x)").unwrap(),
            "nil\t1\n"
        );
        // __newindex: table and function forms.
        assert_eq!(
            exec("store = {}\nt = setmetatable({}, {__newindex = store})\nt.x = 5\nprint(t.x, store.x)").unwrap(),
            "nil\t5\n"
        );
        assert_eq!(
            exec("log = \"\"\nt = setmetatable({}, {__newindex = function(t, k, v) log = k .. \"=\" .. v end})\nt.a = 1\nt.b = 2\nprint(log)").unwrap(),
            "b=2\n"
        );
        // rawset ignores __newindex; existing keys bypass it.
        assert_eq!(
            exec("store = {}\nt = setmetatable({}, {__newindex = store})\nrawset(t, \"x\", 9)\nprint(t.x, store.x)").unwrap(),
            "9\tnil\n"
        );
        assert_eq!(
            exec("t = setmetatable({x = 1}, {__newindex = function() print(\"no\") end})\nt.x = 2\nprint(t.x)").unwrap(),
            "2\n"
        );
        // __eq / __lt / __le (and > / >= swapping).
        assert_eq!(
            exec("mt = {__eq = function(a, b) return a.id == b.id end}\na = setmetatable({id = 1}, mt)\nb = setmetatable({id = 1}, mt)\nc = setmetatable({id = 2}, mt)\nprint(a == b, a == c, a ~= c)").unwrap(),
            "true\tfalse\ttrue\n"
        );
        assert_eq!(
            exec("mt = {__lt = function(a, b) return a.id < b.id end}\na = setmetatable({id = 1}, mt)\nb = setmetatable({id = 2}, mt)\nprint(a < b, b < a, a > b, b > a)").unwrap(),
            "true\tfalse\tfalse\ttrue\n"
        );
        assert_eq!(
            exec("mt = {__le = function(a, b) return a.id <= b.id end}\na = setmetatable({id = 1}, mt)\nb = setmetatable({id = 2}, mt)\nprint(a <= b, b <= a, a >= b, b >= a)").unwrap(),
            "true\tfalse\tfalse\ttrue\n"
        );
        // Lua 5.1 fallback: `a <= b` from `not (b < a)` when __le is absent.
        assert_eq!(
            exec("mt = {__lt = function(a, b) return a.id < b.id end}\na = setmetatable({id = 1}, mt)\nb = setmetatable({id = 2}, mt)\nprint(a <= b, b <= a)").unwrap(),
            "true\tfalse\n"
        );
        // rawequal ignores __eq.
        assert_eq!(
            exec("mt = {__eq = function() return true end}\na = setmetatable({}, mt)\nb = setmetatable({}, mt)\nprint(rawequal(a, b), a == b)").unwrap(),
            "false\ttrue\n"
        );
        // __concat and the arithmetic metamethods (either operand).
        assert_eq!(
            exec("mt = {__concat = function(a, b) return \"C\" end}\nt = setmetatable({}, mt)\nprint(t .. \"x\", \"x\" .. t, t .. t)").unwrap(),
            "C\tC\tC\n"
        );
        assert_eq!(
            exec("mt = {__add = function() return 10 end, __sub = function() return 20 end, __mul = function() return 30 end, __div = function() return 40 end, __mod = function() return 50 end, __unm = function() return 60 end}\nt = setmetatable({}, mt)\nprint(t + 1, 1 - t, t * 2, 2 / t, t % 3, -t)").unwrap(),
            "10\t20\t30\t40\t50\t60\n"
        );
        // __call receives the table as its first argument.
        assert_eq!(
            exec("t = setmetatable({}, {__call = function(self, a, b) return a + b end})\nprint(t(3, 4))").unwrap(),
            "7\n"
        );
        // __tostring (print and tostring).
        assert_eq!(
            exec("t = setmetatable({}, {__tostring = function() return \"custom\" end})\nprint(t)\nprint(tostring(t), tostring(t) .. \"!\")").unwrap(),
            "custom\ncustom\tcustom!\n"
        );
        // __pairs supplies the generic-for iterator.
        assert_eq!(
            exec("t = setmetatable({}, {__pairs = function(t) return pairs({10, 20}) end})\nfor k, v in pairs(t) do print(k, v) end").unwrap(),
            "1\t10\n2\t20\n"
        );
        // Metatable protection still applies.
        assert!(exec("t = {}\nsetmetatable(t, {__metatable = true})\nsetmetatable(t, {})").is_err());
    }

    #[test]
    fn setmetatable_builtin() {
        // Returns the given table (so it can be chained).
        assert_eq!(
            exec("t = {}\nprint(rawequal(setmetatable(t, {}), t))").unwrap(),
            "true\n"
        );
        assert_eq!(exec("t = {}\nprint(setmetatable(t, nil))").unwrap(), "table\n");
        // Setting a metatable twice is fine while it is unprotected.
        assert_eq!(
            exec("t = {}\nsetmetatable(t, {})\nsetmetatable(t, {})").unwrap(),
            ""
        );
        // A `__metatable` field protects the metatable from change...
        assert!(exec("t = {}\nsetmetatable(t, {__metatable = true})\nsetmetatable(t, nil)").is_err());
        assert!(exec("t = {}\nsetmetatable(t, {__metatable = true})\nsetmetatable(t, {})").is_err());
        // ...even a false value protects (Lua: any non-nil __metatable).
        assert!(exec("t = {}\nsetmetatable(t, {__metatable = false})\nsetmetatable(t, {})").is_err());
        // Errors: non-table first arg, non-table/non-nil metatable, arity.
        assert!(exec("setmetatable(1, {})").is_err());
        assert!(exec("setmetatable(\"s\", {})").is_err());
        assert!(exec("setmetatable({}, 1)").is_err());
        assert!(exec("setmetatable({}, \"x\")").is_err());
        assert!(exec("setmetatable({})").is_err());
        assert!(exec("setmetatable({}, {}, 1)").is_err());
    }

    #[test]
    fn tonumber_builtin() {
        // Numbers pass through.
        assert_eq!(exec("print(tonumber(5), tonumber(5.5))").unwrap(), "5\t5.5\n");
        // Decimal strings follow the lexer's conventions.
        assert_eq!(exec("print(tonumber(\"10\"))").unwrap(), "10\n");
        assert_eq!(exec("print(tonumber(\"  10  \"))").unwrap(), "10\n");
        assert_eq!(exec("print(tonumber(\"-3\"), tonumber(\"+7\"))").unwrap(), "-3\t7\n");
        assert_eq!(exec("print(tonumber(\"5.5\"), tonumber(\".5\"), tonumber(\"5.\"))").unwrap(), "5.5\t0.5\t5.0\n");
        assert_eq!(exec("print(tonumber(\"1e3\"), tonumber(\"2.5e-2\"))").unwrap(), "1000.0\t0.025\n");
        assert_eq!(exec("print(tonumber(\"-0.0\"))").unwrap(), "-0.0\n");
        // A too-large integer becomes a float.
        assert_eq!(exec("print(tonumber(\"9223372036854775808\"))").unwrap(), "9.2233720368548e+18\n");
        // Non-numeric strings and non-string values give nil.
        assert_eq!(exec("print(tonumber(\"abc\"), tonumber(\"\"), tonumber(\"5x\"))").unwrap(), "nil\tnil\tnil\n");
        assert_eq!(exec("print(tonumber(\"inf\"), tonumber(\"nan\"))").unwrap(), "nil\tnil\n");
        assert_eq!(exec("print(tonumber(nil), tonumber(true), tonumber({}))").unwrap(), "nil\tnil\tnil\n");
        // Base form (2..36); letters are case-insensitive.
        assert_eq!(exec("print(tonumber(\"ff\", 16), tonumber(\"FF\", 16))").unwrap(), "255\t255\n");
        assert_eq!(exec("print(tonumber(\"101\", 2), tonumber(\"z\", 36))").unwrap(), "5\t35\n");
        assert_eq!(exec("print(tonumber(\"-ff\", 16), tonumber(\"  ff  \", 16))").unwrap(), "-255\t255\n");
        assert_eq!(exec("print(tonumber(\"10\", 10.0))").unwrap(), "10\n");
        // Invalid digits give nil; a nil base acts like no base.
        assert_eq!(
            exec("print(tonumber(\"2\", 2), tonumber(\"12\", 2), tonumber(\"g\", 16))").unwrap(),
            "nil\tnil\tnil\n"
        );
        assert_eq!(exec("print(tonumber(\"10\", nil))").unwrap(), "10\n");
        // Errors: number with a base, base out of range, arity.
        assert!(exec("tonumber(5, 10)").is_err());
        assert!(exec("tonumber(\"10\", 1)").is_err());
        assert!(exec("tonumber(\"10\", 37)").is_err());
        assert!(exec("tonumber()").is_err());
        assert!(exec("tonumber(\"10\", 16, 1)").is_err());
    }

    #[test]
    fn tostring_builtin() {
        assert_eq!(exec("print(tostring(1), tostring(1.5))").unwrap(), "1\t1.5\n");
        assert_eq!(
            exec("print(tostring(nil), tostring(true), tostring(false))").unwrap(),
            "nil\ttrue\tfalse\n"
        );
        assert_eq!(exec("print(tostring(\"abc\"))").unwrap(), "abc\n");
        assert_eq!(exec("print(tostring({}), tostring(print), tostring(next))").unwrap(), "table\tnative\tnative\n");
        // Numbers use the same float formatting as print.
        assert_eq!(exec("print(tostring(1.0/3))").unwrap(), "0.33333333333333\n");
        assert_eq!(exec("print(tostring(1e14), tostring(-0.0))").unwrap(), "1e+14\t-0.0\n");
        // Strings come back as themselves (so they compare/concatenate).
        assert_eq!(exec("print(tostring(\"x\") == \"x\")").unwrap(), "true\n");
        assert_eq!(exec("print(tostring(5) .. \"!\")").unwrap(), "5!\n");
        // Errors: arity.
        assert!(exec("tostring()").is_err());
        assert!(exec("tostring(1, 2)").is_err());
    }

    #[test]
    fn type_builtin() {
        assert_eq!(
            exec("print(type(nil), type(true), type(false))").unwrap(),
            "nil\tboolean\tboolean\n"
        );
        assert_eq!(
            exec("print(type(1), type(1.5))").unwrap(),
            "number\tnumber\n"
        );
        assert_eq!(exec("print(type(\"s\"), type({}))").unwrap(), "string\ttable\n");
        // Every callable value is a function, including native builtins.
        assert_eq!(
            exec("print(type(print), type(next), type(type))").unwrap(),
            "function\tfunction\tfunction\n"
        );
        assert_eq!(
            exec("function f() end\nprint(type(f), type(shell), type(dhcp), type(exit), type(ls))")
                .unwrap(),
            "function\tfunction\tfunction\tfunction\tfunction\n"
        );
        // The result is a string usable in comparisons and concatenation.
        assert_eq!(exec("print(type(1) == \"number\", type({}) .. \"!\")").unwrap(), "true\ttable!\n");
        assert_eq!(exec("print(type(type(1)))").unwrap(), "string\n");
        // Errors: arity.
        assert!(exec("type()").is_err());
        assert!(exec("type(1, 2)").is_err());
    }

    #[test]
    fn version_global() {
        assert_eq!(exec("print(_VERSION)").unwrap(), "Lua 5.5\n");
        assert_eq!(
            exec("print(type(_VERSION), _VERSION == \"Lua 5.5\")").unwrap(),
            "string\ttrue\n"
        );
        // It is an ordinary global: readable, shadowed by a local, and
        // assignable (Lua semantics).
        assert_eq!(exec("local v = _VERSION\nprint(v)").unwrap(), "Lua 5.5\n");
        assert_eq!(exec("_VERSION = \"other\"\nprint(_VERSION)").unwrap(), "other\n");
    }

    #[test]
    fn warn_builtin() {
        // Arguments are concatenated with no separator; numbers are coerced.
        assert_eq!(exec("warn(\"hello\")").unwrap(), "Lua warning: hello\n");
        assert_eq!(exec("warn(\"a\", \"b\", 1, 2.5)").unwrap(), "Lua warning: ab12.5\n");
        // The control messages toggle warnings and emit nothing themselves.
        assert_eq!(exec("warn(\"@off\")").unwrap(), "");
        assert_eq!(
            exec("warn(\"@off\")\nwarn(\"hidden\")\nwarn(\"@on\")\nwarn(\"shown\")").unwrap(),
            "Lua warning: shown\n"
        );
        // Errors: no arguments, or a non-string/non-number value — validated
        // even while warnings are off.
        assert!(exec("warn()").is_err());
        assert!(exec("warn(nil)").is_err());
        assert!(exec("warn({})").is_err());
        assert!(exec("warn(true)").is_err());
        assert!(exec("warn(\"@off\")\nwarn(nil)").is_err());
    }

    #[test]
    fn pcall_builtin() {
        // Success: true plus every result (none, one, two, or many).
        assert_eq!(exec("function f0() end\nprint(pcall(f0))").unwrap(), "true\n");
        assert_eq!(exec("function f1() return 1 end\nprint(pcall(f1))").unwrap(), "true\t1\n");
        assert_eq!(
            exec("function f2() return next({7}) end\nprint(pcall(f2))").unwrap(),
            "true\t1\t7\n"
        );
        assert_eq!(
            exec("function f3() return select(2, \"a\", \"b\", \"c\") end\nprint(pcall(f3))").unwrap(),
            "true\tb\tc\n"
        );
        // Arguments are forwarded.
        assert_eq!(
            exec("function add(a, b) return a + b end\nprint(pcall(add, 3, 4))").unwrap(),
            "true\t7\n"
        );
        // Native builtins returning two values work through pcall.
        assert_eq!(exec("print(pcall(next, {5}))").unwrap(), "true\t1\t5\n");
        // Runtime errors become false plus the message string.
        assert_eq!(
            exec("function bad() return nil + 1 end\nlocal ok, err = pcall(bad)\nprint(ok, type(err))").unwrap(),
            "false\tstring\n"
        );
        // error(v) raises v, which pcall returns unchanged.
        assert_eq!(
            exec("function boom() error(\"boom\") end\nprint(pcall(boom))").unwrap(),
            "false\tboom\n"
        );
        assert_eq!(exec("print(pcall(error, 42))").unwrap(), "false\t42\n");
        assert_eq!(exec("print(pcall(error))").unwrap(), "false\tnil\n");
        assert_eq!(
            exec("function enil() error(nil) end\nprint(pcall(enil))").unwrap(),
            "false\tnil\n"
        );
        assert_eq!(
            exec("t = {code = 7}\nfunction etab() error(t) end\nlocal ok, e = pcall(etab)\nprint(ok, e == t, e.code)").unwrap(),
            "false\ttrue\t7\n"
        );
        // error's level is validated but ignored.
        assert_eq!(
            exec("function ex() error(\"x\", 2) end\nprint(pcall(ex))").unwrap(),
            "false\tx\n"
        );
        assert_eq!(
            exec("function exbad() error(\"x\", \"bad\") end\nprint(pcall(exbad))").unwrap(),
            "false\terror level must be an integer\n"
        );
        // Calling a non-function is caught as well.
        assert_eq!(
            exec("print(pcall(nil))").unwrap(),
            "false\tattempt to call a non-function value\n"
        );
        // Nested pcall: the inner catch does not disturb the outer.
        assert_eq!(
            exec("function inner() error(\"x\") end\nfunction outer() return pcall(inner) end\nlocal ok, ok2, err = pcall(outer)\nprint(ok, ok2, err)").unwrap(),
            "true\tfalse\tx\n"
        );
        // The stack/frames are intact after a catch.
        assert_eq!(
            exec("function bad2() local a = 1 error(\"e\") end\nlocal ok = pcall(bad2)\nprint(ok, 2 + 2)").unwrap(),
            "false\t4\n"
        );
        assert_eq!(
            exec("n = 0\nfunction bad3() error(\"e\") end\nfor i = 1, 3 do if not pcall(bad3) then n = n + 1 end end\nprint(n)").unwrap(),
            "3\n"
        );
        // A runtime error from a global lookup is catchable too.
        assert_eq!(
            exec("function ug() return undefined_global end\nprint(pcall(ug))").unwrap(),
            "false\tundefined variable\n"
        );
        // pcall itself requires a function argument.
        assert!(exec("pcall()").is_err());
        // error() propagates when not protected.
        assert!(exec("error(\"boom\")").is_err());
        assert!(exec("error(1, 2, 3)").is_err());
    }

    #[test]
    fn lua_state_size_budget() {
        let n = core::mem::size_of::<super::LuaState>();
        std::println!("LuaState size: {} bytes", n);
        // The state lives on the caller's stack; keep it within the stack
        // budgets of the firmware targets (ARM64 bare has a 512 KB stack).
        assert!(n < 128 * 1024, "LuaState grew to {} bytes", n);
    }

    #[test]
    fn coroutine_builtin() {
        // status / type / tostring of a thread value.
        assert_eq!(
            exec("co = coroutine.create(print)\nprint(coroutine.status(co), type(co), tostring(co))")
                .unwrap(),
            "suspended\tthread\tthread\n"
        );
        // A native body runs once with the resume arguments.
        assert_eq!(
            exec("co = coroutine.create(print)\nprint(coroutine.resume(co, \"hi\"))\nprint(coroutine.status(co))")
                .unwrap(),
            "hi\ntrue\ndead\n"
        );
        // yield/resume passes values both ways.
        assert_eq!(
            exec("function f(a)\nlocal x, y = coroutine.yield(a + 1, a + 2)\nreturn x .. y\nend\nco = coroutine.create(f)\nprint(coroutine.resume(co, 10))\nprint(coroutine.resume(co, \"a\", \"b\"))\nprint(coroutine.status(co))")
                .unwrap(),
            "true\t11\t12\ntrue\tab\ndead\n"
        );
        // wrap resumes without the leading boolean.
        assert_eq!(
            exec("function gen()\nfor i = 1, 3 do coroutine.yield(i) end\nend\ng = coroutine.wrap(coroutine.create(gen))\nprint(g())\nprint(g())\nprint(g())")
                .unwrap(),
            "1\n2\n3\n"
        );
        // isyieldable / running inside and outside a coroutine.
        assert_eq!(exec("print(coroutine.isyieldable())").unwrap(), "false\n");
        assert_eq!(
            exec("function f()\nlocal c, main = coroutine.running()\nprint(type(c), main, coroutine.isyieldable())\nend\ncoroutine.resume(coroutine.create(f))")
                .unwrap(),
            "thread\tfalse\ttrue\n"
        );
        // Nested resume: a coroutine resumes another coroutine.
        assert_eq!(
            exec("function inner() coroutine.yield(\"i\") end\nfunction outer()\nlocal ci = coroutine.create(inner)\nprint(coroutine.resume(ci))\nend\nco = coroutine.create(outer)\nprint(coroutine.resume(co))")
                .unwrap(),
            "true\ti\ntrue\n"
        );
        // Errors inside a coroutine become false + the error value.
        assert_eq!(
            exec("function f() error(\"boom\") end\nco = coroutine.create(f)\nprint(coroutine.resume(co))\nprint(coroutine.status(co))")
                .unwrap(),
            "false\tboom\ndead\n"
        );
        // Resuming a dead coroutine returns false + a message.
        assert_eq!(
            exec("co = coroutine.create(print)\ncoroutine.resume(co, \"x\")\nprint(coroutine.resume(co))")
                .unwrap(),
            "x\nfalse\tcannot resume dead coroutine\n"
        );
        // close marks a suspended coroutine dead.
        assert_eq!(
            exec("co = coroutine.create(print)\nprint(coroutine.close(co), coroutine.status(co))")
                .unwrap(),
            "true\tdead\n"
        );
        // Yielding from the main thread is an error, and create needs a function.
        assert!(exec("coroutine.yield(1)").is_err());
        assert!(exec("coroutine.create(1)").is_err());
        assert!(exec("coroutine.resume(1)").is_err());
        // The coroutine pool is bounded.
        assert!(exec("a = coroutine.create(print)\nb = coroutine.create(print)\nc = coroutine.create(print)\nd = coroutine.create(print)").is_err());
    }

    #[test]
    fn error_rendering() {
        let mut state = super::LuaState::new();
        let mut buf = [0u8; 64];
        let n = super::eval::error_bytes(&state, LuaError::Msg("boom"), &mut buf);
        assert_eq!(&buf[..n], b"boom");
        let n = super::eval::error_bytes(&state, LuaError::Obj(super::Value::Table(0)), &mut buf);
        assert_eq!(&buf[..n], b"error object is a table value");
        let r = state.intern(b"custom").unwrap();
        let n = super::eval::error_bytes(&state, LuaError::Obj(super::Value::Str(r)), &mut buf);
        assert_eq!(&buf[..n], b"custom");
    }

    #[test]
    fn nil_removes_table_fields() {
        // `t[k] = nil` removes the key (Lua), it does not store a nil value.
        assert_eq!(exec("t = {a = 1}\nt.a = nil\nprint(t.a)").unwrap(), "nil\n");
        assert_eq!(
            exec("t = {a = 1}\nk = \"a\"\nt[k] = nil\nprint(next(t))").unwrap(),
            "nil\n"
        );
        // Removing a missing key is a no-op.
        assert_eq!(
            exec("t = {a = 1}\nt.b = nil\nprint(t.a)").unwrap(),
            "1\n"
        );
    }

    #[test]
    #[test]
    fn multiple_assignment() {
        // A right-hand-side list is not supported (only a call expands).
        assert!(exec("a, b = 1, 2\nprint(a, b)").is_err());
        assert_eq!(exec("a, b = next({9})\nprint(a, b)").unwrap(), "1\t9\n");
        assert_eq!(exec("local a, b = next({8})\nprint(a, b)").unwrap(), "1\t8\n");
        assert_eq!(exec("t = {}\nt.x, t.y = next({5})\nprint(t.x, t.y)").unwrap(), "1\t5\n");
        // A target that is not assignable errors.
        assert!(exec("1, b = next({})").is_err());
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
        // Anonymous function literals are expressions (no upvalues/closures).
        assert_eq!(exec("f = function(a) return a * 2 end\nprint(f(21))").unwrap(), "42\n");
        assert_eq!(exec("print((function() return 5 end)())").unwrap(), "5\n");
        assert_eq!(
            exec("apply = function(g, x) return g(x) end\nprint(apply(function(n) return n + 1 end, 41))").unwrap(),
            "42\n"
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
        // Computed-key fields use Lua's `[expr] = value` form.
        assert_eq!(
            exec("k = \"x\"\nt = {[2] = 5, [k] = 6}\nprint(t[2], t.x)").unwrap(),
            "5\t6\n"
        );
        // The `=` is required.
        assert!(exec("t = {[2] 5}").is_err());
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
    fn fetch_demo(name: &str, _save_as: &str) -> Option<usize> {
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
    fn fetch_count(name: &str, _save_as: &str) -> Option<usize> {
        match name {
            "a.txt" => Some(5),
            "b.txt" => Some(12),
            _ => None,
        }
    }

    fn exec_fetch(src: &str) -> Result<String, LuaError> {
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

    #[test]
    fn fetch_with_dest() {
        // fetch(src, dest) returns the count; ls lists the local dest name.
        assert_eq!(
            exec_fetch("n = fetch(\"a.txt\", \"mine.txt\")\nprint(n)\nls").unwrap(),
            "5\nmine.txt (5 bytes)\n"
        );
        // Without dest, ls shows the source name.
        assert_eq!(exec_fetch("fetch(\"a.txt\")\nls").unwrap(), "a.txt (5 bytes)\n");
        // The dest can come from a variable.
        assert_eq!(
            exec_fetch("d = \"save.bin\"\nprint(fetch(\"b.txt\", d))").unwrap(),
            "12\n"
        );
        // 2-arg form still returns nil on download failure.
        assert_eq!(
            exec_fetch("print(fetch(\"missing.txt\", \"m.bin\"))").unwrap(),
            "nil\n"
        );
    }

    #[test]
    fn fetch_dest_errors() {
        // Non-string dest, or more than two arguments.
        assert!(exec_fetch("fetch(\"a.txt\", 5)").is_err());
        assert!(exec_fetch("fetch(\"a.txt\", true)").is_err());
        assert!(exec_fetch("fetch(\"a.txt\", \"x\", 1)").is_err());
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

    fn exec_dofile(src: &str) -> Result<String, LuaError> {
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

    #[test]
    fn ls_lists_fetched_files() {
        // Fetch twice, then ls lists them in fetch order.
        assert_eq!(
            exec_fetch("fetch(\"a.txt\")\nfetch(\"b.txt\")\nls").unwrap(),
            "a.txt (5 bytes)\nb.txt (12 bytes)\n"
        );
        // The call form works too.
        assert_eq!(exec_fetch("fetch(\"a.txt\")\nls()").unwrap(), "a.txt (5 bytes)\n");
        // No files fetched -> nothing printed.
        assert_eq!(exec_fetch("ls").unwrap(), "");
        // A failed fetch is not listed.
        assert_eq!(exec_fetch("fetch(\"missing.txt\")\nls").unwrap(), "");
        // ls() with an argument errors.
        assert!(exec_fetch("ls(1)").is_err());
        // print(ls) shows the type.
        assert_eq!(exec("print(ls)").unwrap(), "ls\n");
    }

    #[test]
    fn ls_dedupes_refetch() {
        // Re-fetching the same name keeps one entry (length updated).
        assert_eq!(
            exec_fetch("fetch(\"a.txt\")\nfetch(\"a.txt\")\nls").unwrap(),
            "a.txt (5 bytes)\n"
        );
    }

    /// Mock `dhcp()` host callback: network setup succeeds and enables `fetch`.
    fn dhcp_ok() -> Option<fn(source: &str, save_as: &str) -> Option<usize>> {
        Some(fetch_count)
    }

    /// Mock `dhcp()` host callback: network setup fails.
    fn dhcp_fail() -> Option<fn(source: &str, save_as: &str) -> Option<usize>> {
        None
    }

    /// Run a script with a mock `dhcp` callback installed (fetch disabled).
    fn exec_dhcp(src: &str, dhcp: fn() -> Option<fn(source: &str, save_as: &str) -> Option<usize>>) -> Result<String, LuaError> {
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = super::LuaState::new();
        state.register_builtins(putc_test);
        state.set_fetch(None);
        state.set_dhcp(Some(dhcp));
        let first = {
            let mut p = super::parse::Parser::new(src.as_bytes(), &mut state);
            p.parse_script()?
        };
        let entry = super::vm::compile(&mut state, first)?;
        super::vm::exec_script(&mut state, entry)?;
        Ok(OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap()))
    }

    /// Mock `dhcp_info` host callback: formats the negotiated network details.
    fn dhcp_info_test(buf: &mut [u8]) -> usize {
        let s = b"MAC: 52:54:00:12:34:56\nIP: 10.0.0.15\nSubnet: 255.255.255.0\n\
                  Gateway: 10.0.0.1\nTFTP Server: 10.0.0.1\nBootfile: test.lua\n";
        let n = s.len().min(buf.len());
        buf[..n].copy_from_slice(&s[..n]);
        n
    }

    /// Run a script with both a mock `dhcp` and a mock `dhcp_info` callback.
    fn exec_dhcp_info(
        src: &str,
        dhcp: fn() -> Option<fn(source: &str, save_as: &str) -> Option<usize>>,
    ) -> Result<String, LuaError> {
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = super::LuaState::new();
        state.register_builtins(putc_test);
        state.set_fetch(None);
        state.set_dhcp(Some(dhcp));
        state.set_dhcp_info(Some(dhcp_info_test));
        let first = {
            let mut p = super::parse::Parser::new(src.as_bytes(), &mut state);
            p.parse_script()?
        };
        let entry = super::vm::compile(&mut state, first)?;
        super::vm::exec_script(&mut state, entry)?;
        Ok(OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap()))
    }

    #[test]
    fn dhcp_prints_network_info() {
        let info = "MAC: 52:54:00:12:34:56\nIP: 10.0.0.15\nSubnet: 255.255.255.0\n\
                    Gateway: 10.0.0.1\nTFTP Server: 10.0.0.1\nBootfile: test.lua\n";
        // Bare `dhcp` statement prints the details on success.
        assert_eq!(exec_dhcp_info("dhcp", dhcp_ok).unwrap(), info);
        // The call form prints the details and returns true.
        assert_eq!(exec_dhcp_info("print(dhcp())", dhcp_ok).unwrap(), format!("{}true\n", info));
        // On failure nothing is printed and no error.
        assert_eq!(exec_dhcp_info("dhcp", dhcp_fail).unwrap(), "");
        assert_eq!(exec_dhcp_info("print(dhcp())", dhcp_fail).unwrap(), "false\n");
        // fetch() works after dhcp printed its details.
        assert_eq!(
            exec_dhcp_info("dhcp\nprint(fetch(\"a.txt\"))", dhcp_ok).unwrap(),
            format!("{}5\n", info)
        );
    }

    /// Mock `dhcp_values` host callback: fills the structured DHCP result.
    fn dhcp_values_test(v: &mut super::DhcpValues) {
        v.mac = [0x52, 0x54, 0x00, 0x12, 0x34, 0x56];
        v.ip = [10, 0, 0, 15];
        v.subnet = [255, 255, 255, 0];
        v.gateway = [10, 0, 0, 1];
        v.server = [10, 0, 0, 1];
        let bf: &[u8] = b"test.lua";
        v.bootfile[..bf.len()].copy_from_slice(bf);
    }

    /// Run a script with mock `dhcp` and `dhcp_values` callbacks installed.
    fn exec_dhcp_vars(
        src: &str,
        dhcp: fn() -> Option<fn(source: &str, save_as: &str) -> Option<usize>>,
    ) -> Result<String, LuaError> {
        OUT.with(|o| o.borrow_mut().clear());
        let mut state = super::LuaState::new();
        state.register_builtins(putc_test);
        state.set_fetch(None);
        state.set_dhcp(Some(dhcp));
        state.set_dhcp_values(Some(dhcp_values_test));
        let first = {
            let mut p = super::parse::Parser::new(src.as_bytes(), &mut state);
            p.parse_script()?
        };
        let entry = super::vm::compile(&mut state, first)?;
        super::vm::exec_script(&mut state, entry)?;
        Ok(OUT.with(|o| String::from_utf8(o.borrow().clone()).unwrap()))
    }

    #[test]
    fn dhcp_sets_variable_globals() {
        // After a successful dhcp, the fields are readable as strings.
        assert_eq!(exec_dhcp_vars("dhcp\nprint(mac)", dhcp_ok).unwrap(), "52:54:00:12:34:56\n");
        assert_eq!(
            exec_dhcp_vars("dhcp\nprint(ip, subnet)", dhcp_ok).unwrap(),
            "10.0.0.15\t255.255.255.0\n"
        );
        assert_eq!(
            exec_dhcp_vars("dhcp\nprint(gateway, server)", dhcp_ok).unwrap(),
            "10.0.0.1\t10.0.0.1\n"
        );
        assert_eq!(exec_dhcp_vars("dhcp\nprint(bootfile)", dhcp_ok).unwrap(), "test.lua\n");
        // The strings are interned, so equality by content works.
        assert_eq!(
            exec_dhcp_vars("dhcp\nprint(ip == \"10.0.0.15\")", dhcp_ok).unwrap(),
            "true\n"
        );
        assert_eq!(
            exec_dhcp_vars("dhcp\nprint(bootfile == \"test.lua\")", dhcp_ok).unwrap(),
            "true\n"
        );
        // On failure the variables are not defined.
        assert!(exec_dhcp_vars("dhcp\nprint(mac)", dhcp_fail).is_err());
        // dhcp() also sets them (call form).
        assert_eq!(
            exec_dhcp_vars("dhcp()\nprint(server)", dhcp_ok).unwrap(),
            "10.0.0.1\n"
        );
        // tftp_port defaults to 69 and is a number.
        assert_eq!(exec_dhcp_vars("dhcp\nprint(tftp_port)", dhcp_ok).unwrap(), "69\n");
        assert_eq!(
            exec_dhcp_vars("dhcp\nprint(tftp_port + 1)", dhcp_ok).unwrap(),
            "70\n"
        );
        // On failure neither is defined.
        assert!(exec_dhcp_vars("dhcp\nprint(tftp_port)", dhcp_fail).is_err());
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

