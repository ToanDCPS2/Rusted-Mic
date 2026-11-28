//! The tree-walking evaluator and Polymod-style scripted classes: a script
//! class extends an engine class (`Stage`, `Song`, `FunkinSprite`, ...) or
//! another script class; what the script doesn't define falls through to the
//! engine object behind it (`this.base`).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::ast::*;
use crate::value::{Closure, HostRef, MapData, Value};
use crate::Host;

/// Lexical scope.
pub struct Env {
    vars: RefCell<Vec<(Rc<str>, Rc<RefCell<Value>>)>>,
    parent: Option<Rc<Env>>,
}

impl Env {
    pub fn root() -> Rc<Env> {
        Rc::new(Env { vars: RefCell::new(Vec::new()), parent: None })
    }
    fn child(parent: &Rc<Env>) -> Rc<Env> {
        Rc::new(Env { vars: RefCell::new(Vec::new()), parent: Some(parent.clone()) })
    }
    fn lookup(&self, name: &str) -> Option<Rc<RefCell<Value>>> {
        if let Some((_, v)) = self.vars.borrow().iter().rev().find(|(k, _)| &**k == name) {
            return Some(v.clone());
        }
        self.parent.as_ref()?.lookup(name)
    }
    fn define(&self, name: Rc<str>, v: Value) {
        self.vars.borrow_mut().push((name, Rc::new(RefCell::new(v))));
    }
}

pub struct ScriptClass {
    pub decl: Rc<ClassDecl>,
    pub parent: RefCell<Option<Rc<ScriptClass>>>,
    /// The nearest engine (non-script) ancestor, e.g. `Stage`.
    pub host_base: RefCell<Option<Rc<str>>>,
    pub statics: RefCell<HashMap<Rc<str>, Value>>,
    methods: HashMap<Rc<str>, Rc<FuncDef>>,
    static_methods: HashMap<Rc<str>, Rc<FuncDef>>,
}

/// Which accessor of a property.
#[derive(Clone, Copy, PartialEq)]
enum Acc {
    Get,
    Set,
}

impl ScriptClass {
    /// `var name(get, _)` / `var name(_, set)` on this class or an ancestor.
    fn has_accessor(self: &Rc<Self>, name: &str, acc: Acc) -> bool {
        let mut c = Some(self.clone());
        while let Some(cls) = c {
            if cls.decl.fields.iter().any(|f| !f.is_static && &*f.name == name && if acc == Acc::Get { f.getter } else { f.setter }) {
                return true;
            }
            c = cls.parent.borrow().clone();
        }
        false
    }
    fn find_method(self: &Rc<Self>, name: &str) -> Option<(Rc<ScriptClass>, Rc<FuncDef>)> {
        let mut c = Some(self.clone());
        while let Some(cls) = c {
            if let Some(f) = cls.methods.get(name) {
                return Some((cls.clone(), f.clone()));
            }
            c = cls.parent.borrow().clone();
        }
        None
    }
    fn find_static(self: &Rc<Self>, name: &str) -> Option<Value> {
        let mut c = Some(self.clone());
        while let Some(cls) = c {
            if let Some(v) = cls.statics.borrow().get(name) {
                return Some(v.clone());
            }
            if let Some(f) = cls.static_methods.get(name) {
                return Some(Value::Func(Rc::new(Closure { def: f.clone(), env: Env::root(), this: None, class: Some(cls.clone()) })));
            }
            c = cls.parent.borrow().clone();
        }
        None
    }
    fn set_static(self: &Rc<Self>, name: &str, v: &Value) -> bool {
        let mut c = Some(self.clone());
        while let Some(cls) = c {
            if let Some(slot) = cls.statics.borrow_mut().get_mut(name) {
                *slot = v.clone();
                return true;
            }
            c = cls.parent.borrow().clone();
        }
        false
    }
    /// This class or a script ancestor is named `name`, or its engine base is.
    pub fn is_a(self: &Rc<Self>, name: &str) -> bool {
        let mut c = Some(self.clone());
        while let Some(cls) = c {
            if &*cls.decl.name == name {
                return true;
            }
            c = cls.parent.borrow().clone();
        }
        self.host_base.borrow().as_deref() == Some(name)
    }
    pub fn has_method(self: &Rc<Self>, name: &str) -> bool {
        self.find_method(name).is_some()
    }
}

pub struct Instance {
    pub class: Rc<ScriptClass>,
    pub fields: RefCell<HashMap<Rc<str>, Value>>,
    /// The engine object this instance extends (from `super(...)`), or null.
    pub base: RefCell<Value>,
}

impl Instance {
    pub fn base_ref(&self) -> Option<HostRef> {
        match &*self.base.borrow() {
            Value::Host(h) => Some(*h),
            _ => None,
        }
    }
}

/// Non-local control flow.
pub enum Flow {
    Return(Value),
    Break,
    Continue,
    Throw(Value),
    Error(String),
}

type R = Result<Value, Flow>;

#[derive(Clone)]
struct Frame {
    env: Rc<Env>,
    this: Option<Rc<Instance>>,
    class: Option<Rc<ScriptClass>>,
}

impl Frame {
    fn file(&self) -> &str {
        self.class.as_ref().map(|c| &*c.decl.file).unwrap_or("<script>")
    }
    fn imports(&self) -> &[(Rc<str>, Rc<str>)] {
        self.class.as_ref().map(|c| c.decl.imports.as_slice()).unwrap_or(&[])
    }
    fn scope(&self) -> Frame {
        Frame { env: Env::child(&self.env), this: self.this.clone(), class: self.class.clone() }
    }
}

pub struct Interp {
    pub classes: HashMap<Rc<str>, Rc<ScriptClass>>,
    enums: HashMap<Rc<str>, Rc<EnumDecl>>,
    /// Enum constructor name → enum name (for bare `case NORMAL:`).
    enum_values: HashMap<Rc<str>, Rc<str>>,
    pub defines: HashSet<String>,
    builtins: HashMap<&'static str, Value>,
    /// Property accessors running now: (instance, name, is_get).
    in_accessor: Vec<(usize, String, bool)>,
    depth: u32,
    /// Messages already logged (unknown engine fields are reported once).
    warned: HashSet<String>,
    /// Don't log (while the engine probes classes by constructing them).
    pub quiet: bool,
}

fn err(msg: impl Into<String>) -> Flow {
    Flow::Error(msg.into())
}

impl Interp {
    pub fn new(defines: HashSet<String>) -> Self {
        Self {
            classes: HashMap::new(),
            enums: HashMap::new(),
            enum_values: HashMap::new(),
            defines,
            builtins: crate::builtins::globals(),
            in_accessor: Vec::new(),
            depth: 0,
            warned: HashSet::new(),
            quiet: false,
        }
    }

    /// Log `msg` the first time it comes up.
    pub fn warn_once(&mut self, msg: String) {
        if !self.quiet && self.warned.insert(msg.clone()) {
            log::warn!("script: {msg}");
        }
    }

    /// Parse a file and register its classes and enums (call [`Interp::link`] after the last one).
    pub fn load(&mut self, src: &str, file: &str) -> Result<(), crate::ParseError> {
        let m = crate::parse_module(src, file, &self.defines)?;
        for en in m.enums {
            for v in &en.values {
                self.enum_values.insert(v.clone(), en.name.clone());
            }
            self.enums.insert(en.name.clone(), en);
        }
        for c in m.classes {
            let mut methods = HashMap::new();
            let mut static_methods = HashMap::new();
            for f in &c.fields {
                if let FieldKind::Func(d) = &f.kind {
                    if f.is_static {
                        static_methods.insert(f.name.clone(), d.clone());
                    } else {
                        methods.insert(f.name.clone(), d.clone());
                    }
                }
            }
            let cls = ScriptClass { decl: c.clone(), parent: RefCell::new(None), host_base: RefCell::new(None), statics: RefCell::new(HashMap::new()), methods, static_methods };
            if self.classes.insert(c.name.clone(), Rc::new(cls)).is_some() {
                log::warn!("script class {} defined twice; the later one ({file}) wins", c.name);
            }
        }
        Ok(())
    }

    /// Resolve `extends` between script classes and run static initialisers.
    pub fn link(&mut self, host: &mut dyn Host) {
        let all: Vec<Rc<ScriptClass>> = self.classes.values().cloned().collect();
        for c in &all {
            *c.parent.borrow_mut() = c.decl.extends.as_ref().and_then(|p| self.classes.get(p).cloned());
        }
        for c in &all {
            // Walk up to the first ancestor that isn't a script class.
            let mut cur = c.clone();
            let mut seen = 0;
            loop {
                let next = cur.parent.borrow().clone();
                match next {
                    Some(p) if seen < 64 => {
                        cur = p;
                        seen += 1;
                    }
                    _ => break,
                }
            }
            *c.host_base.borrow_mut() = cur.decl.extends.clone();
        }
        for c in &all {
            let frame = Frame { env: Env::root(), this: None, class: Some(c.clone()) };
            for f in &c.decl.fields {
                if let (true, FieldKind::Var(init)) = (f.is_static, &f.kind) {
                    let v = match init {
                        Some(x) => self.eval(host, &frame, x).unwrap_or_else(|fl| {
                            log::warn!("{}: static {}: {}", c.decl.file, f.name, flow_msg(fl));
                            Value::Null
                        }),
                        None => Value::Null,
                    };
                    c.statics.borrow_mut().insert(f.name.clone(), v);
                }
            }
        }
    }

    /// Script classes whose engine ancestor is `base` (`Stage`, `Song`, ...).
    pub fn classes_extending(&self, base: &str) -> Vec<Rc<ScriptClass>> {
        let mut v: Vec<_> = self.classes.values().filter(|c| c.host_base.borrow().as_deref() == Some(base)).cloned().collect();
        v.sort_by(|a, b| a.decl.name.cmp(&b.decl.name));
        v
    }

    pub fn class(&self, name: &str) -> Option<Rc<ScriptClass>> {
        self.classes.get(name).cloned()
    }

    // ------------------------------------------------------------ public API

    /// `new Class(args)`.
    pub fn instantiate(&mut self, host: &mut dyn Host, cls: &Rc<ScriptClass>, args: Vec<Value>) -> Result<Rc<Instance>, String> {
        self.instantiate_inner(host, cls, args).map_err(flow_msg)
    }

    /// Call `obj.name(args)`.
    pub fn call_method(&mut self, host: &mut dyn Host, obj: &Value, name: &str, args: Vec<Value>) -> Result<Value, String> {
        self.call_method_inner(host, obj, name, args).map_err(flow_msg)
    }

    /// Call a function value (a callback the engine kept).
    pub fn call(&mut self, host: &mut dyn Host, f: &Value, args: Vec<Value>) -> Result<Value, String> {
        self.call_value(host, f, None, args).map_err(flow_msg)
    }

    pub fn get(&mut self, host: &mut dyn Host, obj: &Value, name: &str) -> Result<Value, String> {
        self.get_field(host, obj, name, false).map_err(flow_msg)
    }

    pub fn set(&mut self, host: &mut dyn Host, obj: &Value, name: &str, v: Value) -> Result<(), String> {
        self.set_field(host, obj, name, v).map_err(flow_msg)
    }

    /// Run a snippet (tests and debugging).
    pub fn eval_str(&mut self, host: &mut dyn Host, src: &str) -> Result<Value, String> {
        let x = crate::parse_expr(src, &self.defines).map_err(|e| e.to_string())?;
        let frame = Frame { env: Env::root(), this: None, class: None };
        match self.eval(host, &frame, &x) {
            Ok(v) | Err(Flow::Return(v)) => Ok(v),
            Err(f) => Err(flow_msg(f)),
        }
    }

    // ---------------------------------------------------------------- classes

    fn instantiate_inner(&mut self, host: &mut dyn Host, cls: &Rc<ScriptClass>, args: Vec<Value>) -> Result<Rc<Instance>, Flow> {
        let inst = Rc::new(Instance { class: cls.clone(), fields: RefCell::new(HashMap::new()), base: RefCell::new(Value::Null) });
        // Field initialisers, ancestors first.
        let mut chain = vec![cls.clone()];
        loop {
            let next = chain.last().unwrap().parent.borrow().clone();
            let Some(p) = next else { break };
            if chain.len() > 64 {
                break;
            }
            chain.push(p);
        }
        for c in chain.iter().rev() {
            let frame = Frame { env: Env::root(), this: Some(inst.clone()), class: Some(c.clone()) };
            for f in &c.decl.fields {
                if let (false, FieldKind::Var(init)) = (f.is_static, &f.kind) {
                    let v = match init {
                        Some(x) => self.eval(host, &frame, x)?,
                        None => Value::Null,
                    };
                    inst.fields.borrow_mut().insert(f.name.clone(), v);
                }
            }
        }
        self.construct(host, &inst, cls, args)?;
        Ok(inst)
    }

    /// Run `cls`'s constructor (or its nearest ancestor's) on `inst`.
    fn construct(&mut self, host: &mut dyn Host, inst: &Rc<Instance>, cls: &Rc<ScriptClass>, args: Vec<Value>) -> Result<(), Flow> {
        if let Some(f) = cls.methods.get("new") {
            let c = Closure { def: f.clone(), env: Env::root(), this: Some(inst.clone()), class: Some(cls.clone()) };
            self.call_closure(host, &c, args)?;
            return Ok(());
        }
        self.super_construct(host, inst, cls, args)
    }

    /// `super(args)` from a constructor of `cls`.
    fn super_construct(&mut self, host: &mut dyn Host, inst: &Rc<Instance>, cls: &Rc<ScriptClass>, args: Vec<Value>) -> Result<(), Flow> {
        let parent = cls.parent.borrow().clone();
        match parent {
            Some(p) => self.construct(host, inst, &p, args),
            None => {
                let base = cls.decl.extends.clone().unwrap_or_else(|| "".into());
                if base.is_empty() {
                    return Ok(());
                }
                let b = host.construct_base(self, &base, inst, args).map_err(err)?;
                *inst.base.borrow_mut() = b;
                Ok(())
            }
        }
    }

    // -------------------------------------------------------------- calling

    fn call_closure(&mut self, host: &mut dyn Host, c: &Closure, args: Vec<Value>) -> R {
        if self.depth > 256 {
            return Err(err("stack overflow"));
        }
        let env = Env::child(&c.env);
        let frame = Frame { env: env.clone(), this: c.this.clone(), class: c.class.clone() };
        let mut args = args.into_iter();
        for a in &c.def.args {
            let v = match args.next() {
                Some(v) if !v.is_null() || a.default.is_none() => v,
                _ => match &a.default {
                    Some(d) => self.eval(host, &frame, d)?,
                    None => Value::Null,
                },
            };
            env.define(a.name.clone(), v);
        }
        self.depth += 1;
        let r = self.eval(host, &frame, &c.def.body);
        self.depth -= 1;
        match r {
            Ok(_) => Ok(Value::Null),
            Err(Flow::Return(v)) => Ok(v),
            Err(Flow::Break | Flow::Continue) => Ok(Value::Null),
            Err(e) => Err(e),
        }
    }

    pub(crate) fn call_value(&mut self, host: &mut dyn Host, f: &Value, this: Option<&Value>, args: Vec<Value>) -> R {
        match f {
            Value::Func(c) => self.call_closure(host, c, args),
            Value::Native(n) => n(self, host, this, args).map_err(err),
            Value::Null => Err(err("called a null function")),
            Value::Host(h) => match host.call_self(self, *h, args).map_err(err)? {
                Some(v) => Ok(v),
                None => Err(err(format!("{} is not a function", host.describe(*h)))),
            },
            other => Err(err(format!("{} is not a function", other.type_name()))),
        }
    }

    fn bound_method(inst: &Rc<Instance>, name: &str) -> Option<Value> {
        let (cls, def) = inst.class.find_method(name)?;
        Some(Value::Func(Rc::new(Closure { def, env: Env::root(), this: Some(inst.clone()), class: Some(cls) })))
    }

    fn call_method_inner(&mut self, host: &mut dyn Host, obj: &Value, name: &str, args: Vec<Value>) -> R {
        match obj {
            Value::Instance(inst) => {
                if let Some((cls, def)) = inst.class.find_method(name) {
                    let c = Closure { def, env: Env::root(), this: Some(inst.clone()), class: Some(cls) };
                    return self.call_closure(host, &c, args);
                }
                let field = inst.fields.borrow().get(name).cloned();
                if let Some(f) = field {
                    return self.call_value(host, &f, Some(obj), args);
                }
                // Polymod's scripted-class reflection: scriptCall(name, args), scriptGet, scriptSet.
                match name {
                    "scriptCall" => {
                        let method = args.first().map(|v| v.to_string()).unwrap_or_default();
                        let list = match args.get(1) {
                            Some(Value::Array(a)) => a.borrow().clone(),
                            _ => vec![],
                        };
                        return self.call_method_inner(host, obj, &method, list);
                    }
                    "scriptGet" => {
                        let f = args.first().map(|v| v.to_string()).unwrap_or_default();
                        return self.get_field(host, obj, &f, true);
                    }
                    "scriptSet" => {
                        let f = args.first().map(|v| v.to_string()).unwrap_or_default();
                        let v = args.get(1).cloned().unwrap_or(Value::Null);
                        self.set_field(host, obj, &f, v.clone())?;
                        return Ok(v);
                    }
                    _ => {}
                }
                let base = inst.base.borrow().clone();
                if let Value::Host(h) = base {
                    return self.host_call(host, h, name, args);
                }
                Err(err(format!("{} has no method {name}", inst.class.decl.name)))
            }
            Value::Host(h) => self.host_call(host, *h, name, args),
            Value::Object(_) => {
                let f = obj.field(name).ok_or_else(|| err(format!("object has no method {name}")))?;
                self.call_value(host, &f, Some(obj), args)
            }
            Value::Class(c) => {
                let f = c.find_static(name).ok_or_else(|| err(format!("{} has no static {name}", c.decl.name)))?;
                self.call_value(host, &f, None, args)
            }
            Value::Null => Err(err(format!("called {name} on null"))),
            _ => crate::builtins::method(self, host, obj, name, args).map_err(err),
        }
    }

    fn host_call(&mut self, host: &mut dyn Host, h: HostRef, name: &str, args: Vec<Value>) -> R {
        match host.call(self, h, name, args.clone()).map_err(err)? {
            Some(v) => Ok(v),
            None => {
                // A field holding a function (`animation.onFinish.add` style objects return values).
                match host.get(self, h, name).map_err(err)? {
                    Some(f @ (Value::Func(_) | Value::Native(_))) => self.call_value(host, &f, Some(&Value::Host(h)), args),
                    _ => Err(err(format!("no method {name} on {}", host.describe(h)))),
                }
            }
        }
    }

    // --------------------------------------------------------------- fields

    /// Run `get_name()` / `set_name(v)` if `name` is a property of `inst` and
    /// this isn't already inside that accessor (where the raw field is meant).
    fn accessor(&mut self, host: &mut dyn Host, inst: &Rc<Instance>, name: &str, acc: Acc, v: Option<Value>) -> Option<R> {
        if !inst.class.has_accessor(name, acc) {
            return None;
        }
        let key = (Rc::as_ptr(inst) as usize, name.to_owned(), acc == Acc::Get);
        if self.in_accessor.contains(&key) {
            return None;
        }
        let m = format!("{}_{name}", if acc == Acc::Get { "get" } else { "set" });
        inst.class.find_method(&m)?;
        self.in_accessor.push(key.clone());
        let r = self.call_method_inner(host, &Value::Instance(inst.clone()), &m, v.into_iter().collect());
        self.in_accessor.retain(|k| *k != key);
        Some(r)
    }

    fn get_field(&mut self, host: &mut dyn Host, obj: &Value, name: &str, safe: bool) -> R {
        match obj {
            Value::Null if safe => Ok(Value::Null),
            Value::Null => Err(err(format!("read .{name} of null"))),
            Value::Instance(inst) => {
                if let Some(r) = self.accessor(host, inst, name, Acc::Get, None) {
                    return r;
                }
                if let Some(v) = inst.fields.borrow().get(name) {
                    return Ok(v.clone());
                }
                if let Some(m) = Self::bound_method(inst, name) {
                    return Ok(m);
                }
                let base = inst.base.borrow().clone();
                if let Value::Host(h) = base {
                    if let Some(v) = host.get(self, h, name).map_err(err)? {
                        return Ok(v);
                    }
                    return Err(err(format!("no field {name} on {} ({})", inst.class.decl.name, host.describe(h))));
                }
                Err(err(format!("{} has no field {name}", inst.class.decl.name)))
            }
            Value::Host(h) => match host.get(self, *h, name).map_err(err)? {
                Some(v) => Ok(v),
                None => Err(err(format!("no field {name} on {}", host.describe(*h)))),
            },
            Value::Object(_) => Ok(obj.field(name).unwrap_or(Value::Null)),
            Value::Class(c) => c.find_static(name).ok_or_else(|| err(format!("{} has no static {name}", c.decl.name))),
            Value::Enum(en, v) if v.is_empty() => {
                let decl = self.enums.get(en).cloned();
                match decl.and_then(|d| d.values.iter().find(|x| &***x == name).cloned()) {
                    Some(v) => Ok(Value::Enum(en.clone(), v)),
                    None => Err(err(format!("{en} has no value {name}"))),
                }
            }
            _ => crate::builtins::field(obj, name).ok_or_else(|| err(format!("{} has no field {name}", obj.type_name()))),
        }
    }

    fn set_field(&mut self, host: &mut dyn Host, obj: &Value, name: &str, v: Value) -> Result<(), Flow> {
        match obj {
            Value::Instance(inst) => {
                if let Some(r) = self.accessor(host, inst, name, Acc::Set, Some(v.clone())) {
                    return r.map(|_| ());
                }
                if let Some(slot) = inst.fields.borrow_mut().get_mut(name) {
                    *slot = v;
                    return Ok(());
                }
                let base = inst.base.borrow().clone();
                if let Value::Host(h) = base {
                    // A field the engine object doesn't take stays on the script object.
                    if !host.set(self, h, name, v.clone()).map_err(err)? {
                        inst.fields.borrow_mut().insert(name.into(), v);
                    }
                    return Ok(());
                }
                inst.fields.borrow_mut().insert(name.into(), v);
                Ok(())
            }
            Value::Host(h) => self.host_set(host, *h, name, v),
            Value::Object(_) => {
                obj.set_field(name, v);
                Ok(())
            }
            Value::Class(c) => {
                if !c.set_static(name, &v) {
                    c.statics.borrow_mut().insert(name.into(), v);
                }
                Ok(())
            }
            Value::Null => Err(err(format!("set .{name} of null"))),
            other => Err(err(format!("can't set .{name} on {}", other.type_name()))),
        }
    }

    fn host_set(&mut self, host: &mut dyn Host, h: HostRef, name: &str, v: Value) -> Result<(), Flow> {
        if !host.set(self, h, name, v).map_err(err)? {
            let msg = format!("can't set {name} on {}", host.describe(h));
            self.warn_once(msg);
        }
        Ok(())
    }

    fn get_index(&mut self, obj: &Value, idx: &Value) -> R {
        Ok(match obj {
            Value::Array(a) => {
                let i = idx.i();
                a.borrow().get(i as usize).filter(|_| i >= 0).cloned().unwrap_or(Value::Null)
            }
            Value::Map(m) => m.borrow().get(idx).cloned().unwrap_or(Value::Null),
            Value::Str(s) => s.chars().nth(idx.i() as usize).map(|c| Value::from(c.to_string())).unwrap_or(Value::Null),
            Value::Object(_) => obj.field(&idx.to_string()).unwrap_or(Value::Null),
            Value::Null => return Err(err("indexed null")),
            other => return Err(err(format!("can't index {}", other.type_name()))),
        })
    }

    fn set_index(&mut self, obj: &Value, idx: Value, v: Value) -> Result<(), Flow> {
        match obj {
            Value::Array(a) => {
                let i = idx.i().max(0) as usize;
                let mut a = a.borrow_mut();
                if i >= a.len() {
                    a.resize(i + 1, Value::Null);
                }
                a[i] = v;
            }
            Value::Map(m) => m.borrow_mut().set(idx, v),
            Value::Object(_) => {
                obj.set_field(&idx.to_string(), v);
            }
            other => return Err(err(format!("can't index-assign {}", other.type_name()))),
        }
        Ok(())
    }

    // ---------------------------------------------------------- identifiers

    fn lookup(&mut self, host: &mut dyn Host, f: &Frame, name: &str) -> R {
        if let Some(v) = f.env.lookup(name) {
            return Ok(v.borrow().clone());
        }
        if let Some(inst) = &f.this {
            if let Some(r) = self.accessor(host, inst, name, Acc::Get, None) {
                return r;
            }
            if let Some(v) = inst.fields.borrow().get(name) {
                return Ok(v.clone());
            }
            if let Some(m) = Self::bound_method(inst, name) {
                return Ok(m);
            }
        }
        if let Some(c) = &f.class {
            if let Some(v) = c.find_static(name) {
                return Ok(v);
            }
        }
        if let Some(c) = self.classes.get(name) {
            return Ok(Value::Class(c.clone()));
        }
        if self.enums.contains_key(name) {
            return Ok(Value::Enum(name.into(), "".into()));
        }
        if let Some(en) = self.enum_values.get(name) {
            return Ok(Value::Enum(en.clone(), name.into()));
        }
        if let Some(inst) = &f.this {
            if let Some(h) = inst.base_ref() {
                match host.get(self, h, name).map_err(err)? {
                    Some(Value::Host(s)) if host.is_stub(s) => {}
                    Some(v) => return Ok(v),
                    None => {}
                }
            }
        }
        if let Some(v) = self.builtins.get(name) {
            return Ok(v.clone());
        }
        let path = f.imports().iter().find(|(a, _)| &**a == name).map(|(_, p)| p.clone());
        if let Some(v) = host.global(self, name, path.as_deref()) {
            return Ok(v);
        }
        Err(err(format!("unknown identifier {name}")))
    }

    fn assign_ident(&mut self, host: &mut dyn Host, f: &Frame, name: &str, v: Value) -> Result<(), Flow> {
        if let Some(slot) = f.env.lookup(name) {
            *slot.borrow_mut() = v;
            return Ok(());
        }
        if let Some(inst) = &f.this {
            if let Some(r) = self.accessor(host, inst, name, Acc::Set, Some(v.clone())) {
                return r.map(|_| ());
            }
            if let Some(slot) = inst.fields.borrow_mut().get_mut(name) {
                *slot = v;
                return Ok(());
            }
        }
        if let Some(c) = &f.class {
            if c.set_static(name, &v) {
                return Ok(());
            }
        }
        if let Some(inst) = &f.this {
            if let Some(h) = inst.base_ref() {
                // An engine field when the engine object takes it; otherwise hscript
                // keeps the undeclared variable (`rainShaderFilter = ...` in phillyBlazin).
                if host.set(self, h, name, v.clone()).map_err(err)? {
                    return Ok(());
                }
            }
            inst.fields.borrow_mut().insert(name.into(), v);
            return Ok(());
        }
        Err(err(format!("assigned unknown variable {name}")))
    }

    // ------------------------------------------------------------ evaluation

    fn eval(&mut self, host: &mut dyn Host, f: &Frame, e: &Expr) -> R {
        self.eval_inner(host, f, e).map_err(|fl| match fl {
            Flow::Error(m) if !m.starts_with('@') => Flow::Error(format!("@{}:{}: {m}", f.file(), e.line)),
            other => other,
        })
    }

    fn eval_inner(&mut self, host: &mut dyn Host, f: &Frame, e: &Expr) -> R {
        use ExprKind as K;
        match &e.kind {
            K::Const(c) => Ok(match c {
                Const::Null => Value::Null,
                Const::Bool(b) => Value::Bool(*b),
                Const::Int(i) => Value::Int(*i),
                Const::Float(x) => Value::Float(*x),
                Const::Str(s) => Value::Str(s.clone()),
            }),
            K::Ident(n) => self.lookup(host, f, n),
            K::This => Ok(f.this.clone().map(Value::Instance).unwrap_or(Value::Null)),
            K::Super => Ok(f.this.as_ref().map(|i| i.base.borrow().clone()).unwrap_or(Value::Null)),
            K::Field(obj, name, safe) => {
                if let K::Super = obj.kind {
                    return self.super_field(host, f, name);
                }
                let o = self.eval(host, f, obj)?;
                self.get_field(host, &o, name, *safe)
            }
            K::Index(obj, idx) => {
                let o = self.eval(host, f, obj)?;
                let i = self.eval(host, f, idx)?;
                self.get_index(&o, &i)
            }
            K::Call(callee, args) => self.eval_call(host, f, callee, args),
            K::New(path, args) => {
                let args = self.eval_args(host, f, args)?;
                let short = path.rsplit('.').next().unwrap_or(path);
                if let Some(c) = self.classes.get(short).cloned() {
                    return self.instantiate_inner(host, &c, args).map(Value::Instance);
                }
                match short {
                    "Map" | "StringMap" | "IntMap" | "ObjectMap" => return Ok(Value::Map(Rc::new(RefCell::new(MapData::default())))),
                    "Array" => return Ok(Value::array(vec![])),
                    _ => {}
                }
                host.construct(self, short, args).map_err(err)
            }
            K::Bin(op, a, b) => self.eval_bin(host, f, *op, a, b),
            K::Assign(op, target, rhs) => {
                if *op == Some(BinOp::NullCoal) {
                    let cur = self.eval(host, f, target)?;
                    if !cur.is_null() {
                        return Ok(cur);
                    }
                    let v = self.eval(host, f, rhs)?;
                    self.store(host, f, target, v.clone())?;
                    return Ok(v);
                }
                let v = self.eval(host, f, rhs)?;
                match op {
                    None => {
                        self.store(host, f, target, v.clone())?;
                        Ok(v)
                    }
                    Some(op) => {
                        let v = unstub(host, v);
                        self.modify(host, f, target, |old| crate::builtins::binop(*op, &old, &v).map(|n| (n.clone(), n)))
                    }
                }
            }
            K::Unop(op, postfix, x) => match op {
                UnOp::Neg => {
                    let v = self.eval(host, f, x)?;
                    Ok(match v {
                        Value::Int(i) => Value::Int(-i),
                        other => Value::Float(-other.f()),
                    })
                }
                UnOp::Not => Ok(Value::Bool(!self.cond(host, f, x)?)),
                UnOp::BitNot => Ok(Value::Int(!self.eval(host, f, x)?.i())),
                UnOp::Inc | UnOp::Dec => {
                    let d = if *op == UnOp::Inc { 1 } else { -1 };
                    let post = *postfix;
                    self.modify(host, f, x, |old| {
                        let new = match &old {
                            Value::Float(v) => Value::Float(v + d as f64),
                            other => Value::Int(other.i() + d),
                        };
                        Ok((new.clone(), if post { old } else { new }))
                    })
                }
            },
            K::Ternary(c, a, b) | K::If(c, a, Some(b)) => {
                if self.cond(host, f, c)? {
                    self.eval(host, f, a)
                } else {
                    self.eval(host, f, b)
                }
            }
            K::If(c, a, None) => {
                if self.cond(host, f, c)? {
                    self.eval(host, f, a)?;
                }
                Ok(Value::Null)
            }
            K::Block(body) => {
                let inner = f.scope();
                let mut last = Value::Null;
                for x in body {
                    last = self.eval(host, &inner, x)?;
                }
                Ok(last)
            }
            K::Var(vars) => {
                for (name, init) in vars {
                    // Define first so a local function can call itself.
                    f.env.define(name.clone(), Value::Null);
                    if let Some(x) = init {
                        let v = self.eval(host, f, x)?;
                        if let Some(slot) = f.env.lookup(name) {
                            *slot.borrow_mut() = v;
                        }
                    }
                }
                Ok(Value::Null)
            }
            K::Function(def) => Ok(Value::Func(Rc::new(Closure { def: def.clone(), env: f.env.clone(), this: f.this.clone(), class: f.class.clone() }))),
            K::Return(v) => {
                let v = match v {
                    Some(x) => self.eval(host, f, x)?,
                    None => Value::Null,
                };
                Err(Flow::Return(v))
            }
            K::Break => Err(Flow::Break),
            K::Continue => Err(Flow::Continue),
            K::Throw(x) => {
                let v = self.eval(host, f, x)?;
                Err(Flow::Throw(v))
            }
            K::Try(body, catches) => match self.eval(host, f, body) {
                Err(Flow::Throw(v)) => self.catch(host, f, catches, v),
                Err(Flow::Error(m)) => self.catch(host, f, catches, Value::from(m.trim_start_matches('@').to_owned())),
                other => other,
            },
            K::While(c, body, do_while) => {
                let mut first = *do_while;
                let mut guard = 0u64;
                while first || self.cond(host, f, c)? {
                    first = false;
                    guard += 1;
                    if guard > 10_000_000 {
                        return Err(err("loop ran too long"));
                    }
                    match self.eval(host, f, body) {
                        Ok(_) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Null)
            }
            K::For(k, v, it, body) => {
                let items = self.iter_items(host, f, it)?;
                for (key, val) in items {
                    let inner = f.scope();
                    if let Some(k) = k {
                        inner.env.define(k.clone(), key);
                    }
                    inner.env.define(v.clone(), val);
                    match self.eval(host, &inner, body) {
                        Ok(_) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(Value::Null)
            }
            K::Range(..) => Err(err("a range (a...b) can only be iterated")),
            K::Switch(subject, cases, default) => {
                let s = self.eval(host, f, subject)?;
                for c in cases {
                    let mut hit = false;
                    for v in &c.values {
                        let cv = self.eval(host, f, v)?;
                        if cv.equals(&s) {
                            hit = true;
                            break;
                        }
                    }
                    if hit && c.values.is_empty() {
                        hit = true;
                    }
                    if hit {
                        if let Some(g) = &c.guard {
                            if !self.cond(host, f, g)? {
                                continue;
                            }
                        }
                        return self.eval(host, f, &c.body);
                    }
                }
                match default {
                    Some(d) => self.eval(host, f, d),
                    None => Ok(Value::Null),
                }
            }
            K::Array(items) => Ok(Value::array(self.eval_args(host, f, items)?)),
            K::Map(pairs) => {
                let mut m = MapData::default();
                for (k, v) in pairs {
                    let k = self.eval(host, f, k)?;
                    let v = self.eval(host, f, v)?;
                    m.set(k, v);
                }
                Ok(Value::Map(Rc::new(RefCell::new(m))))
            }
            K::Object(fields) => {
                let mut out = Vec::new();
                for (k, v) in fields {
                    out.push((k.clone(), self.eval(host, f, v)?));
                }
                Ok(Value::Object(Rc::new(RefCell::new(out))))
            }
            K::Comprehension(body) => {
                let mut out = Vec::new();
                self.collect(host, f, body, &mut out)?;
                Ok(Value::array(out))
            }
            K::Interp(parts) => {
                let mut s = String::new();
                for p in parts {
                    match p {
                        InterpPart::Lit(l) => s.push_str(l),
                        InterpPart::Expr(x) => {
                            let v = self.eval(host, f, x)?;
                            s.push_str(&v.to_string());
                        }
                    }
                }
                Ok(Value::from(s))
            }
            K::Cast(x) => self.eval(host, f, x),
            K::Is(x, ty) => {
                let v = self.eval(host, f, x)?;
                Ok(Value::Bool(self.is_type(host, &v, ty)))
            }
        }
    }

    pub fn is_type(&mut self, host: &mut dyn Host, v: &Value, ty: &str) -> bool {
        match (v, ty) {
            (Value::Null, _) => false,
            (_, "Dynamic") => true,
            (Value::Int(_), "Int" | "Float") | (Value::Float(_), "Float") | (Value::Bool(_), "Bool") | (Value::Str(_), "String") | (Value::Array(_), "Array") => true,
            (Value::Map(_), "Map" | "IMap" | "StringMap" | "IntMap") => true,
            (Value::Func(_) | Value::Native(_), "Function") => true,
            (Value::Enum(e, _), t) => &**e == t,
            (Value::Instance(i), t) => {
                if i.class.is_a(t) {
                    return true;
                }
                let b = i.base.borrow().clone();
                !b.is_null() && host.is_type(self, &b, t)
            }
            (Value::Host(_), t) => host.is_type(self, v, t),
            _ => false,
        }
    }

    fn catch(&mut self, host: &mut dyn Host, f: &Frame, catches: &[(Rc<str>, Expr)], v: Value) -> R {
        let Some((name, body)) = catches.first() else { return Err(Flow::Throw(v)) };
        let inner = f.scope();
        inner.env.define(name.clone(), v);
        self.eval(host, &inner, body)
    }

    fn collect(&mut self, host: &mut dyn Host, f: &Frame, e: &Expr, out: &mut Vec<Value>) -> Result<(), Flow> {
        match &e.kind {
            ExprKind::For(k, v, it, body) => {
                let items = self.iter_items(host, f, it)?;
                for (key, val) in items {
                    let inner = f.scope();
                    if let Some(k) = k {
                        inner.env.define(k.clone(), key);
                    }
                    inner.env.define(v.clone(), val);
                    match self.collect(host, &inner, body, out) {
                        Ok(()) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(())
            }
            ExprKind::While(c, body, _) => {
                while self.cond(host, f, c)? {
                    match self.collect(host, f, body, out) {
                        Ok(()) | Err(Flow::Continue) => {}
                        Err(Flow::Break) => break,
                        Err(e) => return Err(e),
                    }
                }
                Ok(())
            }
            ExprKind::If(c, a, b) => {
                if self.cond(host, f, c)? {
                    self.collect(host, f, a, out)
                } else if let Some(b) = b {
                    self.collect(host, f, b, out)
                } else {
                    Ok(())
                }
            }
            ExprKind::Block(body) if body.len() == 1 => self.collect(host, f, &body[0], out),
            _ => {
                out.push(self.eval(host, f, e)?);
                Ok(())
            }
        }
    }

    /// (key, value) pairs a `for` walks.
    fn iter_items(&mut self, host: &mut dyn Host, f: &Frame, it: &Expr) -> Result<Vec<(Value, Value)>, Flow> {
        if let ExprKind::Range(a, b) = &it.kind {
            let a = self.eval(host, f, a)?.i();
            let b = self.eval(host, f, b)?.i();
            return Ok((a..b).map(|i| (Value::Int(i), Value::Int(i))).collect());
        }
        let v = self.eval(host, f, it)?;
        self.iterate(host, &v)
    }

    pub(crate) fn iterate(&mut self, host: &mut dyn Host, v: &Value) -> Result<Vec<(Value, Value)>, Flow> {
        Ok(match v {
            Value::Array(a) => a.borrow().iter().enumerate().map(|(i, x)| (Value::Int(i as i64), x.clone())).collect(),
            Value::Map(m) => {
                let m = m.borrow();
                m.keys.iter().cloned().zip(m.values.iter().cloned()).collect()
            }
            Value::Str(s) => s.chars().enumerate().map(|(i, c)| (Value::Int(i as i64), Value::from(c.to_string()))).collect(),
            Value::Object(o) => o.borrow().iter().map(|(k, v)| (Value::Str(k.clone()), v.clone())).collect(),
            Value::Host(h) => match host.iterate(self, *h) {
                Some(items) => items.into_iter().enumerate().map(|(i, x)| (Value::Int(i as i64), x)).collect(),
                None => return Err(err(format!("can't iterate {}", host.describe(*h)))),
            },
            Value::Instance(i) => {
                let b = i.base.borrow().clone();
                match b {
                    Value::Host(_) => return self.iterate(host, &b),
                    _ => return Err(err("can't iterate a script instance")),
                }
            }
            Value::Null => return Err(err("iterated null")),
            other => return Err(err(format!("can't iterate {}", other.type_name()))),
        })
    }

    fn eval_args(&mut self, host: &mut dyn Host, f: &Frame, args: &[Expr]) -> Result<Vec<Value>, Flow> {
        let mut out = Vec::with_capacity(args.len());
        for a in args {
            out.push(self.eval(host, f, a)?);
        }
        Ok(out)
    }

    fn eval_call(&mut self, host: &mut dyn Host, f: &Frame, callee: &Expr, args: &[Expr]) -> R {
        match &callee.kind {
            ExprKind::Super => {
                let args = self.eval_args(host, f, args)?;
                let (Some(inst), Some(cls)) = (&f.this, &f.class) else { return Err(err("super() outside a constructor")) };
                self.super_construct(host, inst, cls, args)?;
                Ok(Value::Null)
            }
            ExprKind::Field(obj, name, safe) => {
                if let ExprKind::Super = obj.kind {
                    let args = self.eval_args(host, f, args)?;
                    return self.super_call(host, f, name, args);
                }
                let o = self.eval(host, f, obj)?;
                if *safe && o.is_null() {
                    return Ok(Value::Null);
                }
                let args = self.eval_args(host, f, args)?;
                self.call_method_inner(host, &o, name, args)
            }
            ExprKind::Ident(name) => {
                // Locals, then methods of `this` (script, then engine), then globals.
                if let Some(v) = f.env.lookup(name) {
                    let v = v.borrow().clone();
                    let args = self.eval_args(host, f, args)?;
                    return self.call_value(host, &v, None, args);
                }
                if let Some(inst) = f.this.clone() {
                    if inst.class.find_method(name).is_some() || inst.fields.borrow().contains_key(&**name) {
                        let args = self.eval_args(host, f, args)?;
                        return self.call_method_inner(host, &Value::Instance(inst), name, args);
                    }
                    let shadowed = self.lookup_local_only(f, name).is_some() || self.classes.contains_key(&**name) || self.builtins.contains_key(&**name);
                    if let (Some(h), false) = (inst.base_ref(), shadowed) {
                        let args = self.eval_args(host, f, args)?;
                        if let Some(v) = host.call(self, h, name, args.clone()).map_err(err)? {
                            return Ok(v);
                        }
                        let fv = self.lookup(host, f, name)?;
                        return self.call_value(host, &fv, None, args);
                    }
                }
                let fv = self.lookup(host, f, name)?;
                let args = self.eval_args(host, f, args)?;
                self.call_value(host, &fv, None, args)
            }
            _ => {
                let fv = self.eval(host, f, callee)?;
                let args = self.eval_args(host, f, args)?;
                self.call_value(host, &fv, None, args)
            }
        }
    }

    /// Statics of the current class (which take precedence over engine methods).
    fn lookup_local_only(&self, f: &Frame, name: &str) -> Option<Value> {
        f.class.as_ref().and_then(|c| c.find_static(name))
    }

    /// `super.name(args)`: the parent script class's method, else the engine's (missing ones do nothing).
    fn super_call(&mut self, host: &mut dyn Host, f: &Frame, name: &str, args: Vec<Value>) -> R {
        let (Some(inst), Some(cls)) = (&f.this, &f.class) else { return Err(err("super outside a method")) };
        let parent = cls.parent.borrow().clone();
        if let Some(p) = parent {
            if let Some((owner, def)) = p.find_method(name) {
                let c = Closure { def, env: Env::root(), this: Some(inst.clone()), class: Some(owner) };
                return self.call_closure(host, &c, args);
            }
        }
        match inst.base_ref() {
            Some(h) => Ok(host.call(self, h, name, args).map_err(err)?.unwrap_or(Value::Null)),
            None => Ok(Value::Null),
        }
    }

    fn super_field(&mut self, host: &mut dyn Host, f: &Frame, name: &str) -> R {
        let Some(inst) = &f.this else { return Err(err("super outside a method")) };
        match inst.base_ref() {
            Some(h) => Ok(host.get(self, h, name).map_err(err)?.unwrap_or(Value::Null)),
            None => Ok(Value::Null),
        }
    }

    fn eval_bin(&mut self, host: &mut dyn Host, f: &Frame, op: BinOp, a: &Expr, b: &Expr) -> R {
        match op {
            BinOp::And => {
                let x = self.eval(host, f, a)?;
                if !unstub(host, x.clone()).truthy() {
                    return Ok(Value::Bool(false));
                }
                Ok(Value::Bool(self.cond(host, f, b)?))
            }
            BinOp::Or => {
                let x = self.eval(host, f, a)?;
                if unstub(host, x.clone()).truthy() {
                    return Ok(Value::Bool(true));
                }
                Ok(Value::Bool(self.cond(host, f, b)?))
            }
            BinOp::NullCoal => {
                let x = self.eval(host, f, a)?;
                if !x.is_null() {
                    return Ok(x);
                }
                self.eval(host, f, b)
            }
            _ => {
                let x = self.eval(host, f, a)?;
                let x = unstub(host, x);
                let y = self.eval(host, f, b)?;
                let y = unstub(host, y);
                crate::builtins::binop(op, &x, &y).map_err(err)
            }
        }
    }

    /// A condition: stubs (unbound engine values) count as false.
    fn cond(&mut self, host: &mut dyn Host, f: &Frame, e: &Expr) -> Result<bool, Flow> {
        let v = self.eval(host, f, e)?;
        Ok(unstub(host, v).truthy())
    }

    fn store(&mut self, host: &mut dyn Host, f: &Frame, target: &Expr, v: Value) -> Result<(), Flow> {
        match &target.kind {
            ExprKind::Ident(n) => self.assign_ident(host, f, n, v),
            ExprKind::Field(obj, name, _) => {
                let o = self.eval(host, f, obj)?;
                self.set_field(host, &o, name, v)
            }
            ExprKind::Index(obj, idx) => {
                let o = self.eval(host, f, obj)?;
                let i = self.eval(host, f, idx)?;
                self.set_index(&o, i, v)
            }
            _ => Err(err("invalid assignment target")),
        }
    }

    /// Read-modify-write of a target; `op` returns (new value, expression result).
    fn modify(&mut self, host: &mut dyn Host, f: &Frame, target: &Expr, op: impl FnOnce(Value) -> Result<(Value, Value), String>) -> R {
        match &target.kind {
            ExprKind::Ident(n) => {
                let old = self.lookup(host, f, n)?;
                let (new, out) = op(unstub(host, old)).map_err(err)?;
                self.assign_ident(host, f, n, new)?;
                Ok(out)
            }
            ExprKind::Field(obj, name, _) => {
                let o = self.eval(host, f, obj)?;
                let old = self.get_field(host, &o, name, false)?;
                let (new, out) = op(unstub(host, old)).map_err(err)?;
                self.set_field(host, &o, name, new)?;
                Ok(out)
            }
            ExprKind::Index(obj, idx) => {
                let o = self.eval(host, f, obj)?;
                let i = self.eval(host, f, idx)?;
                let old = self.get_index(&o, &i)?;
                let (new, out) = op(unstub(host, old)).map_err(err)?;
                self.set_index(&o, i, new)?;
                Ok(out)
            }
            _ => Err(err("invalid assignment target")),
        }
    }
}

pub(crate) fn flow_msg(f: Flow) -> String {
    match f {
        Flow::Error(m) => m.trim_start_matches('@').to_owned(),
        Flow::Throw(v) => format!("uncaught exception: {v}"),
        Flow::Return(_) => "return outside a function".into(),
        Flow::Break | Flow::Continue => "break/continue outside a loop".into(),
    }
}

/// Stubs (unbound engine APIs) act as null in arithmetic and comparisons.
fn unstub(host: &dyn Host, v: Value) -> Value {
    match v {
        Value::Host(h) if host.is_stub(h) => Value::Null,
        v => v,
    }
}
