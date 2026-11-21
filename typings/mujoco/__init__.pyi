# The subset of MuJoCo 3.x that examples/mujoco_sim uses; the compiled
# module's members are not visible to ty. MuJoCo is an optional dependency (the sim extra).
from collections.abc import Sequence
from enum import Enum

import numpy as np

class mjtIntegrator(Enum):
    mjINT_EULER: int
    mjINT_IMPLICITFAST: int

class mjtGeom(Enum):
    mjGEOM_BOX = 6

class MjsCompiler:
    fusestatic: bool
    boundmass: float
    boundinertia: float

class MjsElement:
    name: str

class MjsJoint(MjsElement): ...
class MjsGeom(MjsElement): ...
class MjsMesh(MjsElement): ...

class _Options:
    integrator: int
    timestep: float

class _JointView:
    id: int
    qposadr: np.ndarray
    dofadr: np.ndarray

class _ActuatorView:
    id: int

class _SiteView:
    id: int

class MjModel:
    @classmethod
    def from_xml_path(cls, path: str) -> MjModel: ...
    def site(self, name: str) -> _SiteView: ...
    opt: _Options
    actuator_gainprm: np.ndarray
    actuator_biasprm: np.ndarray
    actuator_forcelimited: np.ndarray
    actuator_forcerange: np.ndarray
    jnt_limited: np.ndarray
    jnt_range: np.ndarray
    def joint(self, name: str) -> _JointView: ...
    def actuator(self, name: str) -> _ActuatorView: ...

class MjData:
    site_xpos: np.ndarray
    site_xmat: np.ndarray
    qpos: np.ndarray
    qvel: np.ndarray
    ctrl: np.ndarray
    actuator_force: np.ndarray
    def __init__(self, model: MjModel) -> None: ...

class MjsFrame:
    def attach_body(self, body: MjsBody, prefix: str, suffix: str) -> MjsBody: ...

class MjsBody(MjsElement):
    pos: np.ndarray
    quat: np.ndarray
    parent: MjsBody
    bodies: list[MjsBody]
    def add_body(
        self,
        *,
        name: str,
        pos: Sequence[float],
        quat: Sequence[float] = ...,
    ) -> MjsBody: ...
    def add_site(
        self, *, name: str, pos: Sequence[float] = ..., quat: Sequence[float] = ...
    ) -> object: ...
    def add_frame(self, *, pos: Sequence[float], quat: Sequence[float]) -> MjsFrame: ...
    def first_body(self) -> MjsBody: ...
    def add_geom(
        self, *, type: mjtGeom, size: Sequence[float], rgba: Sequence[float]
    ) -> object: ...
    def add_camera(
        self,
        *,
        name: str,
        pos: Sequence[float],
        quat: Sequence[float],
        fovy: float,
    ) -> object: ...

class MjSpec:
    worldbody: MjsBody
    compiler: MjsCompiler
    bodies: list[MjsBody]
    joints: list[MjsJoint]
    geoms: list[MjsGeom]
    meshes: list[MjsMesh]
    def body(self, name: str) -> MjsBody: ...
    def delete(self, element: MjsElement) -> None: ...
    @staticmethod
    def from_file(filename: str) -> MjSpec: ...
    def compile(self) -> MjModel: ...

class MjvCamera:
    lookat: np.ndarray
    distance: float
    azimuth: float
    elevation: float

class Renderer:
    def __init__(self, model: MjModel, height: int, width: int) -> None: ...
    def update_scene(self, data: MjData, camera: MjvCamera | str | int) -> None: ...
    def render(self) -> np.ndarray: ...
    def close(self) -> None: ...

def mju_mat2Quat(quat: np.ndarray, mat: np.ndarray) -> None: ...
def mj_forward(model: MjModel, data: MjData) -> None: ...
def mj_step(model: MjModel, data: MjData) -> None: ...
