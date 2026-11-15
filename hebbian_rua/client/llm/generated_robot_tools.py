"""Generated from schemas/robot-tools.json; do not edit."""

import json

ROBOT_TOOL_CATALOG = json.loads(r"""
[
  {
    "description": "Execute steps in order; each step can move and set the gripper together. The result reports completed steps and why execution stopped.",
    "name": "action_sequence",
    "operation": "start_action_sequence",
    "parameters": {
      "$defs": {
        "ActionSequenceMovement": {
          "description": "One movement in an action sequence. The arm settles at each target.",
          "oneOf": [
            {
              "additionalProperties": false,
              "properties": {
                "joints": {
                  "items": {
                    "type": "number"
                  },
                  "type": "array"
                },
                "kind": {
                  "const": "joints",
                  "default": "joints",
                  "type": "string"
                },
                "max_joint_speed": {
                  "anyOf": [
                    {
                      "type": "number"
                    },
                    {
                      "type": "null"
                    }
                  ],
                  "default": null
                }
              },
              "required": [
                "joints"
              ],
              "title": "ActionSequenceJointsMovement",
              "type": "object"
            },
            {
              "additionalProperties": false,
              "properties": {
                "kind": {
                  "const": "pose",
                  "default": "pose",
                  "type": "string"
                },
                "max_joint_speed": {
                  "anyOf": [
                    {
                      "type": "number"
                    },
                    {
                      "type": "null"
                    }
                  ],
                  "default": null
                },
                "orientation": {
                  "$ref": "#/$defs/PoseOrientation"
                },
                "position": {
                  "items": {
                    "type": "number"
                  },
                  "maxItems": 3,
                  "minItems": 3,
                  "type": "array"
                }
              },
              "required": [
                "position",
                "orientation"
              ],
              "title": "ActionSequencePoseMovement",
              "type": "object"
            }
          ]
        },
        "ActionSequenceStep": {
          "additionalProperties": false,
          "description": "A movement and gripper target in the same step start together.",
          "properties": {
            "gripper_opening": {
              "anyOf": [
                {
                  "type": "number"
                },
                {
                  "type": "null"
                }
              ],
              "default": null
            },
            "max_joint_speed": {
              "anyOf": [
                {
                  "type": "number"
                },
                {
                  "type": "null"
                }
              ],
              "default": null,
              "description": "Speed for both resources in rad/s; overrides the movement speed when provided."
            },
            "movement": {
              "anyOf": [
                {
                  "$ref": "#/$defs/ActionSequenceMovement"
                },
                {
                  "type": "null"
                }
              ],
              "default": null
            }
          },
          "type": "object"
        },
        "Approach": {
          "anyOf": [
            {
              "$ref": "#/$defs/NamedApproach"
            },
            {
              "items": {
                "type": "number"
              },
              "maxItems": 3,
              "minItems": 3,
              "title": "ApproachDirection",
              "type": "array"
            }
          ],
          "description": "The direction the jaws point along: a named one, or a nonzero world direction."
        },
        "NamedApproach": {
          "description": "A named approach direction.",
          "oneOf": [
            {
              "const": "down",
              "description": "World -Z.",
              "type": "string"
            },
            {
              "const": "forward",
              "description": "World +X.",
              "type": "string"
            }
          ]
        },
        "PoseOrientation": {
          "description": "A tool orientation as a client gives it.",
          "oneOf": [
            {
              "description": "The approach direction and the jaw yaw about it, in degrees.",
              "properties": {
                "approach": {
                  "$ref": "#/$defs/Approach"
                },
                "jaw_yaw_degrees": {
                  "type": "number"
                },
                "kind": {
                  "const": "grasp",
                  "default": "grasp",
                  "type": "string"
                }
              },
              "required": [
                "approach",
                "jaw_yaw_degrees"
              ],
              "title": "GraspOrientation",
              "type": "object"
            },
            {
              "description": "A raw quaternion, `[x, y, z, w]`.",
              "properties": {
                "kind": {
                  "const": "quaternion",
                  "default": "quaternion",
                  "type": "string"
                },
                "quaternion": {
                  "items": {
                    "type": "number"
                  },
                  "maxItems": 4,
                  "minItems": 4,
                  "type": "array"
                }
              },
              "required": [
                "quaternion"
              ],
              "title": "QuaternionOrientation",
              "type": "object"
            }
          ]
        }
      },
      "additionalProperties": false,
      "properties": {
        "steps": {
          "items": {
            "$ref": "#/$defs/ActionSequenceStep"
          },
          "minItems": 1,
          "type": "array"
        }
      },
      "required": [
        "steps"
      ],
      "type": "object"
    }
  },
  {
    "description": "Preview independent world-frame pose candidates without moving. Supply each candidate with a unique UUID v4 id, an optional label, and a pose. Results retain their ids and input order; each includes a projection or refusal. Omitting orientation lets the solver choose one.",
    "name": "preview_pose",
    "operation": "preview",
    "parameters": {
      "$defs": {
        "Approach": {
          "anyOf": [
            {
              "$ref": "#/$defs/NamedApproach"
            },
            {
              "items": {
                "type": "number"
              },
              "maxItems": 3,
              "minItems": 3,
              "title": "ApproachDirection",
              "type": "array"
            }
          ],
          "description": "The direction the jaws point along: a named one, or a nonzero world direction."
        },
        "NamedApproach": {
          "description": "A named approach direction.",
          "oneOf": [
            {
              "const": "down",
              "description": "World -Z.",
              "type": "string"
            },
            {
              "const": "forward",
              "description": "World +X.",
              "type": "string"
            }
          ]
        },
        "PoseOrientation": {
          "description": "A tool orientation as a client gives it.",
          "oneOf": [
            {
              "description": "The approach direction and the jaw yaw about it, in degrees.",
              "properties": {
                "approach": {
                  "$ref": "#/$defs/Approach"
                },
                "jaw_yaw_degrees": {
                  "type": "number"
                },
                "kind": {
                  "const": "grasp",
                  "default": "grasp",
                  "type": "string"
                }
              },
              "required": [
                "approach",
                "jaw_yaw_degrees"
              ],
              "title": "GraspOrientation",
              "type": "object"
            },
            {
              "description": "A raw quaternion, `[x, y, z, w]`.",
              "properties": {
                "kind": {
                  "const": "quaternion",
                  "default": "quaternion",
                  "type": "string"
                },
                "quaternion": {
                  "items": {
                    "type": "number"
                  },
                  "maxItems": 4,
                  "minItems": 4,
                  "type": "array"
                }
              },
              "required": [
                "quaternion"
              ],
              "title": "QuaternionOrientation",
              "type": "object"
            }
          ]
        },
        "PosePreviewCandidate": {
          "additionalProperties": false,
          "properties": {
            "id": {
              "description": "A caller-generated UUID v4, unique within the batch.",
              "type": "string"
            },
            "label": {
              "anyOf": [
                {
                  "type": "string"
                },
                {
                  "type": "null"
                }
              ],
              "default": null,
              "description": "Optional human-readable context, echoed with the answer."
            },
            "pose": {
              "$ref": "#/$defs/PreviewPose"
            }
          },
          "required": [
            "id",
            "pose"
          ],
          "type": "object"
        },
        "PreviewPose": {
          "additionalProperties": false,
          "description": "Where the arm's controller would take the tool toward a pose, without moving.",
          "properties": {
            "orientation": {
              "anyOf": [
                {
                  "$ref": "#/$defs/PoseOrientation"
                },
                {
                  "type": "null"
                }
              ],
              "default": null,
              "description": "None leaves the orientation to the solver."
            },
            "position": {
              "items": {
                "type": "number"
              },
              "maxItems": 3,
              "minItems": 3,
              "type": "array"
            }
          },
          "required": [
            "position"
          ],
          "type": "object"
        }
      },
      "additionalProperties": false,
      "properties": {
        "poses": {
          "items": {
            "$ref": "#/$defs/PosePreviewCandidate"
          },
          "minItems": 1,
          "type": "array"
        }
      },
      "required": [
        "poses"
      ],
      "type": "object"
    }
  },
  {
    "description": "Move named arms together. Pose targets use a world position and orientation; joint targets use radians. Model guards do not detect collisions between arms or with workspace objects.",
    "name": "move_arms",
    "operation": "move_arms",
    "parameters": {
      "$defs": {
        "Approach": {
          "anyOf": [
            {
              "$ref": "#/$defs/NamedApproach"
            },
            {
              "items": {
                "type": "number"
              },
              "maxItems": 3,
              "minItems": 3,
              "title": "ApproachDirection",
              "type": "array"
            }
          ],
          "description": "The direction the jaws point along: a named one, or a nonzero world direction."
        },
        "NamedApproach": {
          "description": "A named approach direction.",
          "oneOf": [
            {
              "const": "down",
              "description": "World -Z.",
              "type": "string"
            },
            {
              "const": "forward",
              "description": "World +X.",
              "type": "string"
            }
          ]
        },
        "PoseOrientation": {
          "description": "A tool orientation as a client gives it.",
          "oneOf": [
            {
              "description": "The approach direction and the jaw yaw about it, in degrees.",
              "properties": {
                "approach": {
                  "$ref": "#/$defs/Approach"
                },
                "jaw_yaw_degrees": {
                  "type": "number"
                },
                "kind": {
                  "const": "grasp",
                  "default": "grasp",
                  "type": "string"
                }
              },
              "required": [
                "approach",
                "jaw_yaw_degrees"
              ],
              "title": "GraspOrientation",
              "type": "object"
            },
            {
              "description": "A raw quaternion, `[x, y, z, w]`.",
              "properties": {
                "kind": {
                  "const": "quaternion",
                  "default": "quaternion",
                  "type": "string"
                },
                "quaternion": {
                  "items": {
                    "type": "number"
                  },
                  "maxItems": 4,
                  "minItems": 4,
                  "type": "array"
                }
              },
              "required": [
                "quaternion"
              ],
              "title": "QuaternionOrientation",
              "type": "object"
            }
          ]
        },
        "Waypoint": {
          "description": "One corner of a move: joints, or a tool pose solved for joints.",
          "oneOf": [
            {
              "properties": {
                "joints": {
                  "items": {
                    "type": "number"
                  },
                  "type": "array"
                },
                "kind": {
                  "const": "joints",
                  "default": "joints",
                  "type": "string"
                }
              },
              "required": [
                "joints"
              ],
              "title": "JointsWaypoint",
              "type": "object"
            },
            {
              "properties": {
                "kind": {
                  "const": "pose",
                  "default": "pose",
                  "type": "string"
                },
                "orientation": {
                  "$ref": "#/$defs/PoseOrientation"
                },
                "position": {
                  "items": {
                    "type": "number"
                  },
                  "maxItems": 3,
                  "minItems": 3,
                  "type": "array"
                }
              },
              "required": [
                "position",
                "orientation"
              ],
              "title": "PoseWaypoint",
              "type": "object"
            }
          ]
        }
      },
      "additionalProperties": false,
      "description": "Move several arms together: every target is planned before any arm moves,\none refusal refuses all, and when one arm ends without completing the others\nstop and hold. No collision checking between the arms.",
      "properties": {
        "max_joint_speed": {
          "anyOf": [
            {
              "type": "number"
            },
            {
              "type": "null"
            }
          ],
          "default": null
        },
        "targets": {
          "additionalProperties": {
            "$ref": "#/$defs/Waypoint"
          },
          "type": "object"
        }
      },
      "required": [
        "targets"
      ],
      "type": "object"
    }
  },
  {
    "description": "Set a named arm's gripper opening from 0 (closed) to 1 (fully open); intermediate values request partial openings.",
    "name": "set_arm_gripper",
    "operation": "set_gripper",
    "parameters": {
      "additionalProperties": false,
      "description": "Open or close the gripper, 0 closed to 1 open, whether the arm is holding\nor moving. It completes on arrival or on contact.",
      "properties": {
        "arm": {
          "type": "string"
        },
        "max_joint_speed": {
          "anyOf": [
            {
              "type": "number"
            },
            {
              "type": "null"
            }
          ],
          "default": null,
          "description": "Peak gripper speed in rad/s; uses the arm default and maximum."
        },
        "opening": {
          "type": "number"
        }
      },
      "required": [
        "opening",
        "arm"
      ],
      "type": "object"
    }
  },
  {
    "description": "Close the gripper, then return this arm to the joint pose recorded when it was enabled. Closing stops on contact, so a held object stays held; ensure the path and workspace are clear.",
    "name": "rehome",
    "operation": "rehome",
    "parameters": {
      "additionalProperties": false,
      "properties": {},
      "type": "object"
    }
  },
  {
    "description": "Stop this arm's motion; it holds where it stops.",
    "name": "stop",
    "operation": "stop",
    "parameters": {
      "additionalProperties": false,
      "properties": {},
      "type": "object"
    }
  },
  {
    "description": "Return the station state after an optional wait without commanding motion. Camera images can be obtained separately.",
    "name": "observe",
    "operation": "observe",
    "parameters": {
      "additionalProperties": false,
      "properties": {
        "wait_seconds": {
          "default": 0.0,
          "maximum": 10.0,
          "minimum": 0.0,
          "type": "number"
        }
      },
      "type": "object"
    }
  }
]
""")

ROBOT_TOOL_CONTEXT = json.loads(r"""
{
  "frames": "Positions are in the station world frame. Tool inputs use meters, joint angles use radians, and joint speeds use radians per second.",
  "gripper": "Gripper opening is continuous from 0 (closed) to 1 (fully open); 0.5 is half open. Contact is estimated from a stopped-short gripper and does not prove a secure grasp. A step speed overrides its movement speed for both movement and gripper.",
  "limitations": "Model guards can check self-collision and a horizontal surface, but do not detect workspace objects or other arms and do not establish physical clearance. The solver's projected pose is not necessarily the globally closest pose. Camera frames are not hardware-synchronized; retrieval timestamps are not exposure timestamps. Camera extrinsics, table plane and tool-to-fingertip offsets may be unverified.",
  "orientation": "The approach is down (world -Z), forward (world +X), or a world direction vector. Jaw yaw is in degrees about minus approach; zero is projected world +X, or world +Y when the absolute normalized approach X exceeds 0.95. Quaternions are [x, y, z, w]. Pose moves require orientation; preview can choose it.",
  "outcomes": "A completed move may still have pose_limited or path_limited: the planned motion finished but did not reach the requested target. A stopped_short outcome means the arm settled outside tolerance and stopped closing on its target; achieved joint errors describe the remaining error. A refusal means the command was not accepted. Joint targets beyond limits are clamped."
}
""")
