"""Generated from schemas/robot-tools.json; do not edit."""

import json

ROBOT_TOOL_CATALOG = json.loads(r"""
[
  {
    "description": "Run steps in order. Each step admits its commands together and waits for them to finish. The sequence stops on a refusal, incomplete target, or limited target.",
    "name": "action_sequence",
    "operation": "start_action_sequence",
    "parameters": {
      "additionalProperties": false,
      "properties": {
        "steps": {
          "items": {
            "additionalProperties": false,
            "anyOf": [
              {
                "properties": {
                  "movement": {
                    "type": "object"
                  }
                },
                "required": [
                  "movement"
                ]
              },
              {
                "properties": {
                  "gripper_opening": {
                    "type": "number"
                  }
                },
                "required": [
                  "gripper_opening"
                ]
              }
            ],
            "description": "An arm action must name a movement or gripper target. When both are present they start together.",
            "properties": {
              "gripper_opening": {
                "maximum": 1.0,
                "minimum": 0.0,
                "type": "number"
              },
              "max_joint_speed_degrees_per_second": {
                "anyOf": [
                  {
                    "type": "number"
                  },
                  {
                    "type": "null"
                  }
                ],
                "default": null,
                "description": "Peak speed in degrees per second; clamped to the profile maximum."
              },
              "movement": {
                "oneOf": [
                  {
                    "additionalProperties": false,
                    "properties": {
                      "joints_degrees": {
                        "items": {
                          "type": "number"
                        },
                        "type": "array"
                      },
                      "max_joint_speed_degrees_per_second": {
                        "anyOf": [
                          {
                            "type": "number"
                          },
                          {
                            "type": "null"
                          }
                        ],
                        "default": null,
                        "description": "Peak speed in degrees per second; clamped to the profile maximum."
                      }
                    },
                    "required": [
                      "joints_degrees"
                    ],
                    "title": "ActionSequenceJointsMovement",
                    "type": "object"
                  },
                  {
                    "additionalProperties": false,
                    "properties": {
                      "max_joint_speed_degrees_per_second": {
                        "anyOf": [
                          {
                            "type": "number"
                          },
                          {
                            "type": "null"
                          }
                        ],
                        "default": null,
                        "description": "Peak speed in degrees per second; clamped to the profile maximum."
                      },
                      "orientation": {
                        "description": "A tool orientation as a client gives it.",
                        "oneOf": [
                          {
                            "description": "The approach direction and the jaw yaw about it, in degrees.",
                            "properties": {
                              "approach": {
                                "anyOf": [
                                  {
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
                              "jaw_yaw_degrees": {
                                "type": "number"
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
                      "position_mm": {
                        "items": {
                          "type": "number"
                        },
                        "maxItems": 3,
                        "minItems": 3,
                        "type": "array"
                      }
                    },
                    "required": [
                      "position_mm",
                      "orientation"
                    ],
                    "title": "ActionSequencePoseMovement",
                    "type": "object"
                  }
                ]
              }
            },
            "type": "object"
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
    "description": "Resolve pose feasibility without moving: returns projected poses, errors, available jaw yaws or refusal in input order. Execution also checks feasibility; preview is useful when the projection or orientation choice changes your next action. Omitting orientation lets the solver choose one.",
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
        "ModelPreviewCandidate": {
          "additionalProperties": false,
          "properties": {
            "pose": {
              "$ref": "#/$defs/PreviewPose"
            }
          },
          "required": [
            "pose"
          ],
          "type": "object"
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
            "position_mm": {
              "items": {
                "type": "number"
              },
              "maxItems": 3,
              "minItems": 3,
              "type": "array"
            }
          },
          "required": [
            "position_mm"
          ],
          "type": "object"
        }
      },
      "additionalProperties": false,
      "properties": {
        "poses": {
          "items": {
            "$ref": "#/$defs/ModelPreviewCandidate"
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
    "description": "Move named arms together. Pose targets use a world position and orientation; joint targets use degrees. Grouped moves check modeled inter-arm clearance. Workspace objects require separate constraints.",
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
                "joints_degrees": {
                  "items": {
                    "type": "number"
                  },
                  "type": "array"
                }
              },
              "required": [
                "joints_degrees"
              ],
              "title": "JointsWaypoint",
              "type": "object"
            },
            {
              "properties": {
                "orientation": {
                  "$ref": "#/$defs/PoseOrientation"
                },
                "position_mm": {
                  "items": {
                    "type": "number"
                  },
                  "maxItems": 3,
                  "minItems": 3,
                  "type": "array"
                }
              },
              "required": [
                "position_mm",
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
        "max_joint_speed_degrees_per_second": {
          "anyOf": [
            {
              "type": "number"
            },
            {
              "type": "null"
            }
          ],
          "default": null,
          "description": "Peak speed in degrees per second; clamped to the profile maximum.",
          "exclusiveMinimum": 0.0,
          "title": "JointSpeed"
        },
        "targets": {
          "additionalProperties": {
            "$ref": "#/$defs/Waypoint"
          },
          "minProperties": 1,
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
        "max_joint_speed_degrees_per_second": {
          "anyOf": [
            {
              "type": "number"
            },
            {
              "type": "null"
            }
          ],
          "default": null,
          "description": "Peak speed in degrees per second; clamped to the profile maximum.",
          "exclusiveMinimum": 0.0,
          "title": "JointSpeed"
        },
        "opening": {
          "maximum": 1.0,
          "minimum": 0.0,
          "title": "GripperOpening",
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
    "description": "Return this arm to the joint pose recorded when it was enabled. The gripper keeps its current target.",
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
    "description": "Observe without moving. source=current (default) gets fresh state and camera images; source=last_sequence retrieves sampled images from the latest action sequence to investigate what happened during motion. History is only returned when requested and may be unavailable or incomplete. Optionally wait up to 10 seconds before observing.",
    "name": "observe",
    "operation": "observe",
    "parameters": {
      "$defs": {
        "ObservationSource": {
          "description": "Select fresh observations or retained evidence from the latest action sequence.",
          "enum": [
            "current",
            "last_sequence"
          ],
          "type": "string"
        }
      },
      "additionalProperties": false,
      "properties": {
        "source": {
          "$ref": "#/$defs/ObservationSource",
          "default": "current"
        },
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
  "frames": "Positions are in the station world frame. Tool inputs and outputs use millimeters, degrees, and degrees per second.",
  "gripper": "Gripper opening is continuous from 0 (closed) to 1 (fully open); 0.5 is half open. Contact is estimated from a stopped-short gripper and does not prove a secure grasp. A step speed overrides its movement speed for both movement and gripper.",
  "limitations": "Model guards can check self-collision and a horizontal surface, but do not detect workspace objects or other arms and do not establish physical clearance. The solver's projected pose is not necessarily the globally closest pose. Camera frames are not hardware-synchronized; retrieval timestamps are not exposure timestamps. Camera extrinsics, table plane and tool-to-fingertip offsets may be unverified.",
  "motion": "A pose target is solved to joints; the arm follows a joint-space path, not necessarily a straight Cartesian line. Use separate approach, descend and lift steps when clearance requires them. Tool position is the configured tool-frame origin, not the lowest point of the fingers or camera.",
  "orientation": "Approach points from palm toward fingertips: down (world -Z), forward (world +X), or a world vector. Jaw yaw describes the closing axis about minus approach; for down, 0 degrees closes along world X and 90 along world Y. Zero uses projected world +X, switching to +Y when the absolute normalized approach X exceeds 0.95. Quaternions are [x, y, z, w]. Pose moves require orientation; preview can choose it.",
  "outcomes": "A completed move with target_status limited finished at an adjusted target; action sequences stop on pose or path limits. A stopped_short outcome means the arm settled outside tolerance and stopped closing on its target; achieved joint errors describe the remaining error. A refusal means the command was not accepted. Joint targets beyond limits are refused.",
  "surface_guard": "The configured surface height is a nominal or measured table plane. Leeway permits modeled geometry below that plane; it does not lower the table. The guard checks the lowest moving geometry, not just the tool frame, and permits recovery upward from below its limit."
}
""")

ROBOT_TOOL_INPUT_UNITS = json.loads(r"""
[
  {
    "model_field": "position_mm",
    "scale_to_station": 0.001,
    "station_field": "position"
  },
  {
    "model_field": "joints_degrees",
    "scale_to_station": 0.017453292519943295,
    "station_field": "joints"
  },
  {
    "model_field": "max_joint_speed_degrees_per_second",
    "scale_to_station": 0.017453292519943295,
    "station_field": "max_joint_speed"
  }
]
""")
