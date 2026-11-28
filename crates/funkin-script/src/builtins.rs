//! The Haxe standard library scripts lean on: operators, `Std`, `Math`,
//! `StringTools`, `Reflect`, `Lambda`, and String/Array/Map methods.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::ast::BinOp;
use crate::interp::Interp;
use crate::value::Value;
use crate::Host;

fn num_result(x: f64, both_int: bool) -> Value {
    if both_int {
        Value::Int(x as i64)
    } else {
        Value::Float(x)
    }
}

pub fn binop(op: BinOp, a: &Value, b: &Value) -> Result<Value, String> {
    use BinOp::*;
    let ints = matches!(a, Value::Int(_) | Value::Null | Value::Bool(_)) && matches!(b, Value::Int(_) | Value::Null | Value::Bool(_));
    Ok(match op {
        Add => match (a, b) {
            (Value::Str(_), _) | (_, Value::Str(_)) => Value::from(format!("{a}{b}")),
            (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_add(*y)),
            _ => num_result(num(a)? + num(b)?, ints),
        },
        Sub => match (a, b) {
            (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_sub(*y)),
            _ => num_result(num(a)? - num(b)?, ints),
        },
        Mul => match (a, b) {
            (Value::Int(x), Value::Int(y)) => Value::Int(x.wrapping_mul(*y)),
            _ => num_result(num(a)? * num(b)?, ints),
        },
        Div => Value::Float(num(a)? / num(b)?),
        Mod => match (a, b) {
            (Value::Int(x), Value::Int(y)) if *y != 0 => Value::Int(x % y),
            _ => Value::Float(num(a)? % num(b)?),
        },
        Eq => Value::Bool(a.equals(b)),
        Ne => Value::Bool(!a.equals(b)),
        Lt | Le | Gt | Ge => {
            let ord = match (a, b) {
                (Value::Str(x), Value::Str(y)) => x.partial_cmp(y),
                _ => num(a)?.partial_cmp(&num(b)?),
            };
            let Some(o) = ord else { return Ok(Value::Bool(false)) };
            Value::Bool(match op {
                Lt => o.is_lt(),
                Le => o.is_le(),
                Gt => o.is_gt(),
                _ => o.is_ge(),
            })
        }
        BitAnd => Value::Int(a.i() & b.i()),
        BitOr => Value::Int(a.i() | b.i()),
        BitXor => Value::Int(a.i() ^ b.i()),
        Shl => Value::Int(((a.i() as i32).wrapping_shl(b.i() as u32)) as i64),
        Shr => Value::Int(((a.i() as i32) >> (b.i() & 31)) as i64),
        UShr => Value::Int(((a.i() as u32) >> (b.i() & 31)) as i64),
        And => Value::Bool(a.truthy() && b.truthy()),
        Or => Value::Bool(a.truthy() || b.truthy()),
        NullCoal => {
            if a.is_null() {
                b.clone()
            } else {
                a.clone()
            }
        }
    })
}

fn num(v: &Value) -> Result<f64, String> {
    v.as_f64().ok_or_else(|| format!("{} is not a number", v.type_name()))
}

fn arg(args: &[Value], i: usize) -> Value {
    args.get(i).cloned().unwrap_or(Value::Null)
}

fn native(f: impl Fn(&mut Interp, &mut dyn Host, Option<&Value>, Vec<Value>) -> Result<Value, String> + 'static) -> Value {
    Value::native(f)
}

fn math1(f: fn(f64) -> f64) -> Value {
    native(move |_, _, _, a| Ok(Value::Float(f(arg(&a, 0).f()))))
}

/// `Std.parseInt`: leading integer (or hex), else null.
fn parse_int(s: &str) -> Value {
    let t = s.trim();
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(h, 16).map(Value::Int).unwrap_or(Value::Null);
    }
    let end = t.char_indices().take_while(|(i, c)| c.is_ascii_digit() || (*i == 0 && (*c == '-' || *c == '+'))).map(|(i, c)| i + c.len_utf8()).last().unwrap_or(0);
    t[..end].parse().map(Value::Int).unwrap_or(Value::Null)
}

fn parse_float(s: &str) -> Value {
    let t = s.trim();
    let mut end = 0;
    for (i, c) in t.char_indices() {
        if c.is_ascii_digit() || c == '.' || c == 'e' || c == 'E' || ((c == '-' || c == '+') && (i == 0 || t[..i].ends_with(['e', 'E']))) {
            end = i + c.len_utf8();
        } else {
            break;
        }
    }
    t[..end].parse().map(Value::Float).unwrap_or(Value::Float(f64::NAN))
}

thread_local!(static SEED: std::cell::Cell<u64> = std::cell::Cell::new(
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1
));

/// A uniform random number in [0, 1).
pub fn random() -> f64 {
    SEED.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x >> 11) as f64 / (1u64 << 53) as f64
    })
}

fn obj(fields: Vec<(&str, Value)>) -> Value {
    Value::object(fields)
}

/// Global names every script can use.
pub fn globals() -> HashMap<&'static str, Value> {
    let mut g = HashMap::new();
    g.insert(
        "trace",
        native(|_, _, _, a| {
            let s: Vec<String> = a.iter().map(|v| v.to_string()).collect();
            log::info!("script trace: {}", s.join(", "));
            Ok(Value::Null)
        }),
    );
    g.insert(
        "Math",
        obj(vec![
            ("PI", Value::Float(std::f64::consts::PI)),
            ("POSITIVE_INFINITY", Value::Float(f64::INFINITY)),
            ("NEGATIVE_INFINITY", Value::Float(f64::NEG_INFINITY)),
            ("NaN", Value::Float(f64::NAN)),
            ("abs", native(|_, _, _, a| Ok(match arg(&a, 0) { Value::Int(i) => Value::Int(i.abs()), v => Value::Float(v.f().abs()) }))),
            ("floor", native(|_, _, _, a| Ok(Value::Int(arg(&a, 0).f().floor() as i64)))),
            ("ceil", native(|_, _, _, a| Ok(Value::Int(arg(&a, 0).f().ceil() as i64)))),
            ("round", native(|_, _, _, a| Ok(Value::Int((arg(&a, 0).f() + 0.5).floor() as i64)))),
            ("ffloor", math1(f64::floor)),
            ("fceil", math1(f64::ceil)),
            ("fround", native(|_, _, _, a| Ok(Value::Float((arg(&a, 0).f() + 0.5).floor())))),
            ("sqrt", math1(f64::sqrt)),
            ("sin", math1(f64::sin)),
            ("cos", math1(f64::cos)),
            ("tan", math1(f64::tan)),
            ("asin", math1(f64::asin)),
            ("acos", math1(f64::acos)),
            ("atan", math1(f64::atan)),
            ("exp", math1(f64::exp)),
            ("log", math1(f64::ln)),
            ("atan2", native(|_, _, _, a| Ok(Value::Float(arg(&a, 0).f().atan2(arg(&a, 1).f()))))),
            ("pow", native(|_, _, _, a| Ok(Value::Float(arg(&a, 0).f().powf(arg(&a, 1).f()))))),
            ("min", native(|_, _, _, a| Ok(minmax(&a, true)))),
            ("max", native(|_, _, _, a| Ok(minmax(&a, false)))),
            ("random", native(|_, _, _, _| Ok(Value::Float(random())))),
            ("isNaN", native(|_, _, _, a| Ok(Value::Bool(arg(&a, 0).f().is_nan())))),
            ("isFinite", native(|_, _, _, a| Ok(Value::Bool(arg(&a, 0).f().is_finite())))),
        ]),
    );
    g.insert(
        "Std",
        obj(vec![
            ("int", native(|_, _, _, a| Ok(Value::Int(arg(&a, 0).f() as i64)))),
            ("string", native(|_, _, _, a| Ok(Value::from(arg(&a, 0).to_string())))),
            ("parseInt", native(|_, _, _, a| Ok(parse_int(&arg(&a, 0).to_string())))),
            ("parseFloat", native(|_, _, _, a| Ok(parse_float(&arg(&a, 0).to_string())))),
            ("random", native(|_, _, _, a| Ok(Value::Int((random() * arg(&a, 0).f()) as i64)))),
            ("isOfType", native(is_of_type)),
            ("is", native(is_of_type)),
            ("downcast", native(|_, _, _, a| Ok(arg(&a, 0)))),
        ]),
    );
    let string_tools = obj(vec![
        ("startsWith", native(|i, h, _, a| str_method(i, h, &arg(&a, 0), "startsWith", a[1..].to_vec()))),
        ("endsWith", native(|i, h, _, a| str_method(i, h, &arg(&a, 0), "endsWith", a[1..].to_vec()))),
        ("contains", native(|i, h, _, a| str_method(i, h, &arg(&a, 0), "contains", a[1..].to_vec()))),
        ("replace", native(|i, h, _, a| str_method(i, h, &arg(&a, 0), "replace", a[1..].to_vec()))),
        ("trim", native(|i, h, _, a| str_method(i, h, &arg(&a, 0), "trim", vec![]))),
        ("ltrim", native(|_, _, _, a| Ok(Value::from(arg(&a, 0).to_string().trim_start().to_owned())))),
        ("rtrim", native(|_, _, _, a| Ok(Value::from(arg(&a, 0).to_string().trim_end().to_owned())))),
        ("lpad", native(|_, _, _, a| Ok(Value::from(pad(&arg(&a, 0).to_string(), &arg(&a, 1).to_string(), arg(&a, 2).i(), true))))),
        ("rpad", native(|_, _, _, a| Ok(Value::from(pad(&arg(&a, 0).to_string(), &arg(&a, 1).to_string(), arg(&a, 2).i(), false))))),
        ("hex", native(|_, _, _, a| Ok(Value::from(format!("{:0w$X}", arg(&a, 0).i() as u32, w = arg(&a, 1).i().max(0) as usize))))),
    ]);
    g.insert("StringTools", string_tools);
    g.insert(
        "Reflect",
        obj(vec![
            ("field", native(|i, h, _, a| i.get(h, &arg(&a, 0), &arg(&a, 1).to_string()).or(Ok(Value::Null)))),
            ("getProperty", native(|i, h, _, a| i.get(h, &arg(&a, 0), &arg(&a, 1).to_string()).or(Ok(Value::Null)))),
            ("setField", native(|i, h, _, a| i.set(h, &arg(&a, 0), &arg(&a, 1).to_string(), arg(&a, 2)).map(|_| Value::Null))),
            ("setProperty", native(|i, h, _, a| i.set(h, &arg(&a, 0), &arg(&a, 1).to_string(), arg(&a, 2)).map(|_| Value::Null))),
            ("hasField", native(|i, h, _, a| Ok(Value::Bool(i.get(h, &arg(&a, 0), &arg(&a, 1).to_string()).is_ok_and(|v| !v.is_null()))))),
            ("fields", native(|_, _, _, a| Ok(Value::array(match arg(&a, 0) { Value::Object(o) => o.borrow().iter().map(|(k, _)| Value::Str(k.clone())).collect(), _ => vec![] })))),
            ("callMethod", native(|i, h, _, a| {
                let args = match arg(&a, 2) { Value::Array(x) => x.borrow().clone(), _ => vec![] };
                i.call(h, &arg(&a, 1), args)
            })),
            ("isFunction", native(|_, _, _, a| Ok(Value::Bool(matches!(arg(&a, 0), Value::Func(_) | Value::Native(_)))))),
            ("compare", native(|_, _, _, a| Ok(Value::Int(compare(&arg(&a, 0), &arg(&a, 1)) as i64)))),
            ("copy", native(|_, _, _, a| Ok(match arg(&a, 0) { Value::Object(o) => Value::Object(Rc::new(RefCell::new(o.borrow().clone()))), v => v }))),
        ]),
    );
    g.insert(
        "Lambda",
        obj(vec![
            ("count", native(|i, h, _, a| array_method(i, h, &to_array(&arg(&a, 0)), "count", a[1..].to_vec()))),
            ("has", native(|i, h, _, a| array_method(i, h, &to_array(&arg(&a, 0)), "contains", a[1..].to_vec()))),
            ("exists", native(|i, h, _, a| array_method(i, h, &to_array(&arg(&a, 0)), "exists", a[1..].to_vec()))),
            ("find", native(|i, h, _, a| array_method(i, h, &to_array(&arg(&a, 0)), "find", a[1..].to_vec()))),
            ("array", native(|_, _, _, a| Ok(to_array(&arg(&a, 0))))),
            ("map", native(|i, h, _, a| array_method(i, h, &to_array(&arg(&a, 0)), "map", a[1..].to_vec()))),
            ("filter", native(|i, h, _, a| array_method(i, h, &to_array(&arg(&a, 0)), "filter", a[1..].to_vec()))),
        ]),
    );
    g.insert("Type", obj(vec![("getClassName", native(|_, _, _, a| Ok(Value::from(arg(&a, 0).to_string()))))]));
    g
}

fn to_array(v: &Value) -> Value {
    match v {
        Value::Array(_) => v.clone(),
        Value::Map(m) => Value::array(m.borrow().values.clone()),
        _ => Value::array(vec![]),
    }
}

fn is_of_type(i: &mut Interp, h: &mut dyn Host, _: Option<&Value>, a: Vec<Value>) -> Result<Value, String> {
    let ty = match arg(&a, 1) {
        Value::Class(c) => c.decl.name.to_string(),
        Value::Str(s) => s.to_string(),
        v @ Value::Host(_) => h.type_name(i, &v).unwrap_or_default(),
        Value::Object(_) => {
            // Std/Math-style builtin objects stand for their type names.
            return Ok(Value::Bool(false));
        }
        other => other.to_string(),
    };
    Ok(Value::Bool(i.is_type(h, &arg(&a, 0), &ty)))
}

fn minmax(a: &[Value], min: bool) -> Value {
    let (x, y) = (arg(a, 0), arg(a, 1));
    let ints = matches!((&x, &y), (Value::Int(_), Value::Int(_)));
    let (fx, fy) = (x.f(), y.f());
    let r = if fx.is_nan() || fy.is_nan() { f64::NAN } else if min { fx.min(fy) } else { fx.max(fy) };
    num_result(r, ints)
}

fn pad(s: &str, c: &str, len: i64, left: bool) -> String {
    let mut s = s.to_owned();
    if c.is_empty() {
        return s;
    }
    while (s.chars().count() as i64) < len {
        if left {
            s.insert_str(0, c);
        } else {
            s.push_str(c);
        }
    }
    s
}

/// `Reflect.compare`.
pub fn compare(a: &Value, b: &Value) -> i32 {
    let o = match (a, b) {
        (Value::Str(x), Value::Str(y)) => x.cmp(y),
        _ => a.f().partial_cmp(&b.f()).unwrap_or(std::cmp::Ordering::Equal),
    };
    o as i32
}

/// Fields of built-in values (`s.length`, `a.length`).
pub fn field(v: &Value, name: &str) -> Option<Value> {
    match (v, name) {
        (Value::Str(s), "length") => Some(Value::Int(s.chars().count() as i64)),
        (Value::Array(a), "length") => Some(Value::Int(a.borrow().len() as i64)),
        _ => None,
    }
}

/// Methods of built-in values.
pub fn method(i: &mut Interp, h: &mut dyn Host, v: &Value, name: &str, args: Vec<Value>) -> Result<Value, String> {
    match v {
        Value::Str(_) => str_method(i, h, v, name, args),
        Value::Array(_) => array_method(i, h, v, name, args),
        Value::Map(m) => {
            let k = arg(&args, 0);
            Ok(match name {
                "get" => m.borrow().get(&k).cloned().unwrap_or(Value::Null),
                "set" => {
                    m.borrow_mut().set(k, arg(&args, 1));
                    Value::Null
                }
                "exists" => Value::Bool(m.borrow().get(&k).is_some()),
                "remove" => Value::Bool(m.borrow_mut().remove(&k)),
                "clear" => {
                    m.borrow_mut().clear();
                    Value::Null
                }
                "keys" => Value::array(m.borrow().keys.clone()),
                "iterator" => Value::array(m.borrow().values.clone()),
                "copy" => Value::Map(Rc::new(RefCell::new(m.borrow().clone()))),
                "toString" => Value::from(v.to_string()),
                _ => return Err(format!("Map has no method {name}")),
            })
        }
        Value::Int(_) | Value::Float(_) | Value::Bool(_) if name == "toString" => Ok(Value::from(v.to_string())),
        other => Err(format!("{} has no method {name}", other.type_name())),
    }
}

fn str_method(_i: &mut Interp, _h: &mut dyn Host, v: &Value, name: &str, args: Vec<Value>) -> Result<Value, String> {
    let s = v.to_string();
    let a0 = arg(&args, 0);
    let a1 = arg(&args, 1);
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len() as i64;
    let sub = |from: i64, to: i64| -> String { chars[from.clamp(0, n) as usize..to.clamp(0, n).max(from.clamp(0, n)) as usize].iter().collect() };
    Ok(match name {
        "toUpperCase" => Value::from(s.to_uppercase()),
        "toLowerCase" => Value::from(s.to_lowercase()),
        "toString" => Value::from(s),
        "charAt" => Value::from(chars.get(a0.i() as usize).map(|c| c.to_string()).unwrap_or_default()),
        "charCodeAt" => chars.get(a0.i() as usize).map(|c| Value::Int(*c as i64)).unwrap_or(Value::Null),
        "indexOf" => {
            let from = a1.i().clamp(0, n) as usize;
            let hay: String = chars[from..].iter().collect();
            match hay.find(&*a0.to_string()) {
                Some(b) => Value::Int((from + hay[..b].chars().count()) as i64),
                None => Value::Int(-1),
            }
        }
        "lastIndexOf" => match s.rfind(&*a0.to_string()) {
            Some(b) => Value::Int(s[..b].chars().count() as i64),
            None => Value::Int(-1),
        },
        "split" => {
            let d = a0.to_string();
            if d.is_empty() {
                Value::array(chars.iter().map(|c| Value::from(c.to_string())).collect())
            } else {
                Value::array(s.split(&*d).map(Value::from).collect())
            }
        }
        "substr" => {
            let mut from = a0.i();
            if from < 0 {
                from = (n + from).max(0);
            }
            let len = if a1.is_null() { n } else { a1.i() };
            let len = if len < 0 { (n - from + len).max(0) } else { len };
            Value::from(sub(from, from + len))
        }
        "substring" => {
            let (mut a, mut b) = (a0.i().max(0), if a1.is_null() { n } else { a1.i().max(0) });
            if a > b {
                std::mem::swap(&mut a, &mut b);
            }
            Value::from(sub(a, b))
        }
        "startsWith" => Value::Bool(s.starts_with(&*a0.to_string())),
        "endsWith" => Value::Bool(s.ends_with(&*a0.to_string())),
        "contains" => Value::Bool(s.contains(&*a0.to_string())),
        "replace" => Value::from(s.replace(&*a0.to_string(), &a1.to_string())),
        "trim" => Value::from(s.trim().to_owned()),
        "isSpace" => Value::Bool(chars.get(a0.i() as usize).is_some_and(|c| c.is_whitespace())),
        _ => return Err(format!("String has no method {name}")),
    })
}

fn array_method(i: &mut Interp, h: &mut dyn Host, v: &Value, name: &str, args: Vec<Value>) -> Result<Value, String> {
    let Value::Array(a) = v else { return Err("not an array".into()) };
    let a0 = arg(&args, 0);
    let call = |i: &mut Interp, h: &mut dyn Host, f: &Value, x: Vec<Value>| i.call(h, f, x);
    Ok(match name {
        "push" => {
            a.borrow_mut().push(a0);
            Value::Int(a.borrow().len() as i64)
        }
        "pop" => a.borrow_mut().pop().unwrap_or(Value::Null),
        "shift" => {
            let mut b = a.borrow_mut();
            if b.is_empty() {
                Value::Null
            } else {
                b.remove(0)
            }
        }
        "unshift" => {
            a.borrow_mut().insert(0, a0);
            Value::Null
        }
        "insert" => {
            let mut b = a.borrow_mut();
            let len = b.len() as i64;
            let at = a0.i();
            let at = if at < 0 { (len + at).max(0) } else { at.min(len) };
            b.insert(at as usize, arg(&args, 1));
            Value::Null
        }
        "remove" => {
            let mut b = a.borrow_mut();
            match b.iter().position(|x| x.equals(&a0)) {
                Some(p) => {
                    b.remove(p);
                    Value::Bool(true)
                }
                None => Value::Bool(false),
            }
        }
        "indexOf" => Value::Int(a.borrow().iter().position(|x| x.equals(&a0)).map(|p| p as i64).unwrap_or(-1)),
        "lastIndexOf" => Value::Int(a.borrow().iter().rposition(|x| x.equals(&a0)).map(|p| p as i64).unwrap_or(-1)),
        "contains" => Value::Bool(a.borrow().iter().any(|x| x.equals(&a0))),
        "join" => Value::from(a.borrow().iter().map(|x| x.to_string()).collect::<Vec<_>>().join(&a0.to_string())),
        "concat" => {
            let mut out = a.borrow().clone();
            if let Value::Array(b) = &a0 {
                out.extend(b.borrow().iter().cloned());
            }
            Value::array(out)
        }
        "copy" => Value::array(a.borrow().clone()),
        "slice" => {
            let b = a.borrow();
            let n = b.len() as i64;
            let norm = |x: i64| if x < 0 { (n + x).max(0) } else { x.min(n) };
            let from = norm(a0.i());
            let to = if args.len() > 1 { norm(arg(&args, 1).i()) } else { n };
            Value::array(if from < to { b[from as usize..to as usize].to_vec() } else { vec![] })
        }
        "splice" => {
            let mut b = a.borrow_mut();
            let n = b.len() as i64;
            let from = if a0.i() < 0 { (n + a0.i()).max(0) } else { a0.i().min(n) } as usize;
            let len = arg(&args, 1).i().clamp(0, n - from as i64) as usize;
            Value::array(b.drain(from..from + len).collect())
        }
        "reverse" => {
            a.borrow_mut().reverse();
            Value::Null
        }
        "resize" => {
            a.borrow_mut().resize(a0.i().max(0) as usize, Value::Null);
            Value::Null
        }
        "toString" => Value::from(v.to_string()),
        "iterator" => v.clone(),
        "sort" => {
            let mut items = a.borrow().clone();
            let mut failure = None;
            // Insertion sort through the comparator (stable, and calls back into scripts).
            for k in 1..items.len() {
                let mut j = k;
                while j > 0 {
                    let r = match call(i, h, &a0, vec![items[j - 1].clone(), items[j].clone()]) {
                        Ok(r) => r.i(),
                        Err(e) => {
                            failure = Some(e);
                            0
                        }
                    };
                    if r <= 0 {
                        break;
                    }
                    items.swap(j - 1, j);
                    j -= 1;
                }
            }
            if let Some(e) = failure {
                return Err(e);
            }
            *a.borrow_mut() = items;
            Value::Null
        }
        "map" | "filter" | "count" | "exists" | "find" => {
            let items = a.borrow().clone();
            let mut out = Vec::new();
            for x in items {
                let r = call(i, h, &a0, vec![x.clone()])?;
                match name {
                    "map" => out.push(r),
                    "find" if r.truthy() => return Ok(x),
                    "exists" if r.truthy() => return Ok(Value::Bool(true)),
                    "filter" | "count" if r.truthy() => out.push(x),
                    _ => {}
                }
            }
            match name {
                "count" => Value::Int(out.len() as i64),
                "exists" => Value::Bool(false),
                "find" => Value::Null,
                _ => Value::array(out),
            }
        }
        _ => return Err(format!("Array has no method {name}")),
    })
}
