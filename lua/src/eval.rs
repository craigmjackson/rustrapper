//! Tree-walking evaluator.

use super::{LuaState, Node, Op, Value, NO_NODE};

/// Result of executing a statement chain.
#[derive(Clone, Copy)]
pub enum ExecResult {
    Normal,
    Ret(Value),
    /// Two return values (`next` returns key and value; a user function can
    /// forward them with `return next(t)`).
    Ret2(Value, Value),
    /// `break` — unwinds to the innermost loop, which stops iterating.
    Break,
    /// `goto` — jump to the label node. Unwinds through block frames until the
    /// block whose chain contains the target, which resumes execution there.
    Goto(u16),
    Shell,
    Exit,
}

/// Numeric `for` loop iterator state: exact integers when all bounds are
/// integers, floating point when any bound is a float.
#[derive(Clone, Copy)]
enum NumIter {
    Int(i64, i64, i64),
    Float(f64, f64, f64),
}

/// Numeric view of a value, accepting both integers and floats.
fn float_of(v: Value) -> Option<f64> {
    match v {
        Value::Num(n) => Some(n as f64),
        Value::Float(f) => Some(f),
        _ => None,
    }
}

/// `f64::floor` without libm: truncate toward zero via an `i64` cast, then step
/// down when the truncation rounded up. Exact for `|x| < 2^63`.
fn float_floor(x: f64) -> f64 {
    let t = x as i64;
    let tf = t as f64;
    if tf > x {
        tf - 1.0
    } else {
        tf
    }
}

/// Execute the top-level script (its own scope frame).
pub fn exec_script(s: &mut LuaState, first: u16) -> Result<(), &'static str> {
    s.push_frame()?;
    let r = run_chain(s, first);
    s.pop_frame();
    match r {
        Ok(ExecResult::Normal) => Ok(()),
        Ok(ExecResult::Ret(_)) | Ok(ExecResult::Ret2(..)) => Ok(()),
        Ok(ExecResult::Break) => Err("break outside loop"),
        Ok(ExecResult::Goto(_)) => Err("unknown label"),
        Ok(ExecResult::Shell) | Ok(ExecResult::Exit) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Run a block's statement chain, resolving `goto` targets that land inside
/// this chain (forward/backward jumps within the block). A `goto` whose target
/// lies in an enclosing block's chain propagates upward as [`ExecResult::Goto`]
/// after this block's frame has been popped.
fn run_chain(s: &mut LuaState, first: u16) -> Result<ExecResult, &'static str> {
    let mut n = first;
    loop {
        match exec_chain(s, n)? {
            ExecResult::Goto(t) if chain_contains(s, first, t) => n = t,
            r => return Ok(r),
        }
    }
}

/// Whether `target` is a node in the chain that starts at `first`.
fn chain_contains(s: &LuaState, first: u16, target: u16) -> bool {
    let mut n = first;
    while n != NO_NODE {
        if n == target {
            return true;
        }
        n = s.next[n as usize];
    }
    false
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
            ExecResult::Normal => {
                n = match s.nodes[n as usize] {
                    Node::Goto(t) => return Ok(ExecResult::Goto(t)),
                    _ => s.next[n as usize],
                }
            }
            r => return Ok(r),
        }
    }
    Ok(ExecResult::Normal)
}

/// Execute a single line of input in the REPL (resets step counter only).
/// Returns `Ok(ExecResult::Shell)` to continue the REPL, or `Ok(ExecResult::Exit)`
/// if the line was `exit()`.
pub fn run_repl_once(s: &mut LuaState, line: &[u8], _putc: fn(u8)) -> Result<ExecResult, &'static str> {
    // Reset step counter so each line gets a fresh budget.
    s.steps = 0;
    // Reset the value stack and frame (but preserve globals, strings, tables).
    s.vsp = 0;
    s.fsp = 0;
    // Check if the first token is a bare expression (not a keyword).
    // If so, parse it as an expression and print the result (Lua REPL behavior).
    // But skip if it's an assignment (name = ...).
    let mut lex = super::lex::Lexer::new(line);
    let tok = lex.next_token();
    let is_expr_start = matches!(tok, Ok(super::lex::Tok::Name(_, _) | super::lex::Tok::Num(_) | super::lex::Tok::Float(_) | super::lex::Tok::Str(_) | super::lex::Tok::LParen | super::lex::Tok::True | super::lex::Tok::False | super::lex::Tok::Nil | super::lex::Tok::Dot | super::lex::Tok::Minus | super::lex::Tok::Not));
    if is_expr_start {
        let mut p = super::parse::Parser::new(line, s);
        let e = p.parse_expr()?;
        let after = p.current();
        if after == super::lex::Tok::Equals || after == super::lex::Tok::Comma {
            // `x = ...` / `k, v = ...`: an assignment statement, not a bare
            // expression, so it should not print a result.
            let stmt = p.parse_assignment_from(e)?;
            return exec_script(s, stmt).map(|_| ExecResult::Normal);
        }
        let (v, second, n) = eval_multi(s, e)?;
        match v {
            Value::Exit => return Ok(ExecResult::Exit),
            Value::Shell => return Ok(ExecResult::Shell),
            Value::Dhcp => {
                let ok = s.run_dhcp()?;
                if ok {
                    s.emit_dhcp_info();
                }
                tostring(s, Value::Bool(ok))?;
                emit(s, b'\n');
                return Ok(ExecResult::Normal);
            }
            Value::Ls => {
                ls_run(s)?;
                return Ok(ExecResult::Normal);
            }
            _ => {
                if n == 0 {
                    // A call that returns nothing prints as nil (existing
                    // behavior; Lua's REPL prints nothing).
                    tostring(s, Value::Nil)?;
                } else {
                    tostring(s, v)?;
                    if n == 2 {
                        emit(s, b'\t');
                        tostring(s, second)?;
                    }
                }
                emit(s, b'\n');
            }
        }
        return Ok(ExecResult::Normal);
    }
    let first = {
        let mut p = super::parse::Parser::new(line, s);
        p.parse_script()?
    };
    exec_script(s, first).map(|_| ExecResult::Normal)
}

/// Run a block with its own fresh scope frame.
fn exec_block(s: &mut LuaState, first: u16) -> Result<ExecResult, &'static str> {
    s.push_frame()?;
    let r = run_chain(s, first);
    s.pop_frame();
    r
}

fn exec_stmt(s: &mut LuaState, n: u16) -> Result<ExecResult, &'static str> {
    match s.nodes[n as usize] {
        Node::LocalDecl(first_name, val_node) => {
            // Name nodes are chained through `next[]`; a call in the value
            // position can supply two values (`local k, v = next(t)`).
            let (a, b, n) = eval_multi(s, val_node)?;
            let mut name_node = first_name;
            let mut idx = 0u8;
            while name_node != NO_NODE {
                let name = node_name(s, name_node);
                let v = match idx {
                    0 => a,
                    1 if n == 2 => b,
                    _ => Value::Nil,
                };
                s.declare_local(name, v)?;
                name_node = s.next[name_node as usize];
                idx += 1;
            }
            Ok(ExecResult::Normal)
        }
        Node::GlobalDecl(name_node, val_node) => {
            let v = if val_node != NO_NODE {
                eval(s, val_node)?
            } else {
                Value::Nil
            };
            let name = node_name(s, name_node);
            s.set_global(name, v);
            Ok(ExecResult::Normal)
        }
        Node::AssignStmt(first_target, v) => {
            // Targets are chained through `next[]`; all values are computed
            // before any assignment (`k, v = next(t)`).
            let (a, b, n) = eval_multi(s, v)?;
            let mut target = first_target;
            let mut idx = 0u8;
            while target != NO_NODE {
                let vv = match idx {
                    0 => a,
                    1 if n == 2 => b,
                    _ => Value::Nil,
                };
                assign(s, target, vv)?;
                target = s.next[target as usize];
                idx += 1;
            }
            Ok(ExecResult::Normal)
        }
        Node::CallStmt(e) => {
            let v = eval(s, e)?;
            match v {
                Value::Exit => return Ok(ExecResult::Exit),
                Value::Shell => return Ok(ExecResult::Shell),
                Value::Dhcp => {
                    if s.run_dhcp()? {
                        s.emit_dhcp_info();
                    }
                    return Ok(ExecResult::Normal);
                }
                Value::Ls => {
                    ls_run(s)?;
                    return Ok(ExecResult::Normal);
                }
                _ => {}
            }
            Ok(ExecResult::Normal)
        }
        Node::ExprStmt(e) => {
            let v = eval(s, e)?;
            tostring(s, v)?;
            emit(s, b'\n');
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
                    ExecResult::Break => break,
                    r => return Ok(r),
                }
            }
            Ok(ExecResult::Normal)
        }
        Node::RepeatStmt(body, cond) => {
            let mut result = ExecResult::Normal;
            loop {
                s.steps += 1;
                if s.steps > super::MAX_STEPS {
                    return Err("step limit exceeded");
                }
                s.push_frame()?;
                let r = run_chain(s, body);
                match r {
                    Ok(ExecResult::Normal) => {
                        // Evaluate the condition in the body's scope, so locals
                        // declared in the body are visible in `until` (Lua).
                        let c = eval(s, cond);
                        s.pop_frame();
                        if truthy(c?) {
                            break;
                        }
                    }
                    Ok(ExecResult::Break) => {
                        s.pop_frame();
                        break;
                    }
                    Ok(r2) => {
                        s.pop_frame();
                        result = r2;
                        break;
                    }
                    Err(e) => {
                        s.pop_frame();
                        return Err(e);
                    }
                }
            }
            Ok(result)
        }
        Node::ForStmt(var, start, limit, step, body) => {
            let sv = eval(s, start)?;
            let lv = eval(s, limit)?;
            let stv = if step != NO_NODE {
                eval(s, step)?
            } else {
                Value::Num(1)
            };
            // All-integer bounds keep exact integer iteration; if any bound is
            // a float the loop runs in floating point (Lua-like).
            let mut iter = match (sv, lv, stv) {
                (Value::Num(a), Value::Num(b), Value::Num(c)) => NumIter::Int(a, b, c),
                (a, b, c) => NumIter::Float(
                    float_of(a).ok_or("for loop bound must be a number")?,
                    float_of(b).ok_or("for loop bound must be a number")?,
                    float_of(c).ok_or("for loop bound must be a number")?,
                ),
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
                let value = match iter {
                    NumIter::Int(cur, lim, stp) => {
                        if if stp >= 0 { cur > lim } else { cur < lim } {
                            break;
                        }
                        iter = NumIter::Int(cur.wrapping_add(stp), lim, stp);
                        Value::Num(cur)
                    }
                    NumIter::Float(cur, lim, stp) => {
                        if if stp >= 0.0 { cur > lim } else { cur < lim } {
                            break;
                        }
                        iter = NumIter::Float(cur + stp, lim, stp);
                        Value::Float(cur)
                    }
                };
                s.set_local_top(name, value)?;
                match exec_block(s, body)? {
                    ExecResult::Normal => {}
                    ExecResult::Break => break,
                    r => {
                        result = r;
                        break;
                    }
                }
            }
            s.pop_frame();
            Ok(result)
        }
        Node::ForInStmt(kvar, vvar, table, body) => {
            let t = eval(s, table)?;
            let tid = match t {
                Value::Table(i) => i,
                _ => return Err("'for in' requires a table value"),
            };
            let kname = node_name(s, kvar);
            let vname = if vvar != NO_NODE {
                Some(node_name(s, vvar))
            } else {
                None
            };
            s.push_frame()?;
            let mut result = ExecResult::Normal;
            // The length is re-read each iteration and `i` only advances when
            // the current key is still at `i`, so the body may remove the
            // current field (`t[k] = nil`) without skipping or re-visiting
            // entries (Lua allows nil-assigning existing fields while
            // traversing).
            let mut i = 0usize;
            while i < s.tbls[tid as usize].len as usize {
                s.steps += 1;
                if s.steps > super::MAX_STEPS {
                    let e = Err("step limit exceeded");
                    s.pop_frame();
                    return e;
                }
                let slot = s.tbls[tid as usize].slots[i];
                s.set_local_top(kname, slot.key)?;
                if let Some(vn) = vname {
                    s.set_local_top(vn, slot.value)?;
                }
                match exec_block(s, body)? {
                    ExecResult::Normal => {}
                    ExecResult::Break => break,
                    r => {
                        result = r;
                        break;
                    }
                }
                if i >= s.tbls[tid as usize].len as usize
                    || !val_eq(s.tbls[tid as usize].slots[i].key, slot.key)
                {
                    // The current key was removed; the next entry shifted in.
                    continue;
                }
                i += 1;
            }
            s.pop_frame();
            Ok(result)
        }
        Node::BreakStmt => Ok(ExecResult::Break),
        Node::Label(_) => Ok(ExecResult::Normal),
        Node::Goto(t) => Ok(ExecResult::Goto(t)),
        Node::ReturnStmt(v) => {
            if v == NO_NODE {
                return Ok(ExecResult::Ret(Value::Nil));
            }
            // `return next(t)` forwards both values to the caller.
            let (a, b, n) = eval_multi(s, v)?;
            Ok(if n == 2 {
                ExecResult::Ret2(a, b)
            } else {
                ExecResult::Ret(a)
            })
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

/// Evaluate a call node: evaluate the callee and the args (a *final* call
/// argument expands to its multiple results, matching Lua), then invoke the
/// function. Returns [`ExecResult::Ret`] / [`ExecResult::Ret2`] for the
/// results, or [`ExecResult::Normal`] when the function returns nothing.
fn eval_call(s: &mut LuaState, f: u16, first_arg: u16) -> Result<ExecResult, &'static str> {
    let fv = eval(s, f)?;
    let mut argc: u8 = 0;
    let mut n = first_arg;
    while n != NO_NODE {
        let is_last = s.next[n as usize] == NO_NODE;
        let vnode = match s.nodes[n as usize] {
            Node::Arg(v) => v,
            _ => return Err("internal error: expected argument"),
        };
        if is_last {
            let (a, b, cnt) = eval_multi(s, vnode)?;
            if cnt >= 1 {
                s.push_val(a)?;
                argc += 1;
            }
            if cnt == 2 {
                s.push_val(b)?;
                argc += 1;
            }
        } else {
            let v = eval(s, vnode)?;
            s.push_val(v)?;
            argc += 1;
        }
        if argc >= 32 {
            return Err("too many arguments");
        }
        n = s.next[n as usize];
    }
    call(s, fv, argc)
}

/// Evaluate an expression in a multiple-value context. A call may yield two
/// values (e.g. `next`); anything else yields one. Returns `(first, second,
/// count)` where `count` is 0 (a call returning nothing), 1, or 2.
fn eval_multi(s: &mut LuaState, n: u16) -> Result<(Value, Value, u8), &'static str> {
    if let Node::Call(f, first_arg) = s.nodes[n as usize] {
        return match eval_call(s, f, first_arg)? {
            ExecResult::Normal => Ok((Value::Nil, Value::Nil, 0)),
            ExecResult::Ret(v) => Ok((v, Value::Nil, 1)),
            ExecResult::Ret2(a, b) => Ok((a, b, 2)),
            ExecResult::Break => Err("break outside loop"),
            ExecResult::Goto(_) => Err("goto outside function"),
            ExecResult::Shell => Ok((Value::Shell, Value::Nil, 1)),
            ExecResult::Exit => Ok((Value::Exit, Value::Nil, 1)),
        };
    }
    Ok((eval(s, n)?, Value::Nil, 1))
}

/// Evaluate an expression node.
fn eval(s: &mut LuaState, n: u16) -> Result<Value, &'static str> {
    match s.nodes[n as usize] {
        Node::Empty | Node::Nil => Ok(Value::Nil),
        Node::True => Ok(Value::Bool(true)),
        Node::False => Ok(Value::Bool(false)),
        Node::Num(v) => Ok(Value::Num(v)),
        Node::Float(v) => Ok(Value::Float(v)),
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
            Value::Float(v) => Ok(Value::Float(-v)),
            _ => Err("attempt to perform arithmetic on a non-number value"),
        },
        Node::Index(base, key) => {
            let b = eval(s, base)?;
            let k = eval(s, key)?;
            tget(s, b, k)
        }
        Node::Call(f, first_arg) => match eval_call(s, f, first_arg)? {
            ExecResult::Normal => Ok(Value::Nil),
            ExecResult::Ret(v) => Ok(v),
            // Single-value context: keep the first result.
            ExecResult::Ret2(a, _) => Ok(a),
            ExecResult::Break => Err("break outside loop"),
            ExecResult::Goto(_) => Err("goto outside function"),
            ExecResult::Shell => Ok(Value::Shell),
            ExecResult::Exit => Ok(Value::Exit),
        },
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
fn call(s: &mut LuaState, fv: Value, argc: u8) -> Result<ExecResult, &'static str> {
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
            ExecResult::Normal
        }
        Value::Native(1) => {
            // fetch(filename [, dest]) -> byte count, or nil if the download
            // failed. `dest` is the local name the file is saved/recorded under
            // (shown by `ls`); it defaults to the source name.
            if argc != 1 && argc != 2 {
                return Err("fetch expects 1 or 2 arguments");
            }
            match argbuf[0] {
                Value::Str(r) => {
                    let mut nbuf = [0u8; 128];
                    let bytes = s.str_bytes(r);
                    if bytes.len() >= 128 {
                        return Err("fetch filename too long");
                    }
                    nbuf[..bytes.len()].copy_from_slice(bytes);
                    let name = core::str::from_utf8(&nbuf[..bytes.len()])
                        .map_err(|_| "fetch filename must be ASCII")?;

                    let mut dbuf = [0u8; 128];
                    let (save_ref, save_name) = if argc == 2 {
                        match argbuf[1] {
                            Value::Str(dr) => {
                                let dbytes = s.str_bytes(dr);
                                if dbytes.len() >= 128 {
                                    return Err("fetch dest too long");
                                }
                                dbuf[..dbytes.len()].copy_from_slice(dbytes);
                                let n = core::str::from_utf8(&dbuf[..dbytes.len()])
                                    .map_err(|_| "fetch dest must be ASCII")?;
                                (dr, n)
                            }
                            _ => return Err("fetch dest must be a string"),
                        }
                    } else {
                        (r, name)
                    };

                    let fetch = s.fetch;
                    match fetch {
                        Some(f) => match f(name, save_name) {
                            Some(n) => {
                                s.record_fetch_ref(save_ref, n as u64)?;
                                ExecResult::Ret(Value::Num(n as i64))
                            }
                            None => ExecResult::Ret(Value::Nil),
                        },
                        None => return Err("fetch not available (no TFTP server)"),
                    }
                }
                _ => return Err("fetch expects a string filename"),
            }
        }
        Value::Native(2) => {
            // dofile(filename): load, parse, and execute a Lua chunk, returning
            // the chunk's return value (or nil). Errors propagate to the caller.
            if argc != 1 {
                return Err("dofile expects 1 argument");
            }
            match argbuf[0] {
                Value::Str(r) => {
                    let bytes = s.str_bytes(r);
                    if bytes.len() >= 128 {
                        return Err("dofile filename too long");
                    }
                    let mut nbuf = [0u8; 128];
                    nbuf[..bytes.len()].copy_from_slice(bytes);
                    let name = core::str::from_utf8(&nbuf[..bytes.len()])
                        .map_err(|_| "dofile filename must be ASCII")?;
                    ExecResult::Ret(dofile_exec(s, name)?)
                }
                _ => return Err("dofile expects a string filename"),
            }
        }
        Value::Native(3) => {
            // next(table [, index]) -> next index, value; nil at the end.
            // Absent/nil index starts the traversal. An index that is not a
            // key of the table is an error (Lua: "invalid key to 'next'").
            if argc != 1 && argc != 2 {
                return Err("next expects 1 or 2 arguments");
            }
            let tid = match argbuf[0] {
                Value::Table(i) => i,
                _ => return Err("next expects a table"),
            };
            let len = s.tbls[tid as usize].len as usize;
            let start = if argc == 1 || matches!(argbuf[1], Value::Nil) {
                0
            } else {
                let key = argbuf[1];
                let mut found = None;
                for i in 0..len {
                    if val_eq(s.tbls[tid as usize].slots[i].key, key) {
                        found = Some(i + 1);
                        break;
                    }
                }
                match found {
                    Some(i) => i,
                    None => return Err("invalid key to 'next'"),
                }
            };
            if start >= len {
                ExecResult::Ret(Value::Nil)
            } else {
                let slot = s.tbls[tid as usize].slots[start];
                ExecResult::Ret2(slot.key, slot.value)
            }
        }
        Value::Shell => {
            if argc != 0 {
                return Err("shell expects no arguments");
            }
            ExecResult::Shell
        }
        Value::Exit => {
            if argc != 0 {
                return Err("exit expects no arguments");
            }
            ExecResult::Exit
        }
        Value::Dhcp => {
            if argc != 0 {
                return Err("dhcp expects no arguments");
            }
            let ok = s.run_dhcp()?;
            if ok {
                s.emit_dhcp_info();
            }
            ExecResult::Ret(Value::Bool(ok))
        }
        Value::Ls => {
            if argc != 0 {
                return Err("ls expects no arguments");
            }
            ls_run(s)?;
            ExecResult::Normal
        }
        Value::Func(idx) => {
            let fd = s.funcs[idx as usize];
            s.push_frame()?;
            for p in 0..fd.nparams as usize {
                let name = node_name(s, fd.params + p as u16);
                let val = if p < argc as usize { argbuf[p] } else { Value::Nil };
                s.declare_local(name, val)?;
            }
            let r = run_chain(s, fd.body);
            s.pop_frame();
            match r {
                Ok(ExecResult::Normal) => ExecResult::Normal,
                Ok(ExecResult::Ret(v)) => ExecResult::Ret(v),
                Ok(ExecResult::Ret2(a, b)) => ExecResult::Ret2(a, b),
                Ok(ExecResult::Break) => return Err("break outside loop"),
                Ok(ExecResult::Goto(_)) => return Err("goto outside function"),
                Ok(ExecResult::Shell) => ExecResult::Shell,
                Ok(ExecResult::Exit) => ExecResult::Exit,
                Err(e) => return Err(e),
            }
        }
        _ => return Err("attempt to call a non-function value"),
    };

    s.vsp = base as u32;
    Ok(result)
}

/// `dofile(name)`: load a Lua chunk via the host `load` callback, parse it in
/// the current state, and run it in a fresh frame. Returns the chunk's return
/// value (`nil` if it doesn't return). The chunk shares globals with its
/// caller but has its own locals; errors propagate to the caller.
fn dofile_exec(s: &mut LuaState, name: &str) -> Result<Value, &'static str> {
    let load = s.load;
    let mut buf = [0u8; super::DOFILE_CAP];
    let n = match load {
        Some(f) => f(name, &mut buf),
        None => return Err("dofile not available (no file loader)"),
    };
    let n = n.ok_or("cannot open file")?;
    if n > buf.len() {
        return Err("file too large");
    }
    let src = &buf[..n];
    let first = {
        let mut p = super::parse::Parser::new(src, s);
        p.parse_script()?
    };
    s.push_frame()?;
    let r = run_chain(s, first);
    s.pop_frame();
    match r {
        Ok(ExecResult::Normal) => Ok(Value::Nil),
        Ok(ExecResult::Ret(v)) => Ok(v),
        // Single-value context: keep the first result.
        Ok(ExecResult::Ret2(a, _)) => Ok(a),
        Ok(ExecResult::Break) => Err("break outside loop"),
        Ok(ExecResult::Goto(_)) => Err("unknown label"),
        Ok(ExecResult::Shell) => Ok(Value::Shell),
        Ok(ExecResult::Exit) => Ok(Value::Exit),
        Err(e) => Err(e),
    }
}

/// `ls`: print every file downloaded with `fetch()` this run/session, one per
/// line as `name (N bytes)`. Prints nothing when no files have been fetched.
fn ls_run(s: &mut LuaState) -> Result<(), &'static str> {
    for i in 0..s.fetched_n as usize {
        emit_bytes(s, s.str_bytes(s.fetched_names[i]));
        emit(s, b' ');
        emit(s, b'(');
        let (buf, len) = itoa(s.fetched_lens[i] as i64);
        emit_bytes(s, &buf[..len]);
        emit_str(s, b" bytes)\n");
    }
    Ok(())
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
        (Value::Float(x), Value::Float(y)) => x == y,
        // Numbers compare across int/float: `1 == 1.0` is true in Lua.
        (Value::Num(x), Value::Float(y)) | (Value::Float(y), Value::Num(x)) => x as f64 == y,
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
            // Integer/integer stays in exact integer arithmetic (the existing
            // semantics); if either side is a float, promote both to f64.
            if let (Value::Num(x), Value::Num(y)) = (a, b) {
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
                return Ok(Value::Num(r));
            }
            let x = float_of(a).ok_or("attempt to perform arithmetic on a non-number value")?;
            let y = float_of(b).ok_or("attempt to perform arithmetic on a non-number value")?;
            let r = match op {
                Add => x + y,
                Sub => x - y,
                Mul => x * y,
                // Float division by zero yields inf/nan (Lua), unlike integers.
                Div => x / y,
                // Lua float modulo: a - floor(a/b) * b.
                Mod => x - float_floor(x / y) * y,
                _ => 0.0,
            };
            Ok(Value::Float(r))
        }
        Eq => Ok(Value::Bool(val_eq(a, b))),
        Ne => Ok(Value::Bool(!val_eq(a, b))),
        Lt | Le | Gt | Ge => {
            let r = if let (Value::Num(x), Value::Num(y)) = (a, b) {
                match op {
                    Lt => x < y,
                    Le => x <= y,
                    Gt => x > y,
                    Ge => x >= y,
                    _ => false,
                }
            } else {
                let x = float_of(a).ok_or("attempt to compare non-number values")?;
                let y = float_of(b).ok_or("attempt to compare non-number values")?;
                match op {
                    Lt => x < y,
                    Le => x <= y,
                    Gt => x > y,
                    Ge => x >= y,
                    _ => false,
                }
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
        Value::Float(f) => {
            let mut buf = [0u8; FLOAT_BUF];
            let len = fmt_float(f, &mut buf);
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
        Value::Float(f) => {
            let mut buf = [0u8; FLOAT_BUF];
            let len = fmt_float(f, &mut buf);
            emit_bytes(s, &buf[..len]);
        }
        Value::Str(r) => emit_bytes(s, s.str_bytes(r)),
        Value::Table(_) => emit_str(s, b"table"),
        Value::Func(_) => emit_str(s, b"function"),
        Value::Native(_) => emit_str(s, b"native"),
        Value::Shell => emit_str(s, b"shell"),
        Value::Dhcp => emit_str(s, b"dhcp"),
        Value::Exit => emit_str(s, b"exit"),
        Value::Ls => emit_str(s, b"ls"),
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

/// Scratch size for a formatted float (up to `-4.9406564584125e-324.0`).
const FLOAT_BUF: usize = 40;

/// `core::fmt::Write` sink over a fixed byte buffer (no allocation).
struct FmtBuf<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl core::fmt::Write for FmtBuf<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let b = s.as_bytes();
        if self.len + b.len() > self.buf.len() {
            return Err(core::fmt::Error);
        }
        self.buf[self.len..self.len + b.len()].copy_from_slice(b);
        self.len += b.len();
        Ok(())
    }
}

/// Format a float the way Lua's `tostring` does: `%.14g` (14 significant
/// digits) with `.0` appended to integer-looking results. Returns the byte
/// length; `out` must be at least [`FLOAT_BUF`] bytes.
fn fmt_float(v: f64, out: &mut [u8]) -> usize {
    use core::fmt::Write;

    if v.is_nan() {
        out[..3].copy_from_slice(b"nan");
        return 3;
    }
    if v.is_infinite() {
        let s: &[u8] = if v < 0.0 { b"-inf" } else { b"inf" };
        out[..s.len()].copy_from_slice(s);
        return s.len();
    }
    if v == 0.0 {
        let s: &[u8] = if v.is_sign_negative() { b"-0.0" } else { b"0.0" };
        out[..s.len()].copy_from_slice(s);
        return s.len();
    }

    // Render with 14 significant digits and read off the decimal exponent.
    let mut sci = [0u8; 32];
    let n = {
        let mut w = FmtBuf {
            buf: &mut sci,
            len: 0,
        };
        if write!(w, "{:.13e}", v).is_err() {
            return 0;
        }
        w.len
    };
    let epos = match sci[..n].iter().position(|&b| b == b'e') {
        Some(p) => p,
        None => {
            out[..n].copy_from_slice(&sci[..n]);
            return n;
        }
    };
    let mut i = epos + 1;
    let mut exp_neg = false;
    if sci[i] == b'-' {
        exp_neg = true;
        i += 1;
    } else if sci[i] == b'+' {
        i += 1;
    }
    let mut exp: i32 = 0;
    while i < n {
        exp = exp * 10 + (sci[i] - b'0') as i32;
        i += 1;
    }
    if exp_neg {
        exp = -exp;
    }

    if exp < -4 || exp >= 14 {
        // Scientific notation: trim trailing zeros from the mantissa.
        let mut mend = epos;
        while mend > 1 && sci[mend - 1] == b'0' {
            mend -= 1;
        }
        if mend > 1 && sci[mend - 1] == b'.' {
            mend -= 1;
        }
        let mut o = 0;
        out[..mend].copy_from_slice(&sci[..mend]);
        o += mend;
        out[o] = b'e';
        o += 1;
        out[o] = if exp < 0 { b'-' } else { b'+' };
        o += 1;
        let mut mag: u32 = if exp < 0 { (-exp) as u32 } else { exp as u32 };
        if mag < 10 {
            out[o] = b'0';
            o += 1;
        }
        let mut tmp = [0u8; 10];
        let mut t = tmp.len();
        loop {
            t -= 1;
            tmp[t] = b'0' + (mag % 10) as u8;
            mag /= 10;
            if mag == 0 {
                break;
            }
        }
        while t < tmp.len() {
            out[o] = tmp[t];
            o += 1;
            t += 1;
        }
        o
    } else {
        // Fixed notation with `13 - exp` decimals (14 significant digits).
        let prec = (13 - exp) as usize;
        let n2 = {
            let mut w = FmtBuf { buf: out, len: 0 };
            if write!(w, "{:.*}", prec, v).is_err() {
                return 0;
            }
            w.len
        };
        if let Some(dot) = out[..n2].iter().position(|&b| b == b'.') {
            let mut end = n2;
            while end > dot + 1 && out[end - 1] == b'0' {
                end -= 1;
            }
            if end == dot + 1 {
                // Every decimal was zero: keep one (`5.` -> `5.0`).
                out[end] = b'0';
                end += 1;
            }
            end
        } else {
            // No fractional point at all (`12345678901234`): append `.0`.
            out[n2] = b'.';
            out[n2 + 1] = b'0';
            n2 + 2
        }
    }
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
    let nil = matches!(v, Value::Nil);
    let len = s.tbls[tid as usize].len as usize;
    for i in 0..len {
        if val_eq(s.tbls[tid as usize].slots[i].key, k) {
            if nil {
                // Lua: assigning nil removes the field (so `next` never sees
                // it again). Shift the remaining slots down to keep them
                // contiguous for iteration.
                for j in i..len - 1 {
                    s.tbls[tid as usize].slots[j] = s.tbls[tid as usize].slots[j + 1];
                }
                s.tbls[tid as usize].len -= 1;
            } else {
                s.tbls[tid as usize].slots[i].value = v;
            }
            return Ok(());
        }
    }
    if nil {
        // Assigning nil to a non-existent field is a no-op.
        return Ok(());
    }
    if len >= super::TABLE_SLOTS {
        return Err("table full");
    }
    s.tbls[tid as usize].slots[len] = super::TableSlot { key: k, value: v };
    s.tbls[tid as usize].len += 1;
    Ok(())
}
