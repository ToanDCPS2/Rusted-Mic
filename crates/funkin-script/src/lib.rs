//! An interpreter for HScript `.hxc` files (Polymod's scripted classes), so
//! the game's own scripts, and mods, run unchanged on top of the Rust engine.
//!
//! The engine provides a [`Host`] that resolves the Haxe classes and objects
//! scripts use (`FlxG`, `PlayState.instance`, a stage's props, ...).

pub mod ast;
mod builtins;
pub mod interp;
pub mod lexer;
pub mod parser;
pub mod value;

use std::rc::Rc;

pub use builtins::random;
pub use interp::{Instance, Interp, ScriptClass};
pub use parser::{parse_expr, parse_module, ParseError};
pub use value::{HostRef, Value};

/// The engine side: everything a script reaches that isn't script code.
///
/// `get` and `call` return `Ok(None)` for a name the object doesn't have,
/// so the interpreter can fall back (or report it); `Err` is a real failure.
pub trait Host {
    /// A global or imported class (`FlxG`, `Paths`, `PlayState`, `FlxTween`, ...).
    /// `path` is the full import path when the file imported it.
    fn global(&mut self, interp: &mut Interp, name: &str, path: Option<&str>) -> Option<Value>;
    fn get(&mut self, interp: &mut Interp, obj: HostRef, field: &str) -> Result<Option<Value>, String>;
    /// Returns false if the object has no such (settable) field.
    fn set(&mut self, interp: &mut Interp, obj: HostRef, field: &str, v: Value) -> Result<bool, String>;
    fn call(&mut self, interp: &mut Interp, obj: HostRef, method: &str, args: Vec<Value>) -> Result<Option<Value>, String>;
    /// `new T(args)` for an engine class.
    fn construct(&mut self, interp: &mut Interp, class: &str, args: Vec<Value>) -> Result<Value, String>;
    /// `super(args)` in a script class whose nearest engine ancestor is `base`:
    /// make the engine object behind `this` (or return null if there is none).
    fn construct_base(&mut self, interp: &mut Interp, base: &str, this: &Rc<Instance>, args: Vec<Value>) -> Result<Value, String>;
    /// The items of an iterable engine object (a group's members).
    fn iterate(&mut self, _interp: &mut Interp, _obj: HostRef) -> Option<Vec<Value>> {
        None
    }
    /// `v is T` for engine objects (and script instances' engine bases).
    fn is_type(&mut self, _interp: &mut Interp, _v: &Value, _ty: &str) -> bool {
        false
    }
    /// The class name of an engine class value (for `Std.isOfType(x, FlxSprite)`).
    fn type_name(&mut self, _interp: &mut Interp, _v: &Value) -> Option<String> {
        None
    }
    /// Calling an engine value as a function (`stub()`); `None` means it isn't callable.
    fn call_self(&mut self, _interp: &mut Interp, _obj: HostRef, _args: Vec<Value>) -> Result<Option<Value>, String> {
        Ok(None)
    }
    /// An inert stand-in for an unbound engine API: it acts as null in arithmetic.
    fn is_stub(&self, _obj: HostRef) -> bool {
        false
    }
    /// For error messages.
    fn describe(&self, obj: HostRef) -> String {
        format!("engine object {}:{}", obj.kind, obj.id)
    }
}

/// A host with no engine behind it (pure-script tests, tools).
pub struct NoHost;

impl Host for NoHost {
    fn global(&mut self, _: &mut Interp, _: &str, _: Option<&str>) -> Option<Value> {
        None
    }
    fn get(&mut self, _: &mut Interp, _: HostRef, _: &str) -> Result<Option<Value>, String> {
        Ok(None)
    }
    fn set(&mut self, _: &mut Interp, _: HostRef, _: &str, _: Value) -> Result<bool, String> {
        Ok(false)
    }
    fn call(&mut self, _: &mut Interp, _: HostRef, _: &str, _: Vec<Value>) -> Result<Option<Value>, String> {
        Ok(None)
    }
    fn construct(&mut self, _: &mut Interp, class: &str, _: Vec<Value>) -> Result<Value, String> {
        Err(format!("unknown class {class}"))
    }
    fn construct_base(&mut self, _: &mut Interp, _: &str, _: &Rc<Instance>, _: Vec<Value>) -> Result<Value, String> {
        Ok(Value::Null)
    }
}

#[cfg(test)]
mod eval_tests {
    use super::*;

    fn run(src: &str) -> String {
        let mut i = Interp::new(Default::default());
        match i.eval_str(&mut NoHost, src) {
            Ok(v) => v.to_string(),
            Err(e) => format!("error: {e}"),
        }
    }

    #[test]
    fn expressions() {
        assert_eq!(run("1 + 2 * 3"), "7");
        assert_eq!(run("7 / 2"), "3.5");
        assert_eq!(run("var a = [1, 2, 3]; a.push(4); a.length"), "4");
        assert_eq!(run("var s = 'x'; var n = 2; '$s=${n + 1}'"), "x=3");
        assert_eq!(run("var t = 0; for (i in 0...5) t += i; t"), "10");
        assert_eq!(run("[for (i in 0...3) i * i].join(',')"), "0,1,4");
        assert_eq!(run("var m = ['a' => 1]; m.set('b', 2); m.get('b') + m['a']"), "3");
        assert_eq!(run("var f = (x) -> x * 2; f(21)"), "42");
        assert_eq!(run("var x = null; x ?? 5"), "5");
        assert_eq!(run("switch ('b') { case 'a': 1; case 'b', 'c': 2; default: 3; }"), "2");
        assert_eq!(run("var o = {a: 1, b: {c: 'd'}}; o.b?.c"), "d");
        assert_eq!(run("var n = null; n?.x"), "null");
        assert_eq!(run("try { throw 'boom'; } catch (e:Dynamic) { 'caught ' + e; }"), "caught boom");
        assert_eq!(run("var i = 0; while (true) { i++; if (i > 3) break; } i"), "4");
        assert_eq!(run("0xFF >> 4"), "15");
        assert_eq!(run("Math.max(2, 3.5)"), "3.5");
        assert_eq!(run("Std.int(3.9) + Std.parseInt('12px')"), "15");
        assert_eq!(run("var a = [3, 1, 2]; a.sort((x, y) -> x - y); a"), "[1,2,3]");
        assert_eq!(run("'Hello'.toUpperCase().substr(1, 3)"), "ELL");
        assert_eq!(run("function fib(n) { return n < 2 ? n : fib(n - 1) + fib(n - 2); } fib(10)"), "55");
        assert_eq!(run("var a = [1, 2]; [for (x in a) if (x > 1) x]"), "[2]");
    }

    #[test]
    fn classes() {
        let src = r#"
            enum Mood { HAPPY; SAD; }
            class Base {
                var n:Int = 1;
                public function new(x) { n = x; }
                function hello() { return 'base ' + n; }
                static var count = 10;
                static function twice(v) return v * 2;
            }
            class Child extends Base {
                var mood = Mood.HAPPY;
                function new() { super(5); }
                override function hello() { return 'child/' + super.hello() + '/' + Base.twice(count); }
                function moody() { return switch (mood) { case HAPPY: 'yay'; case SAD: 'boo'; }; }
            }
        "#;
        let mut i = Interp::new(Default::default());
        i.load(src, "test.hxc").unwrap();
        i.link(&mut NoHost);
        let c = i.class("Child").unwrap();
        let inst = i.instantiate(&mut NoHost, &c, vec![]).unwrap();
        let v = Value::Instance(inst);
        assert_eq!(i.call_method(&mut NoHost, &v, "hello", vec![]).unwrap().to_string(), "child/base 5/20");
        assert_eq!(i.call_method(&mut NoHost, &v, "moody", vec![]).unwrap().to_string(), "yay");
        assert!(i.is_type(&mut NoHost, &v, "Base"));
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "hxc") {
                out.push(p);
            }
        }
    }

    /// Every script the game ships parses (needs `RUSTED_MIC_TEST_ASSETS`).
    #[test]
    #[ignore]
    fn parses_game_scripts() {
        let root = std::env::var("RUSTED_MIC_TEST_ASSETS").expect("RUSTED_MIC_TEST_ASSETS");
        let mut files = Vec::new();
        walk(&std::path::Path::new(&root).join("scripts"), &mut files);
        assert!(!files.is_empty());
        let defines: HashSet<String> = ["desktop", "sys", "cpp"].iter().map(|s| s.to_string()).collect();
        let mut bad = Vec::new();
        for f in &files {
            let src = std::fs::read_to_string(f).unwrap();
            if let Err(e) = crate::parse_module(&src, &f.display().to_string(), &defines) {
                bad.push(e.to_string());
            }
        }
        assert!(bad.is_empty(), "{} of {} failed:\n{}", bad.len(), files.len(), bad.join("\n"));
    }
}

#[cfg(test)]
mod parse_tests {
    use crate::ast::*;
    use std::collections::HashSet;

    #[test]
    #[ignore]
    fn class_census() {
        let root = std::env::var("RUSTED_MIC_TEST_ASSETS").expect("RUSTED_MIC_TEST_ASSETS");
        let defines = HashSet::new();
        let (mut classes, mut fields, mut files) = (0, 0, 0);
        let mut stack = vec![std::path::Path::new(&root).join("scripts")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files += 1;
                    let m = crate::parse_module(&std::fs::read_to_string(&p).unwrap(), "", &defines).unwrap();
                    classes += m.classes.len();
                    fields += m.classes.iter().map(|c| c.fields.len()).sum::<usize>();
                    if p.ends_with("spookyMansion.hxc") {
                        let c = &m.classes[0];
                        eprintln!("{} extends {:?}: {:?}", c.name, c.extends, c.fields.iter().map(|f| (&f.name, matches!(f.kind, FieldKind::Func(_)))).collect::<Vec<_>>());
                    }
                }
            }
        }
        eprintln!("{files} files, {classes} classes, {fields} fields");
    }
}
