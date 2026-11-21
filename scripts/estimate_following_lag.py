import argparse
import itertools
import json
from pathlib import Path

import numpy as np

# WHY: Slower commands are dominated by static tracking error.
MINIMUM_MOVING_SPEED_RADIANS_PER_SECOND = 0.2
MAXIMUM_CRUISE_ACCELERATION_RADIANS_PER_SECOND_SQUARED = 1.0
SPEED_BANDS = [0.2, 0.75, 1.5, 2.0, 2.4, 10.0]
PEAK_WINDOW_CYCLES = 5


def read_trace(path: Path) -> dict[str, np.ndarray]:
    rows = [json.loads(line) for line in path.read_text().splitlines() if line]
    return {
        "time": np.array([row["readTime"] for row in rows]),
        "command": np.array([row["command"] for row in rows]),
        "positions": np.array([row["positions"] for row in rows]),
    }


def best_delay(time: np.ndarray, command: np.ndarray, measured: np.ndarray) -> float:
    moving = (
        np.abs(np.gradient(command, time)) > MINIMUM_MOVING_SPEED_RADIANS_PER_SECOND
    )
    candidates = np.arange(0.0, 0.3, 0.002)
    costs = [
        np.mean((measured[moving] - np.interp(time - d, time, command)[moving]) ** 2)
        for d in candidates
    ]
    return float(candidates[int(np.argmin(costs))])


def report_joint(traces: list[dict[str, np.ndarray]], joint: int) -> None:
    errors, velocities, delays = [], [], []
    peak_speed = 0.0
    for trace in traces:
        time = trace["time"]
        command = trace["command"][:, joint]
        measured = trace["positions"][:, joint]
        velocity = np.gradient(command, time)
        acceleration = np.gradient(velocity, time)
        cruising = (np.abs(velocity[:-1]) > MINIMUM_MOVING_SPEED_RADIANS_PER_SECOND) & (
            np.abs(acceleration[:-1])
            < MAXIMUM_CRUISE_ACCELERATION_RADIANS_PER_SECOND_SQUARED
        )
        errors.append((command[:-1] - measured[1:])[cruising])
        velocities.append(velocity[:-1][cruising])
        if cruising.sum() >= 10:
            delays.append(best_delay(time, command, measured))
        if len(time) > PEAK_WINDOW_CYCLES:
            reached = np.abs(
                measured[PEAK_WINDOW_CYCLES:] - measured[:-PEAK_WINDOW_CYCLES]
            ) / (time[PEAK_WINDOW_CYCLES:] - time[:-PEAK_WINDOW_CYCLES])
            peak_speed = max(peak_speed, float(reached.max()))
    error = np.concatenate(errors)
    velocity = np.concatenate(velocities)
    print(f"joint {joint}: peak reached speed {peak_speed:.2f} rad/s")
    if error.size < 10:
        print("  too few cruise samples")
        return
    if (velocity > 0).any() and (velocity < 0).any():
        lag, offset = np.polyfit(velocity, error, 1)
    else:
        lag, offset = float(np.median(error / velocity)), 0.0
    print(
        f"  following_lag_seconds {lag:.3f} "
        f"(error = lag * velocity {offset:+.3f} rad, {error.size} cruise samples)"
    )
    speed = np.abs(velocity)
    per_sample_lag = (error - offset) / velocity
    for low, high in itertools.pairwise(SPEED_BANDS):
        band = (speed >= low) & (speed < high)
        if band.sum() >= 5:
            print(
                f"    {low:.2f}-{high:.2f} rad/s: lag median "
                f"{np.median(per_sample_lag[band]):.3f} s, n={band.sum()}"
            )
    print(f"  cross-correlation delay per run: {np.round(delays, 3)} s")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(
        description="Estimate joint lag from controller traces."
    )
    parser.add_argument(
        "traces", type=Path, nargs="+", help="Completed JSONL trace files"
    )
    arguments = parser.parse_args()
    loaded = []
    for trace_path in arguments.traces:
        try:
            trace = read_trace(trace_path)
        except (OSError, ValueError, KeyError) as error:
            parser.error(f"{trace_path}: {error}")
        if len(trace["time"]) < 2:
            parser.error(f"{trace_path}: expected at least two recorded cycles")
        loaded.append(trace)
    for joint_index in range(loaded[0]["command"].shape[1]):
        report_joint(loaded, joint_index)
