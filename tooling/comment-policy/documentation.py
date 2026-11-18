import ast
from pathlib import Path

from policy_models import Comment, SyntaxMatch


def find_documentation(matches: list[SyntaxMatch]) -> list[Comment]:
    python_locations: dict[str, set[tuple[int, int]]] = {}
    findings: list[Comment] = []
    for match in matches:
        if not match.ruleId.startswith("documentation-"):
            continue
        if match.language != "Python":
            findings.append(match)
            continue
        if match.file not in python_locations:
            tree = ast.parse(Path(match.file).read_text())
            python_locations[match.file] = {
                (node.body[0].lineno - 1, node.body[0].col_offset)
                for node in ast.walk(tree)
                if isinstance(
                    node,
                    (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef),
                )
                and ast.get_docstring(node) is not None
            }
        if (match.range.start.line, match.range.start.column) in python_locations[
            match.file
        ]:
            findings.append(match)
    return findings
