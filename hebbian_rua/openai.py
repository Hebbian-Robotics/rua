"""Robot tools for the OpenAI Responses API. Install the openai extra."""

from openai.types.responses import Response, ResponseInputItemParam, ToolParam
from openai.types.responses.response_input_content_param import (
    ResponseInputContentParam,
)
from pydantic import TypeAdapter

from hebbian_rua._tool_turn import RobotToolTurn, decode_arguments, encode_camera
from hebbian_rua.client.llm.robot_tools import RobotTools
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
                "parameters": tool.parameters_schema,
                "strict": False,
            }
            for tool in self.robot.tools
        ]

    async def observe(self) -> ResponseInputItemParam:
        observation = await self.robot.observe()
        content: list[ResponseInputContentParam] = [
            {"type": "input_text", "text": observation.text}
        ]
        content.extend(
            {
                "type": "input_image",
                "image_url": f"data:image/jpeg;base64,{encode_camera(jpeg)}",
                "detail": "auto",
            }
            for jpeg in observation.cameras.values()
        )
        return {"role": "user", "content": content}

    async def execute(
        self, response: Response
    ) -> RobotToolTurn[ResponseInputItemParam]:
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
        # Retain every model output, including reasoning, before executing any motion.
        # Materialize SDK Iterable fields so history can be reused on later turns.
        messages: list[ResponseInputItemParam] = _RESPONSE_INPUT.dump_python(
            _RESPONSE_INPUT.validate_python(
                [output.model_dump(exclude_none=True) for output in response.output]
            ),
            mode="json",
        )
        results = await self.robot.execute(calls)
        messages.extend(
            {
                "type": "function_call_output",
                "call_id": result.call.call_id,
                "output": f"Error: {result.text}" if result.is_error else result.text,
            }
            for result in results
        )
        return RobotToolTurn(messages, results)
