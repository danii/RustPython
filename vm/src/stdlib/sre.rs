mod constants;
mod interp;

pub(crate) use _sre::make_module;

#[pymodule]
mod _sre {
    use itertools::Itertools;
    use rustpython_common::borrow::BorrowValue;
    use rustpython_common::lock::OnceCell;

    use super::constants::SreFlag;
    use super::interp::{self, lower_ascii, lower_unicode, upper_unicode, State};
    use crate::builtins::tuple::PyTupleRef;
    use crate::builtins::{PyDictRef, PyList, PyStr, PyStrRef, PyTypeRef};
    use crate::function::{Args, OptionalArg};
    use crate::pyobject::{Either, IntoPyObject, PyCallable, PyIterable, PyObjectRef, PyRef, PyResult, PyValue, StaticType};
    use crate::VirtualMachine;
    use std::convert::TryFrom;

    #[pyattr]
    pub const CODESIZE: usize = 4;
    #[pyattr]
    pub use super::constants::SRE_MAGIC as MAGIC;
    #[cfg(target_pointer_width = "32")]
    #[pyattr]
    pub const MAXREPEAT: usize = usize::MAX;
    #[cfg(target_pointer_width = "64")]
    #[pyattr]
    pub const MAXREPEAT: usize = u32::MAX as usize;
    #[cfg(target_pointer_width = "32")]
    #[pyattr]
    pub const MAXGROUPS: usize = MAXREPEAT / 4 / 2;
    #[cfg(target_pointer_width = "64")]
    #[pyattr]
    pub const MAXGROUPS: usize = MAXREPEAT / 2;

    #[pyfunction]
    fn getcodesize() -> usize {
        CODESIZE
    }
    #[pyfunction]
    fn ascii_iscased(ch: i32) -> bool {
        (ch >= b'a' as i32 && ch <= b'z' as i32) || (ch >= b'A' as i32 && ch <= b'Z' as i32)
    }
    #[pyfunction]
    fn unicode_iscased(ch: i32) -> bool {
        let ch = ch as u32;
        let ch = match char::try_from(ch) {
            Ok(ch) => ch,
            Err(_) => {
                return false;
            }
        };
        ch != lower_unicode(ch) || ch != upper_unicode(ch)
    }
    #[pyfunction]
    fn ascii_tolower(ch: i32) -> i32 {
        let ch = match char::try_from(ch as u32) {
            Ok(ch) => ch,
            Err(_) => {
                return ch;
            }
        };
        lower_ascii(ch) as i32
    }
    #[pyfunction]
    fn unicode_tolower(ch: i32) -> i32 {
        let ch = match char::try_from(ch as u32) {
            Ok(ch) => ch,
            Err(_) => {
                return ch;
            }
        };
        lower_unicode(ch) as i32
    }

    #[pyfunction]
    fn compile(
        pattern: PyObjectRef,
        flags: u16,
        code: PyObjectRef,
        groups: usize,
        groupindex: PyDictRef,
        indexgroup: PyObjectRef,
        vm: &VirtualMachine,
    ) -> PyResult<Pattern> {
        Ok(Pattern {
            pattern,
            flags: SreFlag::from_bits_truncate(flags),
            code: vm.extract_elements::<u32>(&code)?,
            groups,
            groupindex,
            indexgroup: vm.extract_elements(&indexgroup)?,
        })
    }

    #[derive(FromArgs)]
    struct StringArgs {
        #[pyarg(any)]
        string: PyStrRef,
        #[pyarg(any, default = "0")]
        pos: usize,
        #[pyarg(any, default = "std::isize::MAX as usize")]
        endpos: usize,
    }

    #[derive(FromArgs)]
    struct SubArgs {
        #[pyarg(any)]
        repl: Either<PyCallable, PyStrRef>,
        #[pyarg(any)]
        string: PyStrRef,
        #[pyarg(any, default = "0")]
        count: usize,
    }

    #[pyattr]
    #[pyclass(name = "Pattern")]
    #[derive(Debug)]
    pub(crate) struct Pattern {
        pub pattern: PyObjectRef,
        pub flags: SreFlag,
        pub code: Vec<u32>,
        pub groups: usize,
        pub groupindex: PyDictRef,
        pub indexgroup: Vec<Option<String>>,
    }

    impl PyValue for Pattern {
        fn class(_vm: &VirtualMachine) -> &PyTypeRef {
            Self::static_type()
        }
    }

    #[pyimpl]
    impl Pattern {
        #[pymethod(name = "match")]
        fn pymatch(
            zelf: PyRef<Pattern>,
            string_args: StringArgs,
            vm: &VirtualMachine,
        ) -> Option<PyRef<Match>> {
            interp::pymatch(
                string_args.string,
                string_args.pos,
                string_args.endpos,
                zelf,
            )
            .map(|x| x.into_ref(vm))
        }
        #[pymethod]
        fn fullmatch(
            zelf: PyRef<Pattern>,
            string_args: StringArgs,
            vm: &VirtualMachine,
        ) -> Option<PyRef<Match>> {
            // TODO: need optimize
            let m = Self::pymatch(zelf, string_args, vm);
            if let Some(m) = m {
                if m.regs[0].0 == m.pos as isize && m.regs[0].1 == m.endpos as isize {
                    return Some(m);
                }
            }
            None
        }
        #[pymethod]
        fn search(
            zelf: PyRef<Pattern>,
            string_args: StringArgs,
            vm: &VirtualMachine,
        ) -> Option<PyRef<Match>> {
            interp::search(
                string_args.string,
                string_args.pos,
                string_args.endpos,
                zelf,
            )
            .map(|x| x.into_ref(vm))
        }
        #[pymethod]
        fn findall(&self, string_args: StringArgs) -> Option<PyObjectRef> {
            None
        }
        #[pymethod]
        fn finditer(&self, string_args: StringArgs) -> Option<PyObjectRef> {
            None
        }
        #[pymethod]
        fn scanner(&self, string_args: StringArgs) -> Option<PyObjectRef> {
            None
        }
        #[pymethod]
        fn sub(zelf: PyRef<Pattern>, sub_args: SubArgs, vm: &VirtualMachine) -> PyResult {
            Self::subx(zelf, sub_args, false, vm)
        }
        #[pymethod]
        fn subn(zelf: PyRef<Pattern>, sub_args: SubArgs, vm: &VirtualMachine) -> PyResult {
            Self::subx(zelf, sub_args, true, vm)
        }

        #[pyproperty]
        fn flags(&self) -> u16 {
            self.flags.bits()
        }
        #[pyproperty]
        fn groupindex(&self) -> PyDictRef {
            self.groupindex.clone()
        }
        #[pyproperty]
        fn groups(&self) -> usize {
            self.groups
        }
        #[pyproperty]
        fn pattern(&self) -> PyObjectRef {
            self.pattern.clone()
        }

        fn subx(
            zelf: PyRef<Pattern>,
            sub_args: SubArgs,
            subn: bool,
            vm: &VirtualMachine,
        ) -> PyResult {
            let filter: PyObjectRef = match sub_args.repl {
                Either::A(callable) => callable.into_object(),
                Either::B(s) => {
                    if s.borrow_value().contains('\\') {
                        // handle non-literal strings ; hand it over to the template compiler
                        let re = vm.import("re", &[], 0)?;
                        let func = vm.get_attribute(re, "_subx")?;
                        vm.invoke(&func, (zelf.clone(), s))?
                    } else {
                        s.into_object()
                    }
                }
            };

            let mut sublist: Vec<PyObjectRef> = Vec::new();

            let mut n = 0;
            let mut last_pos = 0;
            while sub_args.count == 0 || n < sub_args.count {
                let m = match interp::search(
                    sub_args.string.clone(),
                    last_pos,
                    std::usize::MAX,
                    zelf.clone(),
                ) {
                    Some(m) => m,
                    None => {
                        break;
                    }
                };
                let start = m.regs[0].0 as usize;
                if last_pos < start {
                    /* get segment before this match */
                    sublist.push(
                        m.string
                            .borrow_value()
                            .chars()
                            .take(start)
                            .skip(last_pos)
                            .collect::<String>()
                            .into_pyobject(vm),
                    );
                }

                last_pos = m.regs[0].1 as usize;
                if last_pos == start {
                    last_pos += 1;
                }

                if vm.is_callable(&filter) {
                    let ret = vm.invoke(&filter, (m.into_ref(vm),))?;
                    sublist.push(ret);
                } else {
                    sublist.push(filter.clone());
                }

                n += 1;
            }

            /* get segment following last match */
            sublist.push(
                sub_args
                    .string
                    .borrow_value()
                    .chars()
                    .skip(last_pos)
                    .collect::<String>()
                    .into_pyobject(vm),
            );

            let list = PyList::from(sublist).into_object(vm);
            let s = vm.ctx.new_str("");
            let ret = vm.call_method(&s, "join", (list,))?;

            Ok(if subn {
                (ret, n).into_pyobject(vm)
            } else {
                ret
            })
        }
    }

    #[pyattr]
    #[pyclass(name = "Match")]
    #[derive(Debug)]
    pub(crate) struct Match {
        string: PyStrRef,
        pattern: PyRef<Pattern>,
        pos: usize,
        endpos: usize,
        lastindex: isize,
        regs: Vec<(isize, isize)>,
        regs_pytuple: OnceCell<PyTupleRef>,
        // lastgroup
    }
    impl PyValue for Match {
        fn class(_vm: &VirtualMachine) -> &PyTypeRef {
            Self::static_type()
        }
    }

    #[pyimpl]
    impl Match {
        pub(crate) fn new(state: &State, pattern: PyRef<Pattern>, string: PyStrRef) -> Self {
            let mut regs = vec![(state.start as isize, state.string_position as isize)];
            for group in 0..pattern.groups {
                let mark_index = 2 * group;
                if mark_index + 1 < state.marks.len() {
                    if let (Some(start), Some(end)) =
                        (state.marks[mark_index], state.marks[mark_index + 1])
                    {
                        regs.push((start as isize, end as isize));
                        continue;
                    }
                }
                regs.push((-1, -1));
            }
            Self {
                string,
                pattern,
                pos: state.start,
                endpos: state.end,
                lastindex: state.lastindex,
                regs,
                regs_pytuple: OnceCell::new(),
            }
        }

        #[pyproperty]
        fn pos(&self) -> usize {
            self.pos
        }
        #[pyproperty]
        fn endpos(&self) -> usize {
            self.endpos
        }
        #[pyproperty]
        fn lastindex(&self) -> isize {
            self.lastindex
        }
        #[pyproperty]
        fn lastgroup(&self) -> Option<String> {
            None
        }
        #[pyproperty]
        fn re(&self) -> PyObjectRef {
            self.pattern.clone().into_object()
        }
        #[pyproperty]
        fn string(&self) -> PyStrRef {
            self.string.clone()
        }
        #[pyproperty]
        fn regs(&self, vm: &VirtualMachine) -> PyTupleRef {
            self.regs_pytuple
                .get_or_init(|| {
                    PyTupleRef::with_elements(
                        self.regs.iter().map(|&x| x.into_pyobject(vm)).collect(),
                        &vm.ctx,
                    )
                })
                .clone()
        }

        #[pymethod]
        fn start(&self, group: OptionalArg<isize>, vm: &VirtualMachine) -> PyResult<isize> {
            self.get_index(group.unwrap_or(0), vm)
                .map(|x| self.regs[x].0)
        }
        #[pymethod]
        fn end(&self, group: OptionalArg<isize>, vm: &VirtualMachine) -> PyResult<isize> {
            self.get_index(group.unwrap_or(0), vm)
                .map(|x| self.regs[x].1)
        }
        #[pymethod]
        fn span(&self, group: OptionalArg<isize>, vm: &VirtualMachine) -> PyResult<(isize, isize)> {
            self.get_index(group.unwrap_or(0), vm).map(|x| self.regs[x])
        }

        #[pymethod]
        fn expand(zelf: PyRef<Match>, template: PyStrRef, vm: &VirtualMachine) -> PyResult {
            let re = vm.import("re", &[], 0)?;
            let func = vm.get_attribute(re, "_expand")?;
            vm.invoke(&func, (zelf.pattern.clone(), zelf, template))
        }

        #[pymethod]
        fn group(&self, args: Args<isize>, vm: &VirtualMachine) -> PyResult {
            let mut args = args.into_vec();
            if args.is_empty() {
                args.push(0);
            }
            let mut v: Vec<PyObjectRef> = args
                .iter()
                .map(|&x| {
                    self.get_index(x, vm)
                        .map(|i| self.get_slice(i).unwrap().into_pyobject(vm))
                })
                .try_collect()?;
            if v.len() == 1 {
                Ok(v.pop().unwrap())
            } else {
                Ok(vm.ctx.new_tuple(v))
            }
        }

        #[pymethod(magic)]
        fn getitem(&self, index: isize, vm: &VirtualMachine) -> Option<String> {
            self.get_index(index, vm)
                .ok()
                .and_then(|i| self.get_slice(i))
        }

        #[pymethod]
        fn groups(
            zelf: PyRef<Match>,
            default: OptionalArg<PyObjectRef>,
            vm: &VirtualMachine,
        ) -> PyTupleRef {
            let default = default.unwrap_or(vm.ctx.none());
            let v: Vec<PyObjectRef> = (1..zelf.regs.len())
                .map(|i| {
                    zelf.get_slice(i)
                        .map(|s| s.into_pyobject(vm))
                        .unwrap_or_else(|| default.clone())
                })
                .collect();
            PyTupleRef::with_elements(v, &vm.ctx)
        }

        #[pymethod(magic)]
        fn repr(zelf: PyRef<Match>) -> String {
            format!(
                "<re.Match object; span=({}, {}), match='{}'>",
                zelf.regs[0].0,
                zelf.regs[0].1,
                zelf.get_slice(0).unwrap()
            )
        }

        fn get_index(&self, group: isize, vm: &VirtualMachine) -> PyResult<usize> {
            // TODO: support key, value index
            if group >= 0 && group as usize <= self.pattern.groups {
                Ok(group as usize)
            } else {
                Err(vm.new_index_error("no such group".to_owned()))
            }
        }

        fn get_slice(&self, group: usize) -> Option<String> {
            let (start, end) = self.regs[group];
            if start < 0 || end < 0 {
                return None;
            }
            Some(
                self.string
                    .borrow_value()
                    .chars()
                    .take(end as usize)
                    .skip(start as usize)
                    .collect(),
            )
        }
    }
}
