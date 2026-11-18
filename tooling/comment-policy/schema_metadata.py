from __future__ import annotations

import ast
import re
from collections import defaultdict
from pathlib import Path

from policy_models import Comment, SyntaxMatch


class ScopeBindings(ast.NodeVisitor):
    def __init__(self) -> None:
        self.names: dict[str, str | None] = {}

    def bind(self, name: str, qualified: str | None) -> None:
        if name in self.names and self.names[name] != qualified:
            self.names[name] = None
        else:
            self.names[name] = qualified

    def visit_Import(self, node: ast.Import) -> None:
        for alias in node.names:
            self.bind(
                alias.asname or alias.name.split(".")[0],
                alias.name if alias.asname else alias.name.split(".")[0],
            )

    def visit_ImportFrom(self, node: ast.ImportFrom) -> None:
        for alias in node.names:
            self.bind(
                alias.asname or alias.name,
                f"{node.module}.{alias.name}" if not node.level else None,
            )

    def visit_Name(self, node: ast.Name) -> None:
        if isinstance(node.ctx, (ast.Store, ast.Del)):
            self.bind(node.id, None)

    def visit_FunctionDef(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        self.bind(node.name, None)

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        self.bind(node.name, None)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        pass


class PythonDescriptions(ast.NodeVisitor):
    def __init__(self) -> None:
        self.scopes: list[dict[str, str | None]] = []
        self.locations: set[tuple[int, int]] = set()

    def qualified_name(self, node: ast.expr) -> str | None:
        if isinstance(node, ast.Name):
            for scope in reversed(self.scopes):
                if node.id in scope:
                    return scope[node.id]
        if isinstance(node, ast.Attribute):
            parent = self.qualified_name(node.value)
            if parent is not None:
                return f"{parent}.{node.attr}"
        return None

    def visit_scope(
        self,
        body: list[ast.stmt],
        arguments: ast.arguments | None = None,
    ) -> None:
        bindings = ScopeBindings()
        for statement in body:
            bindings.visit(statement)
        if arguments is not None:
            for argument in ast.walk(arguments):
                if isinstance(argument, ast.arg):
                    bindings.bind(argument.arg, None)
        self.scopes.append(bindings.names)
        for statement in body:
            self.visit(statement)
        self.scopes.pop()

    def visit_Module(self, node: ast.Module) -> None:
        self.visit_scope(node.body)

    def visit_FunctionDef(self, node: ast.FunctionDef | ast.AsyncFunctionDef) -> None:
        for default in [*node.args.defaults, *node.args.kw_defaults]:
            if default is not None:
                self.visit(default)
        for decorator in node.decorator_list:
            self.visit(decorator)
        self.visit_scope(node.body, node.args)

    visit_AsyncFunctionDef = visit_FunctionDef

    def visit_ClassDef(self, node: ast.ClassDef) -> None:
        for base in node.bases:
            self.visit(base)
        self.visit_scope(node.body)

    def visit_Lambda(self, node: ast.Lambda) -> None:
        bindings = {
            argument.arg: None
            for argument in ast.walk(node.args)
            if isinstance(argument, ast.arg)
        }
        self.scopes.append(bindings)
        self.visit(node.body)
        self.scopes.pop()

    def visit_Call(self, node: ast.Call) -> None:
        qualified = self.qualified_name(node.func)
        if qualified in {
            "pydantic.Field",
            "pydantic.fields.Field",
            "pydantic.ConfigDict",
            "pydantic.config.ConfigDict",
        }:
            for keyword in node.keywords:
                if (
                    keyword.arg == "description"
                    and qualified.endswith(".Field")
                    and not (
                        isinstance(keyword.value, ast.Constant)
                        and keyword.value.value is None
                    )
                ):
                    self.locations.add((node.lineno - 1, node.col_offset))
                if keyword.arg == "json_schema_extra" and isinstance(
                    keyword.value, ast.Dict
                ):
                    for key, value in zip(
                        keyword.value.keys, keyword.value.values, strict=True
                    ):
                        if (
                            isinstance(key, ast.Constant)
                            and key.value == "description"
                            and not (
                                isinstance(value, ast.Constant) and value.value is None
                            )
                        ):
                            self.locations.add((node.lineno - 1, node.col_offset))
        self.generic_visit(node)


ZOD_FACTORIES = frozenset(
    [
        "string",
        "number",
        "bigint",
        "boolean",
        "date",
        "symbol",
        "undefined",
        "null",
        "any",
        "unknown",
        "never",
        "void",
        "array",
        "object",
        "strictObject",
        "looseObject",
        "union",
        "discriminatedUnion",
        "intersection",
        "tuple",
        "record",
        "partialRecord",
        "map",
        "set",
        "enum",
        "nativeEnum",
        "literal",
        "literals",
        "lazy",
        "promise",
        "function",
        "custom",
        "templateLiteral",
        "file",
        "instanceof",
        "coerce",
    ]
)


def zod_bindings(imports: list[SyntaxMatch]) -> tuple[set[str], set[str]]:
    namespaces: set[str] = set()
    factories: set[str] = set()
    for imported in imports:
        declaration = re.match(r"\s*import\s+([\s\S]*?)\s+from\s+['\"]", imported.text)
        if declaration is None:
            continue
        clause = declaration[1].strip()
        namespace = re.fullmatch(r"\*\s+as\s+([\w$]+)", clause)
        if namespace:
            namespaces.add(namespace[1])
        elif clause.startswith("{"):
            for specifier in clause.strip("{} ").split(","):
                binding = re.fullmatch(
                    r"\s*([\w$]+)(?:\s+as\s+([\w$]+))?\s*", specifier
                )
                if binding:
                    original, alias = binding[1], binding[2] or binding[1]
                    if original == "z":
                        namespaces.add(alias)
                    elif original in ZOD_FACTORIES:
                        factories.add(alias)
        elif re.fullmatch(r"[\w$]+", clause):
            namespaces.add(clause)
    return namespaces, factories


def is_zod_expression(
    expression: str, namespaces: set[str], factories: set[str], schemas: set[str]
) -> bool:
    root = re.match(r"^\s*([\w$]+)\s*(.*)", expression, re.DOTALL)
    if root is None:
        return False
    name, suffix = root[1], root[2]
    if name in schemas:
        return not suffix or suffix.startswith(".")
    if name in factories:
        return suffix.startswith("(")
    if name in namespaces:
        factory = re.match(r"^\.\s*([\w$]+)\s*\(", suffix)
        return factory is not None and factory[1] in ZOD_FACTORIES
    return False


def typescript_descriptions(matches: list[SyntaxMatch]) -> list[Comment]:
    namespaces, factories = zod_bindings(
        [match for match in matches if match.ruleId.endswith("-import")]
    )
    schemas: set[str] = set()
    bindings = [match for match in matches if match.ruleId.endswith("-binding")]
    while True:
        discovered = {
            binding.metaVariables.single["NAME"].text
            for binding in bindings
            if is_zod_expression(
                binding.metaVariables.single["VALUE"].text,
                namespaces,
                factories,
                schemas,
            )
        }
        if discovered <= schemas:
            break
        schemas |= discovered
    return [
        match.model_copy(update={"ruleId": "schema-description-zod"})
        for match in matches
        if match.ruleId.endswith("-description")
        and is_zod_expression(
            match.metaVariables.single["RECEIVER"].text, namespaces, factories, schemas
        )
    ]


def find_schema_descriptions(matches: list[SyntaxMatch]) -> list[Comment]:
    by_file: dict[str, list[SyntaxMatch]] = defaultdict(list)
    findings: list[Comment] = []
    for match in matches:
        if match.ruleId == "schema-rust-description":
            findings.append(match)
        elif match.ruleId.startswith("schema-"):
            by_file[match.file].append(match)
    for filename, candidates in by_file.items():
        if candidates[0].language == "Python":
            visitor = PythonDescriptions()
            visitor.visit(ast.parse(Path(filename).read_text()))
            findings.extend(
                candidate.model_copy(update={"ruleId": "schema-description-pydantic"})
                for candidate in candidates
                if (candidate.range.start.line, candidate.range.start.column)
                in visitor.locations
            )
        else:
            findings.extend(typescript_descriptions(candidates))
    return findings
