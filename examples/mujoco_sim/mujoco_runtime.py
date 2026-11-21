# CONSTRAINT: MuJoCo chooses its GL backend on first import; select it before any other
# module imports MuJoCo. OSMesa works without a GPU unless MUJOCO_GL overrides it.

import os
import sys

if sys.platform.startswith("linux"):
    os.environ.setdefault("MUJOCO_GL", "osmesa")
    os.environ.setdefault("PYOPENGL_PLATFORM", os.environ["MUJOCO_GL"])

import mujoco

__all__ = ["mujoco"]
