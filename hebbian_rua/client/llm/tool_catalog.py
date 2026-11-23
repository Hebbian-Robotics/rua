"""Compose the Rust-owned action schema and constrain its execution interface."""

from collections.abc import Callable, Iterable, Sequence
from dataclasses import dataclass, field, replace
from typing import Literal

from pydantic import TypeAdapter

from hebbian_rua.bindings import _native
from hebbian_rua.client.llm.types import RobotToolSpec

SequenceAction = Literal["pose", "joints", "gripper"]
RobotToolName = Literal[
    "action_sequence",
    "preview_pose",
    "move_arms",
    "set_arm_gripper",
    "rehome",
    "stop",
    "observe",
]


@dataclass(frozen=True)
class SingleArmSequence:
    """The adapter supplies the configured arm; the model sees no arm selector."""


@dataclass(frozen=True)
class NamedArmSequence:
    arms: tuple[str, ...]

    def __post_init__(self) -> None:
        if (
            not self.arms
            or any(not arm for arm in self.arms)
            or len(set(self.arms)) != len(self.arms)
        ):
            raise ValueError("Named-arm sequences need nonempty, unique arm names")


@dataclass(frozen=True)
class RobotToolConfiguration:
    sequence_actions: frozenset[SequenceAction] = field(
        default_factory=lambda: frozenset({"pose", "joints", "gripper"})
    )
    sequence_scope: SingleArmSequence | NamedArmSequence = field(
        default_factory=SingleArmSequence
    )
    tools: tuple[RobotToolName, ...] = (
        "action_sequence",
        "preview_pose",
        "stop",
        "observe",
    )

    def __post_init__(self) -> None:
        if not self.sequence_actions <= {"pose", "joints", "gripper"}:
            raise ValueError("Unknown sequence action")
        if "action_sequence" in self.tools and not self.sequence_actions:
            raise ValueError("An action sequence needs at least one available action")
        if len(set(self.tools)) != len(self.tools):
            raise ValueError("Tool names must be unique")


StandaloneToolName = Literal[
    "preview_pose", "move_arms", "set_arm_gripper", "rehome", "stop", "observe"
]


@dataclass(frozen=True)
class ActionSequenceTool:
    scope: SingleArmSequence | NamedArmSequence = field(
        default_factory=SingleArmSequence
    )
    actions: frozenset[SequenceAction] = field(
        default_factory=lambda: frozenset({"pose", "joints", "gripper"})
    )
    name: Literal["action_sequence"] = field(default="action_sequence", init=False)


@dataclass(frozen=True)
class StandaloneTool:
    name: StandaloneToolName


type RobotToolCapability = ActionSequenceTool | StandaloneTool
type ToolTransformation = Callable[
    [tuple[RobotToolCapability, ...]], Iterable[RobotToolCapability]
]


def all_robot_tool_capabilities() -> tuple[RobotToolCapability, ...]:
    name_adapter = TypeAdapter(StandaloneToolName)
    return tuple(
        ActionSequenceTool()
        if name == "action_sequence"
        else StandaloneTool(name_adapter.validate_python(name))
        for name in _native.tool_names()
    )


def default_robot_tool_mapping(
    capabilities: Sequence[RobotToolCapability],
) -> tuple[RobotToolCapability, ...]:
    names = RobotToolConfiguration().tools
    return tuple(tool for tool in capabilities if tool.name in names)


def capabilities_from_configuration(
    configuration: RobotToolConfiguration,
) -> tuple[RobotToolCapability, ...]:
    capabilities_by_name = {tool.name: tool for tool in all_robot_tool_capabilities()}
    return tuple(
        ActionSequenceTool(
            scope=configuration.sequence_scope, actions=configuration.sequence_actions
        )
        if isinstance(capabilities_by_name[name], ActionSequenceTool)
        else capabilities_by_name[name]
        for name in configuration.tools
    )


def configuration_from_capabilities(
    capabilities: Sequence[RobotToolCapability],
) -> RobotToolConfiguration:
    sequence = next(
        (tool for tool in capabilities if isinstance(tool, ActionSequenceTool)), None
    )
    return RobotToolConfiguration(
        sequence_actions=sequence.actions if sequence is not None else frozenset(),
        sequence_scope=sequence.scope if sequence is not None else SingleArmSequence(),
        tools=tuple(tool.name for tool in capabilities),
    )


def filter_sequence_actions(
    keep: Callable[[SequenceAction], bool],
) -> ToolTransformation:
    def transform(
        capabilities: tuple[RobotToolCapability, ...],
    ) -> tuple[RobotToolCapability, ...]:
        return tuple(
            replace(
                tool,
                actions=frozenset(action for action in tool.actions if keep(action)),
            )
            if isinstance(tool, ActionSequenceTool)
            else tool
            for tool in capabilities
        )

    return transform


def native_configuration(configuration: RobotToolConfiguration) -> dict[str, object]:
    scope = configuration.sequence_scope
    return {
        "tools": configuration.tools,
        "actions": sorted(configuration.sequence_actions),
        "arms": scope.arms if isinstance(scope, NamedArmSequence) else None,
    }


def create_robot_tool_catalog(
    configuration: RobotToolConfiguration | Sequence[RobotToolCapability] | None = None,
) -> list[RobotToolSpec]:
    if configuration is None:
        configuration = RobotToolConfiguration()
    elif not isinstance(configuration, RobotToolConfiguration):
        configuration = configuration_from_capabilities(configuration)
    return [
        RobotToolSpec(tool["name"], tool["description"], tool["parameters"])
        for tool in _native.tool_catalog(native_configuration(configuration))
    ]
