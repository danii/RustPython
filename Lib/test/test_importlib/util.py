import abc
import builtins
import contextlib
import errno
import functools
import importlib
from importlib import machinery, util, invalidate_caches
from importlib.abc import ResourceReader
import io
import os
import os.path
from pathlib import Path, PurePath
from test import support
import unittest
import sys
import tempfile
import types

from . import data01
from . import zipdata01


BUILTINS = types.SimpleNamespace()
BUILTINS.good_name = None
BUILTINS.bad_name = None
if 'errno' in sys.builtin_module_names:
    BUILTINS.good_name = 'errno'
if 'importlib' not in sys.builtin_module_names:
    BUILTINS.bad_name = 'importlib'

EXTENSIONS = types.SimpleNamespace()
EXTENSIONS.path = None
EXTENSIONS.ext = None
EXTENSIONS.filename = None
EXTENSIONS.file_path = None
EXTENSIONS.name = '_testcapi'

def _extension_details():
    global EXTENSIONS
    for path in sys.path:
        for ext in machinery.EXTENSION_SUFFIXES:
            filename = EXTENSIONS.name + ext
            file_path = os.path.join(path, filename)
            if os.path.exists(file_path):
                EXTENSIONS.path = path
                EXTENSIONS.ext = ext
                EXTENSIONS.filename = filename
                EXTENSIONS.file_path = file_path
                return

_extension_details()


def import_importlib(module_name):
    """Import a module from importlib both w/ and w/o _frozen_importlib."""
    fresh = ('importlib',) if '.' in module_name else ()
    frozen = support.import_fresh_module(module_name)
    source = support.import_fresh_module(module_name, fresh=fresh,
                                         blocked=('_frozen_importlib', '_frozen_importlib_external'))
    return {'Frozen': frozen, 'Source': source}


def specialize_class(cls, kind, base=None, **kwargs):
    # XXX Support passing in submodule names--load (and cache) them?
    # That would clean up the test modules a bit more.
    if base is None:
        base = unittest.TestCase
    elif not isinstance(base, type):
        base = base[kind]
    name = '{}_{}'.format(kind, cls.__name__)
    bases = (cls, base)
    specialized = types.new_class(name, bases)
    specialized.__module__ = cls.__module__
    specialized._NAME = cls.__name__
    specialized._KIND = kind
    for attr, values in kwargs.items():
        value = values[kind]
        setattr(specialized, attr, value)
    return specialized


def split_frozen(cls, base=None, **kwargs):
    frozen = specialize_class(cls, 'Frozen', base, **kwargs)
    source = specialize_class(cls, 'Source', base, **kwargs)
    return frozen, source


def test_both(test_class, base=None, **kwargs):
    return split_frozen(test_class, base, **kwargs)


CASE_INSENSITIVE_FS = True
# Windows is the only OS that is *always* case-insensitive
# (OS X *can* be case-sensitive).
if sys.platform not in ('win32', 'cygwin'):
    changed_name = __file__.upper()
    if changed_name == __file__:
        changed_name = __file__.lower()
    if not os.path.exists(changed_name):
        CASE_INSENSITIVE_FS = False

source_importlib = import_importlib('importlib')['Source']
__import__ = {'Frozen': staticmethod(builtins.__import__),
              'Source': staticmethod(source_importlib.__import__)}


def case_insensitive_tests(test):
    """Class decorator that nullifies tests requiring a case-insensitive
    file system."""
    return unittest.skipIf(not CASE_INSENSITIVE_FS,
                            "requires a case-insensitive filesystem")(test)


def submodule(parent, name, pkg_dir, content=''):
    path = os.path.join(pkg_dir, name + '.py')
    with open(path, 'w') as subfile:
        subfile.write(content)
    return '{}.{}'.format(parent, name), path


@contextlib.contextmanager
def uncache(*names):
    """Uncache a module from sys.modules.

    A basic sanity check is performed to prevent uncaching modules that either
    cannot/shouldn't be uncached.

    """
    for name in names:
        if name in ('sys', 'marshal', 'imp'):
            raise ValueError(
                "cannot uncache {0}".format(name))
        try:
            del sys.modules[name]
        except KeyError:
            pass
    try:
        yield
    finally:
        for name in names:
            try:
                del sys.modules[name]
            except KeyError:
                pass


@contextlib.contextmanager
def temp_module(name, content='', *, pkg=False):
    conflicts = [n for n in sys.modules if n.partition('.')[0] == name]
    with support.temp_cwd(None) as cwd:
        with uncache(name, *conflicts):
            with support.DirsOnSysPath(cwd):
                invalidate_caches()

                location = os.path.join(cwd, name)
                if pkg:
                    modpath = os.path.join(location, '__init__.py')
                    os.mkdir(name)
                else:
                    modpath = location + '.py'
                    if content is None:
                        # Make sure the module file gets created.
                        content = ''
                if content is not None:
                    # not a namespace package
                    with open(modpath, 'w') as modfile:
                        modfile.write(content)
                yield location


@contextlib.contextmanager
def import_state(**kwargs):
    """Context manager to manage the various importers and stored state in the
    sys module.

    The 'modules' attribute is not supported as the interpreter state stores a
    pointer to the dict that the interpreter uses internally;
    reassigning to sys.modules does not have the desired effect.

    """
    originals = {}
    try:
        for attr, default in (('meta_path', []), ('path', []),
                              ('path_hooks', []),
                              ('path_importer_cache', {})):
            originals[attr] = getattr(sys, attr)
            if attr in kwargs:
                new_value = kwargs[attr]
                del kwargs[attr]
            else:
                new_value = default
            setattr(sys, attr, new_value)
        if len(kwargs):
            raise ValueError(
                    'unrecognized arguments: {0}'.format(kwargs.keys()))
        yield
    finally:
        for attr, value in originals.items():
            setattr(sys, attr, value)


class _ImporterMock:

    """Base class to help with creating importer mocks."""

    def __init__(self, *names, module_code={}):
        self.modules = {}
        self.module_code = {}
        for name in names:
            if not name.endswith('.__init__'):
                import_name = name
            else:
                import_name = name[:-len('.__init__')]
            if '.' not in name:
                package = None
            elif import_name == name:
                package = name.rsplit('.', 1)[0]
            else:
                package = import_name
            module = types.ModuleType(import_name)
            module.__loader__ = self
            module.__file__ = '<mock __file__>'
            module.__package__ = package
            module.attr = name
            if import_name != name:
                module.__path__ = ['<mock __path__>']
            self.modules[import_name] = module
            if import_name in module_code:
                self.module_code[import_name] = module_code[import_name]

    def __getitem__(self, name):
        return self.modules[name]

    def __enter__(self):
        self._uncache = uncache(*self.modules.keys())
        self._uncache.__enter__()
        return self

    def __exit__(self, *exc_info):
        self._uncache.__exit__(None, None, None)


class mock_modules(_ImporterMock):

    """Importer mock using PEP 302 APIs."""

    def find_module(self, fullname, path=None):
        if fullname not in self.modules:
            return None
        else:
            return self

    def load_module(self, fullname):
        if fullname not in self.modules:
            raise ImportError
        else:
            sys.modules[fullname] = self.modules[fullname]
            if fullname in self.module_code:
                try:
                    self.module_code[fullname]()
                except Exception:
                    del sys.modules[fullname]
                    raise
            return self.modules[fullname]


class mock_spec(_ImporterMock):

    """Importer mock using PEP 451 APIs."""

    def find_spec(self, fullname, path=None, parent=None):
        try:
            module = self.modules[fullname]
        except KeyError:
            return None
        spec = util.spec_from_file_location(
                fullname, module.__file__, loader=self,
                submodule_search_locations=getattr(module, '__path__', None))
        return spec

    def create_module(self, spec):
        if spec.name not in self.modules:
            raise ImportError
        return self.modules[spec.name]

    def exec_module(self, module):
        try:
            self.module_code[module.__spec__.name]()
        except KeyError:
            pass


def writes_bytecode_files(fxn):
    """Decorator to protect sys.dont_write_bytecode from mutation and to skip
    tests that require it to be set to False."""
    if sys.dont_write_bytecode:
        return lambda *args, **kwargs: None
    @functools.wraps(fxn)
    def wrapper(*args, **kwargs):
        original = sys.dont_write_bytecode
        sys.dont_write_bytecode = False
        try:
            to_return = fxn(*args, **kwargs)
        finally:
            sys.dont_write_bytecode = original
        return to_return
    return wrapper


def ensure_bytecode_path(bytecode_path):
    """Ensure that the __pycache__ directory for PEP 3147 pyc file exists.

    :param bytecode_path: File system path to PEP 3147 pyc file.
    """
    try:
        os.mkdir(os.path.dirname(bytecode_path))
    except OSError as error:
        if error.errno != errno.EEXIST:
            raise


@contextlib.contextmanager
def temporary_pycache_prefix(prefix):
    """Adjust and restore sys.pycache_prefix."""
    _orig_prefix = sys.pycache_prefix
    sys.pycache_prefix = prefix
    try:
        yield
    finally:
        sys.pycache_prefix = _orig_prefix


@contextlib.contextmanager
def create_modules(*names):
    """Temporarily create each named module with an attribute (named 'attr')
    that contains the name passed into the context manager that caused the
    creation of the module.

    All files are created in a temporary directory returned by
    tempfile.mkdtemp(). This directory is inserted at the beginning of
    sys.path. When the context manager exits all created files (source and
    bytecode) are explicitly deleted.

    No magic is performed when creating packages! This means that if you create
    a module within a package you must also create the package's __init__ as
    well.

    """
    source = 'attr = {0!r}'
    created_paths = []
    mapping = {}
    state_manager = None
    uncache_manager = None
    try:
        temp_dir = tempfile.mkdtemp()
        mapping['.root'] = temp_dir
        import_names = set()
        for name in names:
            if not name.endswith('__init__'):
                import_name = name
            else:
                import_name = name[:-len('.__init__')]
            import_names.add(import_name)
            if import_name in sys.modules:
                del sys.modules[import_name]
            name_parts = name.split('.')
            file_path = temp_dir
            for directory in name_parts[:-1]:
                file_path = os.path.join(file_path, directory)
                if not os.path.exists(file_path):
                    os.mkdir(file_path)
                    created_paths.append(file_path)
            file_path = os.path.join(file_path, name_parts[-1] + '.py')
            with open(file_path, 'w') as file:
                file.write(source.format(name))
            created_paths.append(file_path)
            mapping[name] = file_path
        uncache_manager = uncache(*import_names)
        uncache_manager.__enter__()
        state_manager = import_state(path=[temp_dir])
        state_manager.__enter__()
        yield mapping
    finally:
        if state_manager is not None:
            state_manager.__exit__(None, None, None)
        if uncache_manager is not None:
            uncache_manager.__exit__(None, None, None)
        support.rmtree(temp_dir)


def mock_path_hook(*entries, importer):
    """A mock sys.path_hooks entry."""
    def hook(entry):
        if entry not in entries:
            raise ImportError
        return importer
    return hook


class CASEOKTestBase:

    def caseok_env_changed(self, *, should_exist):
        possibilities = b'PYTHONCASEOK', 'PYTHONCASEOK'
        if any(x in self.importlib._bootstrap_external._os.environ
                    for x in possibilities) != should_exist:
            self.skipTest('os.environ changes not reflected in _os.environ')


def create_package(file, path, is_package=True, contents=()):
    class Reader(ResourceReader):
        def get_resource_reader(self, package):
            return self

        def open_resource(self, path):
            self._path = path
            if isinstance(file, Exception):
                raise file
            else:
                return file

        def resource_path(self, path_):
            self._path = path_
            if isinstance(path, Exception):
                raise path
            else:
                return path

        def is_resource(self, path_):
            self._path = path_
            if isinstance(path, Exception):
                raise path
            for entry in contents:
                parts = entry.split('/')
                if len(parts) == 1 and parts[0] == path_:
                    return True
            return False

        def contents(self):
            if isinstance(path, Exception):
                raise path
            # There's no yield from in baseball, er, Python 2.
            for entry in contents:
                yield entry

    name = 'testingpackage'
    # Unforunately importlib.util.module_from_spec() was not introduced until
    # Python 3.5.
    module = types.ModuleType(name)
    loader = Reader()
    spec = machinery.ModuleSpec(
        name, loader,
        origin='does-not-exist',
        is_package=is_package)
    module.__spec__ = spec
    module.__loader__ = loader
    return module


class CommonResourceTests(abc.ABC):
    @abc.abstractmethod
    def execute(self, package, path):
        raise NotImplementedError

    def test_package_name(self):
        # Passing in the package name should succeed.
        self.execute(data01.__name__, 'utf-8.file')

    def test_package_object(self):
        # Passing in the package itself should succeed.
        self.execute(data01, 'utf-8.file')

    def test_string_path(self):
        # Passing in a string for the path should succeed.
        path = 'utf-8.file'
        self.execute(data01, path)

    @unittest.skipIf(sys.version_info < (3, 6), 'requires os.PathLike support')
    def test_pathlib_path(self):
        # Passing in a pathlib.PurePath object for the path should succeed.
        path = PurePath('utf-8.file')
        self.execute(data01, path)

    def test_absolute_path(self):
        # An absolute path is a ValueError.
        path = Path(__file__)
        full_path = path.parent/'utf-8.file'
        with self.assertRaises(ValueError):
            self.execute(data01, full_path)

    def test_relative_path(self):
        # A reative path is a ValueError.
        with self.assertRaises(ValueError):
            self.execute(data01, '../data01/utf-8.file')

    def test_importing_module_as_side_effect(self):
        # The anchor package can already be imported.
        del sys.modules[data01.__name__]
        self.execute(data01.__name__, 'utf-8.file')

    def test_non_package_by_name(self):
        # The anchor package cannot be a module.
        with self.assertRaises(TypeError):
            self.execute(__name__, 'utf-8.file')

    def test_non_package_by_package(self):
        # The anchor package cannot be a module.
        with self.assertRaises(TypeError):
            module = sys.modules['test.test_importlib.util']
            self.execute(module, 'utf-8.file')

    @unittest.skipIf(sys.version_info < (3,), 'No ResourceReader in Python 2')
    @unittest.skip("TODO: RUSTPYTHON")
    def test_resource_opener(self):
        bytes_data = io.BytesIO(b'Hello, world!')
        package = create_package(file=bytes_data, path=FileNotFoundError())
        self.execute(package, 'utf-8.file')
        self.assertEqual(package.__loader__._path, 'utf-8.file')

    @unittest.skipIf(sys.version_info < (3,), 'No ResourceReader in Python 2')
    @unittest.skip("TODO: RUSTPYTHON")
    def test_resource_path(self):
        bytes_data = io.BytesIO(b'Hello, world!')
        path = __file__
        package = create_package(file=bytes_data, path=path)
        self.execute(package, 'utf-8.file')
        self.assertEqual(package.__loader__._path, 'utf-8.file')

    def test_useless_loader(self):
        package = create_package(file=FileNotFoundError(),
                                 path=FileNotFoundError())
        with self.assertRaises(FileNotFoundError):
            self.execute(package, 'utf-8.file')


class ZipSetupBase:
    ZIP_MODULE = None

    @classmethod
    def setUpClass(cls):
        data_path = Path(cls.ZIP_MODULE.__file__)
        data_dir = data_path.parent
        cls._zip_path = str(data_dir / 'ziptestdata.zip')
        sys.path.append(cls._zip_path)
        cls.data = importlib.import_module('ziptestdata')

    @classmethod
    def tearDownClass(cls):
        try:
            sys.path.remove(cls._zip_path)
        except ValueError:
            pass

        try:
            del sys.path_importer_cache[cls._zip_path]
            del sys.modules[cls.data.__name__]
        except KeyError:
            pass

        try:
            del cls.data
            del cls._zip_path
        except AttributeError:
            pass

    def setUp(self):
        modules = support.modules_setup()
        self.addCleanup(support.modules_cleanup, *modules)


class ZipSetup(ZipSetupBase):
    ZIP_MODULE = zipdata01                          # type: ignore
