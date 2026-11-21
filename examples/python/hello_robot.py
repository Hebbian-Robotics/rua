import argparse
import asyncio

from hebbian_rua import StationClient
from hebbian_rua.client.contract.station_api import (
    ArmMode,
    PreviewRefused,
    ProjectedPoseOutcome,
    QuaternionOrientation,
    UnavailableProjection,
)
from hebbian_rua.client.environment import load_environment
from hebbian_rua.client.station_client import (
    JointSpeedRadiansPerSecond,
    WorldPositionMeters,
)


async def main() -> None:
    parser = argparse.ArgumentParser(description="Preview and move an SO-101 arm.")
    parser.add_argument("--arm", help="Arm name. Defaults to the station's sole arm.")
    arguments = parser.parse_args()
    load_environment()
    async with StationClient() as station:
        try:
            arm_name = await station.resolve_arm_name(arguments.arm)
        except ValueError as error:
            parser.error(str(error))
        arm = station.arm(arm_name)
        # Holding the lease keeps the viewer and other programs off the arm while we run.
        # Only the lease can move the arm; enable, stop and reads are on `arm`.
        async with station.holding_lease("hello robot") as lease:
            moving_arm = lease.arm(arm_name)
            if (await arm.state()).mode != ArmMode.holding:
                enabled = await arm.enable()  # the arm holds its current pose
                if not enabled.completed:
                    raise SystemExit(f"Could not enable the arm: {enabled}")
            target = WorldPositionMeters([0.18, 0.0, 0.16])
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
            speed = JointSpeedRadiansPerSecond(0.25)
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
