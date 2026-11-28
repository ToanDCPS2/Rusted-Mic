//! A recursive-descent parser for `.hxc` files: imports, enums and classes
//! with Haxe expressions. Types are parsed and thrown away, as HScript does.

use std::collections::HashSet;
use std::rc::Rc;

use crate::ast::*;
use crate::lexer::{IPart, Lexer, Tok, Token};

#[derive(Debug, Clone)]
pub struct ParseError {
    pub file: String,
    pub line: u32,
    pub msg: String,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.file, self.line, self.msg)
    }
}

type R<T> = Result<T, ParseError>;

pub struct Parser<'d> {
    toks: Vec<Token>,
    i: usize,
    file: Rc<str>,
    defines: &'d HashSet<String>,
}

fn e(kind: ExprKind, line: u32) -> Expr {
    Expr { kind, line }
}

/// Parse a whole script file.
pub fn parse_module(src: &str, file: &str, defines: &HashSet<String>) -> R<Module> {
    let toks = Lexer::new(src, defines, 1).tokenize().map_err(|l| ParseError { file: file.into(), line: l.line, msg: l.msg })?;
    let mut p = Parser { toks, i: 0, file: file.into(), defines };
    p.module()
}

/// Parse a lone expression (or statement list), e.g. for tests or a console.
pub fn parse_expr(src: &str, defines: &HashSet<String>) -> R<Expr> {
    let toks = Lexer::new(src, defines, 1).tokenize().map_err(|l| ParseError { file: "<expr>".into(), line: l.line, msg: l.msg })?;
    let mut p = Parser { toks, i: 0, file: "<expr>".into(), defines };
    let mut body = Vec::new();
    while !p.at_eof() {
        body.push(p.expr()?);
        p.eat_op(";");
    }
    Ok(e(ExprKind::Block(body), 1))
}

const KEYWORDS: [&str; 26] = [
    "if", "else", "while", "do", "for", "var", "final", "function", "return", "break", "continue", "switch", "case", "default", "try", "catch", "throw", "new",
    "this", "super", "null", "true", "false", "cast", "untyped", "inline",
];

impl Parser<'_> {
    fn tok(&self) -> &Tok {
        &self.toks[self.i.min(self.toks.len() - 1)].tok
    }
    fn tok_at(&self, k: usize) -> &Tok {
        &self.toks[(self.i + k).min(self.toks.len() - 1)].tok
    }
    fn line(&self) -> u32 {
        self.toks[self.i.min(self.toks.len() - 1)].line
    }
    fn at_eof(&self) -> bool {
        *self.tok() == Tok::Eof
    }
    fn err<T>(&self, msg: impl Into<String>) -> R<T> {
        Err(ParseError { file: self.file.to_string(), line: self.line(), msg: format!("{} (at {:?})", msg.into(), self.tok()) })
    }
    fn is_op(&self, op: &str) -> bool {
        matches!(self.tok(), Tok::Op(o) if *o == op)
    }
    fn is_op_at(&self, k: usize, op: &str) -> bool {
        matches!(self.tok_at(k), Tok::Op(o) if *o == op)
    }
    fn eat_op(&mut self, op: &str) -> bool {
        if self.is_op(op) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expect(&mut self, op: &str) -> R<()> {
        if self.eat_op(op) {
            Ok(())
        } else {
            self.err(format!("expected '{op}'"))
        }
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.tok(), Tok::Ident(s) if s == kw)
    }
    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn ident(&mut self) -> R<Rc<str>> {
        match self.tok().clone() {
            Tok::Ident(s) => {
                self.i += 1;
                Ok(s.into())
            }
            _ => self.err("expected identifier"),
        }
    }
    /// Two tokens with nothing between them (`>` `>` is a shift, not generics).
    fn adjacent(&self, k: usize) -> bool {
        let a = &self.toks[(self.i + k).min(self.toks.len() - 1)];
        let b = &self.toks[(self.i + k + 1).min(self.toks.len() - 1)];
        a.end == b.start
    }

    fn skip_meta(&mut self) {
        while let Tok::Meta(_) = self.tok() {
            self.i += 1;
            if self.is_op("(") {
                let _ = self.skip_balanced("(", ")");
            }
        }
    }

    fn skip_balanced(&mut self, open: &str, close: &str) -> R<()> {
        self.expect(open)?;
        let mut depth = 1;
        while depth > 0 {
            if self.at_eof() {
                return self.err(format!("unclosed '{open}'"));
            }
            if self.is_op(open) {
                depth += 1;
            } else if self.is_op(close) {
                depth -= 1;
            }
            self.i += 1;
        }
        Ok(())
    }

    // ----------------------------------------------------------------- types

    fn skip_type(&mut self) -> R<()> {
        self.skip_type_atom()?;
        while self.eat_op("->") {
            self.skip_type_atom()?;
        }
        Ok(())
    }

    fn skip_type_atom(&mut self) -> R<()> {
        self.eat_op("?");
        if self.is_op("(") {
            return self.skip_balanced("(", ")");
        }
        if self.is_op("{") {
            return self.skip_balanced("{", "}");
        }
        self.ident()?;
        while self.is_op(".") {
            self.i += 1;
            self.ident()?;
        }
        if self.is_op("<") {
            self.i += 1;
            let mut depth = 1;
            while depth > 0 {
                if self.at_eof() {
                    return self.err("unclosed type parameters");
                }
                if self.is_op("<") {
                    depth += 1;
                } else if self.is_op(">") {
                    depth -= 1;
                } else if self.is_op(">=") {
                    // `Array<Int>=` written without a space.
                    self.toks[self.i].tok = Tok::Op("=");
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                self.i += 1;
            }
        }
        Ok(())
    }

    fn opt_type(&mut self) -> R<()> {
        if self.eat_op(":") {
            self.skip_type()?;
        }
        Ok(())
    }

    // ---------------------------------------------------------------- module

    fn path(&mut self) -> R<String> {
        let mut s = self.ident()?.to_string();
        while self.is_op(".") {
            self.i += 1;
            if self.eat_op("*") {
                s.push_str(".*");
                break;
            }
            s.push('.');
            s.push_str(&self.ident()?);
        }
        Ok(s)
    }

    fn module(&mut self) -> R<Module> {
        let mut m = Module::default();
        loop {
            self.skip_meta();
            if self.at_eof() {
                break;
            }
            if self.eat_kw("package") {
                if !self.is_op(";") {
                    self.path()?;
                }
                self.expect(";")?;
            } else if self.eat_kw("import") {
                let path = self.path()?;
                let alias = if self.eat_kw("as") || self.eat_kw("in") { self.ident()?.to_string() } else { path.rsplit('.').next().unwrap_or(&path).to_owned() };
                self.expect(";")?;
                m.imports.push((alias.into(), path.into()));
            } else if self.eat_kw("using") {
                self.path()?;
                self.expect(";")?;
            } else if self.eat_kw("enum") {
                let name = self.ident()?;
                self.expect("{")?;
                let mut values = Vec::new();
                while !self.eat_op("}") {
                    self.skip_meta();
                    values.push(self.ident()?);
                    if self.is_op("(") {
                        self.skip_balanced("(", ")")?;
                    }
                    self.expect(";")?;
                }
                m.enums.push(Rc::new(EnumDecl { name, values }));
            } else if self.eat_kw("typedef") {
                self.ident()?;
                self.expect("=")?;
                self.skip_type()?;
                self.eat_op(";");
            } else {
                // Class modifiers.
                while self.eat_kw("private") || self.eat_kw("final") || self.eat_kw("extern") || self.eat_kw("abstract") {}
                if self.eat_kw("class") {
                    let c = self.class(&m.imports)?;
                    m.classes.push(Rc::new(c));
                } else {
                    return self.err("expected import, enum or class");
                }
            }
        }
        Ok(m)
    }

    fn class(&mut self, imports: &[(Rc<str>, Rc<str>)]) -> R<ClassDecl> {
        let name = self.ident()?;
        if self.is_op("<") {
            self.skip_balanced("<", ">")?;
        }
        let mut extends = None;
        loop {
            if self.eat_kw("extends") {
                let p = self.path()?;
                if self.is_op("<") {
                    self.skip_balanced("<", ">")?;
                }
                extends = Some(p.rsplit('.').next().unwrap_or(&p).into());
            } else if self.eat_kw("implements") {
                self.skip_type()?;
            } else {
                break;
            }
        }
        self.expect("{")?;
        let mut fields = Vec::new();
        while !self.eat_op("}") {
            self.skip_meta();
            let mut is_static = false;
            let line = self.line();
            loop {
                if self.eat_kw("static") {
                    is_static = true;
                } else if !(self.eat_kw("public") || self.eat_kw("private") || self.eat_kw("override") || self.eat_kw("inline") || self.eat_kw("dynamic") || self.eat_kw("macro") || self.eat_kw("extern")) {
                    break;
                }
            }
            if self.eat_kw("function") {
                let f = self.function_rest(true)?;
                self.eat_op(";");
                fields.push(Field { name: f.name.clone(), is_static, getter: false, setter: false, kind: FieldKind::Func(Rc::new(f)), line });
            } else if self.eat_kw("var") || self.eat_kw("final") {
                let name = self.ident()?;
                let (mut getter, mut setter) = (false, false);
                if self.eat_op("(") {
                    // Property accessors `var x(get, set)`.
                    let g = self.ident()?;
                    self.expect(",")?;
                    let s = self.ident()?;
                    self.expect(")")?;
                    getter = &*g == "get";
                    setter = &*s == "set";
                }
                self.opt_type()?;
                let init = if self.eat_op("=") { Some(self.expr()?) } else { None };
                self.expect(";")?;
                fields.push(Field { name, is_static, getter, setter, kind: FieldKind::Var(init), line });
            } else {
                return self.err("expected a class field");
            }
        }
        Ok(ClassDecl { name, extends, fields, imports: imports.to_vec(), file: self.file.clone() })
    }

    /// After `function`: `[name](args)[:Ret] body`.
    fn function_rest(&mut self, need_name: bool) -> R<FuncDef> {
        let name: Rc<str> = match self.tok() {
            Tok::Ident(_) => self.ident()?,
            _ if need_name => return self.err("expected function name"),
            _ => "".into(),
        };
        if self.is_op("<") {
            self.skip_balanced("<", ">")?;
        }
        let args = self.args()?;
        self.opt_type()?;
        let body = if self.eat_op(";") { e(ExprKind::Block(vec![]), self.line()) } else { self.expr()? };
        Ok(FuncDef { name, args, body })
    }

    fn args(&mut self) -> R<Vec<Arg>> {
        self.expect("(")?;
        let mut args = Vec::new();
        while !self.eat_op(")") {
            self.skip_meta();
            let opt = self.eat_op("?");
            let name = self.ident()?;
            self.opt_type()?;
            let default = if self.eat_op("=") { Some(self.expr()?) } else { None };
            args.push(Arg { name, opt, default });
            if !self.eat_op(",") {
                self.expect(")")?;
                break;
            }
        }
        Ok(args)
    }

    // ----------------------------------------------------------- expressions

    pub fn expr(&mut self) -> R<Expr> {
        self.skip_meta();
        let line = self.line();
        let lhs = self.ternary()?;
        let op = match self.tok() {
            Tok::Op("=") => Some(None),
            Tok::Op("+=") => Some(Some(BinOp::Add)),
            Tok::Op("-=") => Some(Some(BinOp::Sub)),
            Tok::Op("*=") => Some(Some(BinOp::Mul)),
            Tok::Op("/=") => Some(Some(BinOp::Div)),
            Tok::Op("%=") => Some(Some(BinOp::Mod)),
            Tok::Op("&=") => Some(Some(BinOp::BitAnd)),
            Tok::Op("|=") => Some(Some(BinOp::BitOr)),
            Tok::Op("^=") => Some(Some(BinOp::BitXor)),
            Tok::Op("<<=") => Some(Some(BinOp::Shl)),
            Tok::Op("??=") => Some(Some(BinOp::NullCoal)),
            Tok::Op(">") if self.adjacent(0) && self.is_op_at(1, ">=") => {
                self.i += 1;
                Some(Some(BinOp::Shr))
            }
            _ => None,
        };
        if let Some(op) = op {
            self.i += 1;
            let rhs = self.expr()?;
            return Ok(e(ExprKind::Assign(op, Box::new(lhs), Box::new(rhs)), line));
        }
        Ok(lhs)
    }

    fn ternary(&mut self) -> R<Expr> {
        let line = self.line();
        let c = self.coalesce()?;
        if self.eat_op("?") {
            let a = self.expr()?;
            self.expect(":")?;
            let b = self.expr()?;
            return Ok(e(ExprKind::Ternary(Box::new(c), Box::new(a), Box::new(b)), line));
        }
        Ok(c)
    }

    fn bin_level(&mut self, level: usize) -> R<Expr> {
        if level == LEVELS.len() {
            return self.unary();
        }
        let line = self.line();
        let mut lhs = self.bin_level(level + 1)?;
        loop {
            let Some((op, n)) = self.bin_op(level) else { break };
            self.i += n;
            let rhs = self.bin_level(level + 1)?;
            lhs = match op {
                Some(op) => e(ExprKind::Bin(op, Box::new(lhs), Box::new(rhs)), line),
                None => e(ExprKind::Range(Box::new(lhs), Box::new(rhs)), line),
            };
        }
        // `x is T` binds like a comparison.
        if level == 5 && self.is_kw("is") {
            self.i += 1;
            let mut t = self.ident()?.to_string();
            while self.eat_op(".") {
                t = self.ident()?.to_string();
            }
            lhs = e(ExprKind::Is(Box::new(lhs), t.into()), line);
        }
        Ok(lhs)
    }

    fn coalesce(&mut self) -> R<Expr> {
        self.bin_level(0)
    }

    /// The operator at the cursor for a precedence level: (op, `None` for `...`), tokens used.
    fn bin_op(&self, level: usize) -> Option<(Option<BinOp>, usize)> {
        let Tok::Op(o) = self.tok() else { return None };
        if *o == ">" {
            // `>>`, `>>>` (lexed as separate `>`s) and `>`.
            if self.adjacent(0) && self.is_op_at(1, ">") {
                if self.adjacent(1) && self.is_op_at(2, ">") {
                    return (level == 7).then_some((Some(BinOp::UShr), 3));
                }
                if self.adjacent(1) && self.is_op_at(2, ">=") {
                    return None;
                }
                return (level == 7).then_some((Some(BinOp::Shr), 2));
            }
            if self.adjacent(0) && self.is_op_at(1, ">=") {
                return None;
            }
        }
        LEVELS[level].iter().find(|(s, _)| s == o).map(|(_, op)| (*op, 1))
    }

    fn unary(&mut self) -> R<Expr> {
        let line = self.line();
        let op = match self.tok() {
            Tok::Op("-") => Some(UnOp::Neg),
            Tok::Op("!") => Some(UnOp::Not),
            Tok::Op("~") => Some(UnOp::BitNot),
            Tok::Op("++") => Some(UnOp::Inc),
            Tok::Op("--") => Some(UnOp::Dec),
            _ => None,
        };
        if let Some(op) = op {
            self.i += 1;
            let x = self.unary()?;
            // Fold negative literals.
            if op == UnOp::Neg {
                match x.kind {
                    ExprKind::Const(Const::Int(v)) => return Ok(e(ExprKind::Const(Const::Int(-v)), line)),
                    ExprKind::Const(Const::Float(v)) => return Ok(e(ExprKind::Const(Const::Float(-v)), line)),
                    _ => {}
                }
            }
            return Ok(e(ExprKind::Unop(op, false, Box::new(x)), line));
        }
        let x = self.primary()?;
        self.postfix(x)
    }

    fn postfix(&mut self, mut x: Expr) -> R<Expr> {
        loop {
            let line = self.line();
            if self.is_op(".") || self.is_op("?.") {
                let safe = self.is_op("?.");
                self.i += 1;
                let name = self.ident()?;
                x = e(ExprKind::Field(Box::new(x), name, safe), line);
            } else if self.is_op("(") {
                let args = self.call_args()?;
                x = e(ExprKind::Call(Box::new(x), args), line);
            } else if self.is_op("[") {
                self.i += 1;
                let idx = self.expr()?;
                self.expect("]")?;
                x = e(ExprKind::Index(Box::new(x), Box::new(idx)), line);
            } else if self.is_op("++") || self.is_op("--") {
                let op = if self.is_op("++") { UnOp::Inc } else { UnOp::Dec };
                self.i += 1;
                x = e(ExprKind::Unop(op, true, Box::new(x)), line);
            } else {
                return Ok(x);
            }
        }
    }

    fn call_args(&mut self) -> R<Vec<Expr>> {
        self.expect("(")?;
        let mut args = Vec::new();
        while !self.eat_op(")") {
            args.push(self.expr()?);
            if !self.eat_op(",") {
                self.expect(")")?;
                break;
            }
        }
        Ok(args)
    }

    /// Is the `(` at the cursor the start of an arrow function `(a, b) -> ...`?
    fn paren_is_arrow(&self) -> bool {
        let mut depth = 0;
        let mut k = 0;
        loop {
            match self.tok_at(k) {
                Tok::Op("(") => depth += 1,
                Tok::Op(")") => {
                    depth -= 1;
                    if depth == 0 {
                        return self.is_op_at(k + 1, "->");
                    }
                }
                Tok::Eof => return false,
                _ => {}
            }
            k += 1;
        }
    }

    fn block_body(&mut self) -> R<Vec<Expr>> {
        let mut body = Vec::new();
        while !self.eat_op("}") {
            if self.at_eof() {
                return self.err("unclosed block");
            }
            if self.eat_op(";") {
                continue;
            }
            body.push(self.expr()?);
            self.eat_op(";");
        }
        Ok(body)
    }

    fn primary(&mut self) -> R<Expr> {
        self.skip_meta();
        let line = self.line();
        let t = self.tok().clone();
        match t {
            Tok::Int(v) => {
                self.i += 1;
                Ok(e(ExprKind::Const(Const::Int(v)), line))
            }
            Tok::Float(v) => {
                self.i += 1;
                Ok(e(ExprKind::Const(Const::Float(v)), line))
            }
            Tok::Str(s) => {
                self.i += 1;
                Ok(e(ExprKind::Const(Const::Str(s.into())), line))
            }
            Tok::IStr(parts) => {
                self.i += 1;
                let mut out = Vec::new();
                for p in parts {
                    match p {
                        IPart::Lit(s) => out.push(InterpPart::Lit(s.into())),
                        IPart::Code(src, l) => {
                            let toks = Lexer::new(&src, self.defines, l).tokenize().map_err(|le| ParseError { file: self.file.to_string(), line: le.line, msg: le.msg })?;
                            let mut sub = Parser { toks, i: 0, file: self.file.clone(), defines: self.defines };
                            let x = sub.expr()?;
                            if !sub.at_eof() {
                                return sub.err("junk after interpolated expression");
                            }
                            out.push(InterpPart::Expr(x));
                        }
                    }
                }
                Ok(e(ExprKind::Interp(out), line))
            }
            Tok::Op("(") => {
                if self.paren_is_arrow() {
                    let args = self.args()?;
                    self.expect("->")?;
                    let body = self.expr()?;
                    let body = e(ExprKind::Return(Some(Box::new(body))), line);
                    return Ok(e(ExprKind::Function(Rc::new(FuncDef { name: "".into(), args, body })), line));
                }
                self.i += 1;
                let x = self.expr()?;
                if self.eat_op(":") {
                    self.skip_type()?;
                    self.expect(")")?;
                    return Ok(e(ExprKind::Cast(Box::new(x)), line));
                }
                self.expect(")")?;
                Ok(x)
            }
            Tok::Op("[") => {
                self.i += 1;
                if self.is_kw("for") || self.is_kw("while") {
                    let body = self.expr()?;
                    self.expect("]")?;
                    return Ok(e(ExprKind::Comprehension(Box::new(body)), line));
                }
                let mut items = Vec::new();
                let mut map = Vec::new();
                while !self.eat_op("]") {
                    let k = self.expr()?;
                    if self.eat_op("=>") {
                        map.push((k, self.expr()?));
                    } else {
                        items.push(k);
                    }
                    if !self.eat_op(",") {
                        self.expect("]")?;
                        break;
                    }
                }
                if !map.is_empty() {
                    return Ok(e(ExprKind::Map(map), line));
                }
                Ok(e(ExprKind::Array(items), line))
            }
            Tok::Op("{") => {
                // An object literal starts `{ name:` or `{ "name":` (or is `{}`).
                let is_obj = self.is_op_at(1, "}") || (matches!(self.tok_at(1), Tok::Ident(_) | Tok::Str(_)) && self.is_op_at(2, ":"));
                self.i += 1;
                if is_obj {
                    let mut fields = Vec::new();
                    while !self.eat_op("}") {
                        let name: Rc<str> = match self.tok().clone() {
                            Tok::Ident(s) | Tok::Str(s) => {
                                self.i += 1;
                                s.into()
                            }
                            _ => return self.err("expected field name"),
                        };
                        self.expect(":")?;
                        fields.push((name, self.expr()?));
                        if !self.eat_op(",") {
                            self.expect("}")?;
                            break;
                        }
                    }
                    return Ok(e(ExprKind::Object(fields), line));
                }
                Ok(e(ExprKind::Block(self.block_body()?), line))
            }
            Tok::Ident(id) => self.ident_expr(&id, line),
            _ => self.err("unexpected token"),
        }
    }

    fn ident_expr(&mut self, id: &str, line: u32) -> R<Expr> {
        match id {
            "null" | "true" | "false" | "this" | "super" => {
                self.i += 1;
                Ok(e(
                    match id {
                        "null" => ExprKind::Const(Const::Null),
                        "true" => ExprKind::Const(Const::Bool(true)),
                        "false" => ExprKind::Const(Const::Bool(false)),
                        "this" => ExprKind::This,
                        _ => ExprKind::Super,
                    },
                    line,
                ))
            }
            "var" | "final" => {
                self.i += 1;
                let mut vars = Vec::new();
                loop {
                    let name = self.ident()?;
                    self.opt_type()?;
                    let init = if self.eat_op("=") { Some(self.expr()?) } else { None };
                    vars.push((name, init));
                    if !self.eat_op(",") {
                        break;
                    }
                }
                Ok(e(ExprKind::Var(vars), line))
            }
            "function" => {
                self.i += 1;
                let f = self.function_rest(false)?;
                let named = !f.name.is_empty();
                let name = f.name.clone();
                let fx = e(ExprKind::Function(Rc::new(f)), line);
                if named {
                    // A local named function is a variable holding it.
                    return Ok(e(ExprKind::Var(vec![(name, Some(fx))]), line));
                }
                Ok(fx)
            }
            "if" => {
                self.i += 1;
                self.expect("(")?;
                let c = self.expr()?;
                self.expect(")")?;
                let a = self.expr()?;
                // `if (c) a; else b`
                let save = self.i;
                self.eat_op(";");
                let b = if self.eat_kw("else") {
                    Some(Box::new(self.expr()?))
                } else {
                    self.i = save;
                    None
                };
                Ok(e(ExprKind::If(Box::new(c), Box::new(a), b), line))
            }
            "while" => {
                self.i += 1;
                self.expect("(")?;
                let c = self.expr()?;
                self.expect(")")?;
                let body = self.expr()?;
                Ok(e(ExprKind::While(Box::new(c), Box::new(body), false), line))
            }
            "do" => {
                self.i += 1;
                let body = self.expr()?;
                self.eat_op(";");
                if !self.eat_kw("while") {
                    return self.err("expected while after do");
                }
                self.expect("(")?;
                let c = self.expr()?;
                self.expect(")")?;
                Ok(e(ExprKind::While(Box::new(c), Box::new(body), true), line))
            }
            "for" => {
                self.i += 1;
                self.expect("(")?;
                let a = self.ident()?;
                let (k, v) = if self.eat_op("=>") { (Some(a), self.ident()?) } else { (None, a) };
                if !self.eat_kw("in") {
                    return self.err("expected in");
                }
                let it = self.expr()?;
                self.expect(")")?;
                let body = self.expr()?;
                Ok(e(ExprKind::For(k, v, Box::new(it), Box::new(body)), line))
            }
            "return" => {
                self.i += 1;
                let v = if self.is_op(";") || self.is_op("}") || self.is_op(")") || self.is_op(",") || self.is_kw("case") || self.is_kw("default") {
                    None
                } else {
                    Some(Box::new(self.expr()?))
                };
                Ok(e(ExprKind::Return(v), line))
            }
            "break" => {
                self.i += 1;
                Ok(e(ExprKind::Break, line))
            }
            "continue" => {
                self.i += 1;
                Ok(e(ExprKind::Continue, line))
            }
            "throw" => {
                self.i += 1;
                Ok(e(ExprKind::Throw(Box::new(self.expr()?)), line))
            }
            "untyped" | "inline" => {
                self.i += 1;
                self.expr()
            }
            "cast" => {
                self.i += 1;
                if self.is_op("(") && !self.paren_is_arrow() {
                    // `cast(x, T)` or `cast (x)`.
                    self.i += 1;
                    let x = self.expr()?;
                    if self.eat_op(",") {
                        self.skip_type()?;
                    }
                    self.expect(")")?;
                    let x = self.postfix(x)?;
                    return Ok(e(ExprKind::Cast(Box::new(x)), line));
                }
                let x = self.unary()?;
                Ok(e(ExprKind::Cast(Box::new(x)), line))
            }
            "new" => {
                self.i += 1;
                let path = self.path()?;
                if self.is_op("<") {
                    self.skip_type_params()?;
                }
                let args = self.call_args()?;
                Ok(e(ExprKind::New(path.into(), args), line))
            }
            "try" => {
                self.i += 1;
                let body = self.expr()?;
                let mut catches = Vec::new();
                loop {
                    let save = self.i;
                    self.eat_op(";");
                    if !self.eat_kw("catch") {
                        self.i = save;
                        break;
                    }
                    self.expect("(")?;
                    let name = self.ident()?;
                    self.opt_type()?;
                    self.expect(")")?;
                    catches.push((name, self.expr()?));
                }
                Ok(e(ExprKind::Try(Box::new(body), catches), line))
            }
            "switch" => {
                self.i += 1;
                let subject = self.expr()?;
                self.expect("{")?;
                let mut cases = Vec::new();
                let mut default = None;
                while !self.eat_op("}") {
                    if self.eat_kw("case") {
                        let mut values = Vec::new();
                        let mut wildcard = false;
                        loop {
                            if matches!(self.tok(), Tok::Ident(s) if s == "_") {
                                self.i += 1;
                                wildcard = true;
                            } else {
                                values.push(self.ternary()?);
                            }
                            if !self.eat_op(",") && !self.eat_op("|") {
                                break;
                            }
                        }
                        let guard = if self.eat_kw("if") {
                            self.expect("(")?;
                            let g = self.expr()?;
                            self.expect(")")?;
                            Some(g)
                        } else {
                            None
                        };
                        self.expect(":")?;
                        let body = self.case_body()?;
                        if wildcard && guard.is_none() {
                            default = Some(Box::new(body));
                        } else {
                            cases.push(Case { values, guard, body });
                        }
                    } else if self.eat_kw("default") {
                        self.expect(":")?;
                        default = Some(Box::new(self.case_body()?));
                    } else {
                        return self.err("expected case or default");
                    }
                }
                Ok(e(ExprKind::Switch(Box::new(subject), cases, default), line))
            }
            _ => {
                self.i += 1;
                // `x -> expr` arrow function.
                if self.is_op("->") && !KEYWORDS.contains(&id) {
                    self.i += 1;
                    let body = self.expr()?;
                    let body = e(ExprKind::Return(Some(Box::new(body))), line);
                    let args = vec![Arg { name: id.into(), opt: false, default: None }];
                    return Ok(e(ExprKind::Function(Rc::new(FuncDef { name: "".into(), args, body })), line));
                }
                Ok(e(ExprKind::Ident(id.into()), line))
            }
        }
    }

    fn skip_type_params(&mut self) -> R<()> {
        self.expect("<")?;
        let mut depth = 1;
        while depth > 0 {
            if self.at_eof() {
                return self.err("unclosed type parameters");
            }
            if self.is_op("<") {
                depth += 1;
            } else if self.is_op(">") {
                depth -= 1;
            }
            self.i += 1;
        }
        Ok(())
    }

    fn case_body(&mut self) -> R<Expr> {
        let line = self.line();
        let mut body = Vec::new();
        while !(self.is_kw("case") || self.is_kw("default") || self.is_op("}")) {
            if self.at_eof() {
                return self.err("unclosed switch");
            }
            if self.eat_op(";") {
                continue;
            }
            body.push(self.expr()?);
            self.eat_op(";");
        }
        Ok(e(ExprKind::Block(body), line))
    }
}

/// Binary operator precedence, lowest first (Haxe's table; `...` sits between `&&` and comparisons).
const LEVELS: [&[(&str, Option<BinOp>)]; 10] = [
    &[("??", Some(BinOp::NullCoal))],
    &[("||", Some(BinOp::Or))],
    &[("&&", Some(BinOp::And))],
    &[("...", None)],
    &[],
    &[("==", Some(BinOp::Eq)), ("!=", Some(BinOp::Ne)), ("<", Some(BinOp::Lt)), ("<=", Some(BinOp::Le)), (">", Some(BinOp::Gt)), (">=", Some(BinOp::Ge))],
    &[("|", Some(BinOp::BitOr)), ("&", Some(BinOp::BitAnd)), ("^", Some(BinOp::BitXor))],
    &[("<<", Some(BinOp::Shl))],
    &[("+", Some(BinOp::Add)), ("-", Some(BinOp::Sub))],
    &[("*", Some(BinOp::Mul)), ("/", Some(BinOp::Div)), ("%", Some(BinOp::Mod))],
];
