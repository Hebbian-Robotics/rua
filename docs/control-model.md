# Control model

The station manages robot state and accepts commands through its API. A client
acquires a control lease before sending motion commands. Other clients can
continue reading state and camera snapshots.

In Python, use `station.holding_lease(...)` and send motion through the returned
lease. Power, stop and state access are available through `station.arm(...)`.
Commands return outcomes; check them before proceeding to another action.

The station enforces configured joint limits and protective stops. These checks
require per-arm calibration and do not establish physical workspace clearance.
See [hardware setup](hardware.md) before operating a physical arm, and the
[Python example](../examples/python/hello_robot.py) for a small control program.
