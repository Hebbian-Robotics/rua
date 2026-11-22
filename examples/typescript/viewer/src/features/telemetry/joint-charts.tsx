import { memo, useEffect, useRef, useState } from "react";
import uPlot from "uplot";
import "uplot/dist/uPlot.min.css";
import type { StationState } from "hebbian-rua";
import type { StateTrack } from "../../recording-timeline.js";
import { radiansToDegrees } from "../../shared/measurements.js";
import { stationConnection } from "../../viewer-connection.js";

const JOINT_TICK_INCREMENTS = [15, 30, 45, 90, 180].map(
  (degrees) => (degrees * Math.PI) / 180,
);

const JOINT_COLORS = [
  "--joint-1",
  "--joint-2",
  "--joint-3",
  "--joint-4",
  "--joint-5",
  "--joint-6",
  "--joint-7",
  "--joint-8",
];

function createTelemetryPlot(
  element: HTMLDivElement,
  startTime: number,
  seek: { current: ((time: number) => void) | undefined },
  series: uPlot.Series[],
  verticalRange: [number, number],
  verticalAxis: uPlot.Axis = {},
) {
  const plot = new uPlot(
    {
      width: Math.max(160, element.clientWidth),
      height: 200,
      legend: { show: false },
      scales: { x: { time: false }, y: { range: verticalRange } },
      axes: [
        {
          stroke: () => getComputedStyle(element).color,
          values: (_chart, ticks) =>
            ticks.map((tick) =>
              startTime === 0
                ? new Date(tick * 1000).toLocaleTimeString()
                : `${Math.floor(tick / 60)}:${String(Math.floor(tick % 60)).padStart(2, "0")}`,
            ),
        },
        { stroke: () => getComputedStyle(element).color, ...verticalAxis },
      ],
      series,
    },
    [[], ...series.slice(1).map(() => [])] as uPlot.AlignedData,
    element,
  );
  function click(event: MouseEvent): void {
    seek.current?.(startTime + plot.posToVal(event.offsetX, "x"));
  }
  plot.over.addEventListener("click", click);
  const observer = new ResizeObserver(() =>
    plot.setSize({ width: Math.max(160, element.clientWidth), height: 200 }),
  );
  observer.observe(element);
  return {
    plot,
    dispose: () => {
      observer.disconnect();
      plot.over.removeEventListener("click", click);
      plot.destroy();
    },
  };
}

function JointChart({
  arm,
  jointNames,
  times,
  startTime,
  currentTime,
  showTarget,
  showCommanded,
  onSeek,
}: {
  arm: StateTrack["arms"][number];
  jointNames: string[];
  times: number[];
  startTime: number;
  currentTime: number;
  showTarget: boolean;
  showCommanded: boolean;
  onSeek?: (time: number) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const chart = useRef<uPlot | null>(null);
  const seek = useRef(onSeek);
  seek.current = onSeek;
  const namesKey = JSON.stringify(jointNames);
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const names: string[] = JSON.parse(namesKey);
    const colors = getComputedStyle(element);
    const jointColor = (index: number) =>
      colors.getPropertyValue(
        JOINT_COLORS[index % JOINT_COLORS.length] ?? "--ink",
      );
    const series: uPlot.Series[] = [{}];
    for (const [index, name] of names.entries())
      series.push({
        label: name,
        stroke: () => jointColor(index),
        width: 1.25,
      });
    for (const [index, name] of names.entries())
      series.push({
        label: `${name} target`,
        stroke: () => jointColor(index),
        width: 1,
        dash: [4, 4],
      });
    for (const [index, name] of names.entries())
      series.push({
        label: `${name} commanded`,
        stroke: () => jointColor(index),
        width: 1,
        dash: [1, 3],
      });
    const { plot, dispose } = createTelemetryPlot(
      element,
      startTime,
      seek,
      series,
      [-Math.PI, Math.PI],
      {
        incrs: JOINT_TICK_INCREMENTS,
        values: (_chart, ticks) =>
          ticks.map((radians) => `${Math.round(radiansToDegrees(radians))}°`),
      },
    );
    chart.current = plot;
    return () => {
      dispose();
      chart.current = null;
    };
  }, [namesKey, startTime]);
  useEffect(() => {
    const plot = chart.current;
    if (!plot) return;
    const values = [
      times.map((time) => time - startTime),
      ...arm.measuredJoints,
      ...jointNames.map((_, index) =>
        arm.destinationJoints.map((joints) => joints?.[index] ?? null),
      ),
      ...arm.commandedJoints,
    ];
    plot.setData(values as uPlot.AlignedData);
    if (
      times.length > 1 &&
      currentTime >= (times[0] ?? 0) &&
      currentTime <= (times.at(-1) ?? 0)
    )
      plot.setCursor({
        left: plot.valToPos(currentTime - startTime, "x"),
        top: -10,
      });
  }, [arm, jointNames, times, startTime, currentTime]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: Reapply visibility when a new plot is created for a different start time.
  useEffect(() => {
    const plot = chart.current;
    if (!plot) return;
    const count = (JSON.parse(namesKey) as string[]).length;
    plot.batch(() => {
      for (let index = 0; index < count; index += 1) {
        plot.setSeries(1 + count + index, { show: showTarget });
        plot.setSeries(1 + 2 * count + index, { show: showCommanded });
      }
    });
  }, [namesKey, startTime, showTarget, showCommanded]);
  return (
    <section className="joint-chart">
      <h3>{arm.arm} · joint angles</h3>
      <div className="joint-legend">
        {jointNames.map((name, index) => (
          <span
            key={name}
            style={{
              color: `var(${JOINT_COLORS[index % JOINT_COLORS.length]})`,
            }}
          >
            {name}
          </span>
        ))}
      </div>
      <p>Measured · target dashed · commanded dotted</p>
      <div className="chart-plot" ref={container} />
    </section>
  );
}

function GripperChart({
  arm,
  times,
  startTime,
  onSeek,
}: {
  arm: StateTrack["arms"][number];
  times: number[];
  startTime: number;
  onSeek?: (time: number) => void;
}) {
  const container = useRef<HTMLDivElement>(null);
  const chart = useRef<uPlot | null>(null);
  const seek = useRef(onSeek);
  const latestSampleTime = times.at(-1);
  seek.current = onSeek;
  useEffect(() => {
    const element = container.current;
    if (!element) return;
    const { plot, dispose } = createTelemetryPlot(
      element,
      startTime,
      seek,
      [
        {},
        {
          label: "Gripper",
          stroke: () =>
            getComputedStyle(element).getPropertyValue("--accent-ink"),
          width: 1.5,
        },
      ],
      [0, 1],
      {
        values: (_chart, ticks) =>
          ticks.map((tick) => `${Math.round(tick * 100)}%`),
      },
    );
    chart.current = plot;
    return () => {
      dispose();
      chart.current = null;
    };
  }, [startTime]);
  useEffect(() => {
    if (latestSampleTime === undefined) return;
    chart.current?.setData([
      times.map((time) => time - startTime),
      arm.gripperOpening,
    ] as uPlot.AlignedData);
  }, [arm, times, startTime, latestSampleTime]);
  return (
    <section className="joint-chart">
      <h3>{arm.arm} · gripper opening</h3>
      <p>Measured opening</p>
      <div className="chart-plot" ref={container} />
    </section>
  );
}

export function JointCharts({
  track,
  startTime,
  currentTime,
  showTarget,
  showCommanded,
  onSeek,
}: {
  track: StateTrack;
  startTime: number;
  currentTime: number;
  showTarget: boolean;
  showCommanded: boolean;
  onSeek?: (time: number) => void;
}) {
  return (
    <div className="joint-charts">
      {track.arms.map((arm) => (
        <JointChart
          key={arm.arm}
          arm={arm}
          jointNames={track.jointNames}
          times={track.times}
          startTime={startTime}
          currentTime={currentTime}
          showTarget={showTarget}
          showCommanded={showCommanded}
          onSeek={onSeek}
        />
      ))}
      {track.arms
        .filter((arm) => arm.gripperOpening.some((opening) => opening !== null))
        .map((arm) => (
          <GripperChart
            key={`${arm.arm}-gripper`}
            arm={arm}
            times={track.times}
            startTime={startTime}
            onSeek={onSeek}
          />
        ))}
    </div>
  );
}

function appendState(
  track: StateTrack | null,
  state: StationState,
  now: number,
): StateTrack {
  const compatible =
    track &&
    JSON.stringify([track.jointNames, track.arms.map((arm) => arm.arm)]) ===
      JSON.stringify([state.joint_names, state.arms.map((arm) => arm.arm)]);
  const current: StateTrack =
    compatible && track
      ? track
      : {
          times: [],
          jointNames: state.joint_names,
          arms: state.arms.map((arm) => ({
            arm: arm.arm,
            basePose: arm.base_pose,
            measuredJoints: state.joint_names.map(() => []),
            commandedJoints: state.joint_names.map(() => []),
            destinationJoints: [],
            destinationLimited: [],
            gripperOpening: [],
          })),
        };
  current.times.push(now);
  for (const sample of current.arms) {
    const arm = state.arms.find((candidate) => candidate.arm === sample.arm);
    sample.measuredJoints.forEach((series, index) => {
      series.push(arm?.measured_joints[index] ?? null);
    });
    sample.commandedJoints.forEach((series, index) => {
      series.push(arm?.commanded_joints?.[index] ?? null);
    });
    sample.destinationJoints.push(arm?.active_command?.target_joints ?? null);
    sample.destinationLimited.push(
      Boolean(
        arm?.active_command?.path_limited || arm?.active_command?.pose_limited,
      ),
    );
    sample.gripperOpening.push(arm?.gripper_opening ?? null);
  }
  while (current.times.length > 1200 || (current.times[0] ?? now) < now - 30) {
    current.times.shift();
    for (const arm of current.arms)
      for (const series of [
        ...arm.measuredJoints,
        ...arm.commandedJoints,
        arm.destinationJoints,
        arm.destinationLimited,
        arm.gripperOpening,
      ])
        series.shift();
  }
  return current;
}

export const LiveTelemetry = memo(function LiveTelemetry({
  showTarget,
  showCommanded,
  visible,
}: {
  showTarget: boolean;
  showCommanded: boolean;
  visible: boolean;
}) {
  const [track, setTrack] = useState<StateTrack | null>(null);
  const trackRef = useRef<StateTrack | null>(null);
  const lastSample = useRef(0);
  useEffect(
    () =>
      stationConnection.onState((state) => {
        const now = Date.now() / 1000;
        if (now - lastSample.current < 0.1) return;
        lastSample.current = now;
        trackRef.current = appendState(trackRef.current, state, now);
        setTrack({ ...trackRef.current });
      }),
    [],
  );
  if (!visible) return null;
  if (!track || track.times.length < 2) return <p>Waiting for telemetry.</p>;
  return (
    <JointCharts
      track={track}
      startTime={0}
      currentTime={track.times.at(-1) ?? 0}
      showTarget={showTarget}
      showCommanded={showCommanded}
    />
  );
});
