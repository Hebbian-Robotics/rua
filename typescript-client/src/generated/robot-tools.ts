// Generated from schemas/robot-tools.json and robot-context.json; do not edit.
export const robotToolSpecifications = [
  {
    description:
      "Run steps in order, waiting for each to finish. A move passes through waypoints and settles at its goal. Waypoints must pass through without stopping; make a required stop a goal in a separate step. A waypoint gripper_opening starts at that crossing. A refusal, incomplete move, or limited goal ends the sequence; earlier steps may already have run.",
    name: "action_sequence",
    operation: "start_action_sequence",
    parameters: {
      additionalProperties: false,
      properties: {
        steps: {
          items: {
            additionalProperties: false,
            anyOf: [
              {
                properties: {
                  goal: {
                    type: "object",
                  },
                },
                required: ["goal"],
              },
              {
                properties: {
                  gripper_opening: {
                    type: "number",
                  },
                },
                required: ["gripper_opening"],
              },
            ],
            description:
              "An arm action must name a goal or gripper opening. When both are present they start together.\nThe compiler validates the opening and motion limits with step and arm context.",
            properties: {
              at_seconds: {
                anyOf: [
                  {
                    type: "number",
                  },
                  {
                    type: "null",
                  },
                ],
                description:
                  "Earliest reference arrival in seconds from the start of this move.\nFor one planned arm per step. Time every waypoint and the goal in increasing order.",
                minimum: 0.0,
              },
              goal: {
                oneOf: [
                  {
                    additionalProperties: false,
                    properties: {
                      joints_degrees: {
                        items: {
                          type: "number",
                        },
                        type: "array",
                      },
                      max_joint_speed_degrees_per_second: {
                        anyOf: [
                          {
                            type: "number",
                          },
                          {
                            type: "null",
                          },
                        ],
                        default: null,
                        description:
                          "Peak speed in degrees per second; clamped to the profile maximum.",
                      },
                    },
                    required: ["joints_degrees"],
                    title: "ActionSequenceJointsMovement",
                    type: "object",
                  },
                  {
                    additionalProperties: false,
                    properties: {
                      max_joint_speed_degrees_per_second: {
                        anyOf: [
                          {
                            type: "number",
                          },
                          {
                            type: "null",
                          },
                        ],
                        default: null,
                        description:
                          "Peak speed in degrees per second; clamped to the profile maximum.",
                      },
                      orientation: {
                        description: "A tool orientation as a client gives it.",
                        oneOf: [
                          {
                            additionalProperties: false,
                            description:
                              "The approach direction and the jaw yaw about it.",
                            properties: {
                              approach: {
                                anyOf: [
                                  {
                                    description: "A named approach direction.",
                                    oneOf: [
                                      {
                                        const: "down",
                                        description: "World -Z.",
                                        type: "string",
                                      },
                                      {
                                        const: "forward",
                                        description: "World +X.",
                                        type: "string",
                                      },
                                    ],
                                  },
                                  {
                                    items: {
                                      type: "number",
                                    },
                                    maxItems: 3,
                                    minItems: 3,
                                    title: "ApproachDirection",
                                    type: "array",
                                  },
                                ],
                                description:
                                  "The direction the jaws point along: a named one, or a nonzero world direction.",
                              },
                              jaw_yaw_degrees: {
                                description: "Jaw yaw in degrees.",
                                type: "number",
                              },
                            },
                            required: ["approach", "jaw_yaw_degrees"],
                            title: "GraspOrientation",
                            type: "object",
                          },
                          {
                            additionalProperties: false,
                            description: "A raw quaternion, `[x, y, z, w]`.",
                            properties: {
                              quaternion: {
                                items: {
                                  type: "number",
                                },
                                maxItems: 4,
                                minItems: 4,
                                type: "array",
                              },
                            },
                            required: ["quaternion"],
                            title: "QuaternionOrientation",
                            type: "object",
                          },
                        ],
                      },
                      position_mm: {
                        items: {
                          type: "number",
                        },
                        maxItems: 3,
                        minItems: 3,
                        type: "array",
                      },
                    },
                    required: ["position_mm", "orientation"],
                    title: "ActionSequencePoseMovement",
                    type: "object",
                  },
                ],
              },
              gripper_opening: {
                maximum: 1.0,
                minimum: 0.0,
                type: "number",
              },
              max_joint_acceleration_degrees_per_second_squared: {
                anyOf: [
                  {
                    type: "number",
                  },
                  {
                    type: "null",
                  },
                ],
                description:
                  "Joint acceleration cap in degrees per second squared; defaults to the arm profile.",
                exclusiveMinimum: 0.0,
              },
              max_joint_jerk_degrees_per_second_cubed: {
                anyOf: [
                  {
                    type: "number",
                  },
                  {
                    type: "null",
                  },
                ],
                description:
                  "Joint jerk cap in degrees per second cubed; defaults to the arm profile.",
                exclusiveMinimum: 0.0,
              },
              max_joint_speed_degrees_per_second: {
                anyOf: [
                  {
                    type: "number",
                  },
                  {
                    type: "null",
                  },
                ],
                default: null,
                description:
                  "Peak speed in degrees per second; clamped to the profile maximum.",
              },
              waypoints: {
                default: [],
                description:
                  "Pass through these intermediate configurations before settling at the goal.",
                items: {
                  additionalProperties: false,
                  description:
                    "An intermediate configuration along one continuous move.",
                  properties: {
                    at: {
                      oneOf: [
                        {
                          additionalProperties: false,
                          properties: {
                            joints_degrees: {
                              items: {
                                type: "number",
                              },
                              type: "array",
                            },
                          },
                          required: ["joints_degrees"],
                          type: "object",
                        },
                        {
                          additionalProperties: false,
                          properties: {
                            orientation: {
                              description:
                                "A tool orientation as a client gives it.",
                              oneOf: [
                                {
                                  additionalProperties: false,
                                  description:
                                    "The approach direction and the jaw yaw about it.",
                                  properties: {
                                    approach: {
                                      anyOf: [
                                        {
                                          description:
                                            "A named approach direction.",
                                          oneOf: [
                                            {
                                              const: "down",
                                              description: "World -Z.",
                                              type: "string",
                                            },
                                            {
                                              const: "forward",
                                              description: "World +X.",
                                              type: "string",
                                            },
                                          ],
                                        },
                                        {
                                          items: {
                                            type: "number",
                                          },
                                          maxItems: 3,
                                          minItems: 3,
                                          title: "ApproachDirection",
                                          type: "array",
                                        },
                                      ],
                                      description:
                                        "The direction the jaws point along: a named one, or a nonzero world direction.",
                                    },
                                    jaw_yaw_degrees: {
                                      description: "Jaw yaw in degrees.",
                                      type: "number",
                                    },
                                  },
                                  required: ["approach", "jaw_yaw_degrees"],
                                  title: "GraspOrientation",
                                  type: "object",
                                },
                                {
                                  additionalProperties: false,
                                  description:
                                    "A raw quaternion, `[x, y, z, w]`.",
                                  properties: {
                                    quaternion: {
                                      items: {
                                        type: "number",
                                      },
                                      maxItems: 4,
                                      minItems: 4,
                                      type: "array",
                                    },
                                  },
                                  required: ["quaternion"],
                                  title: "QuaternionOrientation",
                                  type: "object",
                                },
                              ],
                            },
                            position_mm: {
                              items: {
                                type: "number",
                              },
                              maxItems: 3,
                              minItems: 3,
                              type: "array",
                            },
                          },
                          required: ["position_mm", "orientation"],
                          type: "object",
                        },
                      ],
                    },
                    at_seconds: {
                      anyOf: [
                        {
                          type: "number",
                        },
                        {
                          type: "null",
                        },
                      ],
                      description:
                        "Earliest reference crossing in seconds from the start of this move.",
                      minimum: 0.0,
                    },
                    gripper_opening: {
                      anyOf: [
                        {
                          type: "number",
                        },
                        {
                          type: "null",
                        },
                      ],
                      description:
                        "Start moving the gripper when passing this waypoint.",
                      maximum: 1.0,
                      minimum: 0.0,
                      title: "GripperOpening",
                    },
                  },
                  required: ["at"],
                  type: "object",
                },
                type: "array",
              },
            },
            type: "object",
          },
          minItems: 1,
          type: "array",
        },
      },
      required: ["steps"],
      type: "object",
    },
  },
  {
    description:
      "Resolve pose feasibility without moving: returns projected poses, errors, available jaw yaws or refusal in input order. Omitting orientation lets the solver choose one. Preview does not check clearance from workspace objects.",
    name: "preview_pose",
    operation: "preview",
    parameters: {
      $defs: {
        Approach: {
          anyOf: [
            {
              $ref: "#/$defs/NamedApproach",
            },
            {
              items: {
                type: "number",
              },
              maxItems: 3,
              minItems: 3,
              title: "ApproachDirection",
              type: "array",
            },
          ],
          description:
            "The direction the jaws point along: a named one, or a nonzero world direction.",
        },
        ModelPreviewCandidate: {
          additionalProperties: false,
          properties: {
            pose: {
              $ref: "#/$defs/PreviewPose",
            },
          },
          required: ["pose"],
          type: "object",
        },
        NamedApproach: {
          description: "A named approach direction.",
          oneOf: [
            {
              const: "down",
              description: "World -Z.",
              type: "string",
            },
            {
              const: "forward",
              description: "World +X.",
              type: "string",
            },
          ],
        },
        PoseOrientation: {
          description: "A tool orientation as a client gives it.",
          oneOf: [
            {
              additionalProperties: false,
              description: "The approach direction and the jaw yaw about it.",
              properties: {
                approach: {
                  $ref: "#/$defs/Approach",
                },
                jaw_yaw_degrees: {
                  description: "Jaw yaw in degrees.",
                  type: "number",
                },
              },
              required: ["approach", "jaw_yaw_degrees"],
              title: "GraspOrientation",
              type: "object",
            },
            {
              additionalProperties: false,
              description: "A raw quaternion, `[x, y, z, w]`.",
              properties: {
                quaternion: {
                  items: {
                    type: "number",
                  },
                  maxItems: 4,
                  minItems: 4,
                  type: "array",
                },
              },
              required: ["quaternion"],
              title: "QuaternionOrientation",
              type: "object",
            },
          ],
        },
        PreviewPose: {
          additionalProperties: false,
          description:
            "Where the arm's controller would take the tool toward a pose, without moving.",
          properties: {
            max_joint_acceleration_degrees_per_second_squared: {
              anyOf: [
                {
                  type: "number",
                },
                {
                  type: "null",
                },
              ],
              description:
                "Joint acceleration cap in degrees per second squared; defaults to the arm profile.",
              exclusiveMinimum: 0.0,
            },
            max_joint_jerk_degrees_per_second_cubed: {
              anyOf: [
                {
                  type: "number",
                },
                {
                  type: "null",
                },
              ],
              description:
                "Joint jerk cap in degrees per second cubed; defaults to the arm profile.",
              exclusiveMinimum: 0.0,
            },
            max_joint_speed_degrees_per_second: {
              anyOf: [
                {
                  type: "number",
                },
                {
                  type: "null",
                },
              ],
              description:
                "Peak speed in degrees per second; clamped to the profile maximum.",
              exclusiveMinimum: 0.0,
              title: "JointSpeed",
            },
            orientation: {
              anyOf: [
                {
                  $ref: "#/$defs/PoseOrientation",
                },
                {
                  type: "null",
                },
              ],
              default: null,
              description: "None leaves the orientation to the solver.",
            },
            position_mm: {
              items: {
                type: "number",
              },
              maxItems: 3,
              minItems: 3,
              type: "array",
            },
          },
          required: ["position_mm"],
          type: "object",
        },
      },
      additionalProperties: false,
      properties: {
        poses: {
          items: {
            $ref: "#/$defs/ModelPreviewCandidate",
          },
          minItems: 1,
          type: "array",
        },
      },
      required: ["poses"],
      type: "object",
    },
  },
  {
    description:
      "Move named arms together. Pose goals use a world position and orientation; joint goals use degrees. If one arm fails during motion, the others stop. Grouped moves check modeled inter-arm clearance, not workspace objects.",
    name: "move_arms",
    operation: "move_arms",
    parameters: {
      $defs: {
        Approach: {
          anyOf: [
            {
              $ref: "#/$defs/NamedApproach",
            },
            {
              items: {
                type: "number",
              },
              maxItems: 3,
              minItems: 3,
              title: "ApproachDirection",
              type: "array",
            },
          ],
          description:
            "The direction the jaws point along: a named one, or a nonzero world direction.",
        },
        NamedApproach: {
          description: "A named approach direction.",
          oneOf: [
            {
              const: "down",
              description: "World -Z.",
              type: "string",
            },
            {
              const: "forward",
              description: "World +X.",
              type: "string",
            },
          ],
        },
        PoseOrientation: {
          description: "A tool orientation as a client gives it.",
          oneOf: [
            {
              additionalProperties: false,
              description: "The approach direction and the jaw yaw about it.",
              properties: {
                approach: {
                  $ref: "#/$defs/Approach",
                },
                jaw_yaw_degrees: {
                  description: "Jaw yaw in degrees.",
                  type: "number",
                },
              },
              required: ["approach", "jaw_yaw_degrees"],
              title: "GraspOrientation",
              type: "object",
            },
            {
              additionalProperties: false,
              description: "A raw quaternion, `[x, y, z, w]`.",
              properties: {
                quaternion: {
                  items: {
                    type: "number",
                  },
                  maxItems: 4,
                  minItems: 4,
                  type: "array",
                },
              },
              required: ["quaternion"],
              title: "QuaternionOrientation",
              type: "object",
            },
          ],
        },
        Waypoint: {
          description:
            "One corner of a move: joints, or a tool pose solved for joints.",
          oneOf: [
            {
              properties: {
                joints_degrees: {
                  items: {
                    type: "number",
                  },
                  type: "array",
                },
              },
              required: ["joints_degrees"],
              title: "JointsWaypoint",
              type: "object",
            },
            {
              properties: {
                orientation: {
                  $ref: "#/$defs/PoseOrientation",
                },
                position_mm: {
                  items: {
                    type: "number",
                  },
                  maxItems: 3,
                  minItems: 3,
                  type: "array",
                },
              },
              required: ["position_mm", "orientation"],
              title: "PoseWaypoint",
              type: "object",
            },
          ],
        },
      },
      additionalProperties: false,
      description:
        "Move several arms together: every target is planned before any arm moves,\none refusal refuses all, and when one arm ends without completing the others\nstop and hold. Grouped moves check modeled clearance among participating arms.",
      properties: {
        goals: {
          additionalProperties: {
            $ref: "#/$defs/Waypoint",
          },
          minProperties: 1,
          type: "object",
        },
        max_joint_speed_degrees_per_second: {
          anyOf: [
            {
              type: "number",
            },
            {
              type: "null",
            },
          ],
          default: null,
          description:
            "Peak speed in degrees per second; clamped to the profile maximum.",
          exclusiveMinimum: 0.0,
          title: "JointSpeed",
        },
      },
      required: ["goals"],
      type: "object",
    },
  },
  {
    description:
      "Set a named arm's gripper opening from 0 (closed) to 1 (fully open). Contact may stop closing but does not confirm a grasp.",
    name: "set_arm_gripper",
    operation: "set_gripper",
    parameters: {
      additionalProperties: false,
      description:
        "Open or close the gripper, 0 closed to 1 open, whether the arm is holding\nor moving. It completes on arrival or on contact.",
      properties: {
        arm: {
          type: "string",
        },
        max_joint_speed_degrees_per_second: {
          anyOf: [
            {
              type: "number",
            },
            {
              type: "null",
            },
          ],
          default: null,
          description:
            "Peak speed in degrees per second; clamped to the profile maximum.",
          exclusiveMinimum: 0.0,
          title: "JointSpeed",
        },
        opening: {
          maximum: 1.0,
          minimum: 0.0,
          title: "GripperOpening",
          type: "number",
        },
      },
      required: ["opening", "arm"],
      type: "object",
    },
  },
  {
    description:
      "Return this arm to the joint pose and gripper opening recorded when it was enabled.",
    name: "rehome",
    operation: "rehome",
    parameters: {
      additionalProperties: false,
      properties: {},
      type: "object",
    },
  },
  {
    description:
      "Stop this arm's motion and hold position; this does not disable the motors or end the agent run.",
    name: "stop",
    operation: "stop",
    parameters: {
      additionalProperties: false,
      properties: {},
      type: "object",
    },
  },
  {
    description:
      "Observe without moving. source=current (default) gets fresh state and camera images; source=last_sequence retrieves sampled images from the latest action sequence to investigate what happened during motion. History is only returned when requested and may be unavailable or incomplete. Optionally wait up to 10 seconds before observing.",
    name: "observe",
    operation: "observe",
    parameters: {
      $defs: {
        ObservationSource: {
          description:
            "Select fresh observations or retained evidence from the latest action sequence.",
          enum: ["current", "last_sequence"],
          type: "string",
        },
      },
      additionalProperties: false,
      properties: {
        source: {
          $ref: "#/$defs/ObservationSource",
          default: "current",
        },
        wait_seconds: {
          default: 0.0,
          maximum: 10.0,
          minimum: 0.0,
          type: "number",
        },
      },
      type: "object",
    },
  },
];
export const robotToolContext = {
  frames:
    "Positions are in the station world frame. Tool inputs and outputs use millimeters, degrees, and degrees per second.",
  gripper:
    "Gripper opening is continuous from 0 (closed) to 1 (fully open); 0.5 is half open. Contact is estimated from a stopped-short gripper and does not prove a secure grasp. The step speed applies to the arm and gripper and overrides the speed in the goal.",
  limitations:
    "Model guards can check self-collision and a horizontal surface. Grouped moves also check modeled clearance among participating arms. These checks do not detect workspace objects or establish physical clearance. The solver's projected pose is not necessarily the globally closest pose. Camera frames are not hardware-synchronized; retrieval timestamps are not exposure timestamps. Camera extrinsics, table plane and tool-to-fingertip offsets may be unverified.",
  motion:
    "A move passes through optional waypoints in order and settles at its goal. A waypoint gripper_opening starts at that waypoint. For one planned arm per step, optional at_seconds sets an earliest reference arrival from the start of that step. Time every waypoint and the goal in strictly increasing order. Motion limits can make each section take longer, and the next step starts after settling. A pose goal is solved to joints; the arm follows a joint-space path, not necessarily a straight Cartesian line. Use separate approach, descend and lift steps when clearance requires them. Tool position is the configured tool-frame origin, not the lowest point of the fingers or camera.",
  orientation:
    "Approach points from palm toward fingertips: down (world -Z), forward (world +X), or a world vector. Jaw yaw describes the closing axis about minus approach; for down, 0 degrees closes along world X and 90 along world Y. Zero uses projected world +X, switching to +Y when the absolute normalized approach X exceeds 0.95. Quaternions are [x, y, z, w]. Pose moves require orientation; preview can choose it.",
  outcomes:
    "A completed move with goal_status limited finished at an adjusted target; action sequences stop on pose or path limits. A stopped_short outcome means the arm settled outside tolerance and stopped closing on its target; achieved joint errors describe the remaining error. A refusal means the command was not accepted. Joint targets beyond limits are refused.",
  surface_guard:
    "The configured surface height is a nominal or measured table plane. Leeway permits modeled geometry below that plane; it does not lower the table. The guard checks the lowest moving geometry, not just the tool frame, and permits recovery upward from below its limit.",
};
export const robotToolInputUnits = [
  {
    model_field: "position_mm",
    scale_to_station: 0.001,
    station_field: "position",
  },
  {
    model_field: "jaw_yaw_degrees",
    scale_to_station: 0.017453292519943295,
    station_field: "jaw_yaw_radians",
  },
  {
    model_field: "joints_degrees",
    scale_to_station: 0.017453292519943295,
    station_field: "joints",
  },
  {
    model_field: "max_joint_speed_degrees_per_second",
    scale_to_station: 0.017453292519943295,
    station_field: "max_joint_speed",
  },
  {
    model_field: "max_joint_acceleration_degrees_per_second_squared",
    scale_to_station: 0.017453292519943295,
    station_field: "max_joint_acceleration",
  },
  {
    model_field: "max_joint_jerk_degrees_per_second_cubed",
    scale_to_station: 0.017453292519943295,
    station_field: "max_joint_jerk",
  },
] as const;
export const modelOutputUnits = [
  ["position", "position_mm", 1000.0],
  ["goal_position", "goal_position_mm", 1000.0],
  ["destination_position", "destination_position_mm", 1000.0],
  ["tool_position", "tool_position_mm", 1000.0],
  ["surface_height_meters", "surface_height_mm", 1000.0],
  ["leeway_meters", "leeway_mm", 1000.0],
  ["model_points", "model_points_mm", 1000.0],
  ["position_error_meters", "position_error_mm", 1000.0],
  ["position_shortfall_meters", "position_shortfall_mm", 1000.0],
  ["jaw_yaw_radians", "jaw_yaw_degrees", 57.29577951308232],
  ["radians_from_down", "degrees_from_down", 57.29577951308232],
  ["approach_error_radians", "approach_error_degrees", 57.29577951308232],
  ["from_radians", "from_degrees", 57.29577951308232],
  ["to_radians", "to_degrees", 57.29577951308232],
  ["joints", "joints_degrees", 57.29577951308232],
  ["measured_joints", "measured_joints_degrees", 57.29577951308232],
  [
    "measured_joint_velocities",
    "measured_joint_velocities_degrees_per_second",
    57.29577951308232,
  ],
  ["commanded_joints", "commanded_joints_degrees", 57.29577951308232],
  ["rest_pose", "rest_pose_degrees", 57.29577951308232],
  ["destination_joints", "destination_joints_degrees", 57.29577951308232],
  ["goal_joints", "goal_joints_degrees", 57.29577951308232],
  ["joint_error_radians", "joint_error_degrees", 57.29577951308232],
  ["joint_errors_radians", "joint_errors_degrees", 57.29577951308232],
  ["orientation_error_radians", "orientation_error_degrees", 57.29577951308232],
  [
    "orientation_shortfall_radians",
    "orientation_shortfall_degrees",
    57.29577951308232,
  ],
  ["measured_angle_radians", "measured_angle_degrees", 57.29577951308232],
  ["target_angle_radians", "target_angle_degrees", 57.29577951308232],
  ["joint_speed", "joint_speed_degrees_per_second", 57.29577951308232],
  ["max_joint_speed", "max_joint_speed_degrees_per_second", 57.29577951308232],
  [
    "max_joint_acceleration",
    "max_joint_acceleration_degrees_per_second_squared",
    57.29577951308232,
  ],
  [
    "max_joint_jerk",
    "max_joint_jerk_degrees_per_second_cubed",
    57.29577951308232,
  ],
] as const;
export const modelOutcomes = {
  commands: {
    completed: {
      class: "completed",
      limited_target: "limited",
      target: "reached",
    },
    failed: {
      class: "incomplete",
      limited_target: "partial",
      target: "partial",
    },
    faulted: {
      class: "faulted",
      limited_target: "partial",
      target: "partial",
    },
    interrupted: {
      class: "interrupted",
      limited_target: "partial",
      target: "partial",
    },
    pending: {
      class: "incomplete",
      limited_target: "partial",
      target: "partial",
    },
    stopped: {
      class: "incomplete",
      limited_target: "partial",
      target: "partial",
    },
    stopped_short: {
      class: "stopped_short",
      limited_target: "stopped_short",
      target: "stopped_short",
    },
    superseded: {
      class: "incomplete",
      limited_target: "partial",
      target: "partial",
    },
    timed_out: {
      class: "incomplete",
      limited_target: "partial",
      target: "partial",
    },
    tripped: {
      class: "incomplete",
      limited_target: "partial",
      target: "partial",
    },
  },
  status_priority: ["reached", "limited", "partial", "stopped_short"],
} as const;
export const modelResponseFields = {
  arm: [
    "arm",
    "mode",
    "fault",
    "active_trip",
    "interlocks",
    "recovery_issue",
    "revision",
    "sampled_unix_nano",
    "measured_joints",
    "tool",
    "gripper_opening",
  ],
  diagnostic: [
    "id",
    "command_id",
    "group_id",
    "stream_command_id",
    "sequence_id",
    "action_sequence_id",
    "checkpoints",
    "trace_context",
    "model_points",
    "accepted_unix_nano",
    "finished_unix_nano",
  ],
  earlier_step: [
    "label",
    "completion",
    "phase",
    "goal_status",
    "pose_limited",
    "path_limited",
  ],
  state: ["robot", "joint_names", "max_joint_speed", "surface_guard"],
  station_context: ["description", "lease"],
} as const;
