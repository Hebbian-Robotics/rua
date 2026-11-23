# Simulate the SO-101 with MuJoCo

This fixture runs one or two virtual SO-101 arms using the same station and
control APIs as hardware. Follow the [MuJoCo quickstart](../../docs/simulation.md)
to start the station, launch MuJoCo, and try an example. The fixture's Python
environment installs the published Python SDK.

- [`profiles/`](profiles/) contains the single-arm and bimanual station profiles.
- [`scene.py`](scene.py) assembles the arm and table from the SO-101 model.
- [`sim_plant.py`](sim_plant.py) connects the MuJoCo plant to the station.
- [`camera_server.py`](camera_server.py) serves overview and wrist snapshots.

For another simulator, see the [simulator adapter contract](../../docs/simulators.md).
