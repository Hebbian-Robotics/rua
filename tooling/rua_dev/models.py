from enum import StrEnum


class SdkTarget(StrEnum):
    PYTHON = "python"
    RUST = "rust"
    TYPESCRIPT = "typescript"


class BuildPlatform(StrEnum):
    LOCAL = "local"
    MANYLINUX = "manylinux"


class ValidationLane(StrEnum):
    PYTHON = "python"
    VIEWER = "viewer"
    FAST = "fast"
    EXECUTOR = "executor"
    CONTRACTS = "contracts"
    FULL = "full"
