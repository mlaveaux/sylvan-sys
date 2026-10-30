#!/usr/bin/env python3
"""
Generates `src/macros.rs`, which ports Sylvan's function-like and alias macros to Rust.

Sylvan exposes most of its operations through C macros (e.g. `sylvan_and(a, b)` expands to
`RUN(sylvan_and, a, b, 0)`), which bindgen cannot translate. This script parses the macros in
the public Sylvan headers and translates them into Rust functions that call the functions they
expand to. Argument and return types are taken from the signatures in the bindgen output.

Usage: scripts/generate_macros.py [path to bindings.rs]

By default, the most recent `bindings.rs` under `target/` is used, so run `cargo build` first.
Macros that cannot be translated are reported, and should either be ported by hand in
`src/lib.rs` or added to SKIP.
"""

import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SYLVAN = ROOT / "3rd-party" / "sylvan" / "src"
OUTPUT = ROOT / "src" / "macros.rs"
HEADERS = [
    "sylvan_common.h",
    "sylvan_stats.h",
    "sylvan_mtbdd.h",
    "sylvan_bdd.h",
    "sylvan_ldd.h",
    "sylvan_zdd.h",
]

# Macros that are not part of the API, or cannot be expressed as functions.
SKIP = {
    "OPCOUNTER",  # Internal helper for the statistics counters.
    "getabstime",  # Internal timer for the statistics.
    "sylvan_gc_test",  # Expands to `YIELD_NEWFRAME()`, which requires the Lace worker variables.
    "zdd_notify_ondead",  # Refers to the internal `nodes` table.
    "mtbdd_printdot",  # Uses `stdout`, call `mtbdd_fprintdot` instead.
    "mtbdd_printdot_nc",  # Uses `stdout`, call `mtbdd_fprintdot_nc` instead.
    "zdd_printdot",  # Uses `stdout`, call `zdd_fprintdot` instead.
    "sylvan_printdot",  # Alias of `mtbdd_printdot`.
    "sylvan_makemapnode",  # Refers to `mtbdd_makemapnode`, which does not exist.
    "zdd_set_next",  # Refers to `zdd_high`, which does not exist.
    "zdd_enum",  # The task is declared but not implemented.
    "zdd_enum_seq",  # The task is declared but not implemented.
    "zdd_collect",  # The task is declared but not implemented.
    # The following are ported by hand in `src/lib.rs`.
    "lddmc_satcount",  # Returns `long double`, which has no Rust equivalent.
    "mtbdd_leafcount",
    "zdd_isnode",
    "zdd_set_empty",
    "zdd_set_isempty",
    "zdd_set_from_mtbdd",
    "zdd_map_empty",
    "zdd_map_isempty",
}

RUST_KEYWORDS = {"match", "type", "fn", "mod", "ref", "move", "loop", "impl", "use", "in", "box"}


@dataclass
class Signature:
    params: list[str]  # The Rust parameter types.
    ret: str | None  # The Rust return type, or None for `void`.


@dataclass
class Macro:
    name: str
    params: list[str] | None  # None for an alias, i.e. `#define name target`.
    body: str
    header: str


@dataclass
class Function:
    params: list[str]  # Rust parameters, e.g. `a: BDD`.
    signature: Signature
    body: str


class Unsupported(Exception):
    pass


def rust_name(name: str) -> str:
    """Renames C identifiers that are keywords in Rust."""
    return f"{name}_" if name in RUST_KEYWORDS else name


def split_args(text: str) -> list[str]:
    """Splits a comma separated list of Rust parameters, respecting nested brackets."""
    args, depth, current = [], 0, ""
    for c in text:
        if c in "(<":
            depth += 1
        elif c == ")" or (c == ">" and not current.endswith("-")):
            depth -= 1
        if c == "," and depth == 0:
            args.append(current.strip())
            current = ""
        else:
            current += c
    return [arg for arg in args + [current.strip()] if arg]


def parse_bindings(path: Path) -> dict[str, Signature]:
    """Returns the signatures of all foreign functions in the bindgen output."""
    text = path.read_text()
    signatures = {}
    for match in re.finditer(r"pub fn (\w+)\(", text):
        end, depth = match.end(), 1
        while depth:
            depth += {"(": 1, ")": -1}.get(text[end], 0)
            end += 1
        params = split_args(text[match.end() : end - 1])
        ret = re.match(r"\s*(?:->\s*([^;{]+?))?\s*;", text[end:])
        # Skip methods, e.g. the bitfield accessors, which have a body or take `self`.
        if ret and all(":" in p for p in params):
            types = [p.split(":", 1)[1].strip() for p in params]
            signatures[match.group(1)] = Signature(types, ret.group(1))
    return signatures


def parse_macros() -> list[Macro]:
    """Returns the function-like and alias macros of the public headers, in header order."""
    macros = []
    for header in HEADERS:
        text = (SYLVAN / header).read_text().replace("\\\n", " ")
        for line in text.splitlines():
            if m := re.match(r"\s*#define\s+(\w+)\(([^)]*)\)\s*(.*)", line):
                params = [p.strip() for p in m.group(2).split(",") if p.strip()]
                macros.append(Macro(m.group(1), params, m.group(3).strip(), header))
            elif m := re.match(r"\s*#define\s+([a-z]\w*)\s+([a-z_]\w*)\s*$", line):
                macros.append(Macro(m.group(1), None, m.group(2), header))
    return [m for m in macros if m.name not in SKIP]


class Generator:
    """Translates macros, translating the macros they refer to on demand."""

    def __init__(self, bindings: dict[str, Signature], macros: list[Macro]) -> None:
        self.bindings = bindings
        self.macros = {m.name: m for m in macros}
        self.functions: dict[str, Function | Unsupported] = {}

    def signature(self, name: str) -> Signature:
        """Returns the signature of a bindgen function or a (translated) macro."""
        if name in self.bindings:
            return self.bindings[name]
        macro = self.macros.get(name)
        if macro is None:
            raise Unsupported(f"unknown function `{name}`")
        if macro.params is None:
            return self.signature(macro.body)
        function = self.translate(macro)
        if isinstance(function, Unsupported):
            raise Unsupported(f"`{name}` is not supported: {function}")
        return function.signature

    def translate(self, macro: Macro) -> Function | Unsupported:
        if macro.name not in self.functions:
            self.functions[macro.name] = Unsupported("recursive macro")
            try:
                self.functions[macro.name] = MacroParser(self, macro).parse()
            except Unsupported as e:
                self.functions[macro.name] = e
        return self.functions[macro.name]


class MacroParser:
    """Translates the body of a single function-like macro into a Rust expression."""

    def __init__(self, generator: Generator, macro: Macro) -> None:
        assert macro.params is not None
        self.generator = generator
        self.tokens = re.findall(r"\w+|\.\.\.|\S", macro.body.rstrip(";").strip())
        self.pos = 0
        self.param_names = macro.params
        self.types: dict[str, str | None] = {p: None for p in macro.params if p != "..."}
        self.mutable: set[str] = set()
        self.variadic: list[str] = []

    def parse(self) -> Function:
        body, ret = self.expression(None)
        if self.pos != len(self.tokens):
            raise Unsupported(f"unexpected `{' '.join(self.tokens[self.pos :])}`")
        if self.variadic:
            params = self.variadic
        else:
            params = []
            for name, ty in self.types.items():
                if ty is None:
                    raise Unsupported(f"could not infer the type of `{name}`")
                mut = "mut " if name in self.mutable else ""
                params.append(f"{mut}{rust_name(name)}: {ty}")
        types = [p.split(": ", 1)[1] for p in params]
        return Function(params, Signature(types, ret), body)

    def peek(self) -> str | None:
        return self.tokens[self.pos] if self.pos < len(self.tokens) else None

    def take(self, expected: str | None = None) -> str:
        token = self.peek()
        if token is None or (expected is not None and token != expected):
            raise Unsupported(f"expected `{expected}`, found `{token}`")
        self.pos += 1
        return token

    def expression(self, expected: str | None) -> tuple[str, str | None]:
        """Parses one expression of the given type; returns the Rust code and its type."""
        token = self.take()
        if token == "(":
            result = self.expression(expected)
            self.take(")")
            return result
        if token == "&":
            name = self.take()
            if name not in self.types or not expected or not expected.startswith("*mut "):
                raise Unsupported("unsupported use of `&`")
            self.mutable.add(name)
            self.types[name] = expected.removeprefix("*mut ")
            return f"&mut {rust_name(name)}", expected
        if token.isdigit():
            if expected and expected.startswith("*"):
                return "core::ptr::null_mut()", expected
            return token, expected
        if not re.fullmatch(r"\w+", token):
            raise Unsupported(f"unsupported token `{token}`")
        if self.peek() == "(":
            return self.call(token)
        if token in self.types:
            self.types[token] = self.types[token] or expected
            return rust_name(token), expected
        if token.endswith("_CALL") and token in self.generator.bindings:
            return f"Some({token})", expected  # A task, passed as a function pointer.
        raise Unsupported(f"unknown identifier `{token}`")

    def call(self, name: str) -> tuple[str, str | None]:
        """Parses the arguments of a call to `name`, where `RUN(f, ...)` calls `f_RUN(...)`."""
        self.take("(")
        if name == "RUN":
            name = f"{self.take()}_RUN"
            if self.peek() == ",":
                self.take(",")
        signature = self.generator.signature(name)

        if self.peek() == "__VA_ARGS__":
            # Forward all the parameters of the called function.
            self.take()
            self.take(")")
            names = [f"arg{i + 1}" for i in range(len(signature.params))]
            self.variadic = [f"{n}: {t}" for n, t in zip(names, signature.params)]
            return f"{name}({', '.join(names)})", signature.ret

        args = []
        while self.peek() != ")":
            if len(args) == len(signature.params):
                raise Unsupported(f"too many arguments for `{name}`")
            args.append(self.expression(signature.params[len(args)])[0])
            if self.peek() == ",":
                self.take(",")
        self.take(")")
        if len(args) != len(signature.params):
            raise Unsupported(f"too few arguments for `{name}`")
        return f"{name}({', '.join(args)})", signature.ret


def render(generator: Generator, macros: list[Macro]) -> str:
    out = [
        "// This file is generated by `scripts/generate_macros.py`, do not edit it manually.",
        "//",
        "// Rust versions of the function-like and alias macros in the Sylvan headers.",
        "",
        "use crate::*;",
    ]
    header = None
    for macro in macros:
        try:
            generator.signature(macro.name)
        except Unsupported as e:
            print(f"skipped {macro.name}: {e}", file=sys.stderr)
            continue

        if macro.header != header:
            header = macro.header
            out += ["", f"// {header}"]
        out.append("")
        if macro.params is None:
            out.append(f"pub use crate::{macro.body} as {macro.name};")
            continue

        function = generator.functions[macro.name]
        assert isinstance(function, Function)
        ret = f" -> {function.signature.ret}" if function.signature.ret else ""
        out += [
            f"/// Rust version of the C macro `{macro.name}({', '.join(macro.params)})`, which",
            f"/// expands to `{macro.body.rstrip(';').strip()}`.",
            "#[inline]",
            f"pub unsafe fn {macro.name}({', '.join(function.params)}){ret} {{",
            f"    unsafe {{ {function.body} }}",
            "}",
        ]
    return "\n".join(out) + "\n"


def find_bindings() -> Path:
    candidates = list((ROOT / "target").glob("*/build/sylvan-sys-*/out/bindings.rs"))
    if not candidates:
        sys.exit("No bindings.rs found, run `cargo build` first.")
    return max(candidates, key=lambda p: p.stat().st_mtime)


def main() -> None:
    if len(sys.argv) > 2:
        sys.exit(__doc__)
    bindings = parse_bindings(Path(sys.argv[1]) if len(sys.argv) == 2 else find_bindings())
    # Names that bindgen already provides must not be redefined.
    macros = [m for m in parse_macros() if m.name not in bindings]

    OUTPUT.write_text(render(Generator(bindings, macros), macros))
    subprocess.run(["rustfmt", "--edition", "2024", str(OUTPUT)], check=True)


if __name__ == "__main__":
    main()
