//! Tree-walking evaluator.

use super::{LuaState, Node, Op, Value, NO_NODE};

/// Result of executing a statement chain.
#[derive(Clone, Copy)]
pub enum ExecResult {
    Normal,
    Ret(Value),
}

/// Execute the top-level script (its own scope frame).
pub fn exec_script(s: &mut LuaState, first: u16) -> Result<(), &'static str> {
    s.push_frame()?;
    let r = exec_chain(s, first);
    s.pop_frame();
    match r {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Run a statement chain in the current frame.
fn exec_chain(s: &mut LuaState, first: u16) -> Result<ExecResult, &'static str> {
    let mut n = first;
    while n != NO_NODE {
        s.steps += 1;
        if s.steps > super::MAX_STEPS {
            return Err("step limit exceeded");
        }
        match exec_stmt(s, n)? {
            ExecResult::Normal => n = s.next[n as usize],
            r => return Ok(r),
        }
    }
    Ok(ExecResult::Normal)
}

/// Run a block with its own fresh scope frame.
fn exec_block(s: &mut LuaState, first: u16) -> Result<ExecResult, &'static str> {
    s.push_frame()?;
    let r = exec_chain(s, first);
    s.pop_frame();
    r
}

fn exec_stmt(s: &mut LuaState, n: u16) -> Result<ExecResult, &'static str> {
    match s.nodes[n as usize] {
        Node::LocalDecl(name_node, val_node) => {
            let v = eval(s, val_node)?;
            let name = node_name(s, name_node);
            s.declare_local(name, v)?;
            Ok(ExecResult::Normal)
        }
        Node::AssignStmt(t, v) => {
            let vv = eval(s, v)?;
            assign(s, t, vv)?;
            Ok(ExecResult::Normal)
        }
        Node::CallStmt(e) => {
            eval(s, e)?;
            Ok(ExecResult::Normal)
        }
        Node::IfStmt(cond, then_b, els) => {
            let c = eval(s, cond)?;
            if truthy(c) {
                exec_block(s, then_b)
            } else if els != NO_NODE {
                exec_block(s, els)
            } else {
                Ok(ExecResult::Normal)
            }
        }
        Node::WhileStmt(cond, body) => {
            loop {
                s.steps += 1;
                if s.steps > super::MAX_STEPS {
                    return Err("step limit exceeded");
                }
                let c = eval(s, cond)?;
                if !truthy(c) {
                    break;
                }
                match exec_block(s, body)? {
                    ExecResult::Normal => {}
                    r => return Ok(r),
                }
            }
            Ok(ExecResult::Normal)
        }
        Node::ForStmt(var, start, limit, step, body) => {
            let (mut cur, lim, stp) = match (
                eval(s, start)?,
                eval(s, limit)?,
                if step != NO_NODE {
                    eval(s, step)?
                } else {
                    Value::Num(1)
                },
            ) {
                (Value::Num(a), Value::Num(b), Value::Num(c)) => (a, b, c),
                _ => return Err("for loop bound must be a number"),
            };
            let name = node_name(s, var);
            s.push_frame()?;
            let mut result = ExecResult::Normal;
            loop {
                s.steps += 1;
                if s.steps > super::MAX_STEPS {
                    let e = Err("step limit exceeded");
                    s.pop_frame();
                    return e;
                }
                let done = if stp >= 0 { cur > lim } else { cur < lim };
                if done {
                    break;
                }
                s.set_local_top(name, Value::Num(cur))?;
                match exec_block(s, body)? {
                    ExecResult::Normal => {}
                    r => {
                        result = r;
                        break;
                    }
                }
                cur = cur.wrapping_add(stp);
            }
            s.pop_frame();
            Ok(result)
        }
        Node::ReturnStmt(v) => {
            let r = if v != NO_NODE { eval(s, v)? } else { Value::Nil };
            Ok(ExecResult::Ret(r))
        }
        _ => Err("internal error: statement expected"),
    }
}

fn node_name(s: &LuaState, node: u16) -> super::StrRef {
    match s.nodes[node as usize] {
        Node::Var(name) => name,
        _ => 0,
    }
}

/// Evaluate an expression node.
fn eval(s: &mut LuaState, n: u16) -> Result<Value, &'static str> {
    match s.nodes[n as usize] {
        Node::Empty | Node::Nil => Ok(Value::Nil),
        Node::True => Ok(Value::Bool(true)),
        Node::False => Ok(Value::Bool(false)),
        Node::Num(v) => Ok(Value::Num(v)),
        Node::Str(r) => Ok(Value::Str(r)),
        Node::Var(name) => s.lookup(name).ok_or("undefined variable"),
        Node::Bin(op, l, r) => {
            if op == Op::And {
                let lv = eval(s, l)?;
                return if truthy(lv) { eval(s, r) } else { Ok(lv) };
            }
            if op == Op::Or {
                let lv = eval(s, l)?;
                return if truthy(lv) { Ok(lv) } else { eval(s, r) };
            }
            let lv = eval(s, l)?;
            let rv = eval(s, r)?;
            binop(s, op, lv, rv)
        }
        Node::Un(Op::Not, x) => {
            let v = eval(s, x)?;
            Ok(Value::Bool(!truthy(v)))
        }
        Node::Un(Op::Neg, x) => match eval(s, x)? {
            Value::Num(v) => Ok(Value::Num(v.wrapping_neg())),
            _ => Err("attempt to perform arithmetic on a non-number value"),
        },
        Node::Index(base, key) => {
            let b = eval(s, base)?;
            let k = eval(s, key)?;
            tget(s, b, k)
        }
        Node::Call(f, first_arg) => {
            let fv = eval(s, f)?;
            let mut argc: u8 = 0;
            let mut n = first_arg;
            while n != NO_NODE {
                let av = match s.nodes[n as usize] {
                    Node::Arg(v) => eval(s, v)?,
                    _ => return Err("internal error: expected argument"),
                };
                s.push_val(av)?;
                argc += 1;
                if argc >= 32 {
                    return Err("too many arguments");
                }
                n = s.next[n as usize];
            }
            call(s, fv, argc)
        }
        Node::FuncLit(i) => Ok(Value::Func(i)),
        Node::TableLit(first_field) => {
            let tid = new_table(s)?;
            let mut n = first_field;
            while n != NO_NODE {
                let (k, v) = match s.nodes[n as usize] {
                    Node::Field(k, v) => (k, v),
                    _ => return Err("internal error: expected field"),
                };
                let kv = eval(s, k)?;
                let vv = eval(s, v)?;
                tset(s, Value::Table(tid), kv, vv)?;
                n = s.next[n as usize];
            }
            Ok(Value::Table(tid))
        }
        _ => Err("internal error: expression expected"),
    }
}

/// Call a function value with `argc` args on the value stack.
fn call(s: &mut LuaState, fv: Value, argc: u8) -> Result<Value, &'static str> {
    if argc as usize > s.vsp as usize {
        return Err("internal error: arg stack underflow");
    }
    let base = s.vsp as usize - argc as usize;

    // Copy args out to avoid borrowing `vstack` while mutating state.
    let mut argbuf: [Value; 32] = [Value::Nil; 32];
    for i in 0..argc as usize {
        argbuf[i] = s.vstack[base + i];
    }

    let result = match fv {
        Value::Native(0) => {
            for (i, a) in argbuf[..argc as usize].iter().enumerate() {
                if i > 0 {
                    emit(s, b'\t');
                }
                tostring(s, *a)?;
            }
            emit(s, b'\n');
            Value::Nil
        }
        Value::Native(1) => {
            // fetch(filename) -> byte count, or nil if the download failed.
            if argc != 1 {
                return Err("fetch expects 1 argument");
            }
            match argbuf[0] {
                Value::Str(r) => {
                    let fetch = s.fetch;
                    let name = core::str::from_utf8(s.str_bytes(r))
                        .map_err(|_| "fetch filename must be ASCII")?;
                    match fetch {
                        Some(f) => match f(name) {
                            Some(n) => Value::Num(n as i64),
                            None => Value::Nil,
                        },
                        None => return Err("fetch not available"),
                    }
                }
                _ => return Err("fetch expects a string filename"),
            }
        }
        Value::Func(idx) => {
            let fd = s.funcs[idx as usize];
            s.push_frame()?;
            for p in 0..fd.nparams as usize {
                let name = node_name(s, fd.params + p as u16);
                let val = if p < argc as usize { argbuf[p] } else { Value::Nil };
                s.declare_local(name, val)?;
            }
            let r = exec_chain(s, fd.body);
            s.pop_frame();
            match r {
                Ok(ExecResult::Normal) => Value::Nil,
                Ok(ExecResult::Ret(v)) => v,
                Err(e) => return Err(e),
            }
        }
        _ => return Err("attempt to call a non-function value"),
    };

    s.vsp = base as u32;
    Ok(result)
}

fn assign(s: &mut LuaState, target: u16, v: Value) -> Result<(), &'static str> {
    match s.nodes[target as usize] {
        Node::Var(name) => {
            if !s.assign_local(name, v) {
                s.set_global(name, v);
            }
            Ok(())
        }
        Node::Index(base, key) => {
            let b = eval(s, base)?;
            let k = eval(s, key)?;
            tset(s, b, k, v)
        }
        _ => Err("invalid assignment target"),
    }
}

fn truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

fn val_eq(a: Value, b: Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Num(x), Value::Num(y)) => x == y,
        // Strings are interned, so identity equals content equality.
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Table(x), Value::Table(y)) => x == y,
        (Value::Func(x), Value::Func(y)) => x == y,
        (Value::Native(x), Value::Native(y)) => x == y,
        _ => false,
    }
}

fn binop(s: &mut LuaState, op: Op, a: Value, b: Value) -> Result<Value, &'static str> {
    use Op::*;
    match op {
        Add | Sub | Mul | Div | Mod => {
            let (x, y) = match (a, b) {
                (Value::Num(x), Value::Num(y)) => (x, y),
                _ => return Err("attempt to perform arithmetic on a non-number value"),
            };
            let r = match op {
                Add => x.wrapping_add(y),
                Sub => x.wrapping_sub(y),
                Mul => x.wrapping_mul(y),
                Div => {
                    if y == 0 {
                        return Err("division by zero");
                    }
                    x / y
                }
                Mod => {
                    if y == 0 {
                        return Err("division by zero");
                    }
                    x.rem_euclid(y)
                }
                _ => 0,
            };
            Ok(Value::Num(r))
        }
        Eq => Ok(Value::Bool(val_eq(a, b))),
        Ne => Ok(Value::Bool(!val_eq(a, b))),
        Lt | Le | Gt | Ge => {
            let (x, y) = match (a, b) {
                (Value::Num(x), Value::Num(y)) => (x, y),
                _ => return Err("attempt to compare non-number values"),
            };
            let r = match op {
                Lt => x < y,
                Le => x <= y,
                Gt => x > y,
                Ge => x >= y,
                _ => false,
            };
            Ok(Value::Bool(r))
        }
        Concat => {
            let sa = string_of(s, a)?;
            let sb = string_of(s, b)?;
            let ba = s.str_bytes(sa);
            let bb = s.str_bytes(sb);
            let mut tmp = [0u8; 512];
            if ba.len() + bb.len() > tmp.len() {
                return Err("string too long");
            }
            tmp[..ba.len()].copy_from_slice(ba);
            tmp[ba.len()..ba.len() + bb.len()].copy_from_slice(bb);
            Ok(Value::Str(s.intern(&tmp[..ba.len() + bb.len()])?))
        }
        And | Or | Not | Neg => Err("internal error: operator handled elsewhere"),
    }
}

/// Convert a value to an interned string. Numbers are allowed alongside
/// strings (matching Lua's concat coercion); other types are an error.
fn string_of(s: &mut LuaState, v: Value) -> Result<super::StrRef, &'static str> {
    match v {
        Value::Str(r) => Ok(r),
        Value::Num(n) => {
            let (buf, len) = itoa(n);
            s.intern(&buf[..len])
        }
        _ => Err("attempt to concatenate a non-string value"),
    }
}

/// Emit the Lua `tostring` rendering of a value via `putc`.
fn tostring(s: &LuaState, v: Value) -> Result<(), &'static str> {
    match v {
        Value::Nil => emit_str(s, b"nil"),
        Value::Bool(true) => emit_str(s, b"true"),
        Value::Bool(false) => emit_str(s, b"false"),
        Value::Num(n) => {
            let (buf, len) = itoa(n);
            emit_bytes(s, &buf[..len]);
        }
        Value::Str(r) => emit_bytes(s, s.str_bytes(r)),
        Value::Table(_) => emit_str(s, b"table"),
        Value::Func(_) => emit_str(s, b"function"),
        Value::Native(_) => emit_str(s, b"native"),
    }
    Ok(())
}

fn emit(s: &LuaState, b: u8) {
    (s.putc)(b);
}

fn emit_bytes(s: &LuaState, bytes: &[u8]) {
    for &b in bytes {
        (s.putc)(b);
    }
}

fn emit_str(s: &LuaState, bytes: &[u8]) {
    emit_bytes(s, bytes);
}

/// i64 to decimal, zero-padded left in the returned buffer.
/// Returns (buffer, length) with the digits at the start.
fn itoa(n: i64) -> ([u8; 24], usize) {
    let mut buf = [0u8; 24];
    let neg = n < 0;
    let mut v = if neg { n.wrapping_neg() as u64 } else { n as u64 };
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if neg {
        i -= 1;
        buf[i] = b'-';
    }
    let len = buf.len() - i;
    let mut j = 0;
    while j < len {
        buf[j] = buf[i + j];
        j += 1;
    }
    (buf, len)
}

// ── Tables ──────────────────────────────────────────────────────────────────

fn new_table(s: &mut LuaState) -> Result<u16, &'static str> {
    if s.ntables as usize >= super::MAX_TABLES {
        return Err("too many tables");
    }
    let tid = s.ntables;
    s.tbls[tid as usize] = super::TableRec {
        slots: [super::TableSlot {
            key: Value::Nil,
            value: Value::Nil,
        }; super::TABLE_SLOTS],
        len: 0,
    };
    s.ntables += 1;
    Ok(tid as u16)
}

fn tget(s: &LuaState, t: Value, k: Value) -> Result<Value, &'static str> {
    let tid = match t {
        Value::Table(i) => i,
        _ => return Err("attempt to index a non-table value"),
    };
    let rec = &s.tbls[tid as usize];
    for i in 0..rec.len as usize {
        if val_eq(rec.slots[i].key, k) {
            return Ok(rec.slots[i].value);
        }
    }
    Ok(Value::Nil)
}

fn tset(s: &mut LuaState, t: Value, k: Value, v: Value) -> Result<(), &'static str> {
    let tid = match t {
        Value::Table(i) => i,
        _ => return Err("attempt to index a non-table value"),
    };
    let len = s.tbls[tid as usize].len as usize;
    for i in 0..len {
        if val_eq(s.tbls[tid as usize].slots[i].key, k) {
            s.tbls[tid as usize].slots[i].value = v;
            return Ok(());
        }
    }
    if len >= super::TABLE_SLOTS {
        return Err("table full");
    }
    s.tbls[tid as usize].slots[len] = super::TableSlot { key: k, value: v };
    s.tbls[tid as usize].len += 1;
    Ok(())
}
