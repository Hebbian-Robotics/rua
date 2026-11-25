"""Robot tools for google-genai Interactions and GenerateContent. Install the gemini extra."""

from collections.abc import Mapping

from google.genai import types
from google.genai.interactions import (
    ContentParam,
    Interaction,
    StepParam,
    ToolParam,
    UserInputStepParam,
)

from hebbian_rua.client.llm.recovery import capture_when_ready
from hebbian_rua.client.llm.robot_tools import RobotTools
from hebbian_rua.client.llm.tool_turn import (
    ModelObservation,
    RobotToolTurn,
    encode_camera,
)
from hebbian_rua.client.llm.types import RobotToolCall


class GeminiRobotTools:
    """Translate robot tools and a single, complete generate_content candidate.

    Use manual function calling (disable automatic_function_calling). This
    adapter owns function calls in the supplied response. Route mixed toolsets
    yourself using the provider-neutral RobotTools API.
    """

    def __init__(self, robot: RobotTools) -> None:
        self.robot = robot

    @property
    def tools(self) -> list[types.Tool]:
        return [
            types.Tool(
                function_declarations=[
                    types.FunctionDeclaration(
                        name=tool.name,
                        description=tool.description,
                        parameters_json_schema=tool.parameters_schema,
                    )
                    for tool in self.robot.tools
                ]
            )
        ]

    async def observe(
        self, *, wait_for_recovery: bool = False
    ) -> ModelObservation[types.Content]:
        if wait_for_recovery:
            captured, _ = await capture_when_ready(self.robot)
            observation = captured.observation
        else:
            observation = await self.robot.observe()
        return ModelObservation(
            _image_message(observation.text, observation.cameras), observation.revisions
        )

    async def execute(
        self,
        response: types.GenerateContentResponse,
        *,
        expected_revisions: Mapping[str, int],
    ) -> RobotToolTurn[types.Content]:
        if not response.candidates or len(response.candidates) != 1:
            raise ValueError("Expected exactly one Gemini candidate")
        candidate = response.candidates[0]
        if (
            candidate.finish_reason != types.FinishReason.STOP
            or candidate.content is None
        ):
            raise ValueError(
                f"Cannot execute an incomplete model response: {candidate.finish_reason}"
            )
        calls: list[RobotToolCall] = []
        provider_calls: list[types.FunctionCall] = []
        for part in candidate.content.parts or []:
            function_call = part.function_call
            if function_call is None:
                continue
            if (
                not function_call.name
                or function_call.partial_args
                or function_call.will_continue
            ):
                raise ValueError("Expected a complete, named Gemini function call")
            provider_calls.append(function_call)
            # CONSTRAINT: Gemini can omit IDs. The local correlation key is never sent to Google.
            calls.append(
                RobotToolCall(
                    call_id=function_call.id or f"robot-call-{len(calls)}",
                    name=function_call.name,
                    arguments=function_call.args or {},
                )
            )
        results = await self.robot.execute(calls, expected_revisions=expected_revisions)
        # CONSTRAINT: Return the original content intact: thought signatures may be on any part.
        messages = [candidate.content]
        if results:
            messages.append(
                types.Content(
                    role="user",
                    parts=[
                        types.Part(
                            function_response=types.FunctionResponse(
                                id=provider_call.id,
                                name=provider_call.name,
                                response={
                                    "error"
                                    if result.is_error
                                    else "result": result.model_data
                                },
                            )
                        )
                        for provider_call, result in zip(
                            provider_calls, results, strict=True
                        )
                    ],
                )
            )
        messages.extend(
            _image_message(
                f"Images requested by tool call {result.call.call_id}.\n{result.text}",
                result.cameras,
            )
            for result in results
            if result.cameras
        )
        return RobotToolTurn(messages, results)


def _image_message(text: str, cameras: Mapping[str, bytes]) -> types.Content:
    parts = [types.Part.from_text(text=text)]
    for name, jpeg in cameras.items():
        parts.append(types.Part.from_text(text=f"Camera: {name}"))
        parts.append(types.Part.from_bytes(data=jpeg, mime_type="image/jpeg"))
    return types.Content(role="user", parts=parts)


class GeminiInteractionRobotTools:
    """Translate robot tools and new inputs for a stored Gemini interaction."""

    def __init__(self, robot: RobotTools) -> None:
        self.robot = robot

    @property
    def tools(self) -> list[ToolParam]:
        return [
            {
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": tool.parameters_schema,
            }
            for tool in self.robot.tools
        ]

    async def observe(
        self, *, wait_for_recovery: bool = False
    ) -> ModelObservation[StepParam]:
        if wait_for_recovery:
            captured, _ = await capture_when_ready(self.robot)
            observation = captured.observation
        else:
            observation = await self.robot.observe()
        return ModelObservation(
            _observation_input(observation.text, observation.cameras),
            observation.revisions,
        )

    async def execute(
        self, response: Interaction, *, expected_revisions: Mapping[str, int]
    ) -> RobotToolTurn[StepParam]:
        """Execute a complete interaction and return new input steps for its successor."""
        if response.status not in {"completed", "requires_action"}:
            raise ValueError(
                f"Cannot execute an incomplete Gemini interaction: {response.status}"
            )
        calls = [
            RobotToolCall(call_id=step.id, name=step.name, arguments=step.arguments)
            for step in response.steps or ()
            if step.type == "function_call"
        ]
        results = await self.robot.execute(calls, expected_revisions=expected_revisions)
        messages: list[StepParam] = [
            {
                "type": "function_result",
                "name": result.call.name,
                "call_id": result.call.call_id,
                "result": [{"type": "text", "text": result.text}],
                "is_error": result.is_error,
            }
            for result in results
        ]
        messages.extend(
            _observation_input(
                f"Images requested by tool call {result.call.call_id}.\n{result.text}",
                result.cameras,
            )
            for result in results
            if result.cameras
        )
        return RobotToolTurn(messages, results)


def _observation_input(text: str, cameras: Mapping[str, bytes]) -> UserInputStepParam:
    content: list[ContentParam] = [{"type": "text", "text": text}]
    for camera_name, jpeg in cameras.items():
        content.extend(
            [
                {"type": "text", "text": f"Camera: {camera_name}"},
                {
                    "type": "image",
                    "mime_type": "image/jpeg",
                    "data": encode_camera(jpeg),
                },
            ]
        )
    return {"type": "user_input", "content": content}
