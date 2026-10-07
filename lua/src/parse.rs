//! Recursive-descent parser: builds the static AST in [`LuaState`].

use super::lex::{Lexer, Tok};
use super::{LuaState, Node, Op, StrRef, NO_NODE};

const MAX_SCOPES: usize = 16;
const MAX_LABELS_PER_SCOPE: usize = 8;
const MAX_GOTOS_PER_SCOPE: usize = 8;
/// Maximum locals tracked for closure-capture analysis.
const MAX_RESOLVED_LOCALS: usize = 64;
/// Maximum nested function scopes.
const MAX_FUNC_SCOPES: usize = 8;

/// A declared local, for lexical name resolution and closure-capture
/// analysis. Locals captured by a closure are boxed lazily into a cell when
/// the closure is created.
#[derive(Clone, Copy)]
struct LocalRec {
    name: StrRef,
    /// Function nesting level that declared it (0 = the script).
    func: u8,
    /// Statement node patched when the local is captured (for-loop control
    /// variables), or `NO_NODE`.
    decl_node: u16,
    captured: bool,
}

/// One function's upvalue names, accumulated while parsing its body.
#[derive(Clone, Copy)]
struct FuncScope {
    ups: [StrRef; super::MAX_UPVALS],
    nups: u8,
}

impl FuncScope {
    fn empty() -> Self {
        FuncScope {
            ups: [0; super::MAX_UPVALS],
            nups: 0,
        }
    }
}

/// One block's label/goto bookkeeping, used to resolve `goto`/`::label::` at
/// the end of the block. Labels and gotos in nested blocks are scoped: a goto
/// can target a label in its own block or an enclosing block, but a label in a
/// nested block is never visible after that block ends.
#[derive(Clone, Copy)]
struct Scope {
    labels: [(StrRef, u16); MAX_LABELS_PER_SCOPE],
    nlabels: u8,
    gotos: [(u16, StrRef); MAX_GOTOS_PER_SCOPE],
    ngotos: u8,
}

impl Scope {
    fn empty() -> Self {
        Scope {
            labels: [(0, 0); MAX_LABELS_PER_SCOPE],
            nlabels: 0,
            gotos: [(0, 0); MAX_GOTOS_PER_SCOPE],
            ngotos: 0,
        }
    }
}

/// Recursive-descent parser with a single-token lookahead.
///
/// `'l` is the lifetime of the source buffer; `'s` the borrow of the state.
pub struct Parser<'s, 'l> {
    lex: Lexer<'l>,
    cur: Tok,
    /// Nesting depth of `while`/`for`/`repeat` loops. `break` is only valid
    /// when > 0.
    loop_depth: u32,
    /// Label/goto scopes, one per in-progress block.
    scopes: [Scope; MAX_SCOPES],
    n_scopes: usize,
    /// Declared locals, for name resolution and capture analysis.
    locals: [LocalRec; MAX_RESOLVED_LOCALS],
    nlocals: u8,
    /// Function scopes (index 0 is the script), each collecting upvalue names.
    funcs: [FuncScope; MAX_FUNC_SCOPES],
    nfuncs: u8,
    state: &'s mut LuaState,
}

impl<'s, 'l> Parser<'s, 'l> {
    pub fn new(src: &'l [u8], state: &'s mut LuaState) -> Self {
        let mut lex = Lexer::new(src);
        let cur = lex.next_token().unwrap_or(Tok::Eof);
        Parser {
            lex,
            cur,
            loop_depth: 0,
            scopes: [Scope::empty(); MAX_SCOPES],
            n_scopes: 0,
            locals: [LocalRec {
                name: 0,
                func: 0,
                decl_node: NO_NODE,
                captured: false,
            }; MAX_RESOLVED_LOCALS],
            nlocals: 0,
            funcs: [FuncScope::empty(); MAX_FUNC_SCOPES],
            nfuncs: 1,
            state,
        }
    }

    // ── Lexical scope / closure capture analysis ────────────────────────────

    fn func_level(&self) -> u8 {
        self.nfuncs - 1
    }

    fn declare_local_rec(&mut self, name: StrRef, decl_node: u16) -> Result<usize, &'static str> {
        if self.nlocals as usize >= MAX_RESOLVED_LOCALS {
            return Err("too many locals");
        }
        let i = self.nlocals as usize;
        self.locals[i] = LocalRec {
            name,
            func: self.func_level(),
            decl_node,
            captured: false,
        };
        self.nlocals += 1;
        Ok(i)
    }

    /// Record `name` as an upvalue of function `level` (deduplicated).
    fn add_up(&mut self, level: usize, name: StrRef) -> Result<(), &'static str> {
        let fs = &mut self.funcs[level];
        for i in 0..fs.nups as usize {
            if fs.ups[i] == name {
                return Ok(());
            }
        }
        if fs.nups as usize >= super::MAX_UPVALS {
            return Err("too many upvalues");
        }
        fs.ups[fs.nups as usize] = name;
        fs.nups += 1;
        Ok(())
    }

    /// Mark local `i` as captured. `local` declarations are boxed lazily when
    /// the closure is created; captured `for` control variables switch their
    /// statement node to the per-iteration-cell variant.
    fn mark_captured(&mut self, i: usize) -> Result<(), &'static str> {
        if self.locals[i].captured {
            return Ok(());
        }
        self.locals[i].captured = true;
        let node = self.locals[i].decl_node;
        if node == NO_NODE {
            return Ok(());
        }
        match self.state.nodes[node as usize] {
            Node::ForStmt(a, b, c, d, e) => {
                self.state.nodes[node as usize] = Node::ForStmtCell(a, b, c, d, e);
            }
            Node::ForInStmt(a, b, c, d) => {
                self.state.nodes[node as usize] = Node::ForInStmtCell(a, b, c, d);
            }
            _ => {}
        }
        Ok(())
    }

    /// Resolve a name reference: a local of the current function, an outer
    /// local (captured as an upvalue), or a global (no record).
    fn resolve_name(&mut self, name: StrRef) -> Result<(), &'static str> {
        let cur = self.func_level();
        let mut i = self.nlocals as usize;
        while i > 0 {
            i -= 1;
            if self.locals[i].name == name {
                let lf = self.locals[i].func;
                if lf == cur {
                    return Ok(());
                }
                self.mark_captured(i)?;
                for lvl in (lf as usize + 1)..self.nfuncs as usize {
                    self.add_up(lvl, name)?;
                }
                return Ok(());
            }
        }
        Ok(())
    }

    /// Parse `(params) body end` (the `(` is current) and define the function.
    fn parse_func_body(&mut self) -> Result<u16, &'static str> {
        self.expect(Tok::LParen, "expected '(' after 'function'")?;
        if self.nfuncs as usize >= MAX_FUNC_SCOPES {
            return Err("too many nested functions");
        }
        let func_mark = self.nlocals;
        self.funcs[self.nfuncs as usize] = FuncScope::empty();
        self.nfuncs += 1;
        let (params, nparams) = self.parse_params()?;
        for p in 0..nparams as usize {
            let name = match self.state.nodes[params as usize + p] {
                Node::Var(n) => n,
                _ => 0,
            };
            self.declare_local_rec(name, NO_NODE)?;
        }
        let saved_loop = self.loop_depth;
        self.loop_depth = 0;
        let saved_scopes = self.n_scopes;
        self.n_scopes = 0;
        let body = self.parse_block()?;
        self.n_scopes = saved_scopes;
        self.loop_depth = saved_loop;
        self.expect(Tok::End, "expected 'end' to close function")?;
        let fs = self.funcs[self.nfuncs as usize - 1];
        self.nfuncs -= 1;
        self.nlocals = func_mark;
        let ups = &fs.ups[..fs.nups as usize];
        self.state.alloc_func(params, nparams, body, ups)
    }

    fn advance(&mut self) -> Result<(), &'static str> {
        self.cur = self.lex.next_token()?;
        Ok(())
    }

    /// The lookahead token. The REPL uses this after `parse_expr` to tell an
    /// assignment (`x = ...`, `k, v = ...`) from a bare expression.
    pub fn current(&self) -> Tok {
        self.cur
    }

    fn expect(&mut self, t: Tok, msg: &'static str) -> Result<(), &'static str> {
        if self.cur == t {
            self.advance()
        } else {
            Err(msg)
        }
    }

    fn opt_semi(&mut self) {
        if self.cur == Tok::Semi {
            let _ = self.advance();
        }
    }

    /// Consume an identifier, interning it, returning its [`StrRef`].
    fn expect_name(&mut self) -> Result<StrRef, &'static str> {
        match self.cur {
            Tok::Name(off, len) => {
                let r = self
                    .state
                    .intern(&self.lex.src()[off as usize..][..len as usize])?;
                self.advance()?;
                Ok(r)
            }
            _ => Err("expected identifier"),
        }
    }

    fn alloc(&mut self, n: Node) -> Result<u16, &'static str> {
        self.state.alloc_node(n)
    }

    // ── Grammar entry ───────────────────────────────────────────────────────

    pub fn parse_script(&mut self) -> Result<u16, &'static str> {
        let first = self.parse_block()?;
        if self.cur != Tok::Eof {
            return Err("unexpected token after script");
        }
        Ok(first)
    }

    /// Parse a statement block. Stops at `end`/`else`/`elseif`/`until`/EOF,
    /// leaving the terminator token in `cur`. Returns the first statement node.
    fn parse_block(&mut self) -> Result<u16, &'static str> {
        if self.n_scopes >= MAX_SCOPES {
            return Err("script too complex");
        }
        let local_mark = self.nlocals;
        self.scopes[self.n_scopes] = Scope::empty();
        self.n_scopes += 1;
        let mut first: u16 = NO_NODE;
        let mut prev: u16 = NO_NODE;
        loop {
            match self.cur {
                Tok::Eof | Tok::End | Tok::Else | Tok::Elseif | Tok::Until => break,
                _ => {}
            }
            let s = self.parse_stat()?;
            if prev != NO_NODE {
                self.state.next[prev as usize] = s;
            } else {
                first = s;
            }
            prev = s;
        }
        // Locals declared in this block go out of scope at its end.
        self.nlocals = local_mark;
        self.resolve_scope()?;
        Ok(first)
    }

    /// Resolve the innermost block's `goto` statements against the labels of
    /// this block and every enclosing block (innermost first). A `goto` whose
    /// label is found is patched with the label's node index; one that isn't is
    /// deferred to the enclosing block (allowing jumps out of a block). The
    /// scope is popped afterwards, so labels in nested blocks are never visible
    /// to outer blocks (no jumps into a block).
    fn resolve_scope(&mut self) -> Result<(), &'static str> {
        let s = self.n_scopes - 1;
        let ng = self.scopes[s].ngotos as usize;
        let gotos = self.scopes[s].gotos;
        for i in 0..ng {
            let (g_node, g_name) = gotos[i];
            match self.find_label(g_name, s) {
                Some(target) => {
                    self.state.nodes[g_node as usize] = Node::Goto(target);
                }
                None => {
                    if s == 0 {
                        return Err("unknown label");
                    }
                    let p = s - 1;
                    let np = self.scopes[p].ngotos as usize;
                    if np >= MAX_GOTOS_PER_SCOPE {
                        return Err("too many gotos");
                    }
                    self.scopes[p].gotos[np] = (g_node, g_name);
                    self.scopes[p].ngotos += 1;
                }
            }
        }
        self.n_scopes -= 1;
        Ok(())
    }

    /// Find the node index of a label by name, searching scopes from
    /// `innermost` (inclusive) outward.
    fn find_label(&self, name: StrRef, innermost: usize) -> Option<u16> {
        for scope in (0..=innermost).rev() {
            for i in 0..self.scopes[scope].nlabels as usize {
                if self.scopes[scope].labels[i].0 == name {
                    return Some(self.scopes[scope].labels[i].1);
                }
            }
        }
        None
    }

    fn record_label(&mut self, name: StrRef, node: u16) -> Result<(), &'static str> {
        let s = self.n_scopes - 1;
        let n = self.scopes[s].nlabels as usize;
        if n >= MAX_LABELS_PER_SCOPE {
            return Err("too many labels");
        }
        self.scopes[s].labels[n] = (name, node);
        self.scopes[s].nlabels += 1;
        Ok(())
    }

    fn record_goto(&mut self, node: u16, name: StrRef) -> Result<(), &'static str> {
        let s = self.n_scopes - 1;
        let n = self.scopes[s].ngotos as usize;
        if n >= MAX_GOTOS_PER_SCOPE {
            return Err("too many gotos");
        }
        self.scopes[s].gotos[n] = (node, name);
        self.scopes[s].ngotos += 1;
        Ok(())
    }

    fn parse_stat(&mut self) -> Result<u16, &'static str> {
        match self.cur {
            Tok::Local => {
                self.advance()?;
                // `local function name(...) ... end`: declare the local before
                // parsing the body so it can recurse.
                if self.cur == Tok::Function {
                    self.advance()?;
                    let name = self.expect_name()?;
                    self.declare_local_rec(name, NO_NODE)?;
                    let fi = self.parse_func_body()?;
                    let name_node = self.alloc(Node::Var(name))?;
                    return self.alloc(Node::LocalFunc(name_node, fi));
                }
                // One or more names, chained through `next[]`, so a call value
                // can supply two of them (`local k, v = next(t)`).
                let name = self.expect_name()?;
                let first = self.alloc(Node::Var(name))?;
                let mut last = first;
                let mut count = 1usize;
                while self.cur == Tok::Comma {
                    self.advance()?;
                    let name = self.expect_name()?;
                    let node = self.alloc(Node::Var(name))?;
                    self.state.next[last as usize] = node;
                    last = node;
                    count += 1;
                    if count >= super::MAX_LOCALS {
                        return Err("too many locals");
                    }
                }
                self.expect(Tok::Equals, "expected '=' in local declaration")?;
                let v = self.parse_expr()?;
                self.opt_semi();
                let node = self.alloc(Node::LocalDecl(first, v))?;
                // Declare the names after the initializer is parsed.
                let mut np = first;
                while np != NO_NODE {
                    let name = match self.state.nodes[np as usize] {
                        Node::Var(n) => n,
                        _ => 0,
                    };
                    self.declare_local_rec(name, node)?;
                    np = self.state.next[np as usize];
                }
                Ok(node)
            }
            Tok::Global => {
                self.advance()?;
                let name = self.expect_name()?;
                let v = if self.cur == Tok::Equals {
                    self.advance()?;
                    self.parse_expr()?
                } else {
                    NO_NODE
                };
                self.opt_semi();
                let name_node = self.alloc(Node::Var(name))?;
                self.alloc(Node::GlobalDecl(name_node, v))
            }
            Tok::Function => {
                self.advance()?;
                let name = self.expect_name()?;
                let fi = self.parse_func_body()?;
                let name_node = self.alloc(Node::Var(name))?;
                let fl = self.alloc(Node::FuncLit(fi))?;
                self.alloc(Node::AssignStmt(name_node, fl))
            }
            Tok::If => {
                self.advance()?;
                let cond = self.parse_expr()?;
                self.expect(Tok::Then, "expected 'then'")?;
                let then_b = self.parse_block()?;
                let els = self.parse_if_tail()?;
                self.alloc(Node::IfStmt(cond, then_b, els))
            }
            Tok::While => {
                self.advance()?;
                let cond = self.parse_expr()?;
                self.expect(Tok::Do, "expected 'do'")?;
                self.loop_depth += 1;
                let body = self.parse_block()?;
                self.loop_depth -= 1;
                self.expect(Tok::End, "expected 'end' to close while")?;
                self.alloc(Node::WhileStmt(cond, body))
            }
            Tok::For => {
                self.advance()?;
                let name = self.expect_name()?;
                let name_node = self.alloc(Node::Var(name))?;
                if self.cur == Tok::Equals {
                    // Numeric for: `for i = start, limit [, step] do .. end`.
                    self.advance()?;
                    let start = self.parse_expr()?;
                    self.expect(Tok::Comma, "expected ',' in for statement")?;
                    let limit = self.parse_expr()?;
                    let step = if self.cur == Tok::Comma {
                        self.advance()?;
                        self.parse_expr()?
                    } else {
                        NO_NODE
                    };
                    self.expect(Tok::Do, "expected 'do'")?;
                    let mark = self.nlocals;
                    let rec = self.declare_local_rec(name, NO_NODE)?;
                    self.loop_depth += 1;
                    let body = self.parse_block()?;
                    self.loop_depth -= 1;
                    self.expect(Tok::End, "expected 'end' to close for")?;
                    let node = if self.locals[rec].captured {
                        self.alloc(Node::ForStmtCell(name_node, start, limit, step, body))?
                    } else {
                        self.alloc(Node::ForStmt(name_node, start, limit, step, body))?
                    };
                    self.locals[rec].decl_node = node;
                    self.nlocals = mark;
                    Ok(node)
                } else {
                    // Generic for: `for k [, v] in table do .. end`.
                    let vnode = if self.cur == Tok::Comma {
                        self.advance()?;
                        let vname = self.expect_name()?;
                        self.alloc(Node::Var(vname))?
                    } else {
                        NO_NODE
                    };
                    self.expect(Tok::In, "expected 'in' in for statement")?;
                    let table = self.parse_expr()?;
                    self.expect(Tok::Do, "expected 'do'")?;
                    let mark = self.nlocals;
                    let rec = self.declare_local_rec(name, NO_NODE)?;
                    let vrec = if vnode != NO_NODE {
                        let vname = match self.state.nodes[vnode as usize] {
                            Node::Var(n) => n,
                            _ => 0,
                        };
                        Some(self.declare_local_rec(vname, NO_NODE)?)
                    } else {
                        None
                    };
                    self.loop_depth += 1;
                    let body = self.parse_block()?;
                    self.loop_depth -= 1;
                    self.expect(Tok::End, "expected 'end' to close for")?;
                    let captured = self.locals[rec].captured
                        || vrec.map(|r| self.locals[r].captured).unwrap_or(false);
                    let node = if captured {
                        self.alloc(Node::ForInStmtCell(name_node, vnode, table, body))?
                    } else {
                        self.alloc(Node::ForInStmt(name_node, vnode, table, body))?
                    };
                    self.locals[rec].decl_node = node;
                    if let Some(r) = vrec {
                        self.locals[r].decl_node = node;
                    }
                    self.nlocals = mark;
                    Ok(node)
                }
            }
            Tok::Repeat => {
                self.advance()?;
                self.loop_depth += 1;
                let body = self.parse_block()?;
                self.loop_depth -= 1;
                self.expect(Tok::Until, "expected 'until' to close repeat")?;
                let cond = self.parse_expr()?;
                self.alloc(Node::RepeatStmt(body, cond))
            }
            Tok::ColonColon => {
                self.advance()?;
                let name = self.expect_name()?;
                self.expect(Tok::ColonColon, "expected '::' after label name")?;
                let node = self.alloc(Node::Label(name))?;
                self.record_label(name, node)?;
                Ok(node)
            }
            Tok::Goto => {
                self.advance()?;
                let name = self.expect_name()?;
                let node = self.alloc(Node::Goto(0))?;
                self.record_goto(node, name)?;
                Ok(node)
            }
            Tok::Break => {
                self.advance()?;
                if self.loop_depth == 0 {
                    return Err("break outside loop");
                }
                self.opt_semi();
                self.alloc(Node::BreakStmt)
            }
            Tok::Return => {
                self.advance()?;
                let v = if self.at_block_end() {
                    NO_NODE
                } else {
                    self.parse_expr()?
                };
                self.opt_semi();
                self.alloc(Node::ReturnStmt(v))
            }
            _ => self.parse_expr_stat(),
        }
    }

    /// Expression statement: either an assignment or a call.
    fn parse_expr_stat(&mut self) -> Result<u16, &'static str> {
        let e = self.parse_expr()?;
        if self.cur == Tok::Equals || self.cur == Tok::Comma {
            self.parse_assignment_from(e)
        } else {
            self.opt_semi();
            self.alloc(Node::CallStmt(e))
        }
    }

    /// Finish an assignment whose first target has already been parsed by
    /// `parse_expr` (the REPL detects assignments the same way). Remaining
    /// targets are chained through `next[]`.
    pub fn parse_assignment_from(&mut self, first: u16) -> Result<u16, &'static str> {
        if !is_assign_target(self.state, first) {
            return Err("invalid assignment target");
        }
        let mut last = first;
        while self.cur == Tok::Comma {
            self.advance()?;
            let t = self.parse_expr()?;
            if !is_assign_target(self.state, t) {
                return Err("invalid assignment target");
            }
            self.state.next[last as usize] = t;
            last = t;
        }
        self.expect(Tok::Equals, "expected '=' in assignment")?;
        let v = self.parse_expr()?;
        self.opt_semi();
        self.alloc(Node::AssignStmt(first, v))
    }

    fn at_block_end(&self) -> bool {
        matches!(
            self.cur,
            Tok::Eof | Tok::End | Tok::Else | Tok::Elseif | Tok::Until | Tok::Semi
        )
    }

    /// `elseif cond then block` / `else block` / `end` tail of an `if`.
    /// Returns the innermost `else` statement node index ([`NO_NODE`] = no else).
    fn parse_if_tail(&mut self) -> Result<u16, &'static str> {
        match self.cur {
            Tok::End => {
                self.advance()?;
                Ok(NO_NODE)
            }
            Tok::Else => {
                self.advance()?;
                let b = self.parse_block()?;
                self.expect(Tok::End, "expected 'end' after else")?;
                Ok(b)
            }
            Tok::Elseif => {
                self.advance()?;
                let c = self.parse_expr()?;
                self.expect(Tok::Then, "expected 'then'")?;
                let t = self.parse_block()?;
                let tail = self.parse_if_tail()?;
                self.alloc(Node::IfStmt(c, t, tail))
            }
            _ => Err("expected 'end', 'else', or 'elseif'"),
        }
    }

    /// `(name, name, ...)` parameter list. Parameter names are contiguous
    /// [`Node::Var`] nodes.
    fn parse_params(&mut self) -> Result<(u16, u8), &'static str> {
        if self.cur == Tok::RParen {
            self.advance()?;
            return Ok((0, 0));
        }
        let mut first: u16 = 0;
        let mut n: u8 = 0;
        loop {
            let name = self.expect_name()?;
            let name_node = self.alloc(Node::Var(name))?;
            if n == 0 {
                first = name_node;
            }
            n += 1;
            if self.cur == Tok::Comma {
                self.advance()?;
                continue;
            }
            break;
        }
        self.expect(Tok::RParen, "expected ')' after parameters")?;
        Ok((first, n))
    }

    // ── Expression precedence climbing ──────────────────────────────────────

    pub fn parse_expr(&mut self) -> Result<u16, &'static str> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<u16, &'static str> {
        let mut l = self.parse_and()?;
        while self.cur == Tok::Or {
            self.advance()?;
            let r = self.parse_and()?;
            l = self.alloc(Node::Bin(Op::Or, l, r))?;
        }
        Ok(l)
    }

    fn parse_and(&mut self) -> Result<u16, &'static str> {
        let mut l = self.parse_cmp()?;
        while self.cur == Tok::And {
            self.advance()?;
            let r = self.parse_cmp()?;
            l = self.alloc(Node::Bin(Op::And, l, r))?;
        }
        Ok(l)
    }

    fn parse_cmp(&mut self) -> Result<u16, &'static str> {
        let mut l = self.parse_concat()?;
        loop {
            let op = match self.cur {
                Tok::EqEq => Op::Eq,
                Tok::Neq => Op::Ne,
                Tok::Lt => Op::Lt,
                Tok::Le => Op::Le,
                Tok::Gt => Op::Gt,
                Tok::Ge => Op::Ge,
                _ => break,
            };
            self.advance()?;
            let r = self.parse_concat()?;
            l = self.alloc(Node::Bin(op, l, r))?;
        }
        Ok(l)
    }

    /// `..` is right-associative.
    fn parse_concat(&mut self) -> Result<u16, &'static str> {
        let l = self.parse_addsub()?;
        if self.cur == Tok::DotDot {
            self.advance()?;
            let r = self.parse_concat()?;
            self.alloc(Node::Bin(Op::Concat, l, r))
        } else {
            Ok(l)
        }
    }

    fn parse_addsub(&mut self) -> Result<u16, &'static str> {
        let mut l = self.parse_muldiv()?;
        loop {
            let op = match self.cur {
                Tok::Plus => Op::Add,
                Tok::Minus => Op::Sub,
                _ => break,
            };
            self.advance()?;
            let r = self.parse_muldiv()?;
            l = self.alloc(Node::Bin(op, l, r))?;
        }
        Ok(l)
    }

    fn parse_muldiv(&mut self) -> Result<u16, &'static str> {
        let mut l = self.parse_unary()?;
        loop {
            let op = match self.cur {
                Tok::Star => Op::Mul,
                Tok::Slash => Op::Div,
                Tok::Percent => Op::Mod,
                _ => break,
            };
            self.advance()?;
            let r = self.parse_unary()?;
            l = self.alloc(Node::Bin(op, l, r))?;
        }
        Ok(l)
    }

    fn parse_unary(&mut self) -> Result<u16, &'static str> {
        match self.cur {
            Tok::Not => {
                self.advance()?;
                let x = self.parse_unary()?;
                self.alloc(Node::Un(Op::Not, x))
            }
            Tok::Minus => {
                self.advance()?;
                let x = self.parse_unary()?;
                self.alloc(Node::Un(Op::Neg, x))
            }
            _ => self.parse_primary(),
        }
    }

    fn parse_primary(&mut self) -> Result<u16, &'static str> {
        let mut node = match self.cur {
            Tok::Num(v) => {
                self.advance()?;
                self.alloc(Node::Num(v))?
            }
            Tok::Float(v) => {
                self.advance()?;
                self.alloc(Node::Float(v))?
            }
            Tok::Str(_len) => {
                let r = self.state.intern(self.lex.buf())?;
                self.advance()?;
                self.alloc(Node::Str(r))?
            }
            Tok::True => {
                self.advance()?;
                self.alloc(Node::True)?
            }
            Tok::False => {
                self.advance()?;
                self.alloc(Node::False)?
            }
            Tok::Nil => {
                self.advance()?;
                self.alloc(Node::Nil)?
            }
            Tok::Name(off, len) => {
                let r = self
                    .state
                    .intern(&self.lex.src()[off as usize..][..len as usize])?;
                self.advance()?;
                // Record lexical references so closures capture correctly.
                self.resolve_name(r)?;
                self.alloc(Node::Var(r))?
            }
            Tok::LParen => {
                self.advance()?;
                let e = self.parse_expr()?;
                self.expect(Tok::RParen, "expected ')'")?;
                e
            }
            Tok::LBrace => self.parse_table_lit()?,
            Tok::Function => {
                // Anonymous function literal: `function (params) ... end`.
                self.advance()?;
                let fi = self.parse_func_body()?;
                self.alloc(Node::FuncLit(fi))?
            }
            _ => return Err("unexpected token in expression"),
        };

        // Suffix chain: indexing and calls.
        loop {
            match self.cur {
                Tok::Dot => {
                    self.advance()?;
                    let (off, len) = match self.cur {
                        Tok::Name(o, l) => (o, l),
                        _ => return Err("expected name after '.'"),
                    };
                    let name = self
                        .state
                        .intern(&self.lex.src()[off as usize..][..len as usize])?;
                    self.advance()?;
                    let key = self.alloc(Node::Str(name))?;
                    let base = node;
                    node = self.alloc(Node::Index(base, key))?;
                }
                Tok::LBracket => {
                    self.advance()?;
                    let key = self.parse_expr()?;
                    self.expect(Tok::RBracket, "expected ']'")?;
                    let base = node;
                    node = self.alloc(Node::Index(base, key))?;
                }
                Tok::LParen => {
                    self.advance()?;
                    let first_arg = self.parse_arg_list()?;
                    let base = node;
                    node = self.alloc(Node::Call(base, first_arg))?;
                }
                _ => break,
            }
        }
        Ok(node)
    }

    /// Table literal. Fields are [`Node::Field`] nodes chained via `next[]`.
    fn parse_table_lit(&mut self) -> Result<u16, &'static str> {
        self.advance()?; // '{'
        let mut first: u16 = NO_NODE;
        let mut last: u16 = NO_NODE;
        let mut ordinal: u16 = 0;
        if self.cur != Tok::RBrace {
            loop {
                let (key_node, val_node) = match self.cur {
                    Tok::LBracket => {
                        self.advance()?;
                        let k = self.parse_expr()?;
                        self.expect(Tok::RBracket, "expected ']'")?;
                        self.expect(Tok::Equals, "expected '=' after '[expr]' key")?;
                        let v = self.parse_expr()?;
                        (k, v)
                    }
                    Tok::Name(off, len) => {
                        let name = self
                            .state
                            .intern(&self.lex.src()[off as usize..][..len as usize])?;
                        self.advance()?;
                        if self.cur == Tok::Equals {
                            self.advance()?;
                            let k = self.alloc(Node::Str(name))?;
                            let v = self.parse_expr()?;
                            (k, v)
                        } else {
                            ordinal += 1;
                            let k = self.alloc(Node::Num(ordinal as i64))?;
                            let v = self.alloc(Node::Var(name))?;
                            (k, v)
                        }
                    }
                    _ => {
                        ordinal += 1;
                        let k = self.alloc(Node::Num(ordinal as i64))?;
                        let v = self.parse_expr()?;
                        (k, v)
                    }
                };
                let field = self.alloc(Node::Field(key_node, val_node))?;
                if last != NO_NODE {
                    self.state.next[last as usize] = field;
                } else {
                    first = field;
                }
                last = field;
                if self.cur == Tok::Comma {
                    self.advance()?;
                    continue;
                }
                break;
            }
        }
        self.expect(Tok::RBrace, "expected '}' in table literal")?;
        self.alloc(Node::TableLit(first))
    }

    /// `(expr, expr, ...)`. Arguments are [`Node::Arg`] nodes chained via
    /// `next[]`; returns the first one (`NO_NODE` if no args).
    fn parse_arg_list(&mut self) -> Result<u16, &'static str> {
        if self.cur == Tok::RParen {
            self.advance()?;
            return Ok(NO_NODE);
        }
        let mut first: u16 = NO_NODE;
        let mut last: u16 = NO_NODE;
        loop {
            let a = self.parse_expr()?;
            let arg = self.alloc(Node::Arg(a))?;
            if last != NO_NODE {
                self.state.next[last as usize] = arg;
            } else {
                first = arg;
            }
            last = arg;
            if self.cur == Tok::Comma {
                self.advance()?;
                continue;
            }
            break;
        }
        self.expect(Tok::RParen, "expected ')'")?;
        Ok(first)
    }
}

fn is_assign_target(s: &LuaState, node: u16) -> bool {
    matches!(s.nodes[node as usize], Node::Var(_) | Node::Index(..))
}
