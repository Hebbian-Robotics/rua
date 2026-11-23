"""Robot tools for the OpenAI Responses API. Install the openai extra."""

from collections.abc import Mapping

from openai.types.responses import Response, ResponseInputItemParam, ToolParam
from openai.types.responses.response_input_content_param import (
    ResponseInputContentParam,
)
from pydantic import TypeAdapter

from hebbian_rua.client.llm.recovery import capture_when_ready
from hebbian_rua.client.llm.robot_tools import RobotTools
from hebbian_rua.client.llm.tool_turn import (
    ModelObservation,
    RobotToolTurn,
    decode_arguments,
    encode_camera,
)
from hebbian_rua.client.llm.types import RobotToolCall

_RESPONSE_INPUT = TypeAdapter(list[ResponseInputItemParam])


class OpenAIRobotTools:
    """Translate robot tools and complete, non-streaming Responses API replies.

    This adapter owns the function calls in the supplied response. For mixed
    toolsets, route calls yourself using the provider-neutral RobotTools API.
    """

    def __init__(self, robot: RobotTools) -> None:
        self.robot = robot

    @property
    def tools(self) -> list[ToolParam]:
        return [
            {
                "type": "function",
                "name": tool.name,
                "description": tool.description,
                "parameters": dict[str, object](tool.parameters_schema),
                "strict": False,
            }
            for tool in self.robot.tools
        ]

    async def observe(
        self, *, wait_for_recovery: bool = False
    ) -> ModelObservation[ResponseInputItemParam]:
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
        response: Response,
        *,
        expected_revisions: Mapping[str, int],
        include_model_output: bool = True,
    ) -> RobotToolTurn[ResponseInputItemParam]:
        """Format a tool turn. Set include_model_output=False when chaining stored responses."""
        if response.status != "completed":
            raise ValueError(
                f"Cannot execute an incomplete model response: {response.status}"
            )
        calls = [
            RobotToolCall(
                call_id=output.call_id,
                name=output.name,
                arguments=decode_arguments(output.arguments),
            )
            for output in response.output
            if output.type == "function_call"
        ]
        messages: list[ResponseInputItemParam] = []
        if include_model_output:
            # Materialize SDK Iterable fields before motion so the client can replay the response.
            messages = _RESPONSE_INPUT.dump_python(
                _RESPONSE_INPUT.validate_python(
                    [output.model_dump(exclude_none=True) for output in response.output]
                ),
                mode="json",
            )
        results = await self.robot.execute(calls, expected_revisions=expected_revisions)
        messages.extend(
            {
                "type": "function_call_output",
                "call_id": result.call.call_id,
                "output": f"Error: {result.text}" if result.is_error else result.text,
            }
            for result in results
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


def _image_message(text: str, cameras: Mapping[str, bytes]) -> ResponseInputItemParam:
    content: list[ResponseInputContentParam] = [{"type": "input_text", "text": text}]
    for name, jpeg in cameras.items():
        content.append({"type": "input_text", "text": f"Camera: {name}"})
        content.append(
            {
                "type": "input_image",
                "image_url": f"data:image/jpeg;base64,{encode_camera(jpeg)}",
                "detail": "auto",
            }
        )
    return {"role": "user", "content": content}
