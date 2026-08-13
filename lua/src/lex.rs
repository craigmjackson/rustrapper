//! Lexer: converts a source byte buffer into a token stream.
//!
//! Identifier tokens reference back into the source buffer (which the caller
//! must keep alive for the whole parse). String tokens are decoded into a
//! small internal buffer because escape sequences change the byte length.

/// A lexical token.
#[derive(Clone, Copy, PartialEq)]
pub enum Tok {
    Eof,
    Num(i64),
    /// String literal length in bytes, decoded into `Lexer::buf`.
    Str(u16),
    /// Identifier, as (source offset, length).
    Name(u32, u16),

    // keywords
    And,
    Or,
    Not,
    If,
    Then,
    Elseif,
    Else,
    End,
    While,
    Do,
    For,
    In,
    Break,
    Repeat,
    Until,
    Goto,
    Function,
    Local,
    Global,
    Return,
    True,
    False,
    Nil,

    // symbols
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    LParen,
    RParen,
    LBracket,
    RBracket,
    LBrace,
    RBrace,
    Comma,
    Dot,
    DotDot,
    Semi,
    Colon,
    ColonColon,
    Equals,
    EqEq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    /// Decoded bytes of the most recently produced string token.
    buf: [u8; 256],
    buflen: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        Lexer {
            src,
            pos: 0,
            buf: [0u8; 256],
            buflen: 0,
        }
    }

    /// The full source buffer (referenced by name tokens).
    pub fn src(&self) -> &'a [u8] {
        self.src
    }

    /// Decoded bytes of the most recently produced string token.
    pub fn buf(&self) -> &[u8] {
        &self.buf[..self.buflen]
    }

    fn peek(&self) -> u8 {
        if self.pos < self.src.len() {
            self.src[self.pos]
        } else {
            0
        }
    }

    fn bump(&mut self) -> u8 {
        let b = self.peek();
        if self.pos < self.src.len() {
            self.pos += 1;
        }
        b
    }

    /// Produce the next token, skipping whitespace and `--` comments.
    pub fn next_token(&mut self) -> Result<Tok, &'static str> {
        loop {
            match self.peek() {
                b' ' | b'\t' | b'\r' | b'\n' => {
                    self.pos += 1;
                }
                b'-' => {
                    if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'-' {
                        self.pos += 2;
                        while self.pos < self.src.len() && self.src[self.pos] != b'\n' {
                            self.pos += 1;
                        }
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }

        if self.pos >= self.src.len() {
            return Ok(Tok::Eof);
        }

        match self.peek() {
            b'0'..=b'9' => self.lex_number(),
            b'"' | b'\'' => self.lex_string(self.peek()),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.lex_name(),
            b'+' => {
                self.pos += 1;
                Ok(Tok::Plus)
            }
            b'-' => {
                self.pos += 1;
                Ok(Tok::Minus)
            }
            b'*' => {
                self.pos += 1;
                Ok(Tok::Star)
            }
            b'/' => {
                self.pos += 1;
                Ok(Tok::Slash)
            }
            b'%' => {
                self.pos += 1;
                Ok(Tok::Percent)
            }
            b'(' => {
                self.pos += 1;
                Ok(Tok::LParen)
            }
            b')' => {
                self.pos += 1;
                Ok(Tok::RParen)
            }
            b'[' => {
                self.pos += 1;
                Ok(Tok::LBracket)
            }
            b']' => {
                self.pos += 1;
                Ok(Tok::RBracket)
            }
            b'{' => {
                self.pos += 1;
                Ok(Tok::LBrace)
            }
            b'}' => {
                self.pos += 1;
                Ok(Tok::RBrace)
            }
            b',' => {
                self.pos += 1;
                Ok(Tok::Comma)
            }
            b';' => {
                self.pos += 1;
                Ok(Tok::Semi)
            }
            b'.' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'.' {
                    self.pos += 2;
                    Ok(Tok::DotDot)
                } else {
                    self.pos += 1;
                    Ok(Tok::Dot)
                }
            }
            b':' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b':' {
                    self.pos += 2;
                    Ok(Tok::ColonColon)
                } else {
                    self.pos += 1;
                    Ok(Tok::Colon)
                }
            }
            b'=' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    Ok(Tok::EqEq)
                } else {
                    self.pos += 1;
                    Ok(Tok::Equals)
                }
            }
            b'~' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    Ok(Tok::Neq)
                } else {
                    Err("unexpected character '~'")
                }
            }
            b'<' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    Ok(Tok::Le)
                } else {
                    self.pos += 1;
                    Ok(Tok::Lt)
                }
            }
            b'>' => {
                if self.pos + 1 < self.src.len() && self.src[self.pos + 1] == b'=' {
                    self.pos += 2;
                    Ok(Tok::Ge)
                } else {
                    self.pos += 1;
                    Ok(Tok::Gt)
                }
            }
            _c => Err("unexpected character"),
        }
    }

    /// Decimal integer literal. Floats are not part of the subset.
    fn lex_number(&mut self) -> Result<Tok, &'static str> {
        let mut val: i64 = 0;
        let mut any = false;
        while matches!(self.peek(), b'0'..=b'9') {
            val = val.wrapping_mul(10).wrapping_add((self.bump() - b'0') as i64);
            any = true;
        }
        if !any {
            return Err("malformed number");
        }
        Ok(Tok::Num(val))
    }

    /// Identifier or keyword.
    fn lex_name(&mut self) -> Result<Tok, &'static str> {
        let start = self.pos;
        while matches!(self.peek(), b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_') {
            self.pos += 1;
        }
        let len = self.pos - start;
        Ok(match &self.src[start..start + len] {
            b"and" => Tok::And,
            b"break" => Tok::Break,
            b"or" => Tok::Or,
            b"not" => Tok::Not,
            b"if" => Tok::If,
            b"then" => Tok::Then,
            b"elseif" => Tok::Elseif,
            b"else" => Tok::Else,
            b"end" => Tok::End,
            b"while" => Tok::While,
            b"do" => Tok::Do,
            b"for" => Tok::For,
            b"in" => Tok::In,
            b"repeat" => Tok::Repeat,
            b"until" => Tok::Until,
            b"goto" => Tok::Goto,
            b"function" => Tok::Function,
            b"local" => Tok::Local,
            b"global" => Tok::Global,
            b"return" => Tok::Return,
            b"true" => Tok::True,
            b"false" => Tok::False,
            b"nil" => Tok::Nil,
            _ => Tok::Name(start as u32, len as u16),
        })
    }

    /// String literal with escape decoding. The decoded bytes land in `buf`.
    fn lex_string(&mut self, quote: u8) -> Result<Tok, &'static str> {
        self.bump(); // opening quote
        let mut n = 0usize;
        loop {
            let c = self.bump();
            match c {
                0 => return Err("unterminated string"),
                b'\n' => return Err("unterminated string"),
                x if x == quote => break,
                b'\\' => {
                    let d = match self.bump() {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'\\' => b'\\',
                        b'"' => b'"',
                        b'\'' => b'\'',
                        b'0' => 0,
                        _ => return Err("bad escape sequence"),
                    };
                    if n >= self.buf.len() {
                        return Err("string too long");
                    }
                    self.buf[n] = d;
                    n += 1;
                }
                c => {
                    if n >= self.buf.len() {
                        return Err("string too long");
                    }
                    self.buf[n] = c;
                    n += 1;
                }
            }
        }
        self.buflen = n;
        Ok(Tok::Str(n as u16))
    }
}
