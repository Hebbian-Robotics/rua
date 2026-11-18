"""Compose the Rust-owned action schema and constrain its execution interface."""

from collections.abc import Callable, Iterable, Sequence
from copy import deepcopy
from dataclasses import dataclass, field, replace
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, TypeAdapter

from hebbian_rua.client.contract.station_api import (
    ActionSequenceArmAction,
    ActionSequenceStep,
)
from hebbian_rua.client.llm.tools import ROBOT_TOOLS, ActionSequenceInput
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


class SingleArmInput(BaseModel):
    model_config = ConfigDict(extra="forbid")
    steps: list[ActionSequenceArmAction] = Field(min_length=1)


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
        if tool.name == "action_sequence"
        else StandaloneTool(name_adapter.validate_python(tool.name))
        for tool in ROBOT_TOOLS
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


def create_robot_tool_catalog(
    configuration: RobotToolConfiguration | Sequence[RobotToolCapability] | None = None,
) -> list[RobotToolSpec]:
    if configuration is None:
        configuration = default_robot_tool_mapping(all_robot_tool_capabilities())
    if not isinstance(configuration, RobotToolConfiguration):
        configuration = configuration_from_capabilities(configuration)
    specifications_by_name = {tool.name: tool for tool in ROBOT_TOOLS}
    if any(name not in specifications_by_name for name in configuration.tools):
        raise ValueError("Unknown tool in catalog configuration")
    catalog = []
    for name in configuration.tools:
        tool = specifications_by_name[name]
        schema = deepcopy(tool.parameters_schema)
        if tool.name == "action_sequence":
            steps = schema["properties"]["steps"]
            action = steps["items"]
            properties = action["properties"]
            if configuration.sequence_actions & {"pose", "joints"}:
                movement = properties["movement"]
                variants = movement.get("oneOf", [movement])
                available = [
                    variant
                    for variant in variants
                    if (
                        "joints"
                        if "joints_degrees" in variant["properties"]
                        else "pose"
                    )
                    in configuration.sequence_actions
                ]
                properties["movement"] = (
                    available[0] if len(available) == 1 else {"oneOf": available}
                )
            else:
                properties.pop("movement", None)
            if "gripper" not in configuration.sequence_actions:
                properties.pop("gripper_opening", None)
            action.pop("anyOf", None)
            if isinstance(configuration.sequence_scope, NamedArmSequence):
                steps["items"] = {
                    "type": "object",
                    "additionalProperties": False,
                    "required": ["arms"],
                    "properties": {
                        "arms": {
                            "type": "object",
                            "additionalProperties": False,
                            "minProperties": 1,
                            "properties": {
                                arm: deepcopy(action)
                                for arm in configuration.sequence_scope.arms
                            },
                        }
                    },
                }
        description = tool.description
        if tool.name == "action_sequence" and isinstance(
            configuration.sequence_scope, NamedArmSequence
        ):
            description = "Steps run in order; arms, movement and gripper commands in the same step start together. A refusal or incomplete/limited target stops the sequence."
        catalog.append(RobotToolSpec(tool.name, description, schema))
    return catalog


def parse_sequence_arguments(
    arguments: object, configuration: RobotToolConfiguration, selected_arm: str
) -> ActionSequenceInput:
    if isinstance(configuration.sequence_scope, SingleArmSequence):
        # Parse through the canonical action model, then supply the implicit arm.
        single = SingleArmInput.model_validate(arguments)
        parsed = ActionSequenceInput(
            steps=[
                ActionSequenceStep(arms={selected_arm: action})
                for action in single.steps
            ]
        )
    else:
        parsed = ActionSequenceInput.model_validate(arguments)
    for step in parsed.steps:
        if not step.arms:
            raise ValueError("A step needs at least one arm")
        for arm, action in step.arms.items():
            if (
                isinstance(configuration.sequence_scope, NamedArmSequence)
                and arm not in configuration.sequence_scope.arms
            ):
                raise ValueError(f"Arm {arm} is not available in this sequence")
            if action.movement is None and action.gripper_opening is None:
                raise ValueError("An arm action needs a movement or gripper target")
            if (
                action.movement is not None
                and action.movement.kind not in configuration.sequence_actions
            ) or (
                action.gripper_opening is not None
                and "gripper" not in configuration.sequence_actions
            ):
                raise ValueError(
                    "This action is not available in the configured sequence"
                )
    return parsed
