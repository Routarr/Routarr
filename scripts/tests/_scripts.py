"""Load a script of `scripts/` as a module: their file names carry hyphens."""

import importlib.util
import pathlib
import sys

SCRIPTS = pathlib.Path(__file__).resolve().parent.parent
ROOT = SCRIPTS.parent


def load(name: str):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), SCRIPTS / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    # Registered first, as an import does: a dataclass looks its module up while it is built.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module
