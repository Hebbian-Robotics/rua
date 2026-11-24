import ast
import subprocess
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DISTRIBUTION_DIRECTORY = ROOT
PACKAGE_DIRECTORY = ROOT / "hebbian_rua/client"
OWN_PACKAGE = "hebbian_rua"
ALLOWED_THIRD_PARTY = {
    "pydantic": "pydantic",
    "dotenv": "python-dotenv",
}
PROVIDER_ADAPTER_IMPORTS = {
    "anthropic.py": "anthropic",
    "gemini.py": "google",
    "openai.py": "openai",
}
HEAVY_MODULES = (
    "mujoco",
    "numpy",
    "cv2",
    "livekit",
    "fastapi",
    # INVARIANT: The LLM layer is provider-neutral.
    "anthropic",
    "openai",
    "google.genai",
)


def imported_modules(path: Path) -> set[str]:
    modules: set[str] = set()
    for node in ast.walk(ast.parse(path.read_text())):
        if isinstance(node, ast.Import):
            modules |= {alias.name.split(".")[0] for alias in node.names}
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            modules.add(node.module.split(".")[0])
    return modules


def package_sources() -> list[Path]:
    return sorted(PACKAGE_DIRECTORY.rglob("*.py"))


def test_the_client_imports_only_the_standard_library_and_its_declared_dependencies() -> (
    None
):
    for path in package_sources():
        third_party = {
            module
            for module in imported_modules(path)
            if module not in sys.stdlib_module_names and module != OWN_PACKAGE
        }
        allowed_imports = set(ALLOWED_THIRD_PARTY)
        if path.parent == PACKAGE_DIRECTORY / "llm":
            provider_import = PROVIDER_ADAPTER_IMPORTS.get(path.name)
            if provider_import is not None:
                allowed_imports.add(provider_import)
        unexpected = third_party - allowed_imports
        assert not unexpected, f"{path.relative_to(ROOT)} imports {sorted(unexpected)}"


def test_the_declared_dependencies_are_exactly_what_the_client_may_import() -> None:
    project = tomllib.loads((DISTRIBUTION_DIRECTORY / "pyproject.toml").read_text())
    names = {
        entry.split(">")[0].split("=")[0].split("<")[0].strip()
        for entry in project["project"]["dependencies"]
    }
    assert names == set(ALLOWED_THIRD_PARTY.values())


def test_importing_the_client_loads_no_robot_solver_or_video_stack() -> None:
    # WHY: A fresh interpreter: this process has long since imported everything.
    script = (
        "import sys, hebbian_rua.client, hebbian_rua.client.station_client\n"
        "import hebbian_rua.client.llm\n"
        f"print(','.join(m for m in {HEAVY_MODULES!r} if m in sys.modules))"
    )
    result = subprocess.run(
        [sys.executable, "-c", script], capture_output=True, text=True, check=True
    )
    assert result.stdout.strip() == "", f"the client imports {result.stdout.strip()}"
