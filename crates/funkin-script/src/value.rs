//! Runtime values. Haxe is dynamically typed under HScript, so everything is
//! one enum; engine objects are opaque [`HostRef`] handles the [`Host`](crate::Host) resolves.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::rc::Rc;

use crate::ast::FuncDef;
use crate::interp::{Env, Instance, Interp, ScriptClass};
use crate::Host;

/// An engine object: `kind` says what it is (the host's own numbering), `id` which one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct HostRef {
    pub kind: u32,
    pub id: u64,
}

pub type NativeFn = dyn Fn(&mut Interp, &mut dyn Host, Option<&Value>, Vec<Value>) -> Result<Value, String>;

/// A script function with what it closes over.
pub struct Closure {
    pub def: Rc<FuncDef>,
    pub env: Rc<Env>,
    pub this: Option<Rc<Instance>>,
    /// The class whose method this is (for `super` and statics).
    pub class: Option<Rc<ScriptClass>>,
}

pub type Shared<T> = Rc<RefCell<T>>;

#[derive(Clone, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(Rc<str>),
    Array(Shared<Vec<Value>>),
    /// `Map<K, V>` (keys compared by value), in insertion order.
    Map(Shared<MapData>),
    /// An anonymous structure `{a: 1}`.
    Object(Shared<Vec<(Rc<str>, Value)>>),
    Func(Rc<Closure>),
    Native(Rc<NativeFn>),
    Instance(Rc<Instance>),
    /// A script class, for statics and `new`.
    Class(Rc<ScriptClass>),
    /// An enum value (`PupilState.NORMAL`): enum name, constructor name.
    Enum(Rc<str>, Rc<str>),
    Host(HostRef),
}

#[derive(Default, Clone)]
pub struct MapData {
    pub keys: Vec<Value>,
    pub values: Vec<Value>,
    index: HashMap<String, usize>,
}

impl MapData {
    fn key(k: &Value) -> String {
        match k {
            Value::Str(s) => format!("s{s}"),
            Value::Int(i) => format!("i{i}"),
            Value::Float(f) if f.fract() == 0.0 => format!("i{}", *f as i64),
            other => format!("o{}", other.identity()),
        }
    }
    pub fn get(&self, k: &Value) -> Option<&Value> {
        self.index.get(&Self::key(k)).map(|&i| &self.values[i])
    }
    pub fn set(&mut self, k: Value, v: Value) {
        let key = Self::key(&k);
        match self.index.get(&key) {
            Some(&i) => self.values[i] = v,
            None => {
                self.index.insert(key, self.keys.len());
                self.keys.push(k);
                self.values.push(v);
            }
        }
    }
    pub fn remove(&mut self, k: &Value) -> bool {
        let Some(i) = self.index.remove(&Self::key(k)) else { return false };
        self.keys.remove(i);
        self.values.remove(i);
        for v in self.index.values_mut() {
            if *v > i {
                *v -= 1;
            }
        }
        true
    }
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

impl Value {
    pub fn str(s: &str) -> Self {
        Value::Str(s.into())
    }
    pub fn array(v: Vec<Value>) -> Self {
        Value::Array(Rc::new(RefCell::new(v)))
    }
    /// An anonymous object, e.g. a script event.
    pub fn object(fields: Vec<(&str, Value)>) -> Self {
        Value::Object(Rc::new(RefCell::new(fields.into_iter().map(|(k, v)| (Rc::from(k), v)).collect())))
    }
    pub fn native(f: impl Fn(&mut Interp, &mut dyn Host, Option<&Value>, Vec<Value>) -> Result<Value, String> + 'static) -> Self {
        Value::Native(Rc::new(f))
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            _ => true,
        }
    }

    /// As a number (null counts as 0, as the C++ target reads it).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Null => Some(0.0),
            Value::Bool(b) => Some(*b as i64 as f64),
            _ => None,
        }
    }
    pub fn f(&self) -> f64 {
        self.as_f64().unwrap_or(0.0)
    }
    pub fn i(&self) -> i64 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i64,
            Value::Bool(b) => *b as i64,
            _ => 0,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    /// A field of an anonymous object.
    pub fn field(&self, name: &str) -> Option<Value> {
        match self {
            Value::Object(o) => o.borrow().iter().find(|(k, _)| &**k == name).map(|(_, v)| v.clone()),
            _ => None,
        }
    }
    pub fn set_field(&self, name: &str, v: Value) -> bool {
        match self {
            Value::Object(o) => {
                let mut o = o.borrow_mut();
                match o.iter_mut().find(|(k, _)| &**k == name) {
                    Some(slot) => slot.1 = v,
                    None => o.push((name.into(), v)),
                }
                true
            }
            _ => false,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "Bool",
            Value::Int(_) => "Int",
            Value::Float(_) => "Float",
            Value::Str(_) => "String",
            Value::Array(_) => "Array",
            Value::Map(_) => "Map",
            Value::Object(_) => "Object",
            Value::Func(_) | Value::Native(_) => "Function",
            Value::Instance(_) => "Instance",
            Value::Class(_) => "Class",
            Value::Enum(..) => "Enum",
            Value::Host(_) => "Host",
        }
    }

    /// Pointer identity for reference types (for map keys and `==`).
    fn identity(&self) -> String {
        match self {
            Value::Array(a) => format!("a{:p}", Rc::as_ptr(a)),
            Value::Map(a) => format!("m{:p}", Rc::as_ptr(a)),
            Value::Object(a) => format!("o{:p}", Rc::as_ptr(a)),
            Value::Func(a) => format!("f{:p}", Rc::as_ptr(a)),
            Value::Native(a) => format!("n{:p}", Rc::as_ptr(a) as *const u8),
            Value::Instance(a) => format!("i{:p}", Rc::as_ptr(a)),
            Value::Class(a) => format!("c{:p}", Rc::as_ptr(a)),
            Value::Enum(e, n) => format!("e{e}.{n}"),
            Value::Host(h) => format!("h{}:{}", h.kind, h.id),
            other => other.to_string(),
        }
    }

    /// Haxe `==`: numbers by value, strings by content, everything else by reference.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Null, _) | (_, Value::Null) => false,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (a @ (Value::Int(_) | Value::Float(_)), b @ (Value::Int(_) | Value::Float(_))) => a.f() == b.f(),
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b) && a.identity() == b.identity(),
        }
    }
}

/// `Std.string`.
impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => write!(f, "null"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => {
                if x.is_nan() {
                    write!(f, "NaN")
                } else if x.is_infinite() {
                    write!(f, "{}", if *x > 0.0 { "Infinity" } else { "-Infinity" })
                } else if x.fract() == 0.0 && x.abs() < 1e15 {
                    write!(f, "{}", *x as i64)
                } else {
                    write!(f, "{x}")
                }
            }
            Value::Str(s) => write!(f, "{s}"),
            Value::Array(a) => {
                write!(f, "[")?;
                for (i, v) in a.borrow().iter().enumerate() {
                    if i > 0 {
                        write!(f, ",")?;
                    }
                    write!(f, "{v}")?;
                }
                write!(f, "]")
            }
            Value::Map(m) => {
                let m = m.borrow();
                write!(f, "[")?;
                for (i, (k, v)) in m.keys.iter().zip(&m.values).enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k} => {v}")?;
                }
                write!(f, "]")
            }
            Value::Object(o) => {
                write!(f, "{{ ")?;
                for (i, (k, v)) in o.borrow().iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{k}: {v}")?;
                }
                write!(f, " }}")
            }
            Value::Func(c) => write!(f, "<function {}>", c.def.name),
            Value::Native(_) => write!(f, "<function>"),
            Value::Instance(i) => write!(f, "{}", i.class.decl.name),
            Value::Class(c) => write!(f, "{}", c.decl.name),
            Value::Enum(_, n) => write!(f, "{n}"),
            Value::Host(h) => write!(f, "<host {}:{}>", h.kind, h.id),
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Str(s) => write!(f, "{s:?}"),
            other => write!(f, "{other}"),
        }
    }
}

impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(v as i64)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}
impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Float(v as f64)
    }
}
impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Str(v.into())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Str(v.into())
    }
}
