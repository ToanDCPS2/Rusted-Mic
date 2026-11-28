//! The syntax tree for the Haxe subset HScript (Polymod's `hscript-ex`) accepts.

use std::rc::Rc;

#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    UShr,
    NullCoal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    BitNot,
    Inc,
    Dec,
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub line: u32,
}

#[derive(Debug, Clone)]
pub enum InterpPart {
    Lit(Rc<str>),
    Expr(Expr),
}

#[derive(Debug, Clone)]
pub struct Arg {
    pub name: Rc<str>,
    pub opt: bool,
    pub default: Option<Expr>,
}

#[derive(Debug)]
pub struct FuncDef {
    pub name: Rc<str>,
    pub args: Vec<Arg>,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub struct Case {
    pub values: Vec<Expr>,
    pub guard: Option<Expr>,
    pub body: Expr,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Const(Const),
    Ident(Rc<str>),
    This,
    Super,
    /// `a.b` or (`safe`) `a?.b`.
    Field(Box<Expr>, Rc<str>, bool),
    Index(Box<Expr>, Box<Expr>),
    Call(Box<Expr>, Vec<Expr>),
    New(Rc<str>, Vec<Expr>),
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// `a = b` (`None`) or `a op= b`.
    Assign(Option<BinOp>, Box<Expr>, Box<Expr>),
    /// Prefix (`false`) or postfix (`true`) unary operator.
    Unop(UnOp, bool, Box<Expr>),
    Ternary(Box<Expr>, Box<Expr>, Box<Expr>),
    If(Box<Expr>, Box<Expr>, Option<Box<Expr>>),
    Block(Vec<Expr>),
    Var(Vec<(Rc<str>, Option<Expr>)>),
    Function(Rc<FuncDef>),
    Return(Option<Box<Expr>>),
    Break,
    Continue,
    Throw(Box<Expr>),
    Try(Box<Expr>, Vec<(Rc<str>, Expr)>),
    /// `while (c) body` or (`true`) `do body while (c)`.
    While(Box<Expr>, Box<Expr>, bool),
    /// `for (v in iter)` or `for (k => v in iter)`.
    For(Option<Rc<str>>, Rc<str>, Box<Expr>, Box<Expr>),
    /// `a...b`, only meaningful as a `for` iterator.
    Range(Box<Expr>, Box<Expr>),
    Switch(Box<Expr>, Vec<Case>, Option<Box<Expr>>),
    Array(Vec<Expr>),
    Map(Vec<(Expr, Expr)>),
    Object(Vec<(Rc<str>, Expr)>),
    /// `[for (...) ...]` / `[while (...) ...]`: the loop's value expressions are collected.
    Comprehension(Box<Expr>),
    Interp(Vec<InterpPart>),
    /// `cast x`, `cast(x, T)`, `(x : T)`.
    Cast(Box<Expr>),
    Is(Box<Expr>, Rc<str>),
}

#[derive(Debug)]
pub enum FieldKind {
    Var(Option<Expr>),
    Func(Rc<FuncDef>),
}

#[derive(Debug)]
pub struct Field {
    pub name: Rc<str>,
    pub is_static: bool,
    /// `var x(get, set)`: reads call `get_x`, writes call `set_x`.
    pub getter: bool,
    pub setter: bool,
    pub kind: FieldKind,
    pub line: u32,
}

#[derive(Debug)]
pub struct ClassDecl {
    pub name: Rc<str>,
    pub extends: Option<Rc<str>>,
    pub fields: Vec<Field>,
    /// Imports of the file the class is in: short name → full path.
    pub imports: Vec<(Rc<str>, Rc<str>)>,
    pub file: Rc<str>,
}

#[derive(Debug)]
pub struct EnumDecl {
    pub name: Rc<str>,
    pub values: Vec<Rc<str>>,
}

#[derive(Debug, Default)]
pub struct Module {
    pub imports: Vec<(Rc<str>, Rc<str>)>,
    pub classes: Vec<Rc<ClassDecl>>,
    pub enums: Vec<Rc<EnumDecl>>,
}
