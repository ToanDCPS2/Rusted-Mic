//! The credits roll.
//!
//! Official builds hardcode `credits.json` into the executable at compile time:
//! `CreditsDataMacro` turns it into a Haxe object literal, so there is no JSON
//! left, only hxcpp code that builds the object (`Anon_obj::Create`,
//! `setFixed`, `VirtualArray::init`, inlined `new` + field stores). This finds
//! that code in the user's own executable and runs it symbolically:
//!
//! * the function is the one that references the `"header"`, `"body"` and
//!   `"entries"` string literals and builds the most entries;
//! * registers and memory hold symbolic values (string literals, integers,
//!   pointers into the stack frame or into objects it allocates);
//! * calls are recognised by how they are set up (an allocation takes a size,
//!   `setFixed` an object, an index, a name and a value, ...), not by address;
//! * at the end, the object stored into a static with an `entries` field is
//!   rebuilt as JSON.
//!
//! Only 64-bit Windows (PE) builds are handled; anything else returns `None`
//! and the game falls back to `CreditsDataHandler.getFallback()`.

use std::collections::HashMap;

use iced_x86::{Decoder, DecoderOptions, Instruction, Mnemonic, OpKind, Register};
use serde_json::{json, Map, Value};

struct Section {
    va: u32,
    raw: u32,
    size: u32,
    vsize: u32,
}

struct Pe<'a> {
    data: &'a [u8],
    base: u64,
    sections: Vec<(String, Section)>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(o..o + 2)?.try_into().ok()?))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?))
}
fn u64_at(d: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(d.get(o..o + 8)?.try_into().ok()?))
}

impl<'a> Pe<'a> {
    fn parse(data: &'a [u8]) -> Option<Self> {
        if data.get(..2)? != b"MZ" {
            return None;
        }
        let pe = u32_at(data, 0x3c)? as usize;
        if data.get(pe..pe + 4)? != b"PE\0\0" || u16_at(data, pe + 4)? != 0x8664 {
            return None;
        }
        let nsec = u16_at(data, pe + 6)? as usize;
        let opt_size = u16_at(data, pe + 20)? as usize;
        let opt = pe + 24;
        let base = u64_at(data, opt + 24)?;
        let mut sections = Vec::new();
        for i in 0..nsec {
            let o = opt + opt_size + 40 * i;
            let name = String::from_utf8_lossy(data.get(o..o + 8)?).trim_end_matches('\0').to_owned();
            let s = Section { vsize: u32_at(data, o + 8)?, va: u32_at(data, o + 12)?, size: u32_at(data, o + 16)?, raw: u32_at(data, o + 20)? };
            sections.push((name, s));
        }
        Some(Self { data, base, sections })
    }

    fn section(&self, name: &str) -> Option<&Section> {
        self.sections.iter().find(|(n, _)| n == name).map(|(_, s)| s)
    }

    fn bytes(&self, s: &Section) -> &'a [u8] {
        let end = (s.raw as usize + s.size as usize).min(self.data.len());
        &self.data[(s.raw as usize).min(end)..end]
    }

    fn off(&self, rva: u64) -> Option<usize> {
        self.sections.iter().find_map(|(_, s)| {
            let (va, len) = (s.va as u64, s.size.min(s.vsize.max(s.size)) as u64);
            (va..va + len).contains(&rva).then(|| (s.raw as u64 + rva - va) as usize)
        })
    }

    /// An hxcpp string literal (`HX_("...")`): its characters follow a hash and
    /// a flags word (`0x80100000` UTF-8, `0x80300000` UTF-16).
    fn literal(&self, rva: u64) -> Option<String> {
        let o = self.off(rva)?;
        let flags = self.data.get(o.checked_sub(4)?..o)?;
        match flags {
            [0, 0, 0x10, 0x80] => {
                let len = self.data[o..].iter().take(4096).position(|&b| b == 0)?;
                Some(String::from_utf8_lossy(&self.data[o..o + len]).into_owned())
            }
            [0, 0, 0x30, 0x80] => {
                let units: Vec<u16> = self.data[o..].chunks_exact(2).take(4096).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
                Some(String::from_utf16_lossy(&units))
            }
            _ => None,
        }
    }

    /// RVAs of a UTF-8 literal's characters.
    fn find_literal(&self, text: &str) -> Vec<u64> {
        let Some(rd) = self.section(".rdata") else { return Vec::new() };
        let mut needle = vec![0, 0, 0x10, 0x80];
        needle.extend_from_slice(text.as_bytes());
        needle.push(0);
        let bytes = self.bytes(rd);
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(p) = find(&bytes[i..], &needle) {
            out.push(rd.va as u64 + (i + p + 4) as u64);
            i += p + 1;
        }
        out
    }

    /// The function (`.pdata` entry) containing `rva`.
    fn function_at(&self, rva: u64) -> Option<(u64, u64)> {
        let p = self.section(".pdata")?;
        self.bytes(p).chunks_exact(12).find_map(|c| {
            let (a, b) = (u32::from_le_bytes(c[0..4].try_into().unwrap()) as u64, u32::from_le_bytes(c[4..8].try_into().unwrap()) as u64);
            (a <= rva && rva < b).then_some((a, b))
        })
    }

    /// Code addresses with a RIP-relative 32-bit displacement to `target`.
    fn code_refs(&self, target: u64) -> Vec<u64> {
        let Some(t) = self.section(".text") else { return Vec::new() };
        let text = self.bytes(t);
        let mut out = Vec::new();
        for p in 0..text.len().saturating_sub(4) {
            let disp = i32::from_le_bytes(text[p..p + 4].try_into().unwrap()) as i64;
            if (t.va as i64 + p as i64 + 4 + disp) as u64 == target {
                out.push(t.va as u64 + p as u64);
            }
        }
        out
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Region {
    Stack,
    Heap(usize),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum V {
    Int(i64),
    /// Index into [`Emu::strings`].
    Str(usize),
    Ptr(Region, i64),
}

#[derive(Default)]
struct Obj {
    mem: HashMap<i64, (V, u32)>,
    /// `setFixed(index, name, value)`, in call order.
    fields: Vec<(String, Option<V>)>,
    array: bool,
    inline_anon: bool,
}

#[derive(Default)]
struct Emu {
    regs: HashMap<Register, V>,
    /// XMM registers: up to four 4-byte-aligned cells of a 16-byte copy.
    xmm: HashMap<Register, Vec<(i64, (V, u32))>>,
    stack: HashMap<i64, (V, u32)>,
    heap: Vec<Obj>,
    globals: Vec<(u64, V)>,
    strings: Vec<String>,
    string_ids: HashMap<String, usize>,
    nonzero: Option<bool>,
}

const VOLATILE: [Register; 7] = [Register::RAX, Register::RCX, Register::RDX, Register::R8, Register::R9, Register::R10, Register::R11];

impl Emu {
    fn intern(&mut self, s: String) -> V {
        let n = self.strings.len();
        V::Str(*self.string_ids.entry(s.clone()).or_insert_with(|| {
            self.strings.push(s);
            n
        }))
    }

    fn alloc(&mut self) -> usize {
        self.heap.push(Obj::default());
        self.heap.len() - 1
    }

    fn mem(&mut self, r: Region) -> &mut HashMap<i64, (V, u32)> {
        match r {
            Region::Stack => &mut self.stack,
            Region::Heap(i) => &mut self.heap[i].mem,
        }
    }

    fn load(&mut self, a: Option<(Region, i64)>, size: u32) -> Option<V> {
        let (r, o) = a?;
        match self.mem(r).get(&o) {
            Some(&(v, s)) if s == size => Some(v),
            _ => None,
        }
    }

    fn store(&mut self, a: Option<(Region, i64)>, v: Option<V>, size: u32) {
        let Some((r, o)) = a else { return };
        let m = self.mem(r);
        match v {
            Some(v) => {
                m.insert(o, (v, size));
            }
            None => {
                m.remove(&o);
            }
        }
        if size == 8 {
            m.remove(&(o + 4));
        }
    }

    fn reg(&self, r: Register) -> Option<V> {
        self.regs.get(&r.full_register()).copied()
    }

    fn set_reg(&mut self, r: Register, v: Option<V>) {
        let r = r.full_register();
        match v {
            Some(v) => {
                self.regs.insert(r, v);
            }
            None => {
                self.regs.remove(&r);
            }
        }
    }

    /// A memory operand: an address in the frame / an object, or a static.
    fn address(&self, ins: &Instruction) -> Addr {
        if ins.is_ip_rel_memory_operand() {
            return Addr::Global(ins.ip_rel_memory_address());
        }
        let disp = ins.memory_displacement64() as i64;
        let base = match ins.memory_base() {
            Register::None => return Addr::Unknown,
            b => self.reg(b),
        };
        let index = match ins.memory_index() {
            Register::None => Some(0),
            i => match self.reg(i) {
                Some(V::Int(n)) => Some(n * ins.memory_index_scale() as i64),
                _ => None,
            },
        };
        match (base, index) {
            (Some(V::Ptr(r, o)), Some(i)) => Addr::Local(r, o + i + disp),
            _ => Addr::Unknown,
        }
    }

    fn operand(&mut self, ins: &Instruction, i: u32, image: u64) -> Option<V> {
        match ins.op_kind(i) {
            OpKind::Register => self.reg(ins.op_register(i)),
            OpKind::Memory => match self.address(ins) {
                Addr::Local(r, o) => self.load(Some((r, o)), ins.memory_size().size() as u32),
                Addr::Global(g) => self.globals.iter().rev().find(|(a, _)| *a == g - image).map(|(_, v)| *v),
                Addr::Unknown => None,
            },
            OpKind::Immediate8 | OpKind::Immediate16 | OpKind::Immediate32 | OpKind::Immediate64 | OpKind::Immediate8to16 | OpKind::Immediate8to32 | OpKind::Immediate8to64 | OpKind::Immediate32to64 => {
                Some(V::Int(ins.immediate(i) as i64))
            }
            _ => None,
        }
    }

    /// `String` at `a`: length then character pointer.
    fn string_at(&mut self, a: Option<V>) -> Option<String> {
        let Some(V::Ptr(r, o)) = a else { return None };
        match self.load(Some((r, o + 8)), 8)? {
            V::Str(i) => Some(self.strings[i].clone()),
            _ => None,
        }
    }

    fn heap_of(v: Option<V>) -> Option<usize> {
        match v {
            Some(V::Ptr(Region::Heap(i), _)) => Some(i),
            _ => None,
        }
    }

    /// Give `obj` an element buffer at +0x18 (hxcpp arrays keep it there).
    fn make_array(&mut self, obj: usize) {
        let buf = self.alloc();
        self.heap[obj].array = true;
        self.heap[obj].mem.insert(0x18, (V::Ptr(Region::Heap(buf), 0), 8));
    }

    /// A call, recognised by how its arguments are set up (registers not set
    /// since the last call are `None`).
    fn call(&mut self) {
        let [rax, rcx, rdx, r8, r9] = [Register::RAX, Register::RCX, Register::RDX, Register::R8, Register::R9].map(|r| self.reg(r));
        let stack = |v: Option<V>| matches!(v, Some(V::Ptr(Region::Stack, _)));
        let heap = |v: Option<V>| matches!(v, Some(V::Ptr(Region::Heap(_), _)));
        let int = |v: Option<V>| matches!(v, Some(V::Int(_)));
        let mut result = None;
        if rcx.is_none() && rdx.is_none() && r8.is_none() && r9.is_none() && int(rax) {
            // __chkstk: takes and keeps rax.
            result = rax;
        } else if int(rcx) && int(rdx) && r8 == Some(V::Int(0)) && r9.is_none() {
            // operator new(size, isObject, name)
            result = Some(V::Ptr(Region::Heap(self.alloc()), 0));
        } else if heap(rcx) && int(rdx) && r9 == Some(V::Int(8)) {
            // Array_obj ctor (length, reserve, elementSize)
            self.make_array(Self::heap_of(rcx).unwrap());
            result = rcx;
        } else if heap(rcx) && int(rdx) && r8.is_none() && r9.is_none() {
            // Anon_obj ctor, fields then stored inline.
            self.heap[Self::heap_of(rcx).unwrap()].inline_anon = true;
            result = rcx;
        } else if stack(rcx) && int(rdx) && r9.is_none() && (r8.is_none() || r8 == Some(V::Int(0))) {
            // Anon_obj::Create(n) / VirtualArray_obj::__new(n), returned through rcx.
            let o = self.alloc();
            if r8.is_some() {
                self.make_array(o);
            }
            let V::Ptr(r, off) = rcx.unwrap() else { unreachable!() };
            self.store(Some((r, off)), Some(V::Ptr(Region::Heap(o), 0)), 8);
            result = rcx;
        } else if heap(rcx) && int(rdx) && stack(r8) && stack(r9) {
            // setFixed(index, name, value)
            let name = self.string_at(r8);
            let Some(V::Ptr(r, o)) = r9 else { unreachable!() };
            let v = self.load(Some((r, o)), 8);
            if let Some(name) = name {
                self.heap[Self::heap_of(rcx).unwrap()].fields.push((name, v));
            }
            result = rcx;
        } else if heap(rcx) && stack(rdx) && int(r8) && stack(r9) {
            // VirtualArray init(index, value), returned through rdx.
            let Some(V::Ptr(r, o)) = r9 else { unreachable!() };
            let v = self.load(Some((r, o)), 8);
            let arr = Self::heap_of(rcx).unwrap();
            if let (Some(&(V::Ptr(buf, 0), 8)), Some(V::Int(i))) = (self.heap[arr].mem.get(&0x18), r8) {
                self.store(Some((buf, 8 * i)), v, 8);
            }
            let Some(V::Ptr(r, o)) = rdx else { unreachable!() };
            self.store(Some((r, o)), rcx, 8);
            result = rdx;
        } else if stack(rcx) && rdx.is_none() && r8.is_none() && r9.is_none() {
            // ObjectPtr::operator-> (null-checked dereference).
            let Some(V::Ptr(r, o)) = rcx else { unreachable!() };
            result = self.load(Some((r, o)), 8);
        }
        for r in VOLATILE {
            self.regs.remove(&r);
        }
        self.set_reg(Register::RAX, result);
    }

    fn run(&mut self, pe: &Pe, start: u64, end: u64) -> Option<()> {
        let image = pe.base;
        let code = pe.data.get(pe.off(start)?..pe.off(start)? + (end - start) as usize)?;
        let mut decoder = Decoder::with_ip(64, code, image + start, DecoderOptions::NONE);
        let mut ins = Instruction::default();
        self.regs.insert(Register::RSP, V::Ptr(Region::Stack, 0));
        let mut steps = 0;
        let mut pc = start;
        while (start..end).contains(&pc) && steps < 500_000 {
            steps += 1;
            decoder.set_ip(image + pc);
            decoder.set_position((pc - start) as usize).ok()?;
            decoder.decode_out(&mut ins);
            if ins.is_invalid() {
                return None;
            }
            let mut next = pc + ins.len() as u64;
            match ins.mnemonic() {
                Mnemonic::Mov | Mnemonic::Movsxd | Mnemonic::Movzx => {
                    let v = self.operand(&ins, 1, image);
                    self.write(&ins, v, image);
                }
                Mnemonic::Lea => {
                    let v = match self.address(&ins) {
                        Addr::Global(g) => pe.literal(g - image).map(|s| self.intern(s)),
                        Addr::Local(r, o) => Some(V::Ptr(r, o)),
                        Addr::Unknown => None,
                    };
                    self.set_reg(ins.op0_register(), v);
                }
                Mnemonic::Xor if ins.op_kind(0) == OpKind::Register && ins.op_kind(1) == OpKind::Register && ins.op0_register().full_register() == ins.op1_register().full_register() => {
                    self.set_reg(ins.op0_register(), Some(V::Int(0)));
                }
                Mnemonic::Push => {
                    if let Some(V::Ptr(r, o)) = self.reg(Register::RSP) {
                        self.set_reg(Register::RSP, Some(V::Ptr(r, o - 8)));
                    }
                }
                Mnemonic::Sub if ins.op_kind(0) == OpKind::Register && ins.op0_register() == Register::RSP => {
                    if let (Some(V::Ptr(r, o)), Some(V::Int(n))) = (self.reg(Register::RSP), self.operand(&ins, 1, image)) {
                        self.set_reg(Register::RSP, Some(V::Ptr(r, o - n)));
                    }
                }
                Mnemonic::Call => self.call(),
                Mnemonic::Movups | Mnemonic::Movdqu | Mnemonic::Movaps | Mnemonic::Movdqa => {
                    if ins.op_kind(0) == OpKind::Register {
                        let cells = match self.address(&ins) {
                            Addr::Local(r, o) => (0..16).step_by(4).filter_map(|k| self.mem(r).get(&(o + k)).map(|c| (k, *c))).collect(),
                            _ => Vec::new(),
                        };
                        self.xmm.insert(ins.op0_register(), cells);
                    } else if let Addr::Local(r, o) = self.address(&ins) {
                        let cells = self.xmm.get(&ins.op1_register()).cloned().unwrap_or_default();
                        for k in (0..16).step_by(4) {
                            self.mem(r).remove(&(o + k));
                        }
                        for (k, c) in cells {
                            self.mem(r).insert(o + k, c);
                        }
                    }
                }
                Mnemonic::Test => {
                    self.nonzero = match self.operand(&ins, 0, image) {
                        Some(V::Ptr(..) | V::Str(_)) => Some(true),
                        _ => None,
                    };
                }
                Mnemonic::Cmp => self.nonzero = None,
                Mnemonic::Jmp => {
                    if ins.op0_kind() != OpKind::NearBranch64 {
                        break;
                    }
                    next = ins.near_branch64() - image;
                }
                // Null checks pass; everything else falls through.
                Mnemonic::Jne if self.nonzero == Some(true) => next = ins.near_branch64() - image,
                Mnemonic::Ret => break,
                _ => {
                    if ins.op_count() > 0 && ins.op_kind(0) == OpKind::Register {
                        self.set_reg(ins.op0_register(), None);
                    } else if ins.op_count() > 0 && ins.op_kind(0) == OpKind::Memory {
                        self.write(&ins, None, image);
                    }
                }
            }
            pc = next;
        }
        Some(())
    }

    fn write(&mut self, ins: &Instruction, v: Option<V>, image: u64) {
        if ins.op_kind(0) == OpKind::Register {
            self.set_reg(ins.op0_register(), v);
            return;
        }
        match self.address(ins) {
            Addr::Local(r, o) => self.store(Some((r, o)), v, ins.memory_size().size() as u32),
            Addr::Global(g) => {
                if let Some(v) = v {
                    self.globals.push((g - image, v));
                }
            }
            Addr::Unknown => {}
        }
    }

    /// Rebuild a value as JSON.
    fn json(&self, v: Option<V>, depth: u32) -> Value {
        let Some(v) = v else { return Value::Null };
        if depth > 16 {
            return Value::Null;
        }
        match v {
            V::Int(n) => json!(n),
            V::Str(i) => json!(self.strings[i]),
            V::Ptr(Region::Stack, _) => Value::Null,
            V::Ptr(Region::Heap(h), _) => {
                let o = &self.heap[h];
                if !o.fields.is_empty() {
                    let mut m = Map::new();
                    for (k, x) in &o.fields {
                        m.insert(k.clone(), self.json(*x, depth + 1));
                    }
                    return Value::Object(m);
                }
                if o.array {
                    let Some(&(V::Ptr(Region::Heap(buf), 0), _)) = o.mem.get(&0x18) else { return json!([]) };
                    let mut cells: Vec<_> = self.heap[buf].mem.iter().filter(|(_, (_, s))| *s == 8).collect();
                    cells.sort_by_key(|(k, _)| **k);
                    return Value::Array(cells.into_iter().map(|(_, (x, _))| self.json(Some(*x), depth + 1)).collect());
                }
                if o.inline_anon {
                    // Each field: name `String` (length, chars) then its value.
                    let mut cells: Vec<_> = o.mem.iter().collect();
                    cells.sort_by_key(|(k, _)| **k);
                    let mut m = Map::new();
                    for (&k, &(x, _)) in cells {
                        let V::Str(i) = x else { continue };
                        let name = &self.strings[i];
                        if o.mem.get(&(k - 8)).map(|c| c.0) == Some(V::Int(name.chars().count() as i64)) {
                            m.insert(name.clone(), self.json(o.mem.get(&(k + 8)).map(|c| c.0), depth + 1));
                        }
                    }
                    return Value::Object(m);
                }
                Value::Null
            }
        }
    }
}

enum Addr {
    Local(Region, i64),
    Global(u64),
    Unknown,
}

/// Keep only the `credits.json` shape: `entries: [{header?, body: [{line}]}]`.
fn clean(v: &Value) -> Option<Value> {
    let entries: Vec<Value> = v
        .get("entries")?
        .as_array()?
        .iter()
        .filter_map(|e| {
            let header = e.get("header").and_then(Value::as_str);
            let body: Vec<Value> = e
                .get("body")
                .and_then(Value::as_array)
                .map(|b| b.iter().filter_map(|l| l.get("line").and_then(Value::as_str)).map(|l| json!({ "line": l })).collect())
                .unwrap_or_default();
            if header.is_none() && body.is_empty() {
                return None;
            }
            let mut m = Map::new();
            if let Some(h) = header {
                m.insert("header".into(), json!(h));
            }
            m.insert("body".into(), Value::Array(body));
            if e.get("appendBackers").and_then(Value::as_bool) == Some(true) {
                m.insert("appendBackers".into(), json!(true));
            }
            Some(Value::Object(m))
        })
        .collect();
    (!entries.is_empty()).then(|| json!({ "entries": entries }))
}

/// The credits an official executable hardcodes, as `credits.json`.
pub fn extract_credits(exe: &[u8]) -> Option<Value> {
    let pe = Pe::parse(exe)?;
    let mut funcs: Vec<(u64, u64)> = pe
        .find_literal("header")
        .into_iter()
        .flat_map(|lit| pe.code_refs(lit))
        .filter_map(|r| pe.function_at(r))
        .collect();
    funcs.sort();
    funcs.dedup();
    let entries_len = |v: &Value| v["entries"].as_array().map_or(0, Vec::len);
    let mut best: Option<Value> = None;
    for (start, end) in funcs {
        let mut emu = Emu::default();
        if emu.run(&pe, start, end).is_none() {
            continue;
        }
        let roots: Vec<V> = emu.globals.iter().map(|(_, v)| *v).collect();
        for root in roots {
            let Some(found) = clean(&emu.json(Some(root), 0)) else { continue };
            if best.as_ref().is_none_or(|b| entries_len(&found) > entries_len(b)) {
                best = Some(found);
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Needs the real game: `RUSTED_MIC_TEST_ASSETS=<install>/assets`.
    #[test]
    #[ignore]
    fn credits_from_exe() {
        let Some(root) = std::env::var_os("RUSTED_MIC_TEST_ASSETS") else { return };
        let exe = std::fs::read(std::path::Path::new(&root).parent().unwrap().join("Funkin.exe")).unwrap();
        let c = extract_credits(&exe).expect("credits");
        if let Some(out) = std::env::var_os("RUSTED_MIC_DUMP_CREDITS") {
            std::fs::write(out, serde_json::to_string_pretty(&c).unwrap()).unwrap();
        }
        let entries = c["entries"].as_array().unwrap();
        assert!(entries.len() > 30, "{} entries", entries.len());
        assert_eq!(entries[0]["header"], "Friday Night Funkin'");
        assert_eq!(entries[0]["body"][0]["line"], "A video game created by");
        let lines: usize = entries.iter().map(|e| e["body"].as_array().unwrap().len()).sum();
        assert!(lines > 100, "{lines} lines");
    }
}
