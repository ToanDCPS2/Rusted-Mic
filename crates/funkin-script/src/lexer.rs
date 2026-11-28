//! Haxe tokens, with string interpolation split out and `#if` blocks
//! resolved against a set of defines (as Polymod's HScript parser does).

use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq)]
pub enum IPart {
    Lit(String),
    /// `$name` or `${expr}` (the source of the expression, and its line).
    Code(String, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Float(f64),
    Str(String),
    /// A single-quoted string with `$` interpolation.
    IStr(Vec<IPart>),
    Op(&'static str),
    /// `@:meta` or `@meta` (skipped by the parser).
    Meta(String),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub line: u32,
    /// Byte offsets, so the parser can tell `>` `>` (generics) from `>>`.
    pub start: usize,
    pub end: usize,
}

const OPS: [&str; 45] = [
    "...", "<<=", ">>=", "??=", "===", "!==", "=>", "->", "==", "!=", "<=", ">=", "&&", "||", "??", "?.", "++", "--", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<",
    "+", "-", "*", "/", "%", "=", "<", ">", "!", "~", "&", "|", "^", "?", ":", ".", ",", ";",
];
const BRACKETS: [&str; 6] = ["(", ")", "[", "]", "{", "}"];

#[derive(Debug, Clone)]
pub struct LexError {
    pub line: u32,
    pub msg: String,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    i: usize,
    line: u32,
    defines: &'a HashSet<String>,
    /// `#if` nesting: (this branch active, some branch already taken, parent active).
    cond: Vec<(bool, bool, bool)>,
}

impl<'a> Lexer<'a> {
    pub fn new(text: &'a str, defines: &'a HashSet<String>, first_line: u32) -> Self {
        Self { src: text.as_bytes(), text, i: 0, line: first_line, defines, cond: Vec::new() }
    }

    fn active(&self) -> bool {
        self.cond.last().map(|c| c.0).unwrap_or(true)
    }

    fn err<T>(&self, msg: impl Into<String>) -> Result<T, LexError> {
        Err(LexError { line: self.line, msg: msg.into() })
    }

    fn peek(&self, k: usize) -> u8 {
        *self.src.get(self.i + k).unwrap_or(&0)
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, LexError> {
        let mut out = Vec::new();
        loop {
            let t = self.next()?;
            let eof = t.tok == Tok::Eof;
            if eof || self.active() {
                out.push(t);
            }
            if eof {
                return Ok(out);
            }
        }
    }

    fn skip_space(&mut self) -> Result<(), LexError> {
        loop {
            let c = self.peek(0);
            if c == b'\n' {
                self.line += 1;
                self.i += 1;
            } else if c.is_ascii_whitespace() {
                self.i += 1;
            } else if c == b'/' && self.peek(1) == b'/' {
                while self.i < self.src.len() && self.src[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if c == b'/' && self.peek(1) == b'*' {
                self.i += 2;
                loop {
                    if self.i >= self.src.len() {
                        return self.err("unterminated comment");
                    }
                    if self.src[self.i] == b'\n' {
                        self.line += 1;
                    }
                    if self.src[self.i] == b'*' && self.peek(1) == b'/' {
                        self.i += 2;
                        break;
                    }
                    self.i += 1;
                }
            } else {
                return Ok(());
            }
        }
    }

    fn ident(&mut self) -> String {
        let s = self.i;
        while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
            self.i += 1;
        }
        self.text[s..self.i].to_owned()
    }

    /// A `#if` condition: `name`, `!cond`, `(a && b || c)`.
    fn cond_expr(&mut self) -> Result<bool, LexError> {
        self.skip_space()?;
        self.cond_or()
    }
    fn cond_or(&mut self) -> Result<bool, LexError> {
        let mut v = self.cond_and()?;
        loop {
            self.skip_space()?;
            if self.peek(0) == b'|' && self.peek(1) == b'|' {
                self.i += 2;
                let r = self.cond_and()?;
                v = v || r;
            } else {
                return Ok(v);
            }
        }
    }
    fn cond_and(&mut self) -> Result<bool, LexError> {
        let mut v = self.cond_atom()?;
        loop {
            self.skip_space()?;
            if self.peek(0) == b'&' && self.peek(1) == b'&' {
                self.i += 2;
                let r = self.cond_atom()?;
                v = v && r;
            } else {
                return Ok(v);
            }
        }
    }
    fn cond_atom(&mut self) -> Result<bool, LexError> {
        self.skip_space()?;
        match self.peek(0) {
            b'!' => {
                self.i += 1;
                Ok(!self.cond_atom()?)
            }
            b'(' => {
                self.i += 1;
                let v = self.cond_or()?;
                self.skip_space()?;
                if self.peek(0) != b')' {
                    return self.err("expected ) in #if");
                }
                self.i += 1;
                Ok(v)
            }
            c if c.is_ascii_alphabetic() || c == b'_' => {
                let name = self.ident();
                Ok(self.defines.contains(&name))
            }
            _ => self.err("bad #if condition"),
        }
    }

    fn directive(&mut self) -> Result<(), LexError> {
        self.i += 1;
        let d = self.ident();
        match d.as_str() {
            "if" => {
                let parent = self.active();
                let v = self.cond_expr()?;
                self.cond.push((parent && v, v, parent));
            }
            "elseif" => {
                let v = self.cond_expr()?;
                let Some(c) = self.cond.last_mut() else { return self.err("#elseif without #if") };
                let on = c.2 && !c.1 && v;
                c.0 = on;
                c.1 |= v;
            }
            "else" => {
                let Some(c) = self.cond.last_mut() else { return self.err("#else without #if") };
                c.0 = c.2 && !c.1;
                c.1 = true;
            }
            "end" => {
                if self.cond.pop().is_none() {
                    return self.err("#end without #if");
                }
            }
            "error" | "warning" | "line" => {
                while self.i < self.src.len() && self.src[self.i] != b'\n' {
                    self.i += 1;
                }
            }
            other => return self.err(format!("unknown directive #{other}")),
        }
        Ok(())
    }

    fn next(&mut self) -> Result<Token, LexError> {
        loop {
            self.skip_space()?;
            if self.peek(0) == b'#' {
                self.directive()?;
                continue;
            }
            break;
        }
        let start = self.i;
        let line = self.line;
        let tok = self.read()?;
        Ok(Token { tok, line, start, end: self.i })
    }

    fn read(&mut self) -> Result<Tok, LexError> {
        let c = self.peek(0);
        if self.i >= self.src.len() {
            return Ok(Tok::Eof);
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            return Ok(Tok::Ident(self.ident()));
        }
        if c.is_ascii_digit() || (c == b'.' && self.peek(1).is_ascii_digit()) {
            return self.number();
        }
        if c == b'"' || c == b'\'' {
            return self.string(c);
        }
        if c == b'@' {
            self.i += 1;
            if self.peek(0) == b':' {
                self.i += 1;
            }
            return Ok(Tok::Meta(self.ident()));
        }
        for b in BRACKETS {
            if c == b.as_bytes()[0] {
                self.i += 1;
                return Ok(Tok::Op(b));
            }
        }
        for op in OPS {
            if self.src[self.i..].starts_with(op.as_bytes()) {
                self.i += op.len();
                return Ok(Tok::Op(op));
            }
        }
        self.err(format!("unexpected character {:?}", c as char))
    }

    fn number(&mut self) -> Result<Tok, LexError> {
        let s = self.i;
        if self.peek(0) == b'0' && (self.peek(1) == b'x' || self.peek(1) == b'X') {
            self.i += 2;
            let h = self.i;
            while self.peek(0).is_ascii_hexdigit() || self.peek(0) == b'_' {
                self.i += 1;
            }
            let digits: String = self.text[h..self.i].chars().filter(|c| *c != '_').collect();
            return u64::from_str_radix(&digits, 16).map(|v| Tok::Int(v as i64)).or_else(|_| self.err("bad hex number"));
        }
        let mut float = false;
        while self.peek(0).is_ascii_digit() || self.peek(0) == b'_' {
            self.i += 1;
        }
        // `0...n` is a range, not a float.
        if self.peek(0) == b'.' && self.peek(1) != b'.' && !self.peek(1).is_ascii_alphabetic() {
            float = true;
            self.i += 1;
            while self.peek(0).is_ascii_digit() {
                self.i += 1;
            }
        }
        if self.peek(0) == b'e' || self.peek(0) == b'E' {
            float = true;
            self.i += 1;
            if self.peek(0) == b'+' || self.peek(0) == b'-' {
                self.i += 1;
            }
            while self.peek(0).is_ascii_digit() {
                self.i += 1;
            }
        }
        let t: String = self.text[s..self.i].chars().filter(|c| *c != '_').collect();
        if float {
            t.parse().map(Tok::Float).or_else(|_| self.err("bad number"))
        } else {
            match t.parse::<i64>() {
                Ok(v) => Ok(Tok::Int(v)),
                Err(_) => t.parse().map(Tok::Float).or_else(|_| self.err("bad number")),
            }
        }
    }

    fn escape(&mut self, out: &mut String) -> Result<(), LexError> {
        self.i += 1;
        let c = self.peek(0);
        self.i += 1;
        match c {
            b'n' => out.push('\n'),
            b't' => out.push('\t'),
            b'r' => out.push('\r'),
            b'0' => out.push('\0'),
            b'x' => {
                let h = &self.text[self.i..(self.i + 2).min(self.src.len())];
                self.i += 2;
                out.push(u8::from_str_radix(h, 16).map(|v| v as char).or_else(|_| self.err("bad \\x escape"))?);
            }
            b'u' => {
                let (h, skip) = if self.peek(0) == b'{' {
                    let end = self.text[self.i..].find('}').map(|e| self.i + e).unwrap_or(self.src.len());
                    (&self.text[self.i + 1..end], end + 1 - self.i)
                } else {
                    (&self.text[self.i..(self.i + 4).min(self.src.len())], 4)
                };
                self.i += skip;
                let v = u32::from_str_radix(h, 16).ok().and_then(char::from_u32);
                out.push(v.ok_or(LexError { line: self.line, msg: "bad \\u escape".into() })?);
            }
            0 => return self.err("unterminated string"),
            _ => {
                // \\ \' \" \$ and anything else: the character itself.
                let ch = self.text[self.i - 1..].chars().next().unwrap_or('?');
                self.i += ch.len_utf8() - 1;
                out.push(ch);
            }
        }
        Ok(())
    }

    fn string(&mut self, q: u8) -> Result<Tok, LexError> {
        self.i += 1;
        let interp = q == b'\'';
        let mut parts = Vec::new();
        let mut lit = String::new();
        loop {
            if self.i >= self.src.len() {
                return self.err("unterminated string");
            }
            let c = self.src[self.i];
            if c == q {
                self.i += 1;
                break;
            }
            if c == b'\\' {
                self.escape(&mut lit)?;
                continue;
            }
            if c == b'\n' {
                self.line += 1;
            }
            if interp && c == b'$' {
                if self.peek(1) == b'$' {
                    lit.push('$');
                    self.i += 2;
                    continue;
                }
                if self.peek(1) == b'{' {
                    // Find the matching brace (strings inside are rare; braces balance).
                    let line = self.line;
                    let s = self.i + 2;
                    let mut depth = 1;
                    let mut j = s;
                    while j < self.src.len() && depth > 0 {
                        match self.src[j] {
                            b'{' => depth += 1,
                            b'}' => depth -= 1,
                            b'\n' => self.line += 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    if depth != 0 {
                        return self.err("unterminated ${ in string");
                    }
                    if !lit.is_empty() {
                        parts.push(IPart::Lit(std::mem::take(&mut lit)));
                    }
                    parts.push(IPart::Code(self.text[s..j - 1].to_owned(), line));
                    self.i = j;
                    continue;
                }
                let n = self.peek(1);
                if n.is_ascii_alphabetic() || n == b'_' {
                    self.i += 1;
                    if !lit.is_empty() {
                        parts.push(IPart::Lit(std::mem::take(&mut lit)));
                    }
                    let name = self.ident();
                    parts.push(IPart::Code(name, self.line));
                    continue;
                }
            }
            let ch = self.text[self.i..].chars().next().unwrap_or('?');
            lit.push(ch);
            self.i += ch.len_utf8();
        }
        if parts.is_empty() {
            return Ok(Tok::Str(lit));
        }
        if !lit.is_empty() {
            parts.push(IPart::Lit(lit));
        }
        Ok(Tok::IStr(parts))
    }
}
