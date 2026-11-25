import asyncio
import json
import os
import sys
from base64 import b64decode
from collections.abc import Awaitable, Callable, Generator, Mapping, Sequence
from contextlib import contextmanager, nullcontext
from dataclasses import replace
from functools import partial
from itertools import count
from pathlib import Path
from typing import Annotated, Literal, TypedDict, cast
from uuid import uuid4

from cyclopts import App, Parameter
from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.sdk.resources import Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator
from pydantic import BaseModel, ConfigDict, ValidationError, model_validator

from hebbian_rua import (
    RobotToolCall,
    RobotToolResult,
    RobotTools,
    RobotToolSpec,
    StationClient,
    filter_sequence_actions,
)
from hebbian_rua.client.contract.station_api import (
    ActionSequenceRecord,
    ActionSequenceStopped,
    ArmMode,
    CommandOutcomeUnknown,
    CommandRefused,
    ExecutionAuthorityLost,
    RejectionReason,
    RuntimeFailure,
    UncertainAdmission,
)
from hebbian_rua.client.environment import load_environment
from hebbian_rua.client.llm.robot_tools import CallWrapper, CapturedObservation
from hebbian_rua.client.station_client import commands_parented_to
from hebbian_rua.client.station_types import LeaseLost

CLI_HELP = """Build a robot-use agent with OpenAI, Anthropic or Gemini.

Set a model-provider key (OPENAI_API_KEY, ANTHROPIC_API_KEY or GEMINI_API_KEY).
Connect to a station with a real arm whenever available. Reserve simulation
for testing without hardware.
Select required cameras with --camera wrist and optional cameras with
--optional-camera phone_overhead. A station phone invite labeled overhead
publishes as phone_overhead. Configure LiveKit on the station to watch in Room
viewer at https://hebbian-robotics.github.io/livekit-cameras/.
If LiveKit Cloud is unavailable, run a local LiveKit server or restart the
station with --local-only-no-remote-cameras, then open the RUA example viewer.

The provider stores conversation history. The loop executes the robot tools.
"""

type Provider = Literal["openai", "anthropic", "gemini"]

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


UPDATE_PLAN_TOOL = "update_plan"
UPDATE_NOTES_TOOL = "update_notes"
MEMORY_TOOLS = frozenset({UPDATE_PLAN_TOOL, UPDATE_NOTES_TOOL})


class PlanStep(BaseModel):
    model_config = ConfigDict(extra="forbid")
    text: str
    status: Literal["pending", "active", "done", "failed"]


class UpdatePlanArguments(BaseModel):
    model_config = ConfigDict(extra="forbid")
    steps: list[PlanStep]
    note: str | None = None

    @model_validator(mode="after")
    def limit_active_steps(self) -> "UpdatePlanArguments":
        if sum(step.status == "active" for step in self.steps) > 1:
            raise ValueError("A plan may have at most one active step")
        return self


class UpdateNotesArguments(BaseModel):
    model_config = ConfigDict(extra="forbid")
    notes: list[str]


def recoverable_refusal(data: object) -> bool:
    if not isinstance(data, dict) or data.get("status") != "refused":
        return False
    try:
        refusal = CommandRefused.model_validate(data)
    except ValidationError:
        return False
    return refusal.reason in {
        RejectionReason.stale_revision,
        RejectionReason.interlocked,
    }


def sequence_outcome_requires_stop(result: RobotToolResult) -> bool:
    if (
        result.call.name != "action_sequence"
        or not result.is_error
        or not isinstance(result.data, dict)
        or result.data.get("status") == "refused"
    ):
        return False
    try:
        sequence = ActionSequenceRecord.model_validate(result.data)
    except ValidationError:
        return True
    return (
        isinstance(sequence.phase, ActionSequenceStopped)
        and isinstance(
            sequence.phase.failure,
            (UncertainAdmission, RuntimeFailure, ExecutionAuthorityLost),
        )
    ) or any(
        isinstance(arm.record.phase, CommandOutcomeUnknown)
        for step in sequence.steps
        for arm in step
    )


def must_stop_after_tools(results: Sequence[RobotToolResult]) -> bool:
    # INVARIANT: A refusal confirms this command never ran. An interruption
    # without that evidence can leave the executed motion uncertain.
    return any(
        result.halt
        or (result.interrupted and not recoverable_refusal(result.data))
        or sequence_outcome_requires_stop(result)
        for result in results
    )


def needs_live_observation(call: RobotToolCall) -> bool:
    if call.name in MEMORY_TOOLS or call.name == "preview_pose":
        return False
    return not (
        call.name == "observe"
        and isinstance(call.arguments, dict)
        and call.arguments.get("source") == "last_sequence"
    )


def next_observation_revisions(
    previous: Mapping[str, int], results: Sequence[RobotToolResult]
) -> Mapping[str, int] | None:
    if any(recoverable_refusal(result.data) for result in results):
        return None
    live_results = [result for result in results if needs_live_observation(result.call)]
    if not live_results:
        return previous
    # CONSTRAINT: A live observe in a concurrent motion batch can precede motion.
    if len(live_results) == 1 and live_results[0].call.name == "observe":
        return live_results[0].observed_revisions
    return None


class AgentRobotTools(RobotTools):
    finish_reason: str | None = None
    plan: UpdatePlanArguments | None = None
    notes: tuple[str, ...] = ()
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
                name=UPDATE_PLAN_TOOL,
                description=(
                    "Replace the current task plan. Each step has text and status "
                    "(pending, active, done, failed). The plan appears in observations."
                ),
                parameters_schema=UpdatePlanArguments.model_json_schema(),
            ),
            RobotToolSpec(
                name=UPDATE_NOTES_TOOL,
                description=(
                    "Replace this run's notes. Pass the complete list to keep. "
                    "The notes appear with the plan in observations."
                ),
                parameters_schema=UpdateNotesArguments.model_json_schema(),
            ),
            RobotToolSpec(
                name="finish",
                description="End the run and report the task outcome. Call alone.",
                parameters_schema=FinishArguments.model_json_schema(),
            ),
        ]

    def memory_lines(self) -> list[str]:
        lines: list[str] = []
        if self.plan is not None:
            lines.append("Current plan:")
            lines.extend(
                f"{index}. [{step.status}] {' '.join(step.text.splitlines())}"
                for index, step in enumerate(self.plan.steps, start=1)
            )
        if self.notes:
            lines.append("Notes:")
            lines.extend(f"- {' '.join(note.splitlines())}" for note in self.notes)
        return lines

    async def capture(
        self, note: str = "", extra_lines: Sequence[str] = ()
    ) -> CapturedObservation:
        return await super().capture(note, (*self.memory_lines(), *extra_lines))

    async def observation_is_stale(self, revisions: Mapping[str, int]) -> bool:
        if self.station.lease_lost.is_set():
            raise LeaseLost
        arm = await self.station.arm(self.arm).state()
        return arm.revision != revisions.get(self.arm) or arm.mode in {
            ArmMode.disconnected,
            ArmMode.reconnected,
            ArmMode.faulted,
            ArmMode.disabled,
        }

    async def apply_memory_update(self, call: RobotToolCall) -> RobotToolResult:
        try:
            if call.name == UPDATE_PLAN_TOOL:
                self.plan = UpdatePlanArguments.model_validate(call.arguments)
            else:
                self.notes = tuple(
                    UpdateNotesArguments.model_validate(call.arguments).notes
                )
        except ValidationError as error:
            return RobotToolResult(call, str(error), is_error=True, data=str(error))
        return RobotToolResult(call, "ok", is_error=False, data="ok")

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
        if (
            calls
            and expected_revisions is not None
            and await self.observation_is_stale(expected_revisions)
        ):
            detail = (
                "The arm changed before this batch ran. "
                "Re-evaluate from the fresh observation."
            )
            return [
                RobotToolResult(
                    call,
                    detail,
                    is_error=True,
                    interrupted=True,
                    data=CommandRefused(
                        reason=RejectionReason.stale_revision,
                        detail=detail,
                    ).model_dump(mode="json"),
                )
                for call in calls
            ]
        if not any(call.name == "finish" for call in calls):
            wrap_call = wrap or self.trace_tool_call
            results_by_index: dict[int, RobotToolResult] = {}
            robot_calls: list[tuple[int, RobotToolCall]] = []
            # INVARIANT: Apply memory updates in call order before the robot batch.
            for index, call in enumerate(calls):
                if call.name in MEMORY_TOOLS:
                    results_by_index[index] = await wrap_call(
                        call, partial(self.apply_memory_update, call)
                    )
                else:
                    robot_calls.append((index, call))
            robot_results = await super().execute(
                [call for _, call in robot_calls],
                wrap_call,
                expected_revision=expected_revision,
                expected_revisions=expected_revisions,
            )
            memory_lines = self.memory_lines()
            for (index, _), result in zip(robot_calls, robot_results, strict=True):
                if memory_lines and result.observed_revisions is not None:
                    result = replace(
                        result, text="\n".join((*memory_lines, result.text))
                    )
                results_by_index[index] = result
            return [results_by_index[index] for index in range(len(calls))]
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
    robot: AgentRobotTools, task: str, model: str, instructions: str
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
        for turn_number in count(1):
            if revisions is None or await robot.observation_is_stale(revisions):
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
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            must_stop = must_stop_after_tools(turn.results)
            if must_stop or not turn.results:
                print(response.output_text)
                report_stop(must_stop)
                return
            revisions = next_observation_revisions(revisions, turn.results)


async def run_anthropic(
    robot: AgentRobotTools, task: str, model: str, instructions: str
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
        for turn_number in count(1):
            if revisions is None or await robot.observation_is_stale(revisions):
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
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            must_stop = must_stop_after_tools(turn.results)
            if must_stop or not turn.results:
                print(
                    "\n".join(
                        block.text for block in response.content if block.type == "text"
                    )
                )
                report_stop(must_stop)
                return
            revisions = next_observation_revisions(revisions, turn.results)


async def run_gemini(
    robot: AgentRobotTools, task: str, model: str, instructions: str
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
        for turn_number in count(1):
            if revisions is None or await robot.observation_is_stale(revisions):
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
            if robot.finish_reason is not None:
                print(robot.finish_reason)
                return
            must_stop = must_stop_after_tools(turn.results)
            if must_stop or not turn.results:
                print(response.output_text or "")
                report_stop(must_stop)
                return
            revisions = next_observation_revisions(revisions, turn.results)


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
    *,
    optional_cameras: Sequence[str] = (),
    tracer_provider: trace.TracerProvider,
) -> None:
    async with station.holding_lease(task, required_cameras=cameras) as lease:
        robot = AgentRobotTools(
            station, lease, arm_name, cameras, optional_cameras=optional_cameras
        ).map_tools(filter_sequence_actions(lambda action: action != "joints"))
        robot.tracer_provider = tracer_provider
        if directory := os.environ.get("HEBBIAN_RUA_MODEL_INPUTS_DIR"):
            robot.model_inputs = ModelInputCapture(directory)
        instructions = (
            "Use the robot tools to carry out the task. You are operating in a "
            "supervised development workspace. Operate the configured SO-101 using "
            "Cartesian targets and gripper actions. Pose targets are in the station "
            "world frame. In the standard SO-101 layout, +X is forward, +Y is left "
            "when facing +X, and +Z is up. Camera image directions depend on the camera "
            "pose. Choose the next useful action for the task. Preview "
            "answers a specific feasibility question without moving the arm. The loop "
            "supplies arm state and available camera images after action batches when "
            "the run continues. Observe can supply an additional view or wait for a "
            "scene change. For a limited move, pose_limited.advice, when present, "
            "describes the shortfall, where the arm went, and a reachable alternative. "
            "A stopped_short move settled outside tolerance. Its completion detail "
            "describes the remaining error, which does not identify the cause on its "
            "own. Use the outcome and observations to decide whether to revise the "
            "target or finish. Use update_plan to replace the task plan and update_notes "
            "to replace the run's notes. Both may accompany robot tools in the same turn. "
            "Call finish alone to report the task outcome. "
            "Robot context: " + json.dumps(await robot.context())
        )
        arm = station.arm(arm_name)
        if (await arm.state()).mode != ArmMode.holding:
            enabled = await arm.enable()
            if not enabled.completed:
                raise SystemExit(f"Could not enable the arm: {enabled}")
        match provider:
            case "openai":
                await run_openai(robot, task, model, instructions)
            case "anthropic":
                await run_anthropic(robot, task, model, instructions)
            case "gemini":
                await run_gemini(robot, task, model, instructions)


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
    arm: Annotated[
        str | None, Parameter(help="Arm name. Defaults to the station's sole arm.")
    ] = None,
    camera: Annotated[
        tuple[str, ...],
        Parameter(
            help="Required station camera name. Repeat for multiple cameras.",
            negative=False,
        ),
    ] = (),
    optional_camera: Annotated[
        tuple[str, ...],
        Parameter(
            help="Station camera that may be unavailable. Repeat for multiple cameras.",
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
                optional_cameras=optional_camera,
                tracer_provider=tracer_provider,
            )
    return None


if __name__ == "__main__":
    command_line()
