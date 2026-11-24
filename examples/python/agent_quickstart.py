import json
import os
import sys
from collections.abc import Awaitable, Callable, Generator, Mapping, Sequence
from contextlib import contextmanager, nullcontext
from typing import Annotated, Literal, TypedDict, cast
from uuid import uuid4

from cyclopts import App, Parameter, validators
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator
from pydantic import BaseModel, ConfigDict

from hebbian_rua import (
    RobotToolCall,
    RobotToolResult,
    RobotTools,
    RobotToolSpec,
    StationClient,
    filter_sequence_actions,
)
from hebbian_rua.client.contract.station_api import ArmMode
from hebbian_rua.client.environment import load_environment
from hebbian_rua.client.llm.robot_tools import CallWrapper
from hebbian_rua.client.station_client import commands_parented_to

CLI_HELP = """Build a robot-use agent with OpenAI, Anthropic or Gemini.

Set a provider API key, then start a station and run the agent.
The provider stores conversation history. The loop executes the robot tools.
"""

type Provider = Literal["openai", "anthropic", "gemini"]

PROVIDER_PRIORITY: tuple[Provider, ...] = ("openai", "gemini", "anthropic")
DEFAULT_MODELS: Mapping[Provider, str] = {
    "openai": "gpt-6.2-astra",
    "gemini": "gemini-4-neon",
    "anthropic": "claude-fable-5-2",
}
DEFAULT_MAX_TURNS = 20
command_line = App(help=CLI_HELP, version_flags=())


@contextmanager
def agent_tracer() -> Generator[trace.Tracer]:
    if not (
        os.environ.get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        or os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
    ):
        yield trace.NoOpTracer()
        return

    provider = TracerProvider(
        resource=Resource.create(
            {
                "service.name": os.environ.get(
                    "OTEL_SERVICE_NAME", "hebbian-python-agent"
                )
            }
        )
    )
    provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
    try:
        yield provider.get_tracer("hebbian.agent")
    finally:
        provider.shutdown()


@contextmanager
def agent_span(
    tracer: trace.Tracer, name: str, attributes: Mapping[str, str | int]
) -> Generator[trace.Span]:
    with tracer.start_as_current_span(
        name,
        attributes=attributes,
        record_exception=False,
        set_status_on_exception=False,
    ) as span:
        try:
            yield span
        except BaseException:
            span.set_status(trace.StatusCode.ERROR)
            raise


class FinishArguments(BaseModel):
    model_config = ConfigDict(extra="forbid")
    reason: str


class AgentRobotTools(RobotTools):
    finish_reason: str | None = None
    tracer: trace.Tracer = trace.NoOpTracer()

    @property
    def tools(self) -> list[RobotToolSpec]:
        return [
            *super().tools,
            RobotToolSpec(
                name="finish",
                description="End the run and report the task outcome. Call alone.",
                parameters_schema=FinishArguments.model_json_schema(),
            ),
        ]

    async def trace_tool_call(
        self, call: RobotToolCall, execute: Callable[[], Awaitable[RobotToolResult]]
    ) -> RobotToolResult:
        with agent_span(
            self.tracer, f"execute_tool {call.name}", {"gen_ai.tool.name": call.name}
        ) as span:
            headers: dict[str, str] = {}
            TraceContextTextMapPropagator().inject(headers)
            traceparent = headers.get("traceparent")
            parent_command = (
                commands_parented_to(traceparent, headers.get("tracestate"))
                if traceparent is not None
                else nullcontext()
            )
            with parent_command:
                result = await execute()
            if result.is_error:
                span.set_status(trace.StatusCode.ERROR)
            return result

    async def execute(
        self,
        calls: Sequence[RobotToolCall],
        wrap: CallWrapper | None = None,
        *,
        expected_revision: int | None = None,
        expected_revisions: Mapping[str, int] | None = None,
    ) -> list[RobotToolResult]:
        if not any(call.name == "finish" for call in calls):
            return await super().execute(
                calls,
                wrap or self.trace_tool_call,
                expected_revision=expected_revision,
                expected_revisions=expected_revisions,
            )
        with agent_span(
            self.tracer, "execute_tool finish", {"gen_ai.tool.name": "finish"}
        ) as span:
            try:
                if len(calls) != 1:
                    raise ValueError("Call finish alone; this batch was not executed.")
                arguments = FinishArguments.model_validate(calls[0].arguments)
            except ValueError as error:
                span.set_status(trace.StatusCode.ERROR)
                return [
                    RobotToolResult(call, str(error), is_error=True, data=str(error))
                    for call in calls
                ]
            self.finish_reason = arguments.reason
            return [
                RobotToolResult(
                    calls[0],
                    arguments.reason,
                    is_error=False,
                    data={"finished": True, "reason": arguments.reason},
                )
            ]


async def run_openai(
    robot: AgentRobotTools, task: str, model: str, instructions: str, max_turns: int
) -> None:
    from openai import AsyncOpenAI
    from openai.types.responses import ResponseInputItemParam

    from hebbian_rua.client.llm.openai import OpenAIRobotTools

    robot_tools = OpenAIRobotTools(robot)
    pending_input: list[ResponseInputItemParam] = [{"role": "user", "content": task}]
    previous_response_id: str | None = None
    prompt_cache_key = f"hebbian-agent-{uuid4()}"
    tools = robot_tools.tools
    async with AsyncOpenAI() as client:
        revisions: Mapping[str, int] | None = None
        for turn_number in range(1, max_turns + 1):
            if revisions is None:
                observation = await robot_tools.observe(wait_for_recovery=True)
                pending_input.append(observation.message)
                revisions = observation.revisions
            with agent_span(robot.tracer, "model turn", {"hebbian.turn": turn_number}):
                response = await client.responses.create(
                    model=model,
                    instructions=instructions,
                    tools=tools,
                    input=pending_input,
                    previous_response_id=previous_response_id,
                    store=True,
                    prompt_cache_key=prompt_cache_key,
                )
            turn = await robot_tools.execute(
                response, expected_revisions=revisions, include_model_output=False
            )
            previous_response_id = response.id
            pending_input = turn.messages
            revisions = turn.observed_revisions
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            if turn.should_stop or not turn.results:
                print(response.output_text)
                report_stop(turn.should_stop)
                return
    raise SystemExit(f"No task outcome after {max_turns} model turns")


async def run_anthropic(
    robot: AgentRobotTools, task: str, model: str, instructions: str, max_turns: int
) -> None:
    from anthropic import AsyncAnthropic
    from anthropic.types.beta import BetaMessageParam

    from hebbian_rua.client.llm.anthropic import AnthropicThreadRobotTools

    robot_tools = AnthropicThreadRobotTools(robot)
    pending_input: list[BetaMessageParam] = []
    async with AsyncAnthropic() as client:
        thread = client.beta.messages.thread(
            model=model,
            max_tokens=4096,
            system=instructions,
            tools=robot_tools.tools,
            messages=[{"role": "user", "content": task}],
            cache_control={"type": "ephemeral"},
        )
        revisions: Mapping[str, int] | None = None
        for turn_number in range(1, max_turns + 1):
            if revisions is None:
                observation = await robot_tools.observe(wait_for_recovery=True)
                pending_input.append(observation.message)
                revisions = observation.revisions
            with agent_span(robot.tracer, "model turn", {"hebbian.turn": turn_number}):
                response = await thread.send(*pending_input)
            turn = await robot_tools.execute(response, expected_revisions=revisions)
            pending_input = turn.messages
            revisions = turn.observed_revisions
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            if turn.should_stop or not turn.results:
                print(
                    "\n".join(
                        block.text for block in response.content if block.type == "text"
                    )
                )
                report_stop(turn.should_stop)
                return
    raise SystemExit(f"No task outcome after {max_turns} model turns")


async def run_gemini(
    robot: AgentRobotTools, task: str, model: str, instructions: str, max_turns: int
) -> None:
    from google import genai
    from google.genai.interactions import Interaction, ModelParam, StepParam

    from hebbian_rua.client.llm.gemini import GeminiInteractionRobotTools

    class Continuation(TypedDict, total=False):
        previous_interaction_id: str

    robot_tools = GeminiInteractionRobotTools(robot)
    tools = robot_tools.tools
    pending_input: list[StepParam] = [
        {"type": "user_input", "content": [{"type": "text", "text": task}]}
    ]
    continuation: Continuation = {}
    async with genai.Client().aio as client:
        revisions: Mapping[str, int] | None = None
        for turn_number in range(1, max_turns + 1):
            if revisions is None:
                observation = await robot_tools.observe(wait_for_recovery=True)
                pending_input.append(observation.message)
                revisions = observation.revisions
            with agent_span(robot.tracer, "model turn", {"hebbian.turn": turn_number}):
                response = await client.interactions.create(
                    model=cast(ModelParam, model),
                    stream=False,
                    system_instruction=instructions,
                    tools=tools,
                    store=True,
                    input=pending_input,
                    **continuation,
                )
            if not isinstance(response, Interaction) or not response.id:
                raise ValueError("Expected a stored, non-streaming Gemini interaction")
            turn = await robot_tools.execute(response, expected_revisions=revisions)
            continuation = {"previous_interaction_id": response.id}
            pending_input = turn.messages
            revisions = turn.observed_revisions
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            if turn.should_stop or not turn.results:
                print(response.output_text or "")
                report_stop(turn.should_stop)
                return
    raise SystemExit(f"No task outcome after {max_turns} model turns")


def report_stop(should_stop: bool) -> None:
    if should_stop:
        print("Robot execution stopped after a fault or an uncertain outcome.")


async def run_task(
    station: StationClient,
    provider: Provider,
    model: str,
    task: str,
    arm_name: str,
    cameras: list[str],
    max_turns: int = DEFAULT_MAX_TURNS,
    *,
    tracer: trace.Tracer,
) -> None:
    async with station.holding_lease(task, required_cameras=cameras) as lease:
        robot = AgentRobotTools(station, lease, arm_name, cameras).map_tools(
            filter_sequence_actions(lambda action: action != "joints")
        )
        robot.tracer = tracer
        instructions = (
            "Operate the single configured SO-101 using Cartesian targets and "
            "gripper actions. In the base frame, +X is forward from the arm, "
            "+Y is left, and +Z is up. Camera image directions depend on camera "
            "pose. Choose the next useful action; use preview for a "
            "specific feasibility question. Fresh observations follow action "
            "batches automatically. After a limited or incomplete action, read its "
            "summary before replanning: a limited target's pose_limited.advice "
            "gives the shortfall, where the arm went and an alternative to try; a "
            "stopped_short move's completion detail gives how far short it stopped "
            "and where the tool rests, usually against the table or an object "
            "(compare with surface_guard). Check the images, then aim the next "
            "target where it can be reached; do not repeat it. Call finish alone "
            "to report the task outcome and end the run. "
            "Robot context: " + json.dumps(await robot.context())
        )
        arm = station.arm(arm_name)
        if (await arm.state()).mode != ArmMode.holding:
            enabled = await arm.enable()
            if not enabled.completed:
                raise SystemExit(f"Could not enable the arm: {enabled}")
        match provider:
            case "openai":
                await run_openai(robot, task, model, instructions, max_turns)
            case "anthropic":
                await run_anthropic(robot, task, model, instructions, max_turns)
            case "gemini":
                await run_gemini(robot, task, model, instructions, max_turns)


def resolve_model(
    provider: Provider, explicit_model: str | None, environment: Mapping[str, str]
) -> str:
    model_variable = f"{provider.upper()}_MODEL"
    if explicit_model is not None:
        if not explicit_model.strip():
            raise ValueError("Pass a nonempty model ID with --model")
        return explicit_model
    environment_model = environment.get(model_variable)
    return (
        environment_model
        if environment_model and environment_model.strip()
        else DEFAULT_MODELS[provider]
    )


def configured_api_key(
    provider: Provider, environment: Mapping[str, str]
) -> str | None:
    primary_key = environment.get(f"{provider.upper()}_API_KEY")
    if primary_key and primary_key.strip():
        return primary_key
    google_key = environment.get("GOOGLE_API_KEY")
    return (
        google_key
        if provider == "gemini" and google_key and google_key.strip()
        else None
    )


def resolve_provider(
    explicit_provider: Provider | None, environment: Mapping[str, str]
) -> Provider:
    if explicit_provider is not None:
        return explicit_provider
    return next(
        (
            provider
            for provider in PROVIDER_PRIORITY
            if configured_api_key(provider, environment)
        ),
        PROVIDER_PRIORITY[0],
    )


@command_line.default
async def main(
    task: str,
    /,
    *,
    provider: Annotated[
        Provider | None,
        Parameter(
            help="Defaults to the first configured key in order OpenAI, Gemini, Anthropic."
        ),
    ] = None,
    model: Annotated[
        str | None,
        Parameter(
            help="Model ID. Overrides OPENAI_MODEL, ANTHROPIC_MODEL or GEMINI_MODEL."
        ),
    ] = None,
    max_turns: Annotated[
        int,
        Parameter(
            help="Maximum number of model turns.", validator=validators.Number(gte=1)
        ),
    ] = DEFAULT_MAX_TURNS,
    arm: Annotated[
        str | None, Parameter(help="Arm name. Defaults to the station's sole arm.")
    ] = None,
    camera: Annotated[
        tuple[str, ...],
        Parameter(help="Camera name. Repeat for multiple cameras.", negative=False),
    ] = (),
    server: Annotated[
        str | None, Parameter(help="Station URL. Overrides HEBBIAN_STATION_URL.")
    ] = None,
    media_server: Annotated[
        str | None, Parameter(help="Media URL. Overrides HEBBIAN_MEDIA_SERVER.")
    ] = None,
) -> int | None:
    load_environment()
    resolved_provider = resolve_provider(provider, os.environ)
    try:
        resolved_model = resolve_model(resolved_provider, model, os.environ)
    except ValueError as error:
        command_line.error_console.print(str(error), markup=False)
        return 2
    if configured_api_key(resolved_provider, os.environ) is None:
        command_line.error_console.print(
            f"Set {resolved_provider.upper()}_API_KEY before running the agent"
        )
        return 2

    with (
        agent_tracer() as tracer,
        agent_span(
            tracer,
            "agent run",
            {
                "gen_ai.provider.name": resolved_provider,
                "gen_ai.request.model": resolved_model,
            },
        ) as span,
    ):
        span_context = span.get_span_context()
        if span_context.is_valid:
            print(f"Trace ID: {span_context.trace_id:032x}", file=sys.stderr)
        async with StationClient(server, media_server=media_server) as station:
            try:
                arm_name = await station.resolve_arm_name(arm)
            except ValueError as error:
                command_line.error_console.print(str(error), markup=False)
                return 2
            await run_task(
                station,
                resolved_provider,
                resolved_model,
                task,
                arm_name,
                list(camera),
                max_turns,
                tracer=tracer,
            )
    return None


if __name__ == "__main__":
    command_line()
