"""Client library for a Hebbian robot use server you run yourself.

The wire models (`hebbian_robot_use_client.contract`), a typed client for the
station (`hebbian_robot_use_client.station_client`: arms, control of the arms and
command outcomes over the station's HTTP event stream, cameras over HTTP), and
`hebbian_robot_use_client.llm`, the robot as an LLM's tools.

Boundary rules (enforced by tests/test_hebbian_robot_use_client_boundary.py):

- depends on pydantic, httpx2 and python-dotenv, so
  installing it never pulls in robot, solver or video stacks;
- never imports `hebbian_robot_use_tools` (controller, planning, safety, the web
  process) or the examples: the station depends on this
  library for its wire format, not the other way round;
- speaks only the station's network API, so it works against a simulated station and
  against hardware in the same way.
"""
