from __future__ import annotations

import argparse
import json
import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

from documentation import find_documentation
from policy_models import Comment, SyntaxMatch
from pydantic import TypeAdapter, ValidationError
from schema_metadata import find_schema_descriptions

LABEL = re.compile(
    r"^(?://|/\*+|#)\s*(?:\*\s*)?"
    r"(?:WHY|INVARIANT|CONSTRAINT|TODO|SAFETY):\s+[^\s*]",
    re.DOTALL,
)

COMMENT_GUIDANCE = (
    "Code is the primary documentation. Prefer clear names, types and structure\n"
    "over code comments, and remove comments that merely repeat the code.\n"
    "Keep a comment only when it adds a non-obvious reason: a decision, invariant,\n"
    "external constraint, safety argument or unfinished work. Begin its paragraph\n"
    "with WHY:, INVARIANT:, CONSTRAINT:, SAFETY: or TODO: and explain that reason."
)


DOCUMENTATION_GUIDANCE = (
    "Documentation strings and comments are permitted only in the directories\n"
    "listed in tooling/comment-policy/public-interfaces.toml. Remove them, or\n"
    "replace a useful implementation explanation with a WHY:, INVARIANT:,\n"
    "CONSTRAINT:, SAFETY: or TODO: comment. If the code is a public interface,\n"
    "move it into a listed directory. Schema descriptions belong in supported\n"
    "public schemas, not internal documentation. Do not move docstrings into\n"
    "schema metadata merely to bypass this rule."
)

POLICY_GUIDANCE = COMMENT_GUIDANCE + "\n\n" + DOCUMENTATION_GUIDANCE

REPOSITORY_ROOT = Path(__file__).resolve().parents[2]


def load_public_directories() -> list[Path]:
    configuration = tomllib.loads(
        Path(__file__).with_name("public-interfaces.toml").read_text()
    )
    return [REPOSITORY_ROOT / directory for directory in configuration["directories"]]


def is_in_public_interface(file: str, public_directories: list[Path]) -> bool:
    resolved = Path(file).resolve()
    return any(resolved.is_relative_to(directory) for directory in public_directories)


def report_failure(message: str, exit_code: int, *, json_output: bool) -> int:
    if json_output:
        print(json.dumps({"reason": POLICY_GUIDANCE, "findings": [], "error": message}))
    else:
        print(f"{message}\n\n{POLICY_GUIDANCE}", file=sys.stderr)
    return exit_code


def find_unclassified_groups(comments: list[Comment]) -> list[Comment]:
    sources: dict[str, bytes] = {}
    findings: list[Comment] = []
    previous_comment: Comment | None = None
    previous_indentation: bytes | None = None
    for comment in sorted(
        comments, key=lambda comment: (comment.file, comment.range.byteOffset.start)
    ):
        if comment.file not in sources:
            sources[comment.file] = Path(comment.file).read_bytes()
        source = sources[comment.file]
        offset = comment.range.byteOffset.start
        line_start = source.rfind(b"\n", 0, offset) + 1
        prefix = source[line_start:offset]
        indentation = (
            prefix
            if comment.text.startswith(("//", "#")) and not prefix.strip(b" \t")
            else None
        )
        continues_group = (
            previous_comment is not None
            and previous_comment.file == comment.file
            and previous_comment.range.end.line + 1 == comment.range.start.line
            and indentation is not None
            and previous_indentation == indentation
        )
        if not continues_group and not LABEL.match(comment.text):
            findings.append(comment)
        previous_comment = comment
        previous_indentation = indentation
    return findings


def main() -> int:
    parser = argparse.ArgumentParser(description="Check internal comment paragraphs.")
    parser.add_argument("paths", nargs="*", default=["."])
    parser.add_argument("--json", action="store_true", help="Print a JSON report.")
    arguments = parser.parse_args()
    executable = shutil.which("ast-grep")
    if executable is None:
        return report_failure(
            "ast-grep is required; see tooling/comment-policy/README.md.",
            2,
            json_output=arguments.json,
        )
    try:
        result = subprocess.run(
            [
                executable,
                "scan",
                "--config",
                str(Path(__file__).with_name("sgconfig.yml")),
                "--no-ignore",
                "hidden",
                "--json=compact",
                *arguments.paths,
            ],
            capture_output=True,
            text=True,
            check=True,
        )
        comments = TypeAdapter(list[SyntaxMatch]).validate_json(result.stdout)
        findings = find_unclassified_groups(
            [c for c in comments if c.ruleId.startswith("explanatory-comment-")]
        )
        public_directories = load_public_directories()
        internal_matches = [
            match
            for match in comments
            if not is_in_public_interface(match.file, public_directories)
        ]
        findings += find_schema_descriptions(internal_matches)
        findings += find_documentation(internal_matches)
    except subprocess.CalledProcessError as error:
        return report_failure(
            f"ast-grep failed:\n{error.stderr.strip()}", 2, json_output=arguments.json
        )
    except (OSError, SyntaxError, ValidationError) as error:
        return report_failure(
            f"Cannot check comments: {error}", 2, json_output=arguments.json
        )
    if arguments.json:
        print(
            json.dumps(
                {
                    "reason": POLICY_GUIDANCE if findings else None,
                    "findings": [comment.model_dump() for comment in findings],
                }
            )
        )
        return int(bool(findings))
    for comment in findings:
        action = (
            "Remove this documentation or move it into a public interface directory."
            if comment.ruleId.startswith(("documentation-", "schema-"))
            else "Remove this comment or label the special reason it must remain."
        )
        print(f"{comment.file}:{comment.range.start.line + 1}: {action}")
    if findings:
        return report_failure(
            f"{len(findings)} comment or documentation policy violations.",
            1,
            json_output=False,
        )
    print("0 comment or documentation policy violations.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
