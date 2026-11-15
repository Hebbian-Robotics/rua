"""Robot tools for google-genai generate_content. Install the gemini extra."""

from google.genai import types

from hebbian_rua._tool_turn import RobotToolTurn
from hebbian_rua.client.llm.robot_tools import RobotTools
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

    async def observe(self) -> types.Content:
        observation = await self.robot.observe()
        return types.Content(
            role="user",
            parts=[
                types.Part.from_text(text=observation.text),
                *(
                    types.Part.from_bytes(data=jpeg, mime_type="image/jpeg")
                    for jpeg in observation.cameras.values()
                ),
            ],
        )

    async def execute(
        self, response: types.GenerateContentResponse
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
            # Gemini can omit IDs. The local correlation key is never sent to Google.
            calls.append(
                RobotToolCall(
                    call_id=function_call.id or f"robot-call-{len(calls)}",
                    name=function_call.name,
                    arguments=function_call.args or {},
                )
            )
        results = await self.robot.execute(calls)
        # Return the original content intact: thought signatures may be on any part.
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
                                    else "result": result.data
                                },
                            )
                        )
                        for provider_call, result in zip(
                            provider_calls, results, strict=True
                        )
                    ],
                )
            )
        return RobotToolTurn(messages, results)
