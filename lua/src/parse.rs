//! Recursive-descent parser: builds the static AST in [`LuaState`].

use super::lex::{Lexer, Tok};
use super::{LuaState, Node, Op, StrRef, NO_NODE};

/// Recursive-descent parser with a single-token lookahead.
///
/// `'l` is the lifetime of the source buffer; `'s` the borrow of the state.
pub struct Parser<'s, 'l> {
    lex: Lexer<'l>,
    cur: Tok,
    /// Nesting depth of `while`/`for` loops. `break` is only valid when > 0.
    loop_depth: u32,
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
            state,
        }
    }

    fn advance(&mut self) -> Result<(), &'static str> {
        self.cur = self.lex.next_token()?;
        Ok(())
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
        Ok(first)
    }

    fn parse_stat(&mut self) -> Result<u16, &'static str> {
        match self.cur {
            Tok::Local => {
                self.advance()?;
                let name = self.expect_name()?;
                self.expect(Tok::Equals, "expected '=' in local declaration")?;
                let v = self.parse_expr()?;
                self.opt_semi();
                let name_node = self.alloc(Node::Var(name))?;
                self.alloc(Node::LocalDecl(name_node, v))
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
                self.expect(Tok::LParen, "expected '(' after function name")?;
                let (params, nparams) = self.parse_params()?;
                let saved_loop = self.loop_depth;
                self.loop_depth = 0;
                let body = self.parse_block()?;
                self.loop_depth = saved_loop;
                self.expect(Tok::End, "expected 'end' to close function")?;
                let fi = self.state.alloc_func(params, nparams, body)?;
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
                    self.loop_depth += 1;
                    let body = self.parse_block()?;
                    self.loop_depth -= 1;
                    self.expect(Tok::End, "expected 'end' to close for")?;
                    self.alloc(Node::ForStmt(name_node, start, limit, step, body))
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
                    self.loop_depth += 1;
                    let body = self.parse_block()?;
                    self.loop_depth -= 1;
                    self.expect(Tok::End, "expected 'end' to close for")?;
                    self.alloc(Node::ForInStmt(name_node, vnode, table, body))
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
        if self.cur == Tok::Equals {
            if !is_assign_target(self.state, e) {
                return Err("invalid assignment target");
            }
            self.advance()?;
            let v = self.parse_expr()?;
            self.opt_semi();
            self.alloc(Node::AssignStmt(e, v))
        } else {
            self.opt_semi();
            self.alloc(Node::CallStmt(e))
        }
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
                self.alloc(Node::Var(r))?
            }
            Tok::LParen => {
                self.advance()?;
                let e = self.parse_expr()?;
                self.expect(Tok::RParen, "expected ')'")?;
                e
            }
            Tok::LBrace => self.parse_table_lit()?,
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
