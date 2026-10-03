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
    /// N return values (`select` returns an arbitrary count): the top `n`
    /// values of the value stack are the results. `call` moves them down over
    /// the call's arguments before the caller sees them.
    RetN(u8),
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

/// Trim leading/trailing ASCII whitespace.
fn trim_ascii(b: &[u8]) -> &[u8] {
    let mut s = 0;
    let mut e = b.len();
    while s < e && b[s].is_ascii_whitespace() {
        s += 1;
    }
    while e > s && b[e - 1].is_ascii_whitespace() {
        e -= 1;
    }
    &b[s..e]
}

/// `tonumber(e)` for a string: parse an integer or float following the
/// interpreter's decimal lexical conventions. `inf`/`nan`/hex are rejected
/// (core's `f64` parser would otherwise accept `inf`/`nan`).
fn str_to_number(b: &[u8]) -> Option<Value> {
    let s = trim_ascii(b);
    if s.is_empty() {
        return None;
    }
    let text = core::str::from_utf8(s).ok()?;
    if let Ok(i) = text.parse::<i64>() {
        return Some(Value::Num(i));
    }
    let t = text.as_bytes();
    let mut i = 0;
    if t[i] == b'+' || t[i] == b'-' {
        i += 1;
    }
    let mut digits = 0;
    while i < t.len() && t[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < t.len() && t[i] == b'.' {
        i += 1;
        while i < t.len() && t[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    if i < t.len() && (t[i] == b'e' || t[i] == b'E') {
        i += 1;
        if i < t.len() && (t[i] == b'+' || t[i] == b'-') {
            i += 1;
        }
        let mut ed = 0;
        while i < t.len() && t[i].is_ascii_digit() {
            i += 1;
            ed += 1;
        }
        if ed == 0 {
            return None;
        }
    }
    if i != t.len() {
        return None;
    }
    text.parse::<f64>().ok().map(Value::Float)
}

/// `tonumber(e, base)`: parse a decimal/alpha-numeric string as an integer in
/// `base` (2..=36). An optional leading sign is allowed; overflow yields
/// `None`.
fn str_to_base(b: &[u8], base: i64) -> Option<i64> {
    let s = trim_ascii(b);
    if s.is_empty() {
        return None;
    }
    let mut i = 0;
    let mut neg = false;
    if s[i] == b'-' {
        neg = true;
        i += 1;
    } else if s[i] == b'+' {
        i += 1;
    }
    if i >= s.len() {
        return None;
    }
    let mut n: i64 = 0;
    while i < s.len() {
        let c = s[i];
        let d = match c {
            b'0'..=b'9' => (c - b'0') as i64,
            b'a'..=b'z' => (c - b'a') as i64 + 10,
            b'A'..=b'Z' => (c - b'A') as i64 + 10,
            _ => return None,
        };
        if d >= base {
            return None;
        }
        n = n.checked_mul(base)?.checked_add(d)?;
        i += 1;
    }
    Some(if neg { -n } else { n })
}

/// Execute the top-level script (its own scope frame).
pub fn exec_script(s: &mut LuaState, first: u16) -> Result<(), &'static str> {
    s.push_frame()?;
    let r = run_chain(s, first);
    s.pop_frame();
    match r {
        Ok(ExecResult::Normal) => Ok(()),
        Ok(ExecResult::Ret(_)) | Ok(ExecResult::Ret2(..)) | Ok(ExecResult::RetN(_)) => Ok(()),
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
        let n = eval_values(s, e)? as usize;
        let base = s.vsp as usize - n;
        let v = if n >= 1 { s.vstack[base] } else { Value::Nil };
        match v {
            Value::Exit => {
                pop_values(s, n);
                return Ok(ExecResult::Exit);
            }
            Value::Shell => {
                pop_values(s, n);
                return Ok(ExecResult::Shell);
            }
            Value::Dhcp => {
                let ok = s.run_dhcp()?;
                if ok {
                    s.emit_dhcp_info();
                }
                tostring(s, Value::Bool(ok))?;
                emit(s, b'\n');
                pop_values(s, n);
                return Ok(ExecResult::Normal);
            }
            Value::Ls => {
                ls_run(s)?;
                pop_values(s, n);
                return Ok(ExecResult::Normal);
            }
            _ => {
                if n == 0 {
                    // A call that returns nothing prints as nil (existing
                    // behavior; Lua's REPL prints nothing).
                    tostring(s, Value::Nil)?;
                } else {
                    for i in 0..n {
                        if i > 0 {
                            emit(s, b'\t');
                        }
                        tostring(s, s.vstack[base + i])?;
                    }
                }
                emit(s, b'\n');
            }
        }
        pop_values(s, n);
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
            // position can supply any number of values (`local k, v = next(t)`,
            // `local a, b, c = select(1, x, y, z)`); extra names get nil.
            let n = eval_values(s, val_node)? as usize;
            let base = s.vsp as usize - n;
            let mut name_node = first_name;
            let mut idx = 0usize;
            while name_node != NO_NODE {
                let name = node_name(s, name_node);
                let v = if idx < n {
                    s.vstack[base + idx]
                } else {
                    Value::Nil
                };
                s.declare_local(name, v)?;
                name_node = s.next[name_node as usize];
                idx += 1;
            }
            pop_values(s, n);
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
            let n = eval_values(s, v)? as usize;
            let base = s.vsp as usize - n;
            let mut target = first_target;
            let mut idx = 0usize;
            while target != NO_NODE {
                let vv = if idx < n {
                    s.vstack[base + idx]
                } else {
                    Value::Nil
                };
                assign(s, target, vv)?;
                target = s.next[target as usize];
                idx += 1;
            }
            pop_values(s, n);
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
            // Accepts the bare table form (`for k, v in t`) and the Lua
            // iterator form (`for k, v in pairs(t)`), where the call yields
            // `next, t`.
            let n = eval_values(s, table)? as usize;
            let base = s.vsp as usize - n;
            let first = if n >= 1 { s.vstack[base] } else { Value::Nil };
            let second = if n >= 2 { s.vstack[base + 1] } else { Value::Nil };
            pop_values(s, n);
            let tid = match (first, second) {
                (Value::Table(i), _) => i,
                (Value::Native(3), Value::Table(i)) => i,
                _ => return Err("'for in' requires a table or pairs(t)"),
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
            // `return next(t)` / `return select(...)` forwards every result to
            // the caller.
            Ok(ExecResult::RetN(eval_values(s, v)?))
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
            // A final call argument expands; `eval_values` pushes its results
            // directly onto our argument stack.
            argc += eval_values(s, vnode)?;
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

/// Evaluate an expression in a multiple-value context: push its results onto
/// the value stack and return the count. A call may yield any number of values
/// (`select`, `next`, `pairs`); anything else yields one.
fn eval_values(s: &mut LuaState, n: u16) -> Result<u8, &'static str> {
    if let Node::Call(f, first_arg) = s.nodes[n as usize] {
        return match eval_call(s, f, first_arg)? {
            ExecResult::Normal => Ok(0),
            ExecResult::Ret(v) => {
                s.push_val(v)?;
                Ok(1)
            }
            ExecResult::Ret2(a, b) => {
                s.push_val(a)?;
                s.push_val(b)?;
                Ok(2)
            }
            // The results are already on the stack.
            ExecResult::RetN(cnt) => Ok(cnt),
            ExecResult::Break => Err("break outside loop"),
            ExecResult::Goto(_) => Err("goto outside function"),
            ExecResult::Shell => {
                s.push_val(Value::Shell)?;
                Ok(1)
            }
            ExecResult::Exit => {
                s.push_val(Value::Exit)?;
                Ok(1)
            }
        };
    }
    let v = eval(s, n)?;
    s.push_val(v)?;
    Ok(1)
}

/// Pop `count` values that [`eval_values`] left on the stack.
fn pop_values(s: &mut LuaState, count: usize) {
    s.vsp -= count as u32;
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
            ExecResult::RetN(n) => {
                let v = if n == 0 {
                    Value::Nil
                } else {
                    s.vstack[s.vsp as usize - n as usize]
                };
                s.vsp -= n as u32;
                Ok(v)
            }
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
        Value::Native(4) => {
            // pairs(t) -> next, t. Metamethods are not dispatched in this
            // subset (no __pairs), and Lua's third result is nil, so two values
            // suffice:
            // `for k, v in pairs(t)` iterates t and `local f, s, c = pairs(t)`
            // still leaves c == nil.
            if argc != 1 {
                return Err("pairs expects 1 argument");
            }
            match argbuf[0] {
                Value::Table(i) => ExecResult::Ret2(Value::Native(3), Value::Table(i)),
                _ => return Err("pairs expects a table"),
            }
        }
        Value::Native(5) => {
            // rawequal(v1, v2): primitive equality, without metamethods. This
            // subset does not dispatch metamethods, so `==` and `rawequal` agree.
            if argc != 2 {
                return Err("rawequal expects 2 arguments");
            }
            ExecResult::Ret(Value::Bool(val_eq(argbuf[0], argbuf[1])))
        }
        Value::Native(6) => {
            // rawget(table, index): the real `table[index]`, without `__index`.
            // This subset does not dispatch metamethods, so it agrees with `table[index]`.
            if argc != 2 {
                return Err("rawget expects 2 arguments");
            }
            match argbuf[0] {
                Value::Table(_) => ExecResult::Ret(tget(s, argbuf[0], argbuf[1])?),
                _ => return Err("rawget expects a table"),
            }
        }
        Value::Native(7) => {
            // rawlen(v): length of a table or string, without `__len`. Metamethods
            // are not dispatched, so this is the plain length. A table's length
            // is the run of consecutive integer keys starting at 1.
            if argc != 1 {
                return Err("rawlen expects 1 argument");
            }
            let len = match argbuf[0] {
                Value::Str(r) => s.str_bytes(r).len(),
                Value::Table(_) => {
                    let mut n = 0usize;
                    loop {
                        let next = tget(s, argbuf[0], Value::Num((n + 1) as i64))?;
                        if matches!(next, Value::Nil) {
                            break;
                        }
                        n += 1;
                        if n >= super::TABLE_SLOTS {
                            break;
                        }
                    }
                    n
                }
                _ => return Err("rawlen expects a table or a string"),
            };
            ExecResult::Ret(Value::Num(len as i64))
        }
        Value::Native(8) => {
            // rawset(table, index, value): the real assignment without
            // `__newindex` (metamethods are not dispatched, so it agrees with
            // `t[k] = v`).
            // Returns the table.
            if argc != 3 {
                return Err("rawset expects 3 arguments");
            }
            if !matches!(argbuf[0], Value::Table(_)) {
                return Err("rawset expects a table");
            }
            tset(s, argbuf[0], argbuf[1], argbuf[2])?;
            ExecResult::Ret(argbuf[0])
        }
        Value::Native(9) => {
            // select(index, ...): "#" returns the number of extra arguments;
            // a number returns the arguments after position `index` (-1 is the
            // last argument).
            if argc < 1 {
                return Err("select expects at least 1 argument");
            }
            let extras = argc as usize - 1;
            match argbuf[0] {
                Value::Str(r) if s.str_bytes(r) == b"#" => ExecResult::Ret(Value::Num(extras as i64)),
                k => {
                    let mut i = match k {
                        Value::Num(n) => n,
                        Value::Float(f) if f.is_finite() && f == float_floor(f) => f as i64,
                        _ => return Err("select index must be an integer"),
                    };
                    let total = argc as i64;
                    if i < 0 {
                        i += total;
                    } else if i > total {
                        i = total;
                    }
                    if i < 1 {
                        return Err("select index out of range");
                    }
                    let start = i as usize;
                    for j in start..argc as usize {
                        s.push_val(argbuf[j])?;
                    }
                    ExecResult::RetN((argc as usize - start) as u8)
                }
            }
        }
        Value::Native(10) => {
            // setmetatable(table, metatable|nil): stores (`nil` removes) the
            // metatable and returns the table. A metatable whose `__metatable`
            // field is not nil is protected and cannot be changed. This subset
            // stores metatables but does not dispatch metamethods.
            if argc != 2 {
                return Err("setmetatable expects 2 arguments");
            }
            let tid = match argbuf[0] {
                Value::Table(i) => i,
                _ => return Err("setmetatable expects a table"),
            };
            let new_mt = match argbuf[1] {
                Value::Nil => None,
                Value::Table(i) => Some(i),
                _ => return Err("setmetatable expects a table or nil"),
            };
            if let Some(cur) = s.tbls[tid as usize].mt {
                let protected_name = s.intern(b"__metatable")?;
                let protected = tget(s, Value::Table(cur), Value::Str(protected_name))?;
                if !matches!(protected, Value::Nil) {
                    return Err("cannot change a protected metatable");
                }
            }
            s.tbls[tid as usize].mt = new_mt;
            ExecResult::Ret(argbuf[0])
        }
        Value::Native(11) => {
            // tonumber(e [, base]): convert a number or a numeric string. With
            // a base (2..36) the first argument must be a string and the
            // result is an integer (or nil).
            if argc < 1 || argc > 2 {
                return Err("tonumber expects 1 or 2 arguments");
            }
            if argc == 1 || matches!(argbuf[1], Value::Nil) {
                let v = match argbuf[0] {
                    Value::Num(_) | Value::Float(_) => argbuf[0],
                    Value::Str(r) => str_to_number(s.str_bytes(r)).unwrap_or(Value::Nil),
                    _ => Value::Nil,
                };
                ExecResult::Ret(v)
            } else {
                let base = match argbuf[1] {
                    Value::Num(n) => n,
                    Value::Float(f) if f.is_finite() && f == float_floor(f) => f as i64,
                    _ => return Err("tonumber base must be an integer"),
                };
                if !(2..=36).contains(&base) {
                    return Err("tonumber base out of range");
                }
                let r = match argbuf[0] {
                    Value::Str(r) => r,
                    _ => return Err("tonumber expects a string with a base"),
                };
                ExecResult::Ret(match str_to_base(s.str_bytes(r), base) {
                    Some(n) => Value::Num(n),
                    None => Value::Nil,
                })
            }
        }
        Value::Native(12) => {
            // tostring(v): the human-readable string form (the same rendering
            // `print` uses). Metamethods are not dispatched, so a `__tostring`
            // field is not consulted.
            if argc != 1 {
                return Err("tostring expects 1 argument");
            }
            ExecResult::Ret(Value::Str(value_to_string(s, argbuf[0])?))
        }
        Value::Native(13) => {
            // type(v): the Lua type name of a value. Every callable value
            // (including native builtins) reports "function".
            if argc != 1 {
                return Err("type expects 1 argument");
            }
            ExecResult::Ret(Value::Str(s.intern(type_name(argbuf[0]))?))
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
                Ok(ExecResult::RetN(n)) => ExecResult::RetN(n),
                Ok(ExecResult::Break) => return Err("break outside loop"),
                Ok(ExecResult::Goto(_)) => return Err("goto outside function"),
                Ok(ExecResult::Shell) => ExecResult::Shell,
                Ok(ExecResult::Exit) => ExecResult::Exit,
                Err(e) => return Err(e),
            }
        }
        _ => return Err("attempt to call a non-function value"),
    };

    match result {
        ExecResult::RetN(n) => {
            // Move the n results down over the call's arguments; they stay on
            // the stack for the caller to consume.
            let src = s.vsp as usize - n as usize;
            for j in 0..n as usize {
                s.vstack[base + j] = s.vstack[src + j];
            }
            s.vsp = (base + n as usize) as u32;
        }
        _ => s.vsp = base as u32,
    }
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
        Ok(ExecResult::RetN(n)) => {
            let v = if n == 0 {
                Value::Nil
            } else {
                s.vstack[s.vsp as usize - n as usize]
            };
            s.vsp -= n as u32;
            Ok(v)
        }
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
        // Unit builtins compare equal to themselves.
        (Value::Shell, Value::Shell)
        | (Value::Dhcp, Value::Dhcp)
        | (Value::Exit, Value::Exit)
        | (Value::Ls, Value::Ls) => true,
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

/// The Lua type name of a value — the `type` builtin. Every callable value
/// (user functions, native builtins, and the shell/dhcp/exit/ls keywords)
/// reports "function".
fn type_name(v: Value) -> &'static [u8] {
    match v {
        Value::Nil => b"nil",
        Value::Bool(_) => b"boolean",
        Value::Num(_) | Value::Float(_) => b"number",
        Value::Str(_) => b"string",
        Value::Table(_) => b"table",
        Value::Func(_) | Value::Native(_) | Value::Shell | Value::Dhcp | Value::Exit | Value::Ls => {
            b"function"
        }
    }
}

/// Convert a value to an interned string — the `tostring` builtin. Strings are
/// returned as-is (they are already interned); other values use the same
/// rendering as the `tostring` emitter below.
fn value_to_string(s: &mut LuaState, v: Value) -> Result<super::StrRef, &'static str> {
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
        Value::Nil => s.intern(b"nil"),
        Value::Bool(true) => s.intern(b"true"),
        Value::Bool(false) => s.intern(b"false"),
        Value::Table(_) => s.intern(b"table"),
        Value::Func(_) => s.intern(b"function"),
        Value::Native(_) => s.intern(b"native"),
        Value::Shell => s.intern(b"shell"),
        Value::Dhcp => s.intern(b"dhcp"),
        Value::Exit => s.intern(b"exit"),
        Value::Ls => s.intern(b"ls"),
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

    // Render with core::fmt using only the exponential form: it yields the
    // correctly rounded 14 significant digits, and avoids linking the
    // fixed-notation formatting path (worth ~3.6 KB on the firmware targets).
    // The digits are then rearranged into Lua's `%.14g` style below.
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

    // Collect the 14 mantissa digits (`[-]d.ddddddddddddd`), dropping the
    // decimal point and trimming trailing zeros.
    let mstart = if sci[0] == b'-' { 1 } else { 0 };
    let mut digits = [0u8; 14];
    digits[0] = sci[mstart];
    digits[1..].copy_from_slice(&sci[mstart + 2..mstart + 15]);
    let mut nd = 14usize;
    while nd > 1 && digits[nd - 1] == b'0' {
        nd -= 1;
    }

    let mut o = 0usize;
    if sci[0] == b'-' {
        out[o] = b'-';
        o += 1;
    }
    if exp < -4 || exp >= 14 {
        // Scientific notation: `d[.ddd]e±NN`.
        out[o] = digits[0];
        o += 1;
        if nd > 1 {
            out[o] = b'.';
            o += 1;
            out[o..o + nd - 1].copy_from_slice(&digits[1..nd]);
            o += nd - 1;
        }
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
    } else if exp >= 0 {
        // Fixed notation with the integer part first (`exp + 1` digits).
        let ip = exp as usize + 1;
        let mut j = 0;
        while j < ip {
            out[o] = if j < nd { digits[j] } else { b'0' };
            o += 1;
            j += 1;
        }
        out[o] = b'.';
        o += 1;
        if nd > ip {
            out[o..o + nd - ip].copy_from_slice(&digits[ip..nd]);
            o += nd - ip;
        } else {
            // All decimals were zero: keep one (`5` -> `5.0`).
            out[o] = b'0';
            o += 1;
        }
        o
    } else {
        // `0.` then `-exp - 1` zeros then the digits (`0.025`).
        out[o] = b'0';
        o += 1;
        out[o] = b'.';
        o += 1;
        for _ in 0..(-exp - 1) {
            out[o] = b'0';
            o += 1;
        }
        out[o..o + nd].copy_from_slice(&digits[..nd]);
        o += nd;
        o
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
        mt: None,
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
    // Lua: a nil or NaN key can never be assigned (reading is fine).
    if matches!(k, Value::Nil) {
        return Err("table index is nil");
    }
    if matches!(k, Value::Float(f) if f.is_nan()) {
        return Err("table index is NaN");
    }
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
