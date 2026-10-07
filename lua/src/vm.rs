//! Bytecode compiler and stack-machine interpreter.
//!
//! The parser produces AST nodes; this module compiles their statement chains
//! into a flat instruction array ([`LuaState::code`]) and executes it with an
//! explicit operand stack and frame stack. Unlike the original tree-walking
//! evaluator, the VM keeps no execution state on the Rust stack, which is what
//! makes resumable coroutines possible.

use super::eval::{self, ExecResult};
use super::lex::{Lexer, Tok};
use super::parse::Parser;
use super::{
    CoState, Instr, InstrOp, LuaError, LuaState, Node, Op, StrRef, Thread, Value, CO_DEAD,
    CO_NORMAL, CO_RUNNING, CO_SUSPENDED, DOFILE_CAP, MAX_CODE, MAX_COS, MAX_FUNCS, NO_NODE,
    WANT_ALL,
};

/// Sentinel return address marking the root frame of a `run()` invocation.
const RET_HALT: u16 = 0xFFFE;
/// Maximum pending forward jumps (gotos and loop ends) per function.
const MAX_FIXUPS: usize = 256;
/// Maximum labels per function.
const MAX_LABELS: usize = 256;
/// Maximum loop nesting depth tracked by the compiler.
const MAX_LOOPS: usize = 8;
/// Maximum `break`s per loop.
const MAX_LOOP_ENDS: usize = 16;

struct LoopCtx {
    /// Scope depth outside the loop (break pops down to it).
    scope: u8,
    ends: [u16; MAX_LOOP_ENDS],
    nends: u8,
}

struct Compiler {
    labels: [(u16, u16, u8); MAX_LABELS],
    nlabels: u16,
    /// (jump instruction ip, target node, source scope depth)
    fixups: [(u16, u16, u8); MAX_FIXUPS],
    nfixups: u16,
    loops: [LoopCtx; MAX_LOOPS],
    nloops: u8,
    scope: u8,
    pending: [u16; MAX_FUNCS],
    npending: u8,
    compiled: [bool; MAX_FUNCS],
}

impl Compiler {
    fn new() -> Self {
        Compiler {
            labels: [(0, 0, 0); MAX_LABELS],
            nlabels: 0,
            fixups: [(0, 0, 0); MAX_FIXUPS],
            nfixups: 0,
            loops: core::array::from_fn(|_| LoopCtx {
                scope: 0,
                ends: [0; MAX_LOOP_ENDS],
                nends: 0,
            }),
            nloops: 0,
            scope: 0,
            pending: [0; MAX_FUNCS],
            npending: 0,
            compiled: [false; MAX_FUNCS],
        }
    }

    fn reset_function(&mut self) {
        self.nlabels = 0;
        self.nfixups = 0;
        self.nloops = 0;
        self.scope = 0;
    }

    fn emit(&mut self, s: &mut LuaState, op: InstrOp, a: u16, b: u16) -> Result<u16, LuaError> {
        if s.ncode as usize >= MAX_CODE {
            return Err("script too complex (code overflow)".into());
        }
        let at = s.ncode;
        s.code[at as usize] = Instr { op, a, b };
        s.ncode += 1;
        Ok(at)
    }

    fn push_scope(&mut self, s: &mut LuaState) -> Result<(), LuaError> {
        if self.scope as usize >= super::MAX_FRAMES {
            return Err("too many nested blocks".into());
        }
        self.emit(s, InstrOp::PushScope, 0, 0)?;
        self.scope += 1;
        Ok(())
    }

    fn pop_scope(&mut self, s: &mut LuaState) -> Result<(), LuaError> {
        self.emit(s, InstrOp::PopScope, 0, 0)?;
        self.scope -= 1;
        Ok(())
    }

    fn begin_loop(&mut self, scope: u8) -> Result<(), LuaError> {
        if self.nloops as usize >= MAX_LOOPS {
            return Err("too many nested loops".into());
        }
        self.loops[self.nloops as usize].scope = scope;
        self.loops[self.nloops as usize].nends = 0;
        self.nloops += 1;
        Ok(())
    }

    fn emit_break(&mut self, s: &mut LuaState) -> Result<(), LuaError> {
        if self.nloops == 0 {
            return Err("break outside loop".into());
        }
        let li = (self.nloops - 1) as usize;
        let pops = self.scope - self.loops[li].scope;
        let at = self.emit(s, InstrOp::JumpScopes, 0, pops as u16)?;
        let ctx = &mut self.loops[li];
        if ctx.nends as usize >= MAX_LOOP_ENDS {
            return Err("too many breaks in a loop".into());
        }
        ctx.ends[ctx.nends as usize] = at;
        ctx.nends += 1;
        Ok(())
    }

    fn end_loop(&mut self, s: &mut LuaState, end: u16) {
        self.nloops -= 1;
        let ctx = &self.loops[self.nloops as usize];
        for i in 0..ctx.nends as usize {
            s.code[ctx.ends[i] as usize].a = end;
        }
    }

    fn record_label(&mut self, s: &mut LuaState, node: u16) -> Result<(), LuaError> {
        if self.nlabels as usize >= MAX_LABELS {
            return Err("too many labels".into());
        }
        self.labels[self.nlabels as usize] = (node, s.ncode, self.scope);
        self.nlabels += 1;
        Ok(())
    }

    fn add_fixup(&mut self, at: u16, node: u16, depth: u8) -> Result<(), LuaError> {
        if self.nfixups as usize >= MAX_FIXUPS {
            return Err("too many gotos".into());
        }
        self.fixups[self.nfixups as usize] = (at, node, depth);
        self.nfixups += 1;
        Ok(())
    }

    fn resolve_fixups(&mut self, s: &mut LuaState) -> Result<(), LuaError> {
        for i in 0..self.nfixups as usize {
            let (at, node, depth) = self.fixups[i];
            let mut found = None;
            for j in 0..self.nlabels as usize {
                if self.labels[j].0 == node {
                    found = Some((self.labels[j].1, self.labels[j].2));
                    break;
                }
            }
            match found {
                Some((ip, label_depth)) => {
                    s.code[at as usize].a = ip;
                    s.code[at as usize].b = (depth - label_depth) as u16;
                }
                None => return Err("unknown label".into()),
            }
        }
        Ok(())
    }

    // ── Statements ──────────────────────────────────────────────────────────

    fn compile_chain(&mut self, s: &mut LuaState, mut n: u16) -> Result<(), LuaError> {
        while n != NO_NODE {
            self.compile_stmt(s, n)?;
            n = s.next[n as usize];
        }
        Ok(())
    }

    fn compile_stmt(&mut self, s: &mut LuaState, n: u16) -> Result<(), LuaError> {
        match s.nodes[n as usize] {
            Node::LocalDecl(first_name, val_node) => {
                let count = chain_len(s, first_name) as u16;
                self.compile_expr(s, val_node, count)?;
                self.emit(s, InstrOp::DeclareLocal, first_name, count)?;
            }
            Node::GlobalDecl(name_node, val_node) => {
                if val_node != NO_NODE {
                    self.compile_expr(s, val_node, 1)?;
                } else {
                    self.emit(s, InstrOp::PushNil, 0, 0)?;
                }
                self.emit(s, InstrOp::SetGlobal, name_node, 0)?;
            }
            Node::AssignStmt(first_target, val_node) => {
                let count = chain_len(s, first_target) as u16;
                // LHS prefixes (index targets) are evaluated before the RHS.
                let mut t = first_target;
                while t != NO_NODE {
                    if let Node::Index(base, key) = s.nodes[t as usize] {
                        self.compile_expr(s, base, 1)?;
                        self.compile_expr(s, key, 1)?;
                    }
                    t = s.next[t as usize];
                }
                self.compile_expr(s, val_node, count)?;
                self.emit(s, InstrOp::AssignMulti, first_target, count)?;
            }
            Node::CallStmt(e) => {
                self.compile_expr(s, e, 1)?;
                self.emit(s, InstrOp::StmtCheck, 0, 0)?;
            }
            Node::ExprStmt(e) => {
                self.compile_expr(s, e, 1)?;
                self.emit(s, InstrOp::Print, 0, 0)?;
            }
            Node::IfStmt(cond, then_b, els) => {
                self.compile_expr(s, cond, 1)?;
                let jf = self.emit(s, InstrOp::JumpIfFalse, 0, 0)?;
                self.push_scope(s)?;
                self.compile_chain(s, then_b)?;
                self.pop_scope(s)?;
                let jend = self.emit(s, InstrOp::Jump, 0, 0)?;
                s.code[jf as usize].a = s.ncode;
                if els != NO_NODE {
                    self.push_scope(s)?;
                    self.compile_chain(s, els)?;
                    self.pop_scope(s)?;
                }
                s.code[jend as usize].a = s.ncode;
            }
            Node::WhileStmt(cond, body) => {
                let loop_ip = s.ncode;
                self.compile_expr(s, cond, 1)?;
                let jf = self.emit(s, InstrOp::JumpIfFalse, 0, 0)?;
                self.begin_loop(self.scope)?;
                self.push_scope(s)?;
                self.compile_chain(s, body)?;
                self.pop_scope(s)?;
                self.emit(s, InstrOp::Jump, loop_ip, 0)?;
                let end = s.ncode;
                s.code[jf as usize].a = end;
                self.end_loop(s, end);
            }
            Node::RepeatStmt(body, cond) => {
                let loop_ip = s.ncode;
                self.begin_loop(self.scope)?;
                self.push_scope(s)?;
                self.compile_chain(s, body)?;
                self.compile_expr(s, cond, 1)?;
                self.pop_scope(s)?;
                self.emit(s, InstrOp::JumpIfFalse, loop_ip, 0)?;
                let end = s.ncode;
                self.end_loop(s, end);
            }
            Node::ForStmt(var, start, limit, step, body) => {
                self.push_scope(s)?;
                self.compile_expr(s, start, 1)?;
                self.compile_expr(s, limit, 1)?;
                if step != NO_NODE {
                    self.compile_expr(s, step, 1)?;
                } else {
                    self.emit(s, InstrOp::PushInt, 1, 0)?;
                }
                let prep = self.emit(s, InstrOp::ForPrep, 0, var)?;
                let body_ip = s.ncode;
                self.begin_loop(self.scope - 1)?;
                self.push_scope(s)?;
                self.compile_chain(s, body)?;
                self.pop_scope(s)?;
                self.emit(s, InstrOp::ForLoop, body_ip, var)?;
                let for_end = s.ncode;
                self.pop_scope(s)?;
                s.code[prep as usize].a = for_end;
                let end = s.ncode;
                self.end_loop(s, end);
            }
            Node::ForInStmt(kvar, vvar, table, body) => {
                self.push_scope(s)?;
                self.compile_expr(s, table, 2)?;
                self.emit(s, InstrOp::ForInPrep, 0, 0)?;
                let loop_ip = s.ncode;
                let next = self.emit(s, InstrOp::ForInNext, 0, 0)?;
                if vvar != NO_NODE {
                    self.emit(s, InstrOp::SetLocalTop, vvar, 0)?;
                } else {
                    // Keys-only loop: discard the value left on top.
                    self.emit(s, InstrOp::Pop, 0, 0)?;
                }
                self.emit(s, InstrOp::SetLocalTop, kvar, 0)?;
                self.begin_loop(self.scope - 1)?;
                self.push_scope(s)?;
                self.compile_chain(s, body)?;
                self.pop_scope(s)?;
                self.emit(s, InstrOp::ForInAdvance, loop_ip, 0)?;
                let for_end = s.ncode;
                self.pop_scope(s)?;
                s.code[next as usize].a = for_end;
                let end = s.ncode;
                self.end_loop(s, end);
            }
            Node::BreakStmt => {
                self.emit_break(s)?;
            }
            Node::Label(_) => {
                self.record_label(s, n)?;
            }
            Node::Goto(t) => {
                let at = self.emit(s, InstrOp::JumpScopes, 0, 0)?;
                self.add_fixup(at, t, self.scope)?;
            }
            Node::ReturnStmt(v) => {
                if v == NO_NODE {
                    // Leave every nested block scope before returning.
                    for _ in 0..self.scope {
                        self.emit(s, InstrOp::PopScope, 0, 0)?;
                    }
                    self.emit(s, InstrOp::Return, 0, 0)?;
                } else if matches!(s.nodes[v as usize], Node::Call(..)) {
                    self.compile_expr(s, v, WANT_ALL)?;
                    for _ in 0..self.scope {
                        self.emit(s, InstrOp::PopScope, 0, 0)?;
                    }
                    self.emit(s, InstrOp::Return, WANT_ALL, 0)?;
                } else {
                    self.compile_expr(s, v, 1)?;
                    for _ in 0..self.scope {
                        self.emit(s, InstrOp::PopScope, 0, 0)?;
                    }
                    self.emit(s, InstrOp::Return, 1, 0)?;
                }
            }
            _ => return Err("internal error: statement expected".into()),
        }
        Ok(())
    }

    // ── Expressions ─────────────────────────────────────────────────────────

    /// Compile `n`, leaving exactly `want` values on the operand stack
    /// (`WANT_ALL` leaves every result of a call, or one value otherwise).
    fn compile_expr(&mut self, s: &mut LuaState, n: u16, want: u16) -> Result<(), LuaError> {
        if let Node::Call(f, first_arg) = s.nodes[n as usize] {
            self.compile_call(s, f, first_arg, want)?;
            return Ok(());
        }
        match s.nodes[n as usize] {
            Node::Empty | Node::Nil | Node::True | Node::False | Node::Num(_) | Node::Float(_)
            | Node::Str(_) => {
                self.emit(s, InstrOp::PushConst, n, 0)?;
            }
            Node::Var(_) => {
                self.emit(s, InstrOp::GetVar, n, 0)?;
            }
            Node::Bin(op, l, r) => {
                if op == Op::And {
                    self.compile_expr(s, l, 1)?;
                    let j = self.emit(s, InstrOp::JumpIfFalseKeep, 0, 0)?;
                    self.compile_expr(s, r, 1)?;
                    s.code[j as usize].a = s.ncode;
                } else if op == Op::Or {
                    self.compile_expr(s, l, 1)?;
                    let j = self.emit(s, InstrOp::JumpIfTrueKeep, 0, 0)?;
                    self.compile_expr(s, r, 1)?;
                    s.code[j as usize].a = s.ncode;
                } else {
                    self.compile_expr(s, l, 1)?;
                    self.compile_expr(s, r, 1)?;
                    self.emit(s, InstrOp::BinOp, op as u16, 0)?;
                }
            }
            Node::Un(op, x) => {
                self.compile_expr(s, x, 1)?;
                self.emit(s, InstrOp::UnOp, op as u16, 0)?;
            }
            Node::Index(base, key) => {
                self.compile_expr(s, base, 1)?;
                self.compile_expr(s, key, 1)?;
                self.emit(s, InstrOp::GetIndex, 0, 0)?;
            }
            Node::FuncLit(fi) => {
                if !self.compiled[fi as usize] {
                    self.compiled[fi as usize] = true;
                    if self.npending as usize >= MAX_FUNCS {
                        return Err("too many functions".into());
                    }
                    self.pending[self.npending as usize] = fi;
                    self.npending += 1;
                }
                self.emit(s, InstrOp::LoadFunc, fi, 0)?;
            }
            Node::TableLit(first_field) => {
                self.emit(s, InstrOp::NewTable, 0, 0)?;
                let mut f = first_field;
                while f != NO_NODE {
                    let (k, v) = match s.nodes[f as usize] {
                        Node::Field(k, v) => (k, v),
                        _ => return Err("internal error: expected field".into()),
                    };
                    self.emit(s, InstrOp::Dup, 0, 0)?;
                    self.compile_expr(s, k, 1)?;
                    self.compile_expr(s, v, 1)?;
                    self.emit(s, InstrOp::SetIndex, 0, 0)?;
                    f = s.next[f as usize];
                }
            }
            _ => return Err("internal error: expression expected".into()),
        }
        // Non-call expressions yield one value; pad or discard to match.
        if want == 0 {
            self.emit(s, InstrOp::Pop, 0, 0)?;
        } else if want != 1 && want != WANT_ALL {
            for _ in 1..want {
                self.emit(s, InstrOp::PushNil, 0, 0)?;
            }
        }
        Ok(())
    }

    fn compile_call(
        &mut self,
        s: &mut LuaState,
        f: u16,
        first_arg: u16,
        want: u16,
    ) -> Result<(), LuaError> {
        // A final call argument expands to an unknown number of values, so the
        // argument count must be computed at runtime (a dynamic call).
        let mut dynamic = false;
        let mut n = first_arg;
        while n != NO_NODE {
            let is_last = s.next[n as usize] == NO_NODE;
            let vnode = match s.nodes[n as usize] {
                Node::Arg(v) => v,
                _ => return Err("internal error: expected argument".into()),
            };
            if is_last && matches!(s.nodes[vnode as usize], Node::Call(..)) {
                dynamic = true;
            }
            n = s.next[n as usize];
        }
        if dynamic {
            self.emit(s, InstrOp::Mark, 0, 0)?;
        }
        self.compile_expr(s, f, 1)?;
        let mut argc: u16 = 0;
        let mut n = first_arg;
        while n != NO_NODE {
            let is_last = s.next[n as usize] == NO_NODE;
            let vnode = match s.nodes[n as usize] {
                Node::Arg(v) => v,
                _ => return Err("internal error: expected argument".into()),
            };
            self.compile_expr(s, vnode, if is_last { WANT_ALL } else { 1 })?;
            argc += 1;
            if argc > 32 {
                return Err("too many arguments".into());
            }
            n = s.next[n as usize];
        }
        self.emit(s, InstrOp::Call, if dynamic { WANT_ALL } else { argc }, want)?;
        Ok(())
    }
}

fn chain_len(s: &LuaState, mut n: u16) -> usize {
    let mut count = 0usize;
    while n != NO_NODE {
        count += 1;
        n = s.next[n as usize];
    }
    count
}

/// Compile the top-level chain (and every function it references) into
/// bytecode, returning the entry instruction pointer.
pub fn compile(s: &mut LuaState, first: u16) -> Result<u16, LuaError> {
    let mut c = Compiler::new();
    let entry = s.ncode;
    c.compile_chain(s, first)?;
    c.emit(s, InstrOp::Return, 0, 0)?;
    c.resolve_fixups(s)?;
    while c.npending > 0 {
        let fi = c.pending[(c.npending - 1) as usize];
        c.npending -= 1;
        c.reset_function();
        let body = s.funcs[fi as usize].body;
        let fentry = s.ncode;
        s.funcs[fi as usize].entry = fentry;
        c.compile_chain(s, body)?;
        c.emit(s, InstrOp::Return, 0, 0)?;
        c.resolve_fixups(s)?;
    }
    Ok(entry)
}

// ── Interpreter ─────────────────────────────────────────────────────────────

fn node_name(s: &LuaState, node: u16) -> StrRef {
    eval::node_name(s, node)
}

fn ast_op(code: u16) -> Op {
    match code {
        0 => Op::Add,
        1 => Op::Sub,
        2 => Op::Mul,
        3 => Op::Div,
        4 => Op::Mod,
        5 => Op::Eq,
        6 => Op::Ne,
        7 => Op::Lt,
        8 => Op::Le,
        9 => Op::Gt,
        10 => Op::Ge,
        11 => Op::Concat,
        12 => Op::And,
        13 => Op::Or,
        14 => Op::Not,
        _ => Op::Neg,
    }
}

fn pop(s: &mut LuaState) -> Value {
    s.vsp -= 1;
    s.vstack[s.vsp as usize]
}

/// Count the values an [`ExecResult`] carries (already on the stack for
/// `RetN`).
fn result_count(r: ExecResult) -> usize {
    match r {
        ExecResult::Normal | ExecResult::Break | ExecResult::Goto(_) => 0,
        ExecResult::Ret(_) => 1,
        ExecResult::Ret2(..) => 2,
        ExecResult::RetN(n) => n as usize,
        ExecResult::Shell | ExecResult::Exit | ExecResult::Yield(_) => 0,
    }
}

fn result_from_stack(s: &LuaState, base: usize, n: usize) -> ExecResult {
    match n {
        0 => ExecResult::Normal,
        1 => ExecResult::Ret(s.vstack[base]),
        2 => ExecResult::Ret2(s.vstack[base], s.vstack[base + 1]),
        _ => ExecResult::RetN(n as u8),
    }
}

/// Normalize the top `vsp - base` values to exactly `want` (`WANT_ALL` keeps
/// them all).
fn normalize(s: &mut LuaState, base: usize, want: u16) -> Result<(), &'static str> {
    if want == WANT_ALL {
        return Ok(());
    }
    let want = want as usize;
    let n = s.vsp as usize - base;
    if n > want {
        s.vsp = (base + want) as u32;
    } else {
        for _ in n..want {
            s.push_val(Value::Nil)?;
        }
    }
    Ok(())
}

/// Set up a function frame for a call with `argc` arguments at
/// `base + args_offset`. `base` is where the callee sits (the VM's `Call`
/// convention, offset 1) or where the arguments start (the `call_value`
/// convention, offset 0); results are placed at `base` on return.
fn setup_func_frame(
    s: &mut LuaState,
    fi: u16,
    argc: u8,
    base: usize,
    args_offset: usize,
    ret_ip: u16,
    want: u16,
) -> Result<(), LuaError> {
    let fd = s.funcs[fi as usize];
    let mut args = [Value::Nil; 32];
    for i in 0..argc as usize {
        args[i] = s.vstack[base + args_offset + i];
    }
    s.push_frame()?;
    let f = (s.fsp - 1) as usize;
    s.frames[f].ret_ip = ret_ip;
    s.frames[f].want = want;
    s.frames[f].base = base as u32;
    for p in 0..fd.nparams as usize {
        let name = node_name(s, fd.params + p as u16);
        let v = if p < argc as usize { args[p] } else { Value::Nil };
        s.declare_local(name, v)?;
    }
    // The callee/arguments are consumed: the function body's operand stack
    // starts at the results base.
    s.vsp = base as u32;
    Ok(())
}

/// Call any callable value with `argc` arguments already on the operand
/// stack. Function calls run a nested VM invocation (used by `pcall`); the
/// VM's own `Call` instruction handles functions inline so coroutine yields
/// work at any call depth.
pub fn call_value(s: &mut LuaState, fv: Value, argc: u8) -> Result<ExecResult, LuaError> {
    match fv {
        Value::Func(fi) => {
            let base = s.vsp as usize - argc as usize;
            setup_func_frame(s, fi, argc, base, 0, RET_HALT, WANT_ALL)?;
            let entry = s.funcs[fi as usize].entry;
            run(s, entry)
        }
        _ => eval::call(s, fv, argc),
    }
}

/// Execute bytecode starting at `entry` until the root frame returns.
pub fn run(s: &mut LuaState, entry: u16) -> Result<ExecResult, LuaError> {
    let mut ip = entry;
    loop {
        if ip as usize >= s.ncode as usize {
            return Err("internal error: instruction pointer out of range".into());
        }
        let ins = s.code[ip as usize];
        ip += 1;
        match ins.op {
            InstrOp::Halt => return Ok(ExecResult::Normal),
            InstrOp::PushConst => {
                let v = match s.nodes[ins.a as usize] {
                    Node::Num(n) => Value::Num(n),
                    Node::Float(f) => Value::Float(f),
                    Node::Str(r) => Value::Str(r),
                    Node::Nil | Node::Empty => Value::Nil,
                    Node::True => Value::Bool(true),
                    Node::False => Value::Bool(false),
                    _ => return Err("internal error: bad constant".into()),
                };
                s.push_val(v)?;
            }
            InstrOp::PushNil => s.push_val(Value::Nil)?,
            InstrOp::PushInt => s.push_val(Value::Num(ins.a as i16 as i64))?,
            InstrOp::GetVar => {
                let name = node_name(s, ins.a);
                let v = s.lookup(name).ok_or(LuaError::Msg("undefined variable"))?;
                s.push_val(v)?;
            }
            InstrOp::SetVar => {
                let v = pop(s);
                let name = node_name(s, ins.a);
                if !s.assign_local(name, v) {
                    s.set_global(name, v);
                }
            }
            InstrOp::SetGlobal => {
                let v = pop(s);
                let name = node_name(s, ins.a);
                s.set_global(name, v);
            }
            InstrOp::SetLocalTop => {
                let v = pop(s);
                let name = node_name(s, ins.a);
                s.set_local_top(name, v)?;
            }
            InstrOp::DeclareLocal => {
                let count = ins.b as usize;
                let base = s.vsp as usize - count;
                let mut name_node = ins.a;
                let mut idx = 0usize;
                while name_node != NO_NODE {
                    let name = node_name(s, name_node);
                    let v = if idx < count {
                        s.vstack[base + idx]
                    } else {
                        Value::Nil
                    };
                    s.declare_local(name, v)?;
                    name_node = s.next[name_node as usize];
                    idx += 1;
                }
                s.vsp = base as u32;
            }
            InstrOp::GetIndex => {
                let key = pop(s);
                let base = pop(s);
                let v = eval::tget(s, base, key)?;
                s.push_val(v)?;
            }
            InstrOp::SetIndex => {
                let v = pop(s);
                let key = pop(s);
                let base = pop(s);
                eval::tset(s, base, key, v)?;
            }
            InstrOp::AssignMulti => {
                let count = ins.b as usize;
                let values_base = s.vsp as usize - count;
                let mut nidx = 0usize;
                let mut t = ins.a;
                while t != NO_NODE {
                    if matches!(s.nodes[t as usize], Node::Index(..)) {
                        nidx += 1;
                    }
                    t = s.next[t as usize];
                }
                let mut p = values_base - 2 * nidx;
                let mut i = 0usize;
                let mut t = ins.a;
                while t != NO_NODE {
                    let v = if i < count {
                        s.vstack[values_base + i]
                    } else {
                        Value::Nil
                    };
                    match s.nodes[t as usize] {
                        Node::Var(_) => {
                            let name = node_name(s, t);
                            if !s.assign_local(name, v) {
                                s.set_global(name, v);
                            }
                        }
                        Node::Index(..) => {
                            let base = s.vstack[p];
                            let key = s.vstack[p + 1];
                            p += 2;
                            eval::tset(s, base, key, v)?;
                        }
                        _ => return Err("invalid assignment target".into()),
                    }
                    i += 1;
                    t = s.next[t as usize];
                }
                s.vsp = (values_base - 2 * nidx) as u32;
            }
            InstrOp::NewTable => {
                let tid = eval::new_table(s)?;
                s.push_val(Value::Table(tid))?;
            }
            InstrOp::Dup => {
                let v = s.vstack[s.vsp as usize - 1];
                s.push_val(v)?;
            }
            InstrOp::Pop => {
                s.vsp -= 1;
            }
            InstrOp::BinOp => {
                let b = pop(s);
                let a = pop(s);
                let v = eval::binop(s, ast_op(ins.a), a, b)?;
                s.push_val(v)?;
            }
            InstrOp::UnOp => {
                let v = pop(s);
                let r = match ast_op(ins.a) {
                    Op::Not => Value::Bool(!eval::truthy(v)),
                    // `-v` with `__unm` dispatch for non-numbers.
                    Op::Neg => eval::unm(s, v)?,
                    _ => return Err("internal error: bad unary operator".into()),
                };
                s.push_val(r)?;
            }
            InstrOp::Jump => {
                ip = ins.a;
            }
            InstrOp::JumpScopes => {
                for _ in 0..ins.b {
                    s.pop_frame();
                }
                ip = ins.a;
            }
            InstrOp::JumpIfFalse => {
                let v = pop(s);
                if !eval::truthy(v) {
                    ip = ins.a;
                }
            }
            InstrOp::JumpIfFalseKeep => {
                let v = s.vstack[s.vsp as usize - 1];
                if !eval::truthy(v) {
                    ip = ins.a;
                } else {
                    s.vsp -= 1;
                }
            }
            InstrOp::JumpIfTrueKeep => {
                let v = s.vstack[s.vsp as usize - 1];
                if eval::truthy(v) {
                    ip = ins.a;
                } else {
                    s.vsp -= 1;
                }
            }
            InstrOp::LoadFunc => {
                s.push_val(Value::Func(ins.a))?;
            }
            InstrOp::Mark => {
                if s.mark_sp as usize >= super::MAX_FRAMES {
                    return Err("call too complex".into());
                }
                s.call_marks[s.mark_sp as usize] = s.vsp;
                s.mark_sp += 1;
            }
            InstrOp::Call => {
                let (base, argc) = if ins.a == WANT_ALL {
                    if s.mark_sp == 0 {
                        return Err("internal error: missing call mark".into());
                    }
                    s.mark_sp -= 1;
                    let base = s.call_marks[s.mark_sp as usize] as usize;
                    (base, s.vsp as usize - base - 1)
                } else {
                    let argc = ins.a as usize;
                    (s.vsp as usize - argc - 1, argc)
                };
                let fv = s.vstack[base];
                match fv {
                    Value::Func(fi) => {
                        let ret = ip;
                        setup_func_frame(s, fi, argc as u8, base, 1, ret, ins.b)?;
                        ip = s.funcs[fi as usize].entry;
                    }
                    _ => {
                        for i in 0..argc {
                            s.vstack[base + i] = s.vstack[base + 1 + i];
                        }
                        s.vsp = (base + argc) as u32;
                        let r = eval::call(s, fv, argc as u8)?;
                        match r {
                            ExecResult::Normal => {}
                            ExecResult::Ret(v) => s.push_val(v)?,
                            ExecResult::Ret2(a, b) => {
                                s.push_val(a)?;
                                s.push_val(b)?;
                            }
                            ExecResult::RetN(_) => {}
                            ExecResult::Yield(_) => {
                                // Save the continuation so `resume` can re-enter
                                // the VM right after this call.
                                s.yield_ip = ip;
                                s.yield_want = ins.b;
                                return Ok(r);
                            }
                            ExecResult::Shell | ExecResult::Exit => {
                                return Ok(r);
                            }
                            ExecResult::Break | ExecResult::Goto(_) => {
                                return Err("internal error: control flow from call".into());
                            }
                        }
                        normalize(s, base, ins.b)?;
                    }
                }
            }
            InstrOp::Return => {
                let f = (s.fsp - 1) as usize;
                let frame = s.frames[f];
                let base = frame.base as usize;
                let n = if ins.a == WANT_ALL {
                    s.vsp as usize - base
                } else {
                    ins.a as usize
                };
                let src = s.vsp as usize - n;
                for j in 0..n {
                    s.vstack[base + j] = s.vstack[src + j];
                }
                let out_n = if frame.want == WANT_ALL {
                    n
                } else {
                    frame.want as usize
                };
                if out_n > n {
                    for j in n..out_n {
                        s.vstack[base + j] = Value::Nil;
                    }
                }
                s.vsp = (base + out_n) as u32;
                s.pop_frame();
                if frame.ret_ip == RET_HALT {
                    return Ok(result_from_stack(s, base, out_n));
                }
                ip = frame.ret_ip;
            }
            InstrOp::Print => {
                let v = pop(s);
                eval::tostring(s, v)?;
                eval::emit(s, b'\n');
            }
            InstrOp::StmtCheck => {
                let v = pop(s);
                match v {
                    Value::Exit => return Ok(ExecResult::Exit),
                    Value::Shell => return Ok(ExecResult::Shell),
                    Value::Dhcp => {
                        if s.run_dhcp()? {
                            s.emit_dhcp_info();
                        }
                    }
                    Value::Ls => {
                        eval::ls_run(s)?;
                    }
                    _ => {}
                }
            }
            InstrOp::PushScope => {
                s.push_frame()?;
            }
            InstrOp::PopScope => {
                s.pop_frame();
            }
            InstrOp::ForPrep => {
                let step = pop(s);
                let limit = pop(s);
                let start = pop(s);
                let var = ins.b;
                let f = (s.fsp - 1) as usize;
                match (start, limit, step) {
                    (Value::Num(a), Value::Num(b), Value::Num(c)) => {
                        s.frames[f].ctrl[0] = Value::Num(a);
                        s.frames[f].ctrl[1] = Value::Num(b);
                        s.frames[f].ctrl[2] = Value::Num(c);
                        let skip = if c >= 0 { a > b } else { a < b };
                        if skip {
                            ip = ins.a;
                        } else {
                            s.set_local_top(node_name(s, var), Value::Num(a))?;
                        }
                    }
                    (a, b, c) => {
                        let fa = eval::float_of(a).ok_or(LuaError::Msg(
                            "for loop bound must be a number".into(),
                        ))?;
                        let fb = eval::float_of(b).ok_or(LuaError::Msg(
                            "for loop bound must be a number".into(),
                        ))?;
                        let fc = eval::float_of(c).ok_or(LuaError::Msg(
                            "for loop bound must be a number".into(),
                        ))?;
                        s.frames[f].ctrl[0] = Value::Float(fa);
                        s.frames[f].ctrl[1] = Value::Float(fb);
                        s.frames[f].ctrl[2] = Value::Float(fc);
                        let skip = if fc >= 0.0 { fa > fb } else { fa < fb };
                        if skip {
                            ip = ins.a;
                        } else {
                            s.set_local_top(node_name(s, var), Value::Float(fa))?;
                        }
                    }
                }
            }
            InstrOp::ForLoop => {
                let f = (s.fsp - 1) as usize;
                match (s.frames[f].ctrl[0], s.frames[f].ctrl[1], s.frames[f].ctrl[2]) {
                    (Value::Num(cur), Value::Num(lim), Value::Num(stp)) => {
                        let next = cur.wrapping_add(stp);
                        let done = if stp >= 0 { next > lim } else { next < lim };
                        if !done {
                            s.frames[f].ctrl[0] = Value::Num(next);
                            s.set_local_top(node_name(s, ins.b), Value::Num(next))?;
                            ip = ins.a;
                        }
                    }
                    (Value::Float(cur), Value::Float(lim), Value::Float(stp)) => {
                        let next = cur + stp;
                        let done = if stp >= 0.0 { next > lim } else { next < lim };
                        if !done {
                            s.frames[f].ctrl[0] = Value::Float(next);
                            s.set_local_top(node_name(s, ins.b), Value::Float(next))?;
                            ip = ins.a;
                        }
                    }
                    _ => return Err("internal error: bad for control".into()),
                }
            }
            InstrOp::ForInPrep => {
                let second = pop(s);
                let first = pop(s);
                let tid = match (first, second) {
                    (Value::Table(i), _) => i,
                    (Value::Native(3), Value::Table(i)) => i,
                    _ => return Err("'for in' requires a table or pairs(t)".into()),
                };
                let f = (s.fsp - 1) as usize;
                s.frames[f].ctrl[0] = Value::Num(tid as i64);
                s.frames[f].ctrl[1] = Value::Num(0);
                s.frames[f].ctrl[2] = Value::Nil;
            }
            InstrOp::ForInNext => {
                let f = (s.fsp - 1) as usize;
                let tid = match s.frames[f].ctrl[0] {
                    Value::Num(n) => n as usize,
                    _ => return Err("internal error: bad for-in control".into()),
                };
                let i = match s.frames[f].ctrl[1] {
                    Value::Num(n) => n as usize,
                    _ => return Err("internal error: bad for-in control".into()),
                };
                let len = s.tbls[tid].len as usize;
                if i >= len {
                    ip = ins.a;
                } else {
                    let slot = s.tbls[tid].slots[i];
                    s.frames[f].ctrl[2] = slot.key;
                    s.push_val(slot.key)?;
                    s.push_val(slot.value)?;
                }
            }
            InstrOp::ForInAdvance => {
                let f = (s.fsp - 1) as usize;
                let tid = match s.frames[f].ctrl[0] {
                    Value::Num(n) => n as usize,
                    _ => return Err("internal error: bad for-in control".into()),
                };
                let i = match s.frames[f].ctrl[1] {
                    Value::Num(n) => n as usize,
                    _ => return Err("internal error: bad for-in control".into()),
                };
                let key = s.frames[f].ctrl[2];
                let len = s.tbls[tid].len as usize;
                if i >= len || !eval::val_eq(s.tbls[tid].slots[i].key, key) {
                    // The current entry was removed; the next one shifted in.
                } else {
                    s.frames[f].ctrl[1] = Value::Num((i + 1) as i64);
                }
                ip = ins.a;
            }
            InstrOp::Yield => {
                return Err("attempt to yield from outside a coroutine".into());
            }
        }
        s.steps += 1;
        if s.steps > super::MAX_STEPS {
            return Err("step limit exceeded".into());
        }
    }
}

// ── Coroutines ──────────────────────────────────────────────────────────────

/// Collect an `ExecResult`'s values into `out` (for `RetN` the values are on
/// the stack and are popped).
fn collect_results(s: &mut LuaState, r: ExecResult, out: &mut [Value; 32]) -> usize {
    match r {
        ExecResult::Normal | ExecResult::Break | ExecResult::Goto(_) => 0,
        ExecResult::Ret(v) => {
            out[0] = v;
            1
        }
        ExecResult::Ret2(a, b) => {
            out[0] = a;
            out[1] = b;
            2
        }
        ExecResult::RetN(n) => {
            let base = s.vsp as usize - n as usize;
            for i in 0..n as usize {
                out[i] = s.vstack[base + i];
            }
            s.vsp = base as u32;
            n as usize
        }
        ExecResult::Shell | ExecResult::Exit | ExecResult::Yield(_) => 0,
    }
}

/// `coroutine.create(f)`: allocate a coroutine slot.
pub fn co_create(s: &mut LuaState, f: Value) -> Result<Value, LuaError> {
    if !matches!(f, Value::Func(_) | Value::Native(_)) {
        return Err("coroutine.create expects a function".into());
    }
    for i in 0..MAX_COS {
        if s.cos[i].status == CO_DEAD {
            s.threads[i + 1] = Thread::empty();
            s.cos[i] = CoState {
                status: CO_SUSPENDED,
                func: f,
                ip: 0,
                want: 0,
                parent: 0,
                started: false,
            };
            return Ok(Value::Co(i as u8));
        }
    }
    Err("too many coroutines".into())
}

/// `coroutine.resume(co, ...)`: run the coroutine until it yields, returns, or
/// errors. Returns `true` plus the results, or `false` plus the error value.
pub fn co_resume_value(s: &mut LuaState, id: u8, argc: u8) -> Result<ExecResult, LuaError> {
    if id as usize >= MAX_COS {
        return Err("invalid coroutine".into());
    }
    // Copy the resume arguments off the resumer's stack.
    let mut args = [Value::Nil; 32];
    for i in 0..argc as usize {
        args[i] = s.vstack[s.vsp as usize - argc as usize + i];
    }
    s.vsp -= argc as u32;

    let status = s.cos[id as usize].status;
    if status != CO_SUSPENDED {
        // Lua's `resume` reports this as a normal `false, message` result;
        // `wrap` re-raises it.
        let msg: &'static str = if status == CO_DEAD {
            "cannot resume dead coroutine"
        } else {
            "cannot resume non-suspended coroutine"
        };
        let r = s.intern(msg.as_bytes())?;
        s.push_val(Value::Bool(false))?;
        s.push_val(Value::Str(r))?;
        return Ok(ExecResult::RetN(2));
    }
    let parent = s.current;
    s.cos[id as usize].status = CO_RUNNING;
    s.cos[id as usize].parent = parent;
    if parent != 0 {
        s.cos[(parent - 1) as usize].status = CO_NORMAL;
    }
    let co_thread = id + 1;
    s.swap_threads(parent, co_thread);

    let started = s.cos[id as usize].started;
    let result: Result<ExecResult, LuaError>;
    if !started {
        s.cos[id as usize].started = true;
        for i in 0..argc as usize {
            s.push_val(args[i])?;
        }
        let f = s.cos[id as usize].func;
        match f {
            Value::Func(fi) => {
                let base = s.vsp as usize - argc as usize;
                setup_func_frame(s, fi, argc, base, 0, RET_HALT, WANT_ALL)?;
                let entry = s.funcs[fi as usize].entry;
                result = run(s, entry);
            }
            _ => {
                // Native body: call it once with the arguments.
                result = eval::call(s, f, argc);
            }
        }
    } else {
        for i in 0..argc as usize {
            s.push_val(args[i])?;
        }
        let want = s.cos[id as usize].want;
        let base = s.vsp as usize - argc as usize;
        normalize(s, base, want)?;
        let ip = s.cos[id as usize].ip;
        result = run(s, ip);
    }

    // Collect the results (or the error) before switching back.
    let mut out = [Value::Nil; 32];
    let out_n;
    let mut failed: Option<LuaError> = None;
    match result {
        Ok(ExecResult::Yield(n)) => {
            out_n = n as usize;
            let base = s.vsp as usize - out_n;
            for i in 0..out_n {
                out[i] = s.vstack[base + i];
            }
            s.vsp = base as u32;
            s.cos[id as usize].status = CO_SUSPENDED;
            s.cos[id as usize].ip = s.yield_ip;
            s.cos[id as usize].want = s.yield_want;
        }
        Ok(r) => {
            out_n = collect_results(s, r, &mut out);
            s.cos[id as usize].status = CO_DEAD;
        }
        Err(e) => {
            out_n = 0;
            s.cos[id as usize].status = CO_DEAD;
            failed = Some(e);
        }
    }

    // Switch back to the resumer.
    s.swap_threads(co_thread, parent);
    if parent != 0 {
        s.cos[(parent - 1) as usize].status = CO_RUNNING;
    }

    if let Some(e) = failed {
        let v = match e {
            LuaError::Obj(v) => v,
            LuaError::Msg(m) => Value::Str(s.intern(m.as_bytes())?),
        };
        s.push_val(Value::Bool(false))?;
        s.push_val(v)?;
        return Ok(ExecResult::RetN(2));
    }
    s.push_val(Value::Bool(true))?;
    for i in 0..out_n {
        s.push_val(out[i])?;
    }
    Ok(ExecResult::RetN((out_n + 1) as u8))
}

/// Calling a `coroutine.wrap` result: resume and re-raise errors.
pub fn co_wrap_call(s: &mut LuaState, id: u8, argc: u8) -> Result<ExecResult, LuaError> {
    match co_resume_value(s, id, argc)? {
        ExecResult::RetN(n) => {
            let base = s.vsp as usize - n as usize;
            let first = s.vstack[base];
            match first {
                Value::Bool(true) => {
                    let m = n as usize - 1;
                    for j in 0..m {
                        s.vstack[base + j] = s.vstack[base + 1 + j];
                    }
                    s.vsp = (base + m) as u32;
                    Ok(match m {
                        0 => ExecResult::Normal,
                        1 => ExecResult::Ret(s.vstack[base]),
                        2 => ExecResult::Ret2(s.vstack[base], s.vstack[base + 1]),
                        _ => ExecResult::RetN(m as u8),
                    })
                }
                // `false, error`: re-raise the error value.
                _ => {
                    let e = if n >= 2 {
                        s.vstack[base + 1]
                    } else {
                        Value::Nil
                    };
                    Err(LuaError::Obj(e))
                }
            }
        }
        _ => Err("internal error: bad resume result".into()),
    }
}

/// Execute a compiled top-level script (the root frame is pushed here).
pub fn exec_script(s: &mut LuaState, entry: u16) -> Result<(), LuaError> {
    s.push_frame()?;
    let f = (s.fsp - 1) as usize;
    s.frames[f].ret_ip = RET_HALT;
    s.frames[f].want = 0;
    s.frames[f].base = s.vsp;
    match run(s, entry)? {
        ExecResult::Normal
        | ExecResult::Ret(_)
        | ExecResult::Ret2(..)
        | ExecResult::RetN(_)
        | ExecResult::Shell
        | ExecResult::Exit
        | ExecResult::Yield(_) => Ok(()),
        ExecResult::Break => Err("break outside loop".into()),
        ExecResult::Goto(_) => Err("unknown label".into()),
    }
}

/// `dofile`: load, compile, and run a chunk, returning its first result.
pub fn exec_chunk(s: &mut LuaState, name: &str) -> Result<Value, LuaError> {
    let load = s.load;
    let mut buf = [0u8; DOFILE_CAP];
    let n = match load {
        Some(f) => f(name, &mut buf),
        None => return Err("dofile not available (no file loader)".into()),
    };
    let n = n.ok_or(LuaError::Msg("cannot open file".into()))?;
    if n > buf.len() {
        return Err("file too large".into());
    }
    let src = &buf[..n];
    let first = {
        let mut p = Parser::new(src, s);
        p.parse_script()?
    };
    let entry = compile(s, first)?;
    s.push_frame()?;
    let f = (s.fsp - 1) as usize;
    s.frames[f].ret_ip = RET_HALT;
    s.frames[f].want = 1;
    s.frames[f].base = s.vsp;
    match run(s, entry)? {
        ExecResult::Normal => Ok(Value::Nil),
        ExecResult::Ret(v) => Ok(v),
        ExecResult::Ret2(a, _) => Ok(a),
        ExecResult::RetN(n) => {
            let v = if n == 0 {
                Value::Nil
            } else {
                s.vstack[s.vsp as usize - n as usize]
            };
            s.vsp -= n as u32;
            Ok(v)
        }
        ExecResult::Shell => Ok(Value::Shell),
        ExecResult::Exit => Ok(Value::Exit),
        ExecResult::Yield(_) => Err("attempt to yield across a dofile boundary".into()),
        ExecResult::Break => Err("break outside loop".into()),
        ExecResult::Goto(_) => Err("unknown label".into()),
    }
}

/// One line of REPL input: parse, compile, and run it, printing bare
/// expression results like the old evaluator did.
pub fn run_repl_once(s: &mut LuaState, line: &[u8]) -> Result<ExecResult, LuaError> {
    s.steps = 0;
    s.vsp = 0;
    s.fsp = 0;
    s.mark_sp = 0;
    let mut lex = Lexer::new(line);
    let tok = lex.next_token();
    let is_expr_start = matches!(
        tok,
        Ok(Tok::Name(_, _)
            | Tok::Num(_)
            | Tok::Float(_)
            | Tok::Str(_)
            | Tok::LParen
            | Tok::True
            | Tok::False
            | Tok::Nil
            | Tok::Dot
            | Tok::Minus
            | Tok::Not)
    );
    if is_expr_start {
        let mut p = Parser::new(line, s);
        let e = p.parse_expr()?;
        let after = p.current();
        if after == Tok::Equals || after == Tok::Comma {
            let stmt = p.parse_assignment_from(e)?;
            let entry = compile(s, stmt)?;
            return exec_script(s, entry).map(|_| ExecResult::Normal);
        }
        let entry = s.ncode;
        let mut c = Compiler::new();
        c.compile_expr(s, e, WANT_ALL)?;
        c.emit(s, InstrOp::Return, WANT_ALL, 0)?;
        s.push_frame()?;
        let f = (s.fsp - 1) as usize;
        s.frames[f].ret_ip = RET_HALT;
        s.frames[f].want = WANT_ALL;
        s.frames[f].base = 0;
        let r = run(s, entry)?;
        let n = result_count(r);
        let base = 0usize;
        let first = if n >= 1 { s.vstack[base] } else { Value::Nil };
        match first {
            Value::Exit => {
                s.vsp = 0;
                return Ok(ExecResult::Exit);
            }
            Value::Shell => {
                s.vsp = 0;
                return Ok(ExecResult::Shell);
            }
            Value::Dhcp => {
                let ok = s.run_dhcp()?;
                if ok {
                    s.emit_dhcp_info();
                }
                eval::tostring(s, Value::Bool(ok))?;
                eval::emit(s, b'\n');
                s.vsp = 0;
                return Ok(ExecResult::Normal);
            }
            Value::Ls => {
                eval::ls_run(s)?;
                s.vsp = 0;
                return Ok(ExecResult::Normal);
            }
            _ => {
                if n == 0 {
                    eval::tostring(s, Value::Nil)?;
                } else {
                    for i in 0..n {
                        if i > 0 {
                            eval::emit(s, b'\t');
                        }
                        eval::tostring(s, s.vstack[base + i])?;
                    }
                }
                eval::emit(s, b'\n');
            }
        }
        s.vsp = 0;
        return Ok(ExecResult::Normal);
    }
    let first = {
        let mut p = Parser::new(line, s);
        p.parse_script()?
    };
    let entry = compile(s, first)?;
    exec_script(s, entry).map(|_| ExecResult::Normal)
}
