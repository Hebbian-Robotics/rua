from enum import StrEnum


class SdkTarget(StrEnum):
    PYTHON = "python"
    RUST = "rust"
    TYPESCRIPT = "typescript"


class ValidationLane(StrEnum):
    PYTHON = "python"
    VIEWER = "viewer"
    FAST = "fast"
    SERVER = "server"
    CONTRACTS = "contracts"
    FULL = "full"
