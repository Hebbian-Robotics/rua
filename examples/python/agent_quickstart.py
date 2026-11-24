import asyncio
import json
import os
import sys
from base64 import b64decode
from collections.abc import Awaitable, Callable, Generator, Mapping, Sequence
from contextlib import contextmanager, nullcontext
from pathlib import Path
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

Set a model-provider key (OPENAI_API_KEY, ANTHROPIC_API_KEY or GEMINI_API_KEY).
Connect to a station with a real arm whenever available. Reserve simulation
for testing without hardware.
Select station cameras with --camera wrist --camera overhead. Configure LiveKit
on the station to watch in Room viewer at https://hebbian-robotics.github.io/livekit-cameras/.
If LiveKit Cloud is unavailable, run a local LiveKit server or restart the
station with --local-only-no-remote-cameras, then open the RUA example viewer.

The provider stores conversation history. The loop executes the robot tools.
"""

type Provider = Literal["openai", "anthropic", "gemini"]

DEFAULT_MAX_TURNS = 20
command_line = App(help=CLI_HELP, version_flags=())


@contextmanager
def agent_tracer_provider() -> Generator[trace.TracerProvider]:
    if not (
        os.environ.get("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT")
        or os.environ.get("OTEL_EXPORTER_OTLP_ENDPOINT")
    ):
        yield trace.NoOpTracerProvider()
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
        yield provider
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


PRIVATE_FILE_FLAGS = os.O_WRONLY | os.O_CREAT | os.O_EXCL


def request_images(
    messages: Sequence[object],
) -> Generator[tuple[str | None, str | None, bytes]]:
    for message in messages:
        if not isinstance(message, dict) or not isinstance(
            message.get("content"), list
        ):
            continue
        camera = tool_call_id = None
        for part in message["content"]:
            if not isinstance(part, dict):
                continue
            kind = part.get("type")
            text = part.get("text")
            if kind in ("text", "input_text") and isinstance(text, str):
                if text.startswith("Camera: "):
                    camera = text.removeprefix("Camera: ")
                elif text.startswith("Images requested by tool call "):
                    tool_call_id = (
                        text.partition("\n")[0]
                        .removeprefix("Images requested by tool call ")
                        .removesuffix(".")
                    )
                continue
            encoded = part.get("image_url") if kind == "input_image" else None
            if isinstance(encoded, str):
                prefix = "data:image/jpeg;base64,"
                if not encoded.startswith(prefix):
                    continue
                encoded = encoded.removeprefix(prefix)
            elif kind == "image":
                if isinstance(source := part.get("source"), dict):
                    if source.get("media_type") == "image/jpeg":
                        encoded = source.get("data")
                elif part.get("mime_type") == "image/jpeg":
                    encoded = part.get("data")
            if isinstance(encoded, str):
                yield camera, tool_call_id, b64decode(encoded, validate=True)
                camera = None


class ModelInputCapture:
    def __init__(self, directory: str) -> None:
        span_context = trace.get_current_span().get_span_context()
        run_id = (
            f"{span_context.trace_id:032x}" if span_context.is_valid else uuid4().hex
        )
        self.directory = Path(directory).absolute() / run_id
        self.announced = False

    async def save(
        self, messages: Sequence[object], turn: int, span: trace.Span
    ) -> None:
        try:
            manifest = await asyncio.to_thread(self._write, messages, turn)
        except (OSError, ValueError) as error:
            print(f"Could not save model input images: {error}", file=sys.stderr)
            return
        if manifest is None:
            return
        span.set_attribute("hebbian.model_inputs.path", str(manifest))
        if not self.announced:
            self.announced = True
            print(f"Model input images: {self.directory}", file=sys.stderr)

    def _write(self, messages: Sequence[object], turn: int) -> Path | None:
        images = list(request_images(messages))
        if not images:
            return None
        directory = self.directory / f"turn-{turn:04d}"
        self.directory.parent.mkdir(parents=True, exist_ok=True)
        self.directory.mkdir(mode=0o700, exist_ok=True)
        directory.mkdir(mode=0o700)
        entries: list[dict[str, str | None]] = []
        for image_index, (camera, tool_call_id, jpeg) in enumerate(images, start=1):
            filename = f"{image_index}.jpg"
            with os.fdopen(
                os.open(directory / filename, PRIVATE_FILE_FLAGS, 0o600), "wb"
            ) as image:
                image.write(jpeg)
            entries.append(
                {"file": filename, "camera": camera, "tool_call_id": tool_call_id}
            )
        manifest = directory / "index.json"
        with os.fdopen(
            os.open(manifest, PRIVATE_FILE_FLAGS, 0o600), "w", encoding="utf-8"
        ) as index:
            json.dump({"turn": turn, "images": entries}, index, indent=2)
        return manifest


class FinishArguments(BaseModel):
    model_config = ConfigDict(extra="forbid")
    reason: str


class AgentRobotTools(RobotTools):
    finish_reason: str | None = None
    model_inputs: ModelInputCapture | None = None
    tracer_provider: trace.TracerProvider = trace.NoOpTracerProvider()

    @property
    def tracer(self) -> trace.Tracer:
        return self.tracer_provider.get_tracer("hebbian.agent")

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
            with agent_span(
                robot.tracer, "model turn", {"hebbian.turn": turn_number}
            ) as span:
                if robot.model_inputs is not None:
                    await robot.model_inputs.save(pending_input, turn_number, span)
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
    async with AsyncAnthropic(
        open_telemetry={
            "tracer_provider": robot.tracer_provider,
            "propagation": False,
            "traces": {"content_mode": "metadata_only"},
        }
    ) as client:
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
            if robot.model_inputs is None:
                response = await thread.send(*pending_input)
            else:
                with agent_span(
                    robot.tracer, "model turn", {"hebbian.turn": turn_number}
                ) as span:
                    await robot.model_inputs.save(pending_input, turn_number, span)
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
            with agent_span(
                robot.tracer, "model turn", {"hebbian.turn": turn_number}
            ) as span:
                if robot.model_inputs is not None:
                    await robot.model_inputs.save(pending_input, turn_number, span)
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
    tracer_provider: trace.TracerProvider,
) -> None:
    async with station.holding_lease(task, required_cameras=cameras) as lease:
        robot = AgentRobotTools(station, lease, arm_name, cameras).map_tools(
            filter_sequence_actions(lambda action: action != "joints")
        )
        robot.tracer_provider = tracer_provider
        if directory := os.environ.get("HEBBIAN_RUA_MODEL_INPUTS_DIR"):
            robot.model_inputs = ModelInputCapture(directory)
        instructions = (
            "Use the robot tools to carry out the task. You are operating in a "
            "supervised development workspace. Operate the configured SO-101 using "
            "Cartesian targets and gripper actions. In the base frame, +X is forward "
            "from the arm, +Y is left, and +Z is up. Camera image directions depend on "
            "the camera pose. Choose the next useful action for the task. Preview "
            "answers a specific feasibility question without moving the arm. The loop "
            "supplies arm state and available camera images after action batches when "
            "the run continues. Observe can supply an additional view or wait for a "
            "scene change. For a limited move, pose_limited.advice, when present, "
            "describes the shortfall, where the arm went, and a reachable alternative. "
            "A stopped_short move settled outside tolerance. Its completion detail "
            "describes the remaining error, which does not identify the cause on its "
            "own. Use the outcome and observations to decide whether to revise the "
            "target or finish. Call finish alone to report the task outcome. "
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


@command_line.default
async def main(
    task: str,
    /,
    *,
    provider: Annotated[Provider, Parameter(help="Model provider.")],
    model: Annotated[str, Parameter(help="Model ID.")],
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
        Parameter(
            help="Station camera name. Repeat for multiple cameras, such as --camera wrist --camera overhead.",
            negative=False,
        ),
    ] = (),
    server: Annotated[
        str | None, Parameter(help="Station URL. Overrides HEBBIAN_STATION_URL.")
    ] = None,
    media_server: Annotated[
        str | None, Parameter(help="Media URL. Overrides HEBBIAN_MEDIA_SERVER.")
    ] = None,
) -> int | None:
    load_environment()
    model = model.strip()
    if not model:
        command_line.error_console.print("Pass a nonempty model ID with --model")
        return 2
    if configured_api_key(provider, os.environ) is None:
        command_line.error_console.print(
            f"Set {provider.upper()}_API_KEY before running the agent"
        )
        return 2

    with (
        agent_tracer_provider() as tracer_provider,
        agent_span(
            tracer_provider.get_tracer("hebbian.agent"),
            "agent run",
            {
                "gen_ai.provider.name": provider,
                "gen_ai.request.model": model,
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
                provider,
                model,
                task,
                arm_name,
                list(camera),
                max_turns,
                tracer_provider=tracer_provider,
            )
    return None


if __name__ == "__main__":
    command_line()
