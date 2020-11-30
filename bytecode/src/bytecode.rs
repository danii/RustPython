//! Implement python as a virtual machine with bytecodes. This module
//! implements bytecode structure.

use bitflags::bitflags;
use bstr::ByteSlice;
use itertools::Itertools;
use num_bigint::BigInt;
use num_complex::Complex64;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fmt;

/// Sourcecode location.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Location {
    row: usize,
    column: usize,
}

impl Location {
    pub fn new(row: usize, column: usize) -> Self {
        Location { row, column }
    }

    pub fn row(&self) -> usize {
        self.row
    }

    pub fn column(&self) -> usize {
        self.column
    }
}

pub trait Constant: Sized {
    type Name: AsRef<str>;
    fn borrow_constant(&self) -> BorrowedConstant<Self>;
    fn into_data(self) -> ConstantData {
        self.borrow_constant().into_data()
    }
    fn map_constant<Bag: ConstantBag>(self, bag: &Bag) -> Bag::Constant {
        bag.make_constant(self.into_data())
    }
}
impl Constant for ConstantData {
    type Name = String;
    fn borrow_constant(&self) -> BorrowedConstant<Self> {
        use BorrowedConstant::*;
        match self {
            ConstantData::Integer { value } => Integer { value },
            ConstantData::Float { value } => Float { value: *value },
            ConstantData::Complex { value } => Complex { value: *value },
            ConstantData::Boolean { value } => Boolean { value: *value },
            ConstantData::Str { value } => Str { value },
            ConstantData::Bytes { value } => Bytes { value },
            ConstantData::Code { code } => Code { code },
            ConstantData::Tuple { elements } => Tuple {
                elements: Box::new(elements.iter().map(|e| e.borrow_constant())),
            },
            ConstantData::None => None,
            ConstantData::Ellipsis => Ellipsis,
        }
    }
    fn into_data(self) -> ConstantData {
        self
    }
}

pub trait ConstantBag: Sized {
    type Constant: Constant;
    fn make_constant(&self, constant: ConstantData) -> Self::Constant;
    fn make_constant_borrowed<C: Constant>(&self, constant: BorrowedConstant<C>) -> Self::Constant {
        self.make_constant(constant.into_data())
    }
    fn make_name(&self, name: String) -> <Self::Constant as Constant>::Name;
    fn make_name_ref(&self, name: &str) -> <Self::Constant as Constant>::Name {
        self.make_name(name.to_owned())
    }
}

#[derive(Clone)]
pub struct BasicBag;
impl ConstantBag for BasicBag {
    type Constant = ConstantData;
    fn make_constant(&self, constant: ConstantData) -> Self::Constant {
        constant
    }
    fn make_name(&self, name: String) -> <Self::Constant as Constant>::Name {
        name
    }
}

/// Primary container of a single code object. Each python function has
/// a codeobject. Also a module has a codeobject.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeObject<C: Constant = ConstantData> {
    pub instructions: Vec<Instruction>,
    pub locations: Vec<Location>,
    pub flags: CodeFlags,
    pub posonlyarg_count: usize, // Number of positional-only arguments
    pub arg_count: usize,
    pub kwonlyarg_count: usize,
    pub source_path: String,
    pub first_line_number: usize,
    pub obj_name: String, // Name of the object that created this code object
    pub cell2arg: Option<Box<[isize]>>,
    pub constants: Vec<C>,
    #[serde(bound(
        deserialize = "C::Name: serde::Deserialize<'de>",
        serialize = "C::Name: serde::Serialize"
    ))]
    pub names: Vec<C::Name>,
    pub varnames: Vec<C::Name>,
    pub cellvars: Vec<C::Name>,
    pub freevars: Vec<C::Name>,
}

bitflags! {
    #[derive(Serialize, Deserialize)]
    pub struct CodeFlags: u16 {
        const HAS_DEFAULTS = 0x01;
        const HAS_KW_ONLY_DEFAULTS = 0x02;
        const HAS_ANNOTATIONS = 0x04;
        const NEW_LOCALS = 0x08;
        const IS_GENERATOR = 0x10;
        const IS_COROUTINE = 0x20;
        const HAS_VARARGS = 0x40;
        const HAS_VARKEYWORDS = 0x80;
        const IS_OPTIMIZED = 0x0100;
    }
}

impl Default for CodeFlags {
    fn default() -> Self {
        Self::NEW_LOCALS
    }
}

impl CodeFlags {
    pub const NAME_MAPPING: &'static [(&'static str, CodeFlags)] = &[
        ("GENERATOR", CodeFlags::IS_GENERATOR),
        ("COROUTINE", CodeFlags::IS_COROUTINE),
        (
            "ASYNC_GENERATOR",
            Self::from_bits_truncate(Self::IS_GENERATOR.bits | Self::IS_COROUTINE.bits),
        ),
        ("VARARGS", CodeFlags::HAS_VARARGS),
        ("VARKEYWORDS", CodeFlags::HAS_VARKEYWORDS),
    ];
}

#[derive(Serialize, Debug, Deserialize, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd)]
#[repr(transparent)]
// XXX: if you add a new instruction that stores a Label, make sure to add it in
// compile::CodeInfo::finalize_code and CodeObject::label_targets
pub struct Label(pub usize);
impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// Transforms a value prior to formatting it.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConversionFlag {
    /// Converts by calling `str(<value>)`.
    Str,
    /// Converts by calling `ascii(<value>)`.
    Ascii,
    /// Converts by calling `repr(<value>)`.
    Repr,
}

pub type NameIdx = usize;

/// A Single bytecode instruction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Instruction {
    Import {
        name_idx: Option<NameIdx>,
        symbols_idx: Vec<NameIdx>,
        level: usize,
    },
    ImportStar,
    ImportFrom {
        idx: NameIdx,
    },
    LoadFast(NameIdx),
    LoadNameAny(NameIdx),
    LoadGlobal(NameIdx),
    LoadDeref(NameIdx),
    LoadClassDeref(NameIdx),
    StoreFast(NameIdx),
    StoreLocal(NameIdx),
    StoreGlobal(NameIdx),
    StoreDeref(NameIdx),
    DeleteFast(NameIdx),
    DeleteLocal(NameIdx),
    DeleteGlobal(NameIdx),
    DeleteDeref(NameIdx),
    LoadClosure(NameIdx),
    Subscript,
    StoreSubscript,
    DeleteSubscript,
    StoreAttr {
        idx: NameIdx,
    },
    DeleteAttr {
        idx: NameIdx,
    },
    LoadConst {
        /// index into constants vec
        idx: usize,
    },
    UnaryOperation {
        op: UnaryOperator,
    },
    BinaryOperation {
        op: BinaryOperator,
        inplace: bool,
    },
    LoadAttr {
        idx: NameIdx,
    },
    CompareOperation {
        op: ComparisonOperator,
    },
    Pop,
    Rotate {
        amount: usize,
    },
    Duplicate,
    GetIter,
    Continue,
    Break,
    Jump {
        target: Label,
    },
    /// Pop the top of the stack, and jump if this value is true.
    JumpIfTrue {
        target: Label,
    },
    /// Pop the top of the stack, and jump if this value is false.
    JumpIfFalse {
        target: Label,
    },
    /// Peek at the top of the stack, and jump if this value is true.
    /// Otherwise, pop top of stack.
    JumpIfTrueOrPop {
        target: Label,
    },
    /// Peek at the top of the stack, and jump if this value is false.
    /// Otherwise, pop top of stack.
    JumpIfFalseOrPop {
        target: Label,
    },
    MakeFunction,
    CallFunction {
        typ: CallType,
    },
    ForIter {
        target: Label,
    },
    ReturnValue,
    YieldValue,
    YieldFrom,
    SetupAnnotation,
    SetupLoop {
        start: Label,
        end: Label,
    },

    /// Setup a finally handler, which will be called whenever one of this events occurs:
    /// - the block is popped
    /// - the function returns
    /// - an exception is returned
    SetupFinally {
        handler: Label,
    },

    /// Enter a finally block, without returning, excepting, just because we are there.
    EnterFinally,

    /// Marker bytecode for the end of a finally sequence.
    /// When this bytecode is executed, the eval loop does one of those things:
    /// - Continue at a certain bytecode position
    /// - Propagate the exception
    /// - Return from a function
    /// - Do nothing at all, just continue
    EndFinally,

    SetupExcept {
        handler: Label,
    },
    SetupWith {
        end: Label,
    },
    WithCleanupStart,
    WithCleanupFinish,
    PopBlock,
    Raise {
        argc: usize,
    },
    BuildString {
        size: usize,
    },
    BuildTuple {
        size: usize,
        unpack: bool,
    },
    BuildList {
        size: usize,
        unpack: bool,
    },
    BuildSet {
        size: usize,
        unpack: bool,
    },
    BuildMap {
        size: usize,
        unpack: bool,
        for_call: bool,
    },
    BuildSlice {
        size: usize,
    },
    ListAppend {
        i: usize,
    },
    SetAdd {
        i: usize,
    },
    MapAdd {
        i: usize,
    },

    PrintExpr,
    LoadBuildClass,
    UnpackSequence {
        size: usize,
    },
    UnpackEx {
        before: usize,
        after: usize,
    },
    FormatValue {
        conversion: Option<ConversionFlag>,
    },
    PopException,
    Reverse {
        amount: usize,
    },
    GetAwaitable,
    BeforeAsyncWith,
    SetupAsyncWith {
        end: Label,
    },
    GetAIter,
    GetANext,

    /// Reverse order evaluation in MapAdd
    /// required to support named expressions of Python 3.8 in dict comprehension
    /// today (including Py3.9) only required in dict comprehension.
    MapAddRev {
        i: usize,
    },
}

use self::Instruction::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CallType {
    Positional(usize),
    Keyword(usize),
    Ex(bool),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConstantData {
    Integer { value: BigInt },
    Float { value: f64 },
    Complex { value: Complex64 },
    Boolean { value: bool },
    Str { value: String },
    Bytes { value: Vec<u8> },
    Code { code: Box<CodeObject> },
    Tuple { elements: Vec<ConstantData> },
    None,
    Ellipsis,
}

pub enum BorrowedConstant<'a, C: Constant> {
    Integer { value: &'a BigInt },
    Float { value: f64 },
    Complex { value: Complex64 },
    Boolean { value: bool },
    Str { value: &'a str },
    Bytes { value: &'a [u8] },
    Code { code: &'a CodeObject<C> },
    Tuple { elements: BorrowedTupleIter<'a, C> },
    None,
    Ellipsis,
}
type BorrowedTupleIter<'a, C> = Box<dyn Iterator<Item = BorrowedConstant<'a, C>> + 'a>;
impl<C: Constant> BorrowedConstant<'_, C> {
    // takes `self` because we need to consume the iterator
    pub fn fmt_display(self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BorrowedConstant::Integer { value } => write!(f, "{}", value),
            BorrowedConstant::Float { value } => write!(f, "{}", value),
            BorrowedConstant::Complex { value } => write!(f, "{}", value),
            BorrowedConstant::Boolean { value } => {
                write!(f, "{}", if value { "True" } else { "False" })
            }
            BorrowedConstant::Str { value } => write!(f, "{:?}", value),
            BorrowedConstant::Bytes { value } => write!(f, "b{:?}", value.as_bstr()),
            BorrowedConstant::Code { code } => write!(f, "{:?}", code),
            BorrowedConstant::Tuple { elements } => {
                write!(f, "(")?;
                let mut first = true;
                for c in elements {
                    if first {
                        first = false
                    } else {
                        write!(f, ", ")?;
                    }
                    c.fmt_display(f)?;
                }
                write!(f, ")")
            }
            BorrowedConstant::None => write!(f, "None"),
            BorrowedConstant::Ellipsis => write!(f, "..."),
        }
    }
    pub fn into_data(self) -> ConstantData {
        use ConstantData::*;
        match self {
            BorrowedConstant::Integer { value } => Integer {
                value: value.clone(),
            },
            BorrowedConstant::Float { value } => Float { value },
            BorrowedConstant::Complex { value } => Complex { value },
            BorrowedConstant::Boolean { value } => Boolean { value },
            BorrowedConstant::Str { value } => Str {
                value: value.to_owned(),
            },
            BorrowedConstant::Bytes { value } => Bytes {
                value: value.to_owned(),
            },
            BorrowedConstant::Code { code } => Code {
                code: Box::new(code.map_clone_bag(&BasicBag)),
            },
            BorrowedConstant::Tuple { elements } => Tuple {
                elements: elements.map(BorrowedConstant::into_data).collect(),
            },
            BorrowedConstant::None => None,
            BorrowedConstant::Ellipsis => Ellipsis,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ComparisonOperator {
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
    Equal,
    NotEqual,
    In,
    NotIn,
    Is,
    IsNot,
    ExceptionMatch,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum BinaryOperator {
    Power,
    Multiply,
    MatrixMultiply,
    Divide,
    FloorDivide,
    Modulo,
    Add,
    Subtract,
    Lshift,
    Rshift,
    And,
    Xor,
    Or,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UnaryOperator {
    Not,
    Invert,
    Minus,
    Plus,
}

/*
Maintain a stack of blocks on the VM.
pub enum BlockType {
    Loop,
    Except,
}
*/

pub struct Arguments<'a, N: AsRef<str>> {
    pub posonlyargs: &'a [N],
    pub args: &'a [N],
    pub vararg: Option<&'a N>,
    pub kwonlyargs: &'a [N],
    pub varkwarg: Option<&'a N>,
}

impl<N: AsRef<str>> fmt::Debug for Arguments<'_, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        macro_rules! fmt_slice {
            ($x:expr) => {
                format_args!("[{}]", $x.iter().map(AsRef::as_ref).format(", "))
            };
        }
        f.debug_struct("Arguments")
            .field("posonlyargs", &fmt_slice!(self.posonlyargs))
            .field("args", &fmt_slice!(self.posonlyargs))
            .field("vararg", &self.vararg.map(N::as_ref))
            .field("kwonlyargs", &fmt_slice!(self.kwonlyargs))
            .field("varkwarg", &self.varkwarg.map(N::as_ref))
            .finish()
    }
}

impl<C: Constant> CodeObject<C> {
    pub fn new(
        flags: CodeFlags,
        posonlyarg_count: usize,
        arg_count: usize,
        kwonlyarg_count: usize,
        source_path: String,
        first_line_number: usize,
        obj_name: String,
    ) -> Self {
        CodeObject {
            instructions: Vec::new(),
            locations: Vec::new(),
            flags,
            posonlyarg_count,
            arg_count,
            kwonlyarg_count,
            source_path,
            first_line_number,
            obj_name,
            cell2arg: None,
            constants: Vec::new(),
            names: Vec::new(),
            varnames: Vec::new(),
            cellvars: Vec::new(),
            freevars: Vec::new(),
        }
    }

    // like inspect.getargs
    pub fn arg_names(&self) -> Arguments<C::Name> {
        let nargs = self.arg_count;
        let nkwargs = self.kwonlyarg_count;
        let mut varargspos = nargs + nkwargs;
        let posonlyargs = &self.varnames[..self.posonlyarg_count];
        let args = &self.varnames[..nargs];
        let kwonlyargs = &self.varnames[nargs..varargspos];

        let vararg = if self.flags.contains(CodeFlags::HAS_VARARGS) {
            let vararg = &self.varnames[varargspos];
            varargspos += 1;
            Some(vararg)
        } else {
            None
        };
        let varkwarg = if self.flags.contains(CodeFlags::HAS_VARKEYWORDS) {
            Some(&self.varnames[varargspos])
        } else {
            None
        };

        Arguments {
            posonlyargs,
            args,
            vararg,
            kwonlyargs,
            varkwarg,
        }
    }

    pub fn label_targets(&self) -> BTreeSet<Label> {
        let mut label_targets = BTreeSet::new();
        for instruction in &self.instructions {
            match instruction {
                Jump { target: l }
                | JumpIfTrue { target: l }
                | JumpIfFalse { target: l }
                | JumpIfTrueOrPop { target: l }
                | JumpIfFalseOrPop { target: l }
                | ForIter { target: l }
                | SetupFinally { handler: l }
                | SetupExcept { handler: l }
                | SetupWith { end: l }
                | SetupAsyncWith { end: l } => {
                    label_targets.insert(*l);
                }
                SetupLoop { start, end } => {
                    label_targets.insert(*start);
                    label_targets.insert(*end);
                }

                #[rustfmt::skip]
                Import { .. } | ImportStar | ImportFrom { .. } | LoadFast(_) | LoadNameAny(_)
                | LoadGlobal(_) | LoadDeref(_) | LoadClassDeref(_) | StoreFast(_) | StoreLocal(_)
                | StoreGlobal(_) | StoreDeref(_) | DeleteFast(_) | DeleteLocal(_) | DeleteGlobal(_)
                | DeleteDeref(_) | LoadClosure(_) | Subscript | StoreSubscript | DeleteSubscript
                | StoreAttr { .. } | DeleteAttr { .. } | LoadConst { .. } | UnaryOperation { .. }
                | BinaryOperation { .. } | LoadAttr { .. } | CompareOperation { .. } | Pop
                | Rotate { .. } | Duplicate | GetIter | Continue | Break | MakeFunction
                | CallFunction { .. } | ReturnValue | YieldValue | YieldFrom | SetupAnnotation
                | EnterFinally | EndFinally | WithCleanupStart | WithCleanupFinish | PopBlock
                | Raise { .. } | BuildString { .. } | BuildTuple { .. } | BuildList { .. }
                | BuildSet { .. } | BuildMap { .. } | BuildSlice { .. } | ListAppend { .. }
                | SetAdd { .. } | MapAdd { .. } | PrintExpr | LoadBuildClass | UnpackSequence { .. }
                | UnpackEx { .. } | FormatValue { .. } | PopException | Reverse { .. }
                | GetAwaitable | BeforeAsyncWith | GetAIter | GetANext | MapAddRev { .. } => {}
            }
        }
        label_targets
    }

    fn display_inner(
        &self,
        f: &mut fmt::Formatter,
        expand_codeobjects: bool,
        level: usize,
    ) -> fmt::Result {
        let label_targets = self.label_targets();

        for (offset, instruction) in self.instructions.iter().enumerate() {
            let arrow = if label_targets.contains(&Label(offset)) {
                ">>"
            } else {
                "  "
            };
            for _ in 0..level {
                write!(f, "          ")?;
            }
            write!(f, "{} {:5} ", arrow, offset)?;
            instruction.fmt_dis(
                f,
                &self.constants,
                &self.names,
                &self.varnames,
                &self.cellvars,
                &self.freevars,
                expand_codeobjects,
                level,
            )?;
        }
        Ok(())
    }

    pub fn display_expand_codeobjects<'a>(&'a self) -> impl fmt::Display + 'a {
        struct Display<'a, C: Constant>(&'a CodeObject<C>);
        impl<C: Constant> fmt::Display for Display<'_, C> {
            fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
                self.0.display_inner(f, true, 1)
            }
        }
        Display(self)
    }

    pub fn map_bag<Bag: ConstantBag>(self, bag: &Bag) -> CodeObject<Bag::Constant> {
        let map_names = |names: Vec<C::Name>| {
            names
                .into_iter()
                .map(|x| bag.make_name_ref(x.as_ref()))
                .collect::<Vec<_>>()
        };
        CodeObject {
            constants: self
                .constants
                .into_iter()
                .map(|x| x.map_constant(bag))
                .collect(),
            names: map_names(self.names),
            varnames: map_names(self.varnames),
            cellvars: map_names(self.cellvars),
            freevars: map_names(self.freevars),

            instructions: self.instructions,
            locations: self.locations,
            flags: self.flags,
            posonlyarg_count: self.posonlyarg_count,
            arg_count: self.arg_count,
            kwonlyarg_count: self.kwonlyarg_count,
            source_path: self.source_path,
            first_line_number: self.first_line_number,
            obj_name: self.obj_name,
            cell2arg: self.cell2arg,
        }
    }

    pub fn map_clone_bag<Bag: ConstantBag>(&self, bag: &Bag) -> CodeObject<Bag::Constant> {
        let map_names = |names: &[C::Name]| {
            names
                .iter()
                .map(|x| bag.make_name_ref(x.as_ref()))
                .collect()
        };
        CodeObject {
            constants: self
                .constants
                .iter()
                .map(|x| bag.make_constant_borrowed(x.borrow_constant()))
                .collect(),
            names: map_names(&self.names),
            varnames: map_names(&self.varnames),
            cellvars: map_names(&self.cellvars),
            freevars: map_names(&self.freevars),

            instructions: self.instructions.clone(),
            locations: self.locations.clone(),
            flags: self.flags,
            posonlyarg_count: self.posonlyarg_count,
            arg_count: self.arg_count,
            kwonlyarg_count: self.kwonlyarg_count,
            source_path: self.source_path.clone(),
            first_line_number: self.first_line_number,
            obj_name: self.obj_name.clone(),
            cell2arg: self.cell2arg.clone(),
        }
    }
}

impl CodeObject<ConstantData> {
    /// Load a code object from bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, Box<dyn std::error::Error>> {
        let reader = lz_fear::framed::LZ4FrameReader::new(data)?;
        Ok(bincode::deserialize_from(reader.into_read())?)
    }

    /// Serialize this bytecode to bytes.
    pub fn to_bytes(&self) -> Vec<u8> {
        let data = bincode::serialize(&self).expect("CodeObject is not serializable");
        let mut out = Vec::new();
        lz_fear::framed::CompressionSettings::default()
            .compress_with_size_unchecked(data.as_slice(), &mut out, data.len() as u64)
            .unwrap();
        out
    }
}

impl<C: Constant> fmt::Display for CodeObject<C> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.display_inner(f, false, 1)?;
        for constant in &self.constants {
            if let BorrowedConstant::Code { code } = constant.borrow_constant() {
                write!(f, "\nDisassembly of {:?}\n", code)?;
                code.fmt(f)?;
            }
        }
        Ok(())
    }
}

impl Instruction {
    fn fmt_dis<C: Constant>(
        &self,
        f: &mut fmt::Formatter,
        constants: &[C],
        names: &[C::Name],
        varnames: &[C::Name],
        cellvars: &[C::Name],
        freevars: &[C::Name],
        expand_codeobjects: bool,
        level: usize,
    ) -> fmt::Result {
        macro_rules! w {
            ($variant:ident) => {
                writeln!(f, stringify!($variant))
            };
            ($variant:ident, $var:expr) => {
                writeln!(f, "{:20} ({})", stringify!($variant), $var)
            };
            ($variant:ident, $var1:expr, $var2:expr) => {
                writeln!(f, "{:20} ({}, {})", stringify!($variant), $var1, $var2)
            };
            ($variant:ident, $var1:expr, $var2:expr, $var3:expr) => {
                writeln!(
                    f,
                    "{:20} ({}, {}, {})",
                    stringify!($variant),
                    $var1,
                    $var2,
                    $var3
                )
            };
        }

        let cellname = |i: usize| {
            cellvars
                .get(i)
                .unwrap_or_else(|| &freevars[i - cellvars.len()])
                .as_ref()
        };

        match self {
            Import {
                name_idx,
                symbols_idx,
                level,
            } => w!(
                Import,
                format!("{:?}", name_idx.map(|idx| names[idx].as_ref())),
                format!(
                    "({:?})",
                    symbols_idx
                        .iter()
                        .map(|&idx| names[idx].as_ref())
                        .format(", ")
                ),
                level
            ),
            ImportStar => w!(ImportStar),
            ImportFrom { idx } => w!(ImportFrom, names[*idx].as_ref()),
            LoadFast(idx) => w!(LoadFast, *idx, varnames[*idx].as_ref()),
            LoadNameAny(idx) => w!(LoadNameAny, *idx, names[*idx].as_ref()),
            LoadGlobal(idx) => w!(LoadGlobal, *idx, names[*idx].as_ref()),
            LoadDeref(idx) => w!(LoadDeref, *idx, cellname(*idx)),
            LoadClassDeref(idx) => w!(LoadClassDeref, *idx, cellname(*idx)),
            StoreFast(idx) => w!(StoreFast, *idx, varnames[*idx].as_ref()),
            StoreLocal(idx) => w!(StoreLocal, *idx, names[*idx].as_ref()),
            StoreGlobal(idx) => w!(StoreGlobal, *idx, names[*idx].as_ref()),
            StoreDeref(idx) => w!(StoreDeref, *idx, cellname(*idx)),
            DeleteFast(idx) => w!(DeleteFast, *idx, varnames[*idx].as_ref()),
            DeleteLocal(idx) => w!(DeleteLocal, *idx, names[*idx].as_ref()),
            DeleteGlobal(idx) => w!(DeleteGlobal, *idx, names[*idx].as_ref()),
            DeleteDeref(idx) => w!(DeleteDeref, *idx, cellname(*idx)),
            LoadClosure(i) => w!(LoadClosure, *i, cellname(*i)),
            Subscript => w!(Subscript),
            StoreSubscript => w!(StoreSubscript),
            DeleteSubscript => w!(DeleteSubscript),
            StoreAttr { idx } => w!(StoreAttr, names[*idx].as_ref()),
            DeleteAttr { idx } => w!(DeleteAttr, names[*idx].as_ref()),
            LoadConst { idx } => {
                let value = &constants[*idx];
                match value.borrow_constant() {
                    BorrowedConstant::Code { code } if expand_codeobjects => {
                        writeln!(f, "{:20} ({:?}):", "LoadConst", code)?;
                        code.display_inner(f, true, level + 1)?;
                        Ok(())
                    }
                    c => {
                        write!(f, "{:20} (", "LoadConst")?;
                        c.fmt_display(f)?;
                        writeln!(f, ")")
                    }
                }
            }
            UnaryOperation { op } => w!(UnaryOperation, format!("{:?}", op)),
            BinaryOperation { op, inplace } => w!(BinaryOperation, format!("{:?}", op), inplace),
            LoadAttr { idx } => w!(LoadAttr, names[*idx].as_ref()),
            CompareOperation { op } => w!(CompareOperation, format!("{:?}", op)),
            Pop => w!(Pop),
            Rotate { amount } => w!(Rotate, amount),
            Duplicate => w!(Duplicate),
            GetIter => w!(GetIter),
            Continue => w!(Continue),
            Break => w!(Break),
            Jump { target } => w!(Jump, target),
            JumpIfTrue { target } => w!(JumpIfTrue, target),
            JumpIfFalse { target } => w!(JumpIfFalse, target),
            JumpIfTrueOrPop { target } => w!(JumpIfTrueOrPop, target),
            JumpIfFalseOrPop { target } => w!(JumpIfFalseOrPop, target),
            MakeFunction => w!(MakeFunction),
            CallFunction { typ } => w!(CallFunction, format!("{:?}", typ)),
            ForIter { target } => w!(ForIter, target),
            ReturnValue => w!(ReturnValue),
            YieldValue => w!(YieldValue),
            YieldFrom => w!(YieldFrom),
            SetupAnnotation => w!(SetupAnnotation),
            SetupLoop { start, end } => w!(SetupLoop, start, end),
            SetupExcept { handler } => w!(SetupExcept, handler),
            SetupFinally { handler } => w!(SetupFinally, handler),
            EnterFinally => w!(EnterFinally),
            EndFinally => w!(EndFinally),
            SetupWith { end } => w!(SetupWith, end),
            WithCleanupStart => w!(WithCleanupStart),
            WithCleanupFinish => w!(WithCleanupFinish),
            BeforeAsyncWith => w!(BeforeAsyncWith),
            SetupAsyncWith { end } => w!(SetupAsyncWith, end),
            PopBlock => w!(PopBlock),
            Raise { argc } => w!(Raise, argc),
            BuildString { size } => w!(BuildString, size),
            BuildTuple { size, unpack } => w!(BuildTuple, size, unpack),
            BuildList { size, unpack } => w!(BuildList, size, unpack),
            BuildSet { size, unpack } => w!(BuildSet, size, unpack),
            BuildMap {
                size,
                unpack,
                for_call,
            } => w!(BuildMap, size, unpack, for_call),
            BuildSlice { size } => w!(BuildSlice, size),
            ListAppend { i } => w!(ListAppend, i),
            SetAdd { i } => w!(SetAdd, i),
            MapAddRev { i } => w!(MapAddRev, i),
            PrintExpr => w!(PrintExpr),
            LoadBuildClass => w!(LoadBuildClass),
            UnpackSequence { size } => w!(UnpackSequence, size),
            UnpackEx { before, after } => w!(UnpackEx, before, after),
            FormatValue { .. } => w!(FormatValue), // TODO: write conversion
            PopException => w!(PopException),
            Reverse { amount } => w!(Reverse, amount),
            GetAwaitable => w!(GetAwaitable),
            GetAIter => w!(GetAIter),
            GetANext => w!(GetANext),
            MapAdd { i } => w!(MapAdd, i),
        }
    }
}

impl fmt::Display for ConstantData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.borrow_constant().fmt_display(f)
    }
}

impl<C: Constant> fmt::Debug for CodeObject<C> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "<code object {} at ??? file {:?}, line {}>",
            self.obj_name, self.source_path, self.first_line_number
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct FrozenModule<C: Constant = ConstantData> {
    #[serde(bound(
        deserialize = "C: serde::Deserialize<'de>, C::Name: serde::Deserialize<'de>",
        serialize = "C: serde::Serialize, C::Name: serde::Serialize"
    ))]
    pub code: CodeObject<C>,
    pub package: bool,
}
