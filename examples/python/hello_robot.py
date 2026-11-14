"""The smallest script that moves a robot: connect, move, open the gripper.

    # Start a station first (see the README), then:
    uv run --project examples/python python examples/python/hello_robot.py

Poses are tool positions in metres, in the robot's frame. Every move returns an
outcome: "completed" means the planned motion finished; the plan may be limited. To control how the gripper points,
pass an orientation:
`lease.arm("follower").move_to_pose(target, GraspOrientation(approach="down", jaw_yaw_degrees=180))`.
Motion exists only on the lease; power, stop and reads are on `station.arm(...)`.
"""

import asyncio
import os

from hebbian_robot_use_client.contract.station_api import (
    ArmMode,
    PreviewRefused,
    ProjectedPoseOutcome,
    QuaternionOrientation,
    UnavailableProjection,
)
from hebbian_robot_use_client.station_client import (
    DEFAULT_SERVER,
    JointSpeedRadiansPerSecond,
    WorldPositionMeters,
)

from hebbian_rua import StationClient


async def main() -> None:
    async with StationClient(
        os.environ.get("HEBBIAN_STATION_URL", DEFAULT_SERVER)
    ) as station:
        arm = station.arm("follower")
        # Holding the lease keeps the viewer and other programs off the arm while we run.
        # Only the lease can move the arm; enable, stop and reads are on `arm`.
        async with station.holding_lease("hello robot") as lease:
            moving_arm = lease.arm("follower")
            if (await arm.state()).mode != ArmMode.holding:
                enabled = await arm.enable()  # the arm holds its current pose
                if not enabled.completed:
                    raise SystemExit(f"Could not enable the arm: {enabled}")
            target = WorldPositionMeters([0.22, 0.0, 0.10])
            preview = await arm.preview_pose(
                target
            )  # let the solver choose an orientation
            if isinstance(preview, PreviewRefused):
                raise SystemExit(f"Preview refused: {preview.reason}: {preview.detail}")
            match preview.outcome:
                case ProjectedPoseOutcome(pose=pose, within_tolerance=True):
                    orientation = QuaternionOrientation(quaternion=pose.quaternion)
                case ProjectedPoseOutcome(reason=reason):
                    raise SystemExit(
                        f"Preview only found an approximate pose: {reason}"
                    )
                case UnavailableProjection(reason=reason):
                    raise SystemExit(f"Preview is unavailable: {reason}")
            speed = JointSpeedRadiansPerSecond(0.5)
            result = await moving_arm.move_to_pose(
                target, orientation, max_joint_speed=speed
            )
            print(result.outcome, result.detail)  # "completed", or why not
            if not result.completed:
                raise SystemExit("Move did not complete")
            if result.was_limited:
                raise SystemExit("The planned move was limited before the target")
            print((await moving_arm.set_gripper(1.0)).outcome)  # 0 is closed, 1 is open
            print((await arm.state()).tool)  # where the tool actually is


if __name__ == "__main__":
    asyncio.run(main())
