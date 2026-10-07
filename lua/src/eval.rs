//! Runtime helpers (values, tables, operators, formatting) and the native
//! builtin dispatch used by the bytecode VM in [`crate::vm`].

use super::{LuaError, LuaState, Node, Op, Value};

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
    /// A coroutine `yield`: the top `n` values of the value stack are the
    /// yielded results. Propagates out of the VM loop to the resume driver.
    Yield(u8),
}

/// Numeric view of a value, accepting both integers and floats.
pub(crate) fn float_of(v: Value) -> Option<f64> {
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

pub(crate) fn node_name(s: &LuaState, node: u16) -> super::StrRef {
    match s.nodes[node as usize] {
        Node::Var(name) => name,
        _ => 0,
    }
}

/// Call a function value with `argc` args on the value stack.
pub(crate) fn call(s: &mut LuaState, fv: Value, argc: u8) -> Result<ExecResult, LuaError> {
    if argc as usize > s.vsp as usize {
        return Err("internal error: arg stack underflow".into());
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
                return Err("fetch expects 1 or 2 arguments".into());
            }
            match argbuf[0] {
                Value::Str(r) => {
                    let mut nbuf = [0u8; 128];
                    let bytes = s.str_bytes(r);
                    if bytes.len() >= 128 {
                        return Err("fetch filename too long".into());
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
                                    return Err("fetch dest too long".into());
                                }
                                dbuf[..dbytes.len()].copy_from_slice(dbytes);
                                let n = core::str::from_utf8(&dbuf[..dbytes.len()])
                                    .map_err(|_| "fetch dest must be ASCII")?;
                                (dr, n)
                            }
                            _ => return Err("fetch dest must be a string".into()),
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
                        None => return Err("fetch not available (no TFTP server)".into()),
                    }
                }
                _ => return Err("fetch expects a string filename".into()),
            }
        }
        Value::Native(2) => {
            // dofile(filename): load, parse, and execute a Lua chunk, returning
            // the chunk's return value (or nil). Errors propagate to the caller.
            if argc != 1 {
                return Err("dofile expects 1 argument".into());
            }
            match argbuf[0] {
                Value::Str(r) => {
                    let bytes = s.str_bytes(r);
                    if bytes.len() >= 128 {
                        return Err("dofile filename too long".into());
                    }
                    let mut nbuf = [0u8; 128];
                    nbuf[..bytes.len()].copy_from_slice(bytes);
                    let name = core::str::from_utf8(&nbuf[..bytes.len()])
                        .map_err(|_| "dofile filename must be ASCII")?;
                    ExecResult::Ret(crate::vm::exec_chunk(s, name)?)
                }
                _ => return Err("dofile expects a string filename".into()),
            }
        }
        Value::Native(3) => {
            // next(table [, index]) -> next index, value; nil at the end.
            // Absent/nil index starts the traversal. An index that is not a
            // key of the table is an error (Lua: "invalid key to 'next'").
            if argc != 1 && argc != 2 {
                return Err("next expects 1 or 2 arguments".into());
            }
            let tid = match argbuf[0] {
                Value::Table(i) => i,
                _ => return Err("next expects a table".into()),
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
                    None => return Err("invalid key to 'next'".into()),
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
            // pairs(t) -> next, t (Lua's third result is nil, so two values
            // suffice). A `__pairs` metamethod is dispatched: it is called
            // with `t` and all of its results are returned.
            if argc != 1 {
                return Err("pairs expects 1 argument".into());
            }
            let tid = match argbuf[0] {
                Value::Table(i) => i,
                _ => return Err("pairs expects a table".into()),
            };
            let h = mt_lookup(s, tid, MM_PAIRS)?;
            if !matches!(h, Value::Nil) {
                s.push_val(argbuf[0])?;
                enter_mm(s)?;
                let r = crate::vm::call_value(s, h, 1);
                leave_mm(s);
                r?
            } else {
                ExecResult::Ret2(Value::Native(3), Value::Table(tid))
            }
        }
        Value::Native(5) => {
            // rawequal(v1, v2): primitive equality, never the __eq metamethod.
            if argc != 2 {
                return Err("rawequal expects 2 arguments".into());
            }
            ExecResult::Ret(Value::Bool(val_eq(argbuf[0], argbuf[1])))
        }
        Value::Native(6) => {
            // rawget(table, index): the real `table[index]`, never `__index`.
            if argc != 2 {
                return Err("rawget expects 2 arguments".into());
            }
            match argbuf[0] {
                Value::Table(tid) => ExecResult::Ret(raw_get(s, tid, argbuf[1])),
                _ => return Err("rawget expects a table".into()),
            }
        }
        Value::Native(7) => {
            // rawlen(v): length of a table or string, never `__len`. A table's length
            // is the run of consecutive integer keys starting at 1.
            if argc != 1 {
                return Err("rawlen expects 1 argument".into());
            }
            let len = match argbuf[0] {
                Value::Str(r) => s.str_bytes(r).len(),
                Value::Table(tid) => {
                    let mut n = 0usize;
                    loop {
                        let next = raw_get(s, tid, Value::Num((n + 1) as i64));
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
                _ => return Err("rawlen expects a table or a string".into()),
            };
            ExecResult::Ret(Value::Num(len as i64))
        }
        Value::Native(8) => {
            // rawset(table, index, value): the real assignment, never `__newindex`.
            // Returns the table.
            if argc != 3 {
                return Err("rawset expects 3 arguments".into());
            }
            let tid = match argbuf[0] {
                Value::Table(i) => i,
                _ => return Err("rawset expects a table".into()),
            };
            raw_set(s, tid, argbuf[1], argbuf[2])?;
            ExecResult::Ret(argbuf[0])
        }
        Value::Native(9) => {
            // select(index, ...): "#" returns the number of extra arguments;
            // a number returns the arguments after position `index` (-1 is the
            // last argument).
            if argc < 1 {
                return Err("select expects at least 1 argument".into());
            }
            let extras = argc as usize - 1;
            match argbuf[0] {
                Value::Str(r) if s.str_bytes(r) == b"#" => ExecResult::Ret(Value::Num(extras as i64)),
                k => {
                    let mut i = match k {
                        Value::Num(n) => n,
                        Value::Float(f) if f.is_finite() && f == float_floor(f) => f as i64,
                        _ => return Err("select index must be an integer".into()),
                    };
                    let total = argc as i64;
                    if i < 0 {
                        i += total;
                    } else if i > total {
                        i = total;
                    }
                    if i < 1 {
                        return Err("select index out of range".into());
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
            // field is not nil is protected and cannot be changed.
            if argc != 2 {
                return Err("setmetatable expects 2 arguments".into());
            }
            let tid = match argbuf[0] {
                Value::Table(i) => i,
                _ => return Err("setmetatable expects a table".into()),
            };
            let new_mt = match argbuf[1] {
                Value::Nil => None,
                Value::Table(i) => Some(i),
                _ => return Err("setmetatable expects a table or nil".into()),
            };
            if let Some(cur) = s.tbls[tid as usize].mt {
                let protected_name = s.intern(b"__metatable")?;
                let protected = raw_get(s, cur, Value::Str(protected_name));
                if !matches!(protected, Value::Nil) {
                    return Err("cannot change a protected metatable".into());
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
                return Err("tonumber expects 1 or 2 arguments".into());
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
                    _ => return Err("tonumber base must be an integer".into()),
                };
                if !(2..=36).contains(&base) {
                    return Err("tonumber base out of range".into());
                }
                let r = match argbuf[0] {
                    Value::Str(r) => r,
                    _ => return Err("tonumber expects a string with a base".into()),
                };
                ExecResult::Ret(match str_to_base(s.str_bytes(r), base) {
                    Some(n) => Value::Num(n),
                    None => Value::Nil,
                })
            }
        }
        Value::Native(12) => {
            // tostring(v): the human-readable string form (the same rendering
            // `print` uses). A table's `__tostring` metamethod is dispatched.
            if argc != 1 {
                return Err("tostring expects 1 argument".into());
            }
            ExecResult::Ret(Value::Str(value_to_string(s, argbuf[0])?))
        }
        Value::Native(13) => {
            // type(v): the Lua type name of a value. Every callable value
            // (including native builtins) reports "function".
            if argc != 1 {
                return Err("type expects 1 argument".into());
            }
            ExecResult::Ret(Value::Str(s.intern(type_name(argbuf[0]))?))
        }
        Value::Native(14) => {
            // warn(msg, ...): concatenate the string (or number) arguments and
            // emit "Lua warning: <msg>". The control messages "@on"/"@off"
            // toggle warnings instead of emitting. Only the single-argument
            // form can be a control message (Lua concatenates first, but the
            // multi-argument spelling of a control message is pathological).
            if argc == 0 {
                return Err("warn expects at least 1 argument".into());
            }
            let mut control = false;
            if argc == 1 {
                if let Value::Str(r) = argbuf[0] {
                    if s.str_bytes(r) == b"@on" {
                        s.warn_on = true;
                        control = true;
                    } else if s.str_bytes(r) == b"@off" {
                        s.warn_on = false;
                        control = true;
                    }
                }
            }
            // Arguments are validated even while warnings are off, like Lua.
            for i in 0..argc as usize {
                match argbuf[i] {
                    Value::Str(_) | Value::Num(_) | Value::Float(_) => {}
                    _ => return Err("warn expects string arguments".into()),
                }
            }
            if !control && s.warn_on {
                emit_str(s, b"Lua warning: ");
                for i in 0..argc as usize {
                    tostring(s, argbuf[i])?;
                }
                emit(s, b'\n');
            }
            ExecResult::Normal
        }
        Value::Native(15) => {
            // pcall(f, ...): protected call. Returns true plus the results on
            // success, or false plus the error object (or the interned error
            // message) on failure. `Shell`/`Exit` are control flow, not
            // errors, so they propagate to the REPL.
            if argc < 1 {
                return Err("pcall expects at least 1 argument".into());
            }
            let saved_vsp = base;
            let saved_fsp = s.fsp;
            let saved_marks = s.mark_sp;
            match call(s, argbuf[0], argc - 1) {
                Ok(ExecResult::Normal) => ExecResult::Ret(Value::Bool(true)),
                Ok(ExecResult::Ret(v)) => ExecResult::Ret2(Value::Bool(true), v),
                Ok(ExecResult::Ret2(a, b)) => {
                    // Three results: push true,a,b then rotate it to the front.
                    s.push_val(a)?;
                    s.push_val(b)?;
                    s.push_val(Value::Bool(true))?;
                    let top = s.vsp as usize;
                    let start = top - 3;
                    let t = s.vstack[top - 1];
                    for j in (start + 1..top).rev() {
                        s.vstack[j] = s.vstack[j - 1];
                    }
                    s.vstack[start] = t;
                    ExecResult::RetN(3)
                }
                Ok(ExecResult::RetN(n)) => {
                    // Results are on the stack; insert `true` before them.
                    s.push_val(Value::Bool(true))?;
                    let top = s.vsp as usize;
                    let start = top - (n as usize + 1);
                    let t = s.vstack[top - 1];
                    for j in (start + 1..top).rev() {
                        s.vstack[j] = s.vstack[j - 1];
                    }
                    s.vstack[start] = t;
                    ExecResult::RetN(n + 1)
                }
                Ok(ExecResult::Shell) => ExecResult::Shell,
                Ok(ExecResult::Exit) => ExecResult::Exit,
                // Yielding across `pcall` is not supported (Lua 5.1 also
                // rejects it): the protected call unwinds, so the resume
                // continuation would be lost.
                Ok(ExecResult::Yield(_)) => {
                    return Err("attempt to yield across a protected call".into())
                }
                Ok(ExecResult::Break) => return Err("break outside loop".into()),
                Ok(ExecResult::Goto(_)) => return Err("goto outside function".into()),
                Err(e) => {
                    // Unwind to the marks saved before the call: the failed
                    // call may have left frames/values behind.
                    s.vsp = saved_vsp as u32;
                    s.fsp = saved_fsp;
                    s.mark_sp = saved_marks;
                    let msg = match e {
                        LuaError::Obj(v) => v,
                        LuaError::Msg(m) => Value::Str(s.intern(m.as_bytes())?),
                    };
                    ExecResult::Ret2(Value::Bool(false), msg)
                }
            }
        }
        Value::Native(16) => {
            // error(v [, level]): raise `v` as an error object. `level` is
            // validated but ignored — this interpreter has no source
            // positions to attach. With no arguments the error object is nil
            // (matching Lua).
            if argc > 2 {
                return Err("error expects at most 2 arguments".into());
            }
            if argc == 2 {
                match argbuf[1] {
                    Value::Num(_) => {}
                    Value::Float(f) if f.is_finite() && f == float_floor(f) => {}
                    _ => return Err("error level must be an integer".into()),
                }
            }
            let obj = if argc >= 1 { argbuf[0] } else { Value::Nil };
            return Err(LuaError::Obj(obj));
        }
        Value::Native(17) => {
            // coroutine.create(f) -> thread.
            if argc != 1 {
                return Err("coroutine.create expects 1 argument".into());
            }
            ExecResult::Ret(crate::vm::co_create(s, argbuf[0])?)
        }
        Value::Native(18) => {
            // coroutine.resume(co, ...) -> true + results / false + error.
            if argc < 1 {
                return Err("coroutine.resume expects at least 1 argument".into());
            }
            let id = match argbuf[0] {
                Value::Co(i) => i,
                _ => return Err("coroutine.resume expects a coroutine".into()),
            };
            crate::vm::co_resume_value(s, id, argc - 1)?
        }
        Value::Native(19) => {
            // coroutine.yield(...): suspend the running coroutine.
            if s.current == 0 {
                return Err("attempt to yield from outside a coroutine".into());
            }
            for i in 0..argc as usize {
                s.push_val(argbuf[i])?;
            }
            ExecResult::Yield(argc)
        }
        Value::Native(20) => {
            // coroutine.status(co) -> "suspended" | "running" | "normal" | "dead".
            if argc != 1 {
                return Err("coroutine.status expects 1 argument".into());
            }
            let id = match argbuf[0] {
                Value::Co(i) => i,
                _ => return Err("coroutine.status expects a coroutine".into()),
            };
            let name: &[u8] = match s.cos[id as usize].status {
                crate::CO_SUSPENDED => b"suspended",
                crate::CO_RUNNING => b"running",
                crate::CO_NORMAL => b"normal",
                _ => b"dead",
            };
            ExecResult::Ret(Value::Str(s.intern(name)?))
        }
        Value::Native(21) => {
            // coroutine.wrap(co) -> a function that resumes it and re-raises
            // errors.
            if argc != 1 {
                return Err("coroutine.wrap expects 1 argument".into());
            }
            match argbuf[0] {
                Value::Co(i) => ExecResult::Ret(Value::Wrapped(i)),
                _ => return Err("coroutine.wrap expects a coroutine".into()),
            }
        }
        Value::Native(22) => {
            // coroutine.isyieldable() -> whether the running thread is a
            // coroutine.
            if argc != 0 {
                return Err("coroutine.isyieldable expects no arguments".into());
            }
            ExecResult::Ret(Value::Bool(s.current != 0))
        }
        Value::Native(23) => {
            // coroutine.running() -> thread [, is_main]; main has no thread
            // value in this subset, so it reports nil, true.
            if argc != 0 {
                return Err("coroutine.running expects no arguments".into());
            }
            if s.current == 0 {
                ExecResult::Ret2(Value::Nil, Value::Bool(true))
            } else {
                ExecResult::Ret2(Value::Co(s.current - 1), Value::Bool(false))
            }
        }
        Value::Native(24) => {
            // coroutine.close(co): mark a suspended/dead coroutine dead.
            if argc != 1 {
                return Err("coroutine.close expects 1 argument".into());
            }
            let id = match argbuf[0] {
                Value::Co(i) => i,
                _ => return Err("coroutine.close expects a coroutine".into()),
            };
            match s.cos[id as usize].status {
                crate::CO_DEAD => ExecResult::Ret(Value::Bool(true)),
                crate::CO_SUSPENDED => {
                    s.cos[id as usize].status = crate::CO_DEAD;
                    ExecResult::Ret(Value::Bool(true))
                }
                _ => return Err("cannot close a running coroutine".into()),
            }
        }
        Value::Shell => {
            if argc != 0 {
                return Err("shell expects no arguments".into());
            }
            ExecResult::Shell
        }
        Value::Exit => {
            if argc != 0 {
                return Err("exit expects no arguments".into());
            }
            ExecResult::Exit
        }
        Value::Dhcp => {
            if argc != 0 {
                return Err("dhcp expects no arguments".into());
            }
            let ok = s.run_dhcp()?;
            if ok {
                s.emit_dhcp_info();
            }
            ExecResult::Ret(Value::Bool(ok))
        }
        Value::Ls => {
            if argc != 0 {
                return Err("ls expects no arguments".into());
            }
            ls_run(s)?;
            ExecResult::Normal
        }
        Value::Func(_) | Value::Closure(_) => {
            // User functions (and closures) are executed by the VM (a nested
            // run, used by `pcall`; the VM's own Call instruction runs them
            // inline). Fall through so the result's stack layout is
            // normalized below.
            crate::vm::call_value(s, fv, argc)?
        }
        Value::Wrapped(id) => {
            // Calling a wrapped coroutine resumes it and re-raises errors.
            crate::vm::co_wrap_call(s, id, argc)?
        }
        Value::Co(_) => return Err("attempt to call a thread value".into()),
        _ => {
            // Callable tables: `__call` receives the table as its first
            // argument.
            let h = mm_of(s, fv, MM_CALL)?;
            if matches!(h, Value::Nil) {
                return Err("attempt to call a non-function value".into());
            }
            if argc >= 32 {
                return Err("too many arguments".into());
            }
            // [args...] -> [table, args...].
            for i in (0..argc as usize).rev() {
                s.vstack[base + 1 + i] = s.vstack[base + i];
            }
            s.vstack[base] = fv;
            s.vsp += 1;
            enter_mm(s)?;
            let r = call(s, h, argc + 1);
            leave_mm(s);
            return r;
        }
    };

    match result {
        // `Yield(n)` carries its n values on the stack, like `RetN`.
        ExecResult::RetN(n) | ExecResult::Yield(n) => {
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

/// `ls`: print every file downloaded with `fetch()` this run/session, one per
/// line as `name (N bytes)`. Prints nothing when no files have been fetched.
pub(crate) fn ls_run(s: &mut LuaState) -> Result<(), LuaError> {
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

pub(crate) fn truthy(v: Value) -> bool {
    !matches!(v, Value::Nil | Value::Bool(false))
}

// ── Metamethods ─────────────────────────────────────────────────────────────

/// Metamethod name indices, parallel to `MM_NAMES`.
pub(crate) const MM_INDEX: usize = 0;
pub(crate) const MM_NEWINDEX: usize = 1;
pub(crate) const MM_EQ: usize = 2;
pub(crate) const MM_LT: usize = 3;
pub(crate) const MM_LE: usize = 4;
pub(crate) const MM_CONCAT: usize = 5;
pub(crate) const MM_ADD: usize = 6;
pub(crate) const MM_SUB: usize = 7;
pub(crate) const MM_MUL: usize = 8;
pub(crate) const MM_DIV: usize = 9;
pub(crate) const MM_MOD: usize = 10;
pub(crate) const MM_UNM: usize = 11;
pub(crate) const MM_CALL: usize = 12;
pub(crate) const MM_TOSTRING: usize = 13;
pub(crate) const MM_PAIRS: usize = 14;

const MM_NAMES: [&[u8]; super::MM_COUNT] = [
    b"__index",
    b"__newindex",
    b"__eq",
    b"__lt",
    b"__le",
    b"__concat",
    b"__add",
    b"__sub",
    b"__mul",
    b"__div",
    b"__mod",
    b"__unm",
    b"__call",
    b"__tostring",
    b"__pairs",
];

/// Maximum metamethod dispatch nesting before a loop error is raised.
const MAX_MM_DEPTH: u16 = 100;

/// Intern the metamethod names (once per `reset`) and return the `i`-th.
fn mm_name(s: &mut LuaState, i: usize) -> Result<super::StrRef, LuaError> {
    if !s.mm_ready {
        for (j, n) in MM_NAMES.iter().enumerate() {
            let r = s.intern(n)?;
            s.mm_refs[j] = r;
        }
        s.mm_ready = true;
    }
    Ok(s.mm_refs[i])
}

/// Raw slot lookup: no metamethods.
pub(crate) fn raw_get(s: &LuaState, tid: u16, k: Value) -> Value {
    let rec = &s.tbls[tid as usize];
    for i in 0..rec.len as usize {
        if val_eq(rec.slots[i].key, k) {
            return rec.slots[i].value;
        }
    }
    Value::Nil
}

/// Whether the key is present in the table (raw).
fn raw_has(s: &LuaState, tid: u16, k: Value) -> bool {
    let rec = &s.tbls[tid as usize];
    for i in 0..rec.len as usize {
        if val_eq(rec.slots[i].key, k) {
            return true;
        }
    }
    false
}

/// Raw slot write (update, remove-on-nil, or append): no metamethods.
pub(crate) fn raw_set(s: &mut LuaState, tid: u16, k: Value, v: Value) -> Result<(), LuaError> {
    // Lua: a nil or NaN key can never be assigned (reading is fine).
    if matches!(k, Value::Nil) {
        return Err("table index is nil".into());
    }
    if matches!(k, Value::Float(f) if f.is_nan()) {
        return Err("table index is NaN".into());
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
        return Err("table full".into());
    }
    s.tbls[tid as usize].slots[len] = super::TableSlot { key: k, value: v };
    s.tbls[tid as usize].len += 1;
    Ok(())
}

/// The value of the `i`-th metamethod in `tid`'s metatable, or `nil`.
pub(crate) fn mt_lookup(s: &mut LuaState, tid: u16, i: usize) -> Result<Value, LuaError> {
    let mt = match s.tbls[tid as usize].mt {
        Some(m) => m,
        None => return Ok(Value::Nil),
    };
    let name = mm_name(s, i)?;
    Ok(raw_get(s, mt, Value::Str(name)))
}

/// The `i`-th metamethod of `v` (only tables can have metatables here).
pub(crate) fn mm_of(s: &mut LuaState, v: Value, i: usize) -> Result<Value, LuaError> {
    match v {
        Value::Table(tid) => mt_lookup(s, tid, i),
        _ => Ok(Value::Nil),
    }
}

/// Call a metamethod with `args`, returning its first result.
pub(crate) fn call_mm(s: &mut LuaState, f: Value, args: &[Value]) -> Result<Value, LuaError> {
    if args.len() > 32 {
        return Err("too many arguments".into());
    }
    let base = s.vsp as usize;
    for a in args {
        s.push_val(*a)?;
    }
    let r = crate::vm::call_value(s, f, args.len() as u8)?;
    match r {
        ExecResult::Normal => {
            s.vsp = base as u32;
            Ok(Value::Nil)
        }
        ExecResult::Ret(v) => {
            s.vsp = base as u32;
            Ok(v)
        }
        ExecResult::Ret2(a, _) => {
            s.vsp = base as u32;
            Ok(a)
        }
        ExecResult::RetN(n) => {
            let v = if n == 0 {
                Value::Nil
            } else {
                s.vstack[s.vsp as usize - n as usize]
            };
            s.vsp = base as u32;
            Ok(v)
        }
        ExecResult::Shell | ExecResult::Exit => {
            s.vsp = base as u32;
            Ok(Value::Nil)
        }
        ExecResult::Yield(_) => Err("attempt to yield across a metamethod".into()),
        ExecResult::Break | ExecResult::Goto(_) => {
            Err("internal error: control flow from metamethod".into())
        }
    }
}

/// Enter metamethod dispatch, failing on a (likely) loop.
fn enter_mm(s: &mut LuaState) -> Result<(), LuaError> {
    if s.mm_depth >= MAX_MM_DEPTH {
        return Err("metamethod chain too long (possible loop)".into());
    }
    s.mm_depth += 1;
    Ok(())
}

fn leave_mm(s: &mut LuaState) {
    s.mm_depth -= 1;
}

/// Equality with `__eq` dispatch (used by `==`/`~=`).
pub(crate) fn eq_values(s: &mut LuaState, a: Value, b: Value) -> Result<bool, LuaError> {
    if val_eq(a, b) {
        return Ok(true);
    }
    if matches!((a, b), (Value::Table(_), Value::Table(_))) {
        let h = binop_mm(s, a, b, MM_EQ)?;
        if let Some(f) = h {
            enter_mm(s)?;
            let r = call_mm(s, f, &[a, b]);
            leave_mm(s);
            return Ok(truthy(r?));
        }
    }
    Ok(false)
}

/// The first operand's metamethod, else the second's.
fn binop_mm(s: &mut LuaState, a: Value, b: Value, i: usize) -> Result<Option<Value>, LuaError> {
    let h = mm_of(s, a, i)?;
    if !matches!(h, Value::Nil) {
        return Ok(Some(h));
    }
    let h = mm_of(s, b, i)?;
    if !matches!(h, Value::Nil) {
        return Ok(Some(h));
    }
    Ok(None)
}

/// Unary minus with `__unm` dispatch.
pub(crate) fn unm(s: &mut LuaState, v: Value) -> Result<Value, LuaError> {
    match v {
        Value::Num(n) => Ok(Value::Num(n.wrapping_neg())),
        Value::Float(f) => Ok(Value::Float(-f)),
        _ => match mm_of(s, v, MM_UNM)? {
            Value::Nil => Err("attempt to perform arithmetic on a non-number value".into()),
            f => {
                enter_mm(s)?;
                let r = call_mm(s, f, &[v]);
                leave_mm(s);
                r
            }
        },
    }
}

pub(crate) fn val_eq(a: Value, b: Value) -> bool {
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
        (Value::Co(x), Value::Co(y)) => x == y,
        (Value::Wrapped(x), Value::Wrapped(y)) => x == y,
        (Value::Closure(x), Value::Closure(y)) => x == y,
        // Unit builtins compare equal to themselves.
        (Value::Shell, Value::Shell)
        | (Value::Dhcp, Value::Dhcp)
        | (Value::Exit, Value::Exit)
        | (Value::Ls, Value::Ls) => true,
        _ => false,
    }
}

pub(crate) fn binop(s: &mut LuaState, op: Op, a: Value, b: Value) -> Result<Value, LuaError> {
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
                            return Err("division by zero".into());
                        }
                        x / y
                    }
                    Mod => {
                        if y == 0 {
                            return Err("division by zero".into());
                        }
                        x.rem_euclid(y)
                    }
                    _ => 0,
                };
                return Ok(Value::Num(r));
            }
            if let (Some(x), Some(y)) = (float_of(a), float_of(b)) {
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
                return Ok(Value::Float(r));
            }
            // Non-numbers: dispatch `__add`/`__sub`/... from either operand.
            let idx = match op {
                Add => MM_ADD,
                Sub => MM_SUB,
                Mul => MM_MUL,
                Div => MM_DIV,
                _ => MM_MOD,
            };
            match binop_mm(s, a, b, idx)? {
                Some(f) => {
                    enter_mm(s)?;
                    let r = call_mm(s, f, &[a, b]);
                    leave_mm(s);
                    r
                }
                None => Err("attempt to perform arithmetic on a non-number value".into()),
            }
        }
        Eq => Ok(Value::Bool(eq_values(s, a, b)?)),
        Ne => Ok(Value::Bool(!eq_values(s, a, b)?)),
        Lt | Le | Gt | Ge => {
            // `a > b` is `b < a`; `a >= b` is `b <= a` (Lua).
            let (op2, x, y) = match op {
                Gt => (Lt, b, a),
                Ge => (Le, b, a),
                _ => (op, a, b),
            };
            let r = if let (Value::Num(x1), Value::Num(y1)) = (x, y) {
                if op2 == Lt {
                    x1 < y1
                } else {
                    x1 <= y1
                }
            } else if let (Some(x1), Some(y1)) = (float_of(x), float_of(y)) {
                if op2 == Lt {
                    x1 < y1
                } else {
                    x1 <= y1
                }
            } else {
                // Non-numbers: dispatch `__lt`/`__le`.
                let idx = if op2 == Lt { MM_LT } else { MM_LE };
                let h = binop_mm(s, x, y, idx)?;
                if h.is_none() && op2 == Le {
                    // Lua 5.1 fallback: `a <= b` is `not (b < a)`.
                    if let Some(f) = binop_mm(s, y, x, MM_LT)? {
                        enter_mm(s)?;
                        let r = call_mm(s, f, &[y, x]);
                        leave_mm(s);
                        return Ok(Value::Bool(!truthy(r?)));
                    }
                }
                match h {
                    Some(f) => {
                        enter_mm(s)?;
                        let r = call_mm(s, f, &[x, y]);
                        leave_mm(s);
                        truthy(r?)
                    }
                    None => return Err("attempt to compare non-number values".into()),
                }
            };
            Ok(Value::Bool(r))
        }
        Concat => {
            if matches!(a, Value::Str(_) | Value::Num(_) | Value::Float(_))
                && matches!(b, Value::Str(_) | Value::Num(_) | Value::Float(_))
            {
                let sa = string_of(s, a)?;
                let sb = string_of(s, b)?;
                let ba = s.str_bytes(sa);
                let bb = s.str_bytes(sb);
                let mut tmp = [0u8; 512];
                if ba.len() + bb.len() > tmp.len() {
                    return Err("string too long".into());
                }
                tmp[..ba.len()].copy_from_slice(ba);
                tmp[ba.len()..ba.len() + bb.len()].copy_from_slice(bb);
                return Ok(Value::Str(s.intern(&tmp[..ba.len() + bb.len()])?));
            }
            match binop_mm(s, a, b, MM_CONCAT)? {
                Some(f) => {
                    enter_mm(s)?;
                    let r = call_mm(s, f, &[a, b]);
                    leave_mm(s);
                    r
                }
                None => Err("attempt to concatenate a non-string value".into()),
            }
        }
        And | Or | Not | Neg => Err("internal error: operator handled elsewhere".into()),
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
        _ => Err("attempt to concatenate a non-string value".into()),
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
        Value::Func(_)
        | Value::Closure(_)
        | Value::Native(_)
        | Value::Shell
        | Value::Dhcp
        | Value::Exit
        | Value::Ls
        | Value::Wrapped(_) => b"function",
        Value::Co(_) => b"thread",
    }
}

/// Render a runtime error into `out` without allocating: static messages and
/// string error objects are copied verbatim; any other error object becomes
/// `error object is a <type> value` (matching standalone Lua). Returns the
/// number of bytes written (truncated to `out.len()`).
pub fn error_bytes(s: &LuaState, e: LuaError, out: &mut [u8]) -> usize {
    fn copy(out: &mut [u8], bytes: &[u8]) -> usize {
        let n = bytes.len().min(out.len());
        out[..n].copy_from_slice(&bytes[..n]);
        n
    }
    match e {
        LuaError::Msg(m) => copy(out, m.as_bytes()),
        LuaError::Obj(Value::Str(r)) => copy(out, s.str_bytes(r)),
        LuaError::Obj(v) => {
            let mut n = copy(out, b"error object is a ");
            n += copy(&mut out[n..], type_name(v));
            n += copy(&mut out[n..], b" value");
            n
        }
    }
}

/// Emit a runtime error through `putc` (no truncation; used by the REPL).
pub fn emit_error(s: &LuaState, e: LuaError, putc: fn(u8)) {
    match e {
        LuaError::Msg(m) => {
            for &b in m.as_bytes() {
                putc(b);
            }
        }
        LuaError::Obj(Value::Str(r)) => {
            for &b in s.str_bytes(r) {
                putc(b);
            }
        }
        LuaError::Obj(v) => {
            for &b in b"error object is a " {
                putc(b);
            }
            for &b in type_name(v) {
                putc(b);
            }
            for &b in b" value" {
                putc(b);
            }
        }
    }
}

/// Convert a value to an interned string — the `tostring` builtin. Strings are
/// returned as-is (they are already interned); a table's `__tostring`
/// metamethod is dispatched; other values use the same rendering as the
/// `tostring` emitter below.
fn value_to_string(s: &mut LuaState, v: Value) -> Result<super::StrRef, LuaError> {
    if let Value::Table(tid) = v {
        let h = mt_lookup(s, tid, MM_TOSTRING)?;
        if !matches!(h, Value::Nil) {
            enter_mm(s)?;
            let r = call_mm(s, h, &[v]);
            leave_mm(s);
            return match r? {
                Value::Str(sr) => Ok(sr),
                Value::Num(n) => {
                    let (buf, len) = itoa(n);
                    Ok(s.intern(&buf[..len])?)
                }
                Value::Float(f) => {
                    let mut buf = [0u8; FLOAT_BUF];
                    let len = fmt_float(f, &mut buf);
                    Ok(s.intern(&buf[..len])?)
                }
                _ => Err("'__tostring' must return a string".into()),
            };
        }
    }
    match v {
        Value::Str(r) => Ok(r),
        Value::Num(n) => {
            let (buf, len) = itoa(n);
            Ok(s.intern(&buf[..len])?)
        }
        Value::Float(f) => {
            let mut buf = [0u8; FLOAT_BUF];
            let len = fmt_float(f, &mut buf);
            Ok(s.intern(&buf[..len])?)
        }
        Value::Nil => Ok(s.intern(b"nil")?),
        Value::Bool(true) => Ok(s.intern(b"true")?),
        Value::Bool(false) => Ok(s.intern(b"false")?),
        Value::Table(_) => Ok(s.intern(b"table")?),
        Value::Func(_) | Value::Closure(_) => Ok(s.intern(b"function")?),
        Value::Native(_) => Ok(s.intern(b"native")?),
        Value::Shell => Ok(s.intern(b"shell")?),
        Value::Dhcp => Ok(s.intern(b"dhcp")?),
        Value::Exit => Ok(s.intern(b"exit")?),
        Value::Ls => Ok(s.intern(b"ls")?),
        Value::Co(_) => Ok(s.intern(b"thread")?),
        Value::Wrapped(_) => Ok(s.intern(b"function")?),
    }
}

/// Emit the Lua `tostring` rendering of a value via `putc`. A table's
/// `__tostring` metamethod is dispatched (it must return a string).
pub(crate) fn tostring(s: &mut LuaState, v: Value) -> Result<(), LuaError> {
    if let Value::Table(tid) = v {
        let h = mt_lookup(s, tid, MM_TOSTRING)?;
        if !matches!(h, Value::Nil) {
            enter_mm(s)?;
            let r = call_mm(s, h, &[v]);
            leave_mm(s);
            let rv = r?;
            match rv {
                Value::Str(sr) => {
                    emit_bytes(s, s.str_bytes(sr));
                    return Ok(());
                }
                Value::Num(_) | Value::Float(_) => {
                    let sr = value_to_string(s, rv)?;
                    emit_bytes(s, s.str_bytes(sr));
                    return Ok(());
                }
                _ => return Err("'__tostring' must return a string".into()),
            }
        }
    }
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
        Value::Func(_) | Value::Closure(_) => emit_str(s, b"function"),
        Value::Native(_) => emit_str(s, b"native"),
        Value::Shell => emit_str(s, b"shell"),
        Value::Dhcp => emit_str(s, b"dhcp"),
        Value::Exit => emit_str(s, b"exit"),
        Value::Ls => emit_str(s, b"ls"),
        Value::Co(_) => emit_str(s, b"thread"),
        Value::Wrapped(_) => emit_str(s, b"function"),
    }
    Ok(())
}

pub(crate) fn emit(s: &LuaState, b: u8) {
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

pub(crate) fn new_table(s: &mut LuaState) -> Result<u16, LuaError> {
    if s.ntables as usize >= super::MAX_TABLES {
        return Err("too many tables".into());
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

/// `t[k]` with `__index` dispatch: a missing key consults the metatable's
/// `__index` (a table is looked up recursively, a function is called with
/// `(t, k)`).
pub(crate) fn tget(s: &mut LuaState, t: Value, k: Value) -> Result<Value, LuaError> {
    let tid = match t {
        Value::Table(i) => i,
        _ => return Err("attempt to index a non-table value".into()),
    };
    let v = raw_get(s, tid, k);
    if !matches!(v, Value::Nil) {
        return Ok(v);
    }
    let h = mt_lookup(s, tid, MM_INDEX)?;
    match h {
        Value::Nil => Ok(Value::Nil),
        Value::Table(_) => {
            enter_mm(s)?;
            let r = tget(s, h, k);
            leave_mm(s);
            r
        }
        _ => {
            enter_mm(s)?;
            let r = call_mm(s, h, &[t, k]);
            leave_mm(s);
            r
        }
    }
}

/// `t[k] = v` with `__newindex` dispatch: assigning an absent key consults
/// the metatable's `__newindex` (a table is assigned recursively, a function
/// is called with `(t, k, v)`).
pub(crate) fn tset(s: &mut LuaState, t: Value, k: Value, v: Value) -> Result<(), LuaError> {
    let tid = match t {
        Value::Table(i) => i,
        _ => return Err("attempt to index a non-table value".into()),
    };
    if raw_has(s, tid, k) {
        return raw_set(s, tid, k, v);
    }
    let h = mt_lookup(s, tid, MM_NEWINDEX)?;
    match h {
        Value::Nil => raw_set(s, tid, k, v),
        Value::Table(_) => {
            enter_mm(s)?;
            let r = tset(s, h, k, v);
            leave_mm(s);
            r
        }
        _ => {
            enter_mm(s)?;
            let r = call_mm(s, h, &[t, k, v]).map(|_| ());
            leave_mm(s);
            r
        }
    }
}
