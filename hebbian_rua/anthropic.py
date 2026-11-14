"""Robot tools for Anthropic Messages. Install the anthropic extra."""

from anthropic.types import Message, MessageParam, ToolParam, ToolResultBlockParam
from anthropic.types.content_block_param import ContentBlockParam
from hebbian_robot_use_client.llm.robot_tools import RobotTools
from hebbian_robot_use_client.llm.types import RobotToolCall
from pydantic import TypeAdapter

from hebbian_rua._tool_turn import RobotToolTurn, encode_camera

_ASSISTANT_CONTENT = TypeAdapter(list[ContentBlockParam])


class AnthropicRobotTools:
    """Translate robot tools and complete, non-streaming Messages API replies.

    This adapter owns tool_use blocks in the supplied response. For mixed
    toolsets, route calls yourself using the provider-neutral RobotTools API.
    """

    def __init__(self, robot: RobotTools) -> None:
        self.robot = robot

    @property
    def tools(self) -> list[ToolParam]:
        return [
            {
                "name": tool.name,
                "description": tool.description,
                "input_schema": tool.parameters_schema,
            }
            for tool in self.robot.tools
        ]

    async def observe(self) -> MessageParam:
        observation = await self.robot.observe()
        content: list[ContentBlockParam] = [{"type": "text", "text": observation.text}]
        content.extend(
            {
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": encode_camera(jpeg),
                },
            }
            for jpeg in observation.cameras.values()
        )
        return {"role": "user", "content": content}

    async def execute(self, response: Message) -> RobotToolTurn[MessageParam]:
        if response.stop_reason not in {"tool_use", "end_turn", "stop_sequence"}:
            raise ValueError(
                f"Cannot execute an incomplete model response: {response.stop_reason}"
            )
        calls = [
            RobotToolCall(call_id=block.id, name=block.name, arguments=block.input)
            for block in response.content
            if block.type == "tool_use"
        ]
        # Materialize SDK Iterable fields so history can be reused on later turns.
        content: list[ContentBlockParam] = _ASSISTANT_CONTENT.dump_python(
            _ASSISTANT_CONTENT.validate_python(
                [block.model_dump(exclude_none=True) for block in response.content]
            ),
            mode="json",
        )
        messages: list[MessageParam] = [{"role": "assistant", "content": content}]
        results = await self.robot.execute(calls)
        if results:
            outputs: list[ToolResultBlockParam] = [
                {
                    "type": "tool_result",
                    "tool_use_id": result.call.call_id,
                    "content": result.text,
                    "is_error": result.is_error,
                }
                for result in results
            ]
            messages.append({"role": "user", "content": outputs})
        return RobotToolTurn(messages, results)
