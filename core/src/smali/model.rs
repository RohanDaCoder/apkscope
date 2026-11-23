//! The class model. Every string, type, prototype, field and method a class refers to is an id
//! into its [`Source`]: the file's own entry, or one an edit added. A model is only meaningful
//! with the source it was read from or parsed into, as text is with the file it is in.

pub use super::source::{FieldId, Id, MethodId, ProtoId, Source, StrId, TypeId};
use crate::bytecode;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Member {
    Field(FieldId),
    Method(MethodId),
}

/// `method_handle_item`: kinds 0-3 are field accessors, 4-8 invokes.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Handle {
    pub kind: u16,
    pub member: Member,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Value {
    Byte(i8),
    Short(i16),
    Char(u16),
    Int(i32),
    Long(i64),
    Float(u32),
    Double(u64),
    MethodType(ProtoId),
    MethodHandle(Handle),
    Str(StrId),
    Type(TypeId),
    Field(FieldId),
    Method(MethodId),
    Enum(FieldId),
    Array(Vec<Value>),
    Anno(Anno),
    Null,
    Bool(bool),
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Anno {
    pub vis: u8,
    pub ty: TypeId,
    pub elems: Vec<(StrId, Value)>,
}

/// What an instruction acts on besides its registers: nothing, or one of these. Which kind an
/// instruction has is its opcode's, so an instruction has no literal to mistake for a 0, and no
/// string where its opcode takes a type.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Operand {
    None,
    /// A literal number; a float or a double as its bits.
    Lit(i64),
    /// The label a branch goes to, or of the payload a switch or an array fill uses.
    Label(u32),
    Str(StrId),
    Type(TypeId),
    Field(FieldId),
    Method(MethodId),
    Proto(ProtoId),
    /// bootstrap handle, name, method type, extra arguments
    CallSite(Arc<Vec<Value>>),
    Handle(Handle),
    /// `invoke-polymorphic`: the method, and the prototype it is called with.
    Polymorphic(MethodId, ProtoId),
}

impl Operand {
    /// Whether this is the kind of operand the opcode `op` takes.
    pub fn fits(&self, op: u8) -> bool {
        let Some((_, f, kind)) = bytecode::info(op) else {
            return false;
        };
        let polymorphic = matches!(f, bytecode::Fmt::F45cc | bytecode::Fmt::F4rcc);
        match self {
            Operand::None => kind == bytecode::Ref::None && !f.has_literal() && !f.has_target(),
            Operand::Lit(_) => f.has_literal(),
            Operand::Label(_) => f.has_target(),
            Operand::Str(_) => kind == bytecode::Ref::String,
            Operand::Type(_) => kind == bytecode::Ref::Type,
            Operand::Field(_) => kind == bytecode::Ref::Field,
            Operand::Method(_) => kind == bytecode::Ref::Method && !polymorphic,
            Operand::Proto(_) => kind == bytecode::Ref::Proto,
            Operand::CallSite(_) => kind == bytecode::Ref::CallSite,
            Operand::Handle(_) => kind == bytecode::Ref::MethodHandle,
            Operand::Polymorphic(..) => kind == bytecode::Ref::Method && polymorphic,
        }
    }
}

/// An instruction: its opcode, its registers, and the one operand its opcode takes.
#[derive(Clone, PartialEq, Debug)]
pub struct Ins {
    pub op: u8,
    /// for /range forms regs[0] is the first register and n the count
    pub regs: [u16; 5],
    pub n: u8,
    pub operand: Operand,
}

impl Ins {
    /// Whether this instruction has the operand `o`. A literal is compared by value, however
    /// wide the instruction that holds it, and the method of an `invoke-polymorphic` is its
    /// method.
    pub fn has(&self, o: &Operand) -> bool {
        match (&self.operand, o) {
            (Operand::Polymorphic(m, _), Operand::Method(x)) => m == x,
            (own, o) => own == o,
        }
    }

    /// Give this instruction the operand `to` where it has `from`; whether it did. One whose
    /// opcode takes another kind of operand than `to` is left as it is.
    pub fn replace(&mut self, from: &Operand, to: &Operand) -> bool {
        if !self.has(from) {
            return false;
        }
        match (&mut self.operand, to) {
            (Operand::Polymorphic(m, _), Operand::Method(x)) => *m = *x,
            (own, to) if to.fits(self.op) => *own = to.clone(),
            _ => return false,
        }
        true
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Item {
    Label(u32),
    Ins(Ins),
    Packed {
        first: i32,
        targets: Vec<u32>,
    },
    Sparse(Vec<(i32, u32)>),
    Array {
        width: u16,
        data: Vec<u8>,
    },
    Line(u32),
    Local {
        reg: u16,
        name: Option<StrId>,
        ty: Option<TypeId>,
        sig: Option<StrId>,
    },
    EndLocal(u16),
    Restart(u16),
    Prologue,
    Epilogue,
    SetFile(Option<StrId>),
}

#[derive(Clone, PartialEq, Debug)]
pub struct Try {
    pub start: u32,
    pub end: u32,
    pub handlers: Vec<(Option<TypeId>, u32)>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Code {
    pub registers: u16,
    pub items: Vec<Item>,
    pub tries: Vec<Try>,
    pub debug: bool,
    pub param_names: Vec<Option<StrId>>,
}

/// A field a class defines: its name and type are those of `id`.
#[derive(Clone, PartialEq, Debug)]
pub struct Field {
    pub id: FieldId,
    pub access: u32,
    pub hidden: Option<u32>,
    pub init: Option<Value>,
    pub annos: Vec<Anno>,
}

/// A method a class defines: its name and prototype are those of `id`.
#[derive(Clone, PartialEq, Debug)]
pub struct Method {
    pub id: MethodId,
    pub access: u32,
    pub hidden: Option<u32>,
    pub code: Option<Code>,
    pub annos: Vec<Anno>,
    pub param_annos: Option<Vec<Vec<Anno>>>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct Class {
    pub name: TypeId,
    pub access: u32,
    pub sup: Option<TypeId>,
    pub ifaces: Vec<TypeId>,
    pub source: Option<StrId>,
    pub annos: Vec<Anno>,
    pub fields: Vec<Field>,
    pub methods: Vec<Method>,
}

pub const ACC_STATIC: u32 = 0x8;
pub const ACC_PRIVATE: u32 = 0x2;
pub const ACC_CONSTRUCTOR: u32 = 0x10000;

impl Method {
    /// direct methods: static, private or constructors
    pub fn is_direct(&self) -> bool {
        self.access & (ACC_STATIC | ACC_PRIVATE | ACC_CONSTRUCTOR) != 0
    }
    /// registers taken by the arguments, `this` included
    pub fn ins(&self, src: &Source) -> u16 {
        let params = src.params(src.method(self.id).proto);
        let words: u16 = params.map(|p| width(src.descriptor(p))).sum();
        words + u16::from(self.access & ACC_STATIC == 0)
    }
}

/// registers a value of type `t` takes
pub fn width(t: &[u8]) -> u16 {
    if matches!(t, b"J" | b"D") { 2 } else { 1 }
}

/// MUTF-8 bytes to UTF-16 units.
pub fn utf16(b: &[u8]) -> Vec<u16> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let c = b[i] as u16;
        let nx = |k: usize| b.get(i + k).map_or(0, |&x| x as u16 & 0x3f);
        let (u, n) = match c {
            0..=0x7f => (c, 1),
            0xc0..=0xdf => ((c & 0x1f) << 6 | nx(1), 2),
            0xe0..=0xef => ((c & 0x0f) << 12 | nx(1) << 6 | nx(2), 3),
            _ => (0xfffd, 1),
        };
        out.push(u);
        i += n;
    }
    out
}

/// UTF-16 units to MUTF-8 bytes.
pub fn mutf8(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len());
    for &u in units {
        match u {
            1..=0x7f => out.push(u as u8),
            0 | 0x80..=0x7ff => out.extend([0xc0 | (u >> 6) as u8, 0x80 | (u & 0x3f) as u8]),
            _ => out.extend([
                0xe0 | (u >> 12) as u8,
                0x80 | ((u >> 6) & 0x3f) as u8,
                0x80 | (u & 0x3f) as u8,
            ]),
        }
    }
    out
}
