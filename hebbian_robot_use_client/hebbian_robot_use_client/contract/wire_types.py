"""Types shared by the client's payload models: pydantic and the standard library only.

Keeping them free of the driver and simulator stacks (MuJoCo) means importing the
contract never loads any of them; tests/test_hebbian_robot_use_client_boundary.py
guards that. The station's own types are generated from Rust into `station_api.py`.
"""

from typing import Annotated, Literal

from pydantic import BaseModel, ConfigDict, Field
from pydantic.alias_generators import to_camel

FiniteFloat = Annotated[float, Field(strict=True, allow_inf_nan=False)]
# Which tool-frame axis a gripper's jaws close along.
type JawClosingAxis = Literal["x", "y"]


class CamelCaseModel(BaseModel):
    """JSON uses camelCase keys; Python code uses snake_case attribute names."""

    model_config = ConfigDict(
        extra="forbid",
        frozen=True,
        alias_generator=to_camel,
        validate_by_alias=True,
        validate_by_name=True,
        serialize_by_alias=True,
        # Serialized output always includes defaulted fields, so consumers of
        # the exported schemas can rely on them being present.
        json_schema_serialization_defaults_required=True,
    )
