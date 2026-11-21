import * as z from "zod";
export const apiSchemas = z.object({
  Achieved: z
    .object({
      joint_error_radians: z.union([z.number(), z.null()]).optional(),
      joint_errors_radians: z
        .union([
          z
            .array(z.number())
            .describe("Each joint's absolute distance from the target joints."),
          z
            .null()
            .describe("Each joint's absolute distance from the target joints."),
        ])
        .describe("Each joint's absolute distance from the target joints.")
        .optional(),
      joints: z.array(z.number()),
      orientation: z
        .union([
          z
            .object({
              approach: z.array(z.number()).min(3).max(3),
              degrees_from_down: z.number(),
              jaw_yaw_degrees: z.number(),
              jaw_yaw_reference: z
                .enum(["world_x", "world_y"])
                .describe("The world axis jaw yaw zero is projected from."),
              target: z
                .union([
                  z
                    .object({
                      approach_error_degrees: z.number(),
                      jaw_yaw_degrees: z.number(),
                      jaw_yaw_reference: z
                        .enum(["world_x", "world_y"])
                        .describe(
                          "The world axis jaw yaw zero is projected from.",
                        ),
                      orientation_error_degrees: z.number(),
                    })
                    .describe(
                      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                    ),
                  z.null(),
                ])
                .optional(),
            })
            .describe(
              "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
            ),
          z.null(),
        ])
        .describe(
          "The tool orientation in grasp terms, against the target's when there is one.",
        )
        .optional(),
      orientation_error_radians: z.union([z.number(), z.null()]).optional(),
      position: z
        .union([
          z
            .array(z.number())
            .min(3)
            .max(3)
            .describe(
              "The tool pose at `joints`; absent when the controller has no tool link.",
            ),
          z
            .null()
            .describe(
              "The tool pose at `joints`; absent when the controller has no tool link.",
            ),
        ])
        .describe(
          "The tool pose at `joints`; absent when the controller has no tool link.",
        )
        .optional(),
      position_error_meters: z.union([z.number(), z.null()]).optional(),
      quaternion: z
        .union([z.array(z.number()).min(4).max(4), z.null()])
        .optional(),
      sampled_at: z.number(),
    })
    .describe("Where the arm was, measured, when a command ended."),
  ActionSequenceArmAction: z
    .object({
      gripper_opening: z.union([z.number(), z.null()]).default(null),
      max_joint_speed: z
        .union([
          z
            .number()
            .describe(
              "Speed for both resources in rad/s; overrides the movement speed when provided.",
            ),
          z
            .null()
            .describe(
              "Speed for both resources in rad/s; overrides the movement speed when provided.",
            ),
        ])
        .describe(
          "Speed for both resources in rad/s; overrides the movement speed when provided.",
        )
        .default(null),
      movement: z
        .union([
          z
            .union([
              z
                .object({
                  joints: z.array(z.number()),
                  kind: z.literal("joints"),
                  max_joint_speed: z
                    .union([z.number(), z.null()])
                    .default(null),
                })
                .strict(),
              z
                .object({
                  kind: z.literal("pose"),
                  max_joint_speed: z
                    .union([z.number(), z.null()])
                    .default(null),
                  orientation: z
                    .union([
                      z
                        .object({
                          approach: z
                            .union([
                              z
                                .union([
                                  z.literal("down").describe("World -Z."),
                                  z.literal("forward").describe("World +X."),
                                ])
                                .describe("A named approach direction."),
                              z.array(z.number()).min(3).max(3),
                            ])
                            .describe(
                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                            ),
                          jaw_yaw_degrees: z.number(),
                          kind: z.literal("grasp"),
                        })
                        .describe(
                          "The approach direction and the jaw yaw about it, in degrees.",
                        ),
                      z
                        .object({
                          kind: z.literal("quaternion"),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("A raw quaternion, `[x, y, z, w]`."),
                    ])
                    .describe("A tool orientation as a client gives it."),
                  position: z.array(z.number()).min(3).max(3),
                })
                .strict(),
            ])
            .describe(
              "One movement in an action sequence. The arm settles at each target.",
            ),
          z.null(),
        ])
        .default(null),
    })
    .strict()
    .describe("A movement and gripper target in the same step start together."),
  ActionSequenceMovement: z
    .union([
      z
        .object({
          joints: z.array(z.number()),
          kind: z.literal("joints"),
          max_joint_speed: z.union([z.number(), z.null()]).default(null),
        })
        .strict(),
      z
        .object({
          kind: z.literal("pose"),
          max_joint_speed: z.union([z.number(), z.null()]).default(null),
          orientation: z
            .union([
              z
                .object({
                  approach: z
                    .union([
                      z
                        .union([
                          z.literal("down").describe("World -Z."),
                          z.literal("forward").describe("World +X."),
                        ])
                        .describe("A named approach direction."),
                      z.array(z.number()).min(3).max(3),
                    ])
                    .describe(
                      "The direction the jaws point along: a named one, or a nonzero world direction.",
                    ),
                  jaw_yaw_degrees: z.number(),
                  kind: z.literal("grasp"),
                })
                .describe(
                  "The approach direction and the jaw yaw about it, in degrees.",
                ),
              z
                .object({
                  kind: z.literal("quaternion"),
                  quaternion: z.array(z.number()).min(4).max(4),
                })
                .describe("A raw quaternion, `[x, y, z, w]`."),
            ])
            .describe("A tool orientation as a client gives it."),
          position: z.array(z.number()).min(3).max(3),
        })
        .strict(),
    ])
    .describe(
      "One movement in an action sequence. The arm settles at each target.",
    ),
  ActionSequencePhase: z
    .union([
      z.object({ state: z.literal("running"), step: z.number().int().gte(0) }),
      z.object({ state: z.literal("completed") }),
      z.object({
        failure: z
          .union([
            z.object({
              arm: z.string(),
              detail: z.string(),
              reason: z
                .enum([
                  "arm_busy",
                  "not_enabled",
                  "stale_revision",
                  "interlocked",
                  "faulted",
                  "invalid_target",
                  "stream_ended",
                  "unreachable",
                  "lease_held_elsewhere",
                  "malformed",
                ])
                .describe("Why a command was not accepted."),
              stage: z.literal("preparation"),
              step: z.number().int().gte(0),
            }),
            z.object({
              detail: z.string(),
              reason: z
                .enum([
                  "arm_busy",
                  "not_enabled",
                  "stale_revision",
                  "interlocked",
                  "faulted",
                  "invalid_target",
                  "stream_ended",
                  "unreachable",
                  "lease_held_elsewhere",
                  "malformed",
                ])
                .describe("Why a command was not accepted."),
              stage: z.literal("admission"),
              step: z.number().int().gte(0),
            }),
            z.object({
              detail: z.string(),
              stage: z.literal("admission_unknown"),
              step: z.number().int().gte(0),
            }),
            z.object({
              detail: z.string(),
              stage: z.literal("execution"),
              step: z.number().int().gte(0),
            }),
            z.object({
              detail: z.string(),
              stage: z.literal("cancellation"),
              step: z.number().int().gte(0),
            }),
            z.object({
              detail: z.string(),
              stage: z.literal("authority"),
              step: z.number().int().gte(0),
            }),
            z.object({
              detail: z.string(),
              stage: z.literal("runtime"),
              step: z.number().int().gte(0),
            }),
          ])
          .describe(
            "The boundary where an execution stopped; uncertain admission may already have acted.",
          ),
        state: z.literal("stopped"),
      }),
    ])
    .describe(
      "Progress of an action sequence, including the records of each finished step.",
    ),
  ActionSequenceRecord: z
    .object({
      arms: z.array(z.string()),
      compilation_id: z.string(),
      continuation: z.union([
        z
          .object({
            next_step: z.number().int().gte(0),
            prior_execution_id: z.string(),
          })
          .strict()
          .describe(
            "Explicitly select the unattempted suffix of a finished execution.",
          ),
        z.null(),
      ]),
      id: z.string(),
      phase: z
        .union([
          z.object({
            state: z.literal("running"),
            step: z.number().int().gte(0),
          }),
          z.object({ state: z.literal("completed") }),
          z.object({
            failure: z
              .union([
                z.object({
                  arm: z.string(),
                  detail: z.string(),
                  reason: z
                    .enum([
                      "arm_busy",
                      "not_enabled",
                      "stale_revision",
                      "interlocked",
                      "faulted",
                      "invalid_target",
                      "stream_ended",
                      "unreachable",
                      "lease_held_elsewhere",
                      "malformed",
                    ])
                    .describe("Why a command was not accepted."),
                  stage: z.literal("preparation"),
                  step: z.number().int().gte(0),
                }),
                z.object({
                  detail: z.string(),
                  reason: z
                    .enum([
                      "arm_busy",
                      "not_enabled",
                      "stale_revision",
                      "interlocked",
                      "faulted",
                      "invalid_target",
                      "stream_ended",
                      "unreachable",
                      "lease_held_elsewhere",
                      "malformed",
                    ])
                    .describe("Why a command was not accepted."),
                  stage: z.literal("admission"),
                  step: z.number().int().gte(0),
                }),
                z.object({
                  detail: z.string(),
                  stage: z.literal("admission_unknown"),
                  step: z.number().int().gte(0),
                }),
                z.object({
                  detail: z.string(),
                  stage: z.literal("execution"),
                  step: z.number().int().gte(0),
                }),
                z.object({
                  detail: z.string(),
                  stage: z.literal("cancellation"),
                  step: z.number().int().gte(0),
                }),
                z.object({
                  detail: z.string(),
                  stage: z.literal("authority"),
                  step: z.number().int().gte(0),
                }),
                z.object({
                  detail: z.string(),
                  stage: z.literal("runtime"),
                  step: z.number().int().gte(0),
                }),
              ])
              .describe(
                "The boundary where an execution stopped; uncertain admission may already have acted.",
              ),
            state: z.literal("stopped"),
          }),
        ])
        .describe(
          "Progress of an action sequence, including the records of each finished step.",
        ),
      requested_steps: z
        .array(
          z
            .object({
              arms: z.record(
                z.string(),
                z
                  .object({
                    gripper_opening: z
                      .union([z.number(), z.null()])
                      .default(null),
                    max_joint_speed: z
                      .union([
                        z
                          .number()
                          .describe(
                            "Speed for both resources in rad/s; overrides the movement speed when provided.",
                          ),
                        z
                          .null()
                          .describe(
                            "Speed for both resources in rad/s; overrides the movement speed when provided.",
                          ),
                      ])
                      .describe(
                        "Speed for both resources in rad/s; overrides the movement speed when provided.",
                      )
                      .default(null),
                    movement: z
                      .union([
                        z
                          .union([
                            z
                              .object({
                                joints: z.array(z.number()),
                                kind: z.literal("joints"),
                                max_joint_speed: z
                                  .union([z.number(), z.null()])
                                  .default(null),
                              })
                              .strict(),
                            z
                              .object({
                                kind: z.literal("pose"),
                                max_joint_speed: z
                                  .union([z.number(), z.null()])
                                  .default(null),
                                orientation: z
                                  .union([
                                    z
                                      .object({
                                        approach: z
                                          .union([
                                            z
                                              .union([
                                                z
                                                  .literal("down")
                                                  .describe("World -Z."),
                                                z
                                                  .literal("forward")
                                                  .describe("World +X."),
                                              ])
                                              .describe(
                                                "A named approach direction.",
                                              ),
                                            z.array(z.number()).min(3).max(3),
                                          ])
                                          .describe(
                                            "The direction the jaws point along: a named one, or a nonzero world direction.",
                                          ),
                                        jaw_yaw_degrees: z.number(),
                                        kind: z.literal("grasp"),
                                      })
                                      .describe(
                                        "The approach direction and the jaw yaw about it, in degrees.",
                                      ),
                                    z
                                      .object({
                                        kind: z.literal("quaternion"),
                                        quaternion: z
                                          .array(z.number())
                                          .min(4)
                                          .max(4),
                                      })
                                      .describe(
                                        "A raw quaternion, `[x, y, z, w]`.",
                                      ),
                                  ])
                                  .describe(
                                    "A tool orientation as a client gives it.",
                                  ),
                                position: z.array(z.number()).min(3).max(3),
                              })
                              .strict(),
                          ])
                          .describe(
                            "One movement in an action sequence. The arm settles at each target.",
                          ),
                        z.null(),
                      ])
                      .default(null),
                  })
                  .strict()
                  .describe(
                    "A movement and gripper target in the same step start together.",
                  ),
              ),
            })
            .strict()
            .describe(
              "One synchronized step; a single-arm action is a map with one entry.",
            ),
        )
        .describe("The submitted steps, including those not yet executed."),
      steps: z
        .array(
          z.array(
            z
              .object({
                arm: z.string(),
                record: z
                  .object({
                    accepted_at: z.number(),
                    achieved: z.union([
                      z
                        .object({
                          joint_error_radians: z.union([z.number(), z.null()]),
                          joint_errors_radians: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                            ])
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            ),
                          joints: z.array(z.number()),
                          orientation: z
                            .union([
                              z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  degrees_from_down: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  target: z.union([
                                    z
                                      .object({
                                        approach_error_degrees: z.number(),
                                        jaw_yaw_degrees: z.number(),
                                        jaw_yaw_reference: z
                                          .enum(["world_x", "world_y"])
                                          .describe(
                                            "The world axis jaw yaw zero is projected from.",
                                          ),
                                        orientation_error_degrees: z.number(),
                                      })
                                      .describe(
                                        "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                      ),
                                    z.null(),
                                  ]),
                                })
                                .describe(
                                  "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The tool orientation in grasp terms, against the target's when there is one.",
                            ),
                          orientation_error_radians: z.union([
                            z.number(),
                            z.null(),
                          ]),
                          position: z
                            .union([
                              z
                                .array(z.number())
                                .min(3)
                                .max(3)
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                            ])
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            ),
                          position_error_meters: z.union([
                            z.number(),
                            z.null(),
                          ]),
                          quaternion: z.union([
                            z.array(z.number()).min(4).max(4),
                            z.null(),
                          ]),
                          sampled_at: z.number(),
                        })
                        .describe(
                          "Where the arm was, measured, when a command ended.",
                        ),
                      z.null(),
                    ]),
                    checkpoints: z.array(
                      z
                        .object({
                          label: z.string(),
                          measured_joints: z.array(z.number()),
                          target_position: z.union([
                            z.array(z.number()).min(3).max(3),
                            z.null(),
                          ]),
                          target_quaternion: z.union([
                            z.array(z.number()).min(4).max(4),
                            z.null(),
                          ]),
                        })
                        .describe(
                          "Where the arm stood, as measured, when it settled at the end of a segment.",
                        ),
                    ),
                    command_id: z.string(),
                    completion: z.union([
                      z
                        .object({
                          detail: z.union([z.string(), z.null()]),
                          finished_at: z.number(),
                          gripper: z
                            .union([
                              z
                                .union([
                                  z
                                    .object({
                                      kind: z.literal("reached"),
                                      measured_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw arrived at the angle it was asked for.",
                                    ),
                                  z
                                    .object({
                                      kind: z.literal("stopped_short"),
                                      measured_angle_radians: z.number(),
                                      target_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                    ),
                                ])
                                .describe(
                                  "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "How a gripper command ended; null for every other command.",
                            ),
                          outcome: z
                            .enum([
                              "completed",
                              "stopped",
                              "tripped",
                              "timed_out",
                              "stopped_short",
                              "superseded",
                              "faulted",
                              "interrupted",
                              "failed",
                            ])
                            .describe("How a command ended."),
                        })
                        .describe(
                          "The outcome and timestamp are recorded together when a command ends.",
                        ),
                      z.null(),
                    ]),
                    goal_joints: z
                      .union([
                        z
                          .array(z.number())
                          .describe(
                            "The newest goal of a joint stream, after clamping to the joint limits.",
                          ),
                        z
                          .null()
                          .describe(
                            "The newest goal of a joint stream, after clamping to the joint limits.",
                          ),
                      ])
                      .describe(
                        "The newest goal of a joint stream, after clamping to the joint limits.",
                      ),
                    group_id: z
                      .union([
                        z
                          .string()
                          .describe(
                            "The grouped motion this command is one arm's part of, if it is.",
                          ),
                        z
                          .null()
                          .describe(
                            "The grouped motion this command is one arm's part of, if it is.",
                          ),
                      ])
                      .describe(
                        "The grouped motion this command is one arm's part of, if it is.",
                      ),
                    joint_speed: z
                      .union([
                        z
                          .number()
                          .describe(
                            "Speed actually used, after clamping, for motions.",
                          ),
                        z
                          .null()
                          .describe(
                            "Speed actually used, after clamping, for motions.",
                          ),
                      ])
                      .describe(
                        "Speed actually used, after clamping, for motions.",
                      ),
                    label: z
                      .string()
                      .describe(
                        "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                      ),
                    path_limited: z.union([
                      z
                        .object({ reason: z.string() })
                        .describe(
                          "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                        ),
                      z.null(),
                    ]),
                    pose_limited: z.union([
                      z
                        .object({
                          advice: z
                            .string()
                            .describe(
                              "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                            ),
                          orientation_shortfall_radians: z
                            .number()
                            .describe(
                              "From the closest pose's orientation to the requested one.",
                            ),
                          position_shortfall_meters: z
                            .number()
                            .describe(
                              "From the closest pose's tool position to the requested one.",
                            ),
                          reason: z
                            .union([
                              z
                                .literal("at_joint_limit")
                                .describe(
                                  "The best pose it found has a joint at its limit, and no step improves it.",
                                ),
                              z
                                .literal("stalled")
                                .describe(
                                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                ),
                              z
                                .literal("iteration_limit")
                                .describe(
                                  "It was still improving when it ran out of iterations.",
                                ),
                              z
                                .literal("out_of_reach")
                                .describe(
                                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                ),
                            ])
                            .describe("Why a solve stopped short."),
                          waypoint: z
                            .number()
                            .int()
                            .gte(0)
                            .describe(
                              "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                            ),
                        })
                        .describe(
                          "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                        ),
                      z.null(),
                    ]),
                    target_joints: z.union([z.array(z.number()), z.null()]),
                    target_position: z.union([
                      z.array(z.number()).min(3).max(3),
                      z.null(),
                    ]),
                    target_quaternion: z.union([
                      z.array(z.number()).min(4).max(4),
                      z.null(),
                    ]),
                    target_status: z.union([
                      z
                        .enum([
                          "reached",
                          "stopped_short",
                          "partial",
                          "not_started",
                        ])
                        .describe(
                          "Whether the arm ended where the command was aimed.",
                        ),
                      z.null(),
                    ]),
                    trace_context: z
                      .union([
                        z
                          .object({
                            traceparent: z
                              .string()
                              .describe(
                                "`00-<trace id>-<parent span id>-<flags>`.",
                              ),
                            tracestate: z
                              .union([z.string(), z.null()])
                              .default(null),
                          })
                          .describe(
                            "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                          ),
                        z.null(),
                      ])
                      .describe(
                        "The trace context the command arrived with, if any.",
                      ),
                  })
                  .strict()
                  .describe("One command, from acceptance to its end."),
              })
              .describe("A command record and the arm it belongs to."),
          ),
        )
        .describe(
          "Command records for steps that have finished or been interrupted.",
        ),
    })
    .describe("The station's retained action sequence state."),
  ActionSequenceRequest: z
    .object({
      expected_revisions: z
        .record(z.string(), z.number().int().gte(0))
        .describe(
          "Revisions fence every participating arm throughout the sequence.",
        )
        .default({}),
      steps: z.array(
        z
          .object({
            arms: z.record(
              z.string(),
              z
                .object({
                  gripper_opening: z
                    .union([z.number(), z.null()])
                    .default(null),
                  max_joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Speed for both resources in rad/s; overrides the movement speed when provided.",
                        ),
                      z
                        .null()
                        .describe(
                          "Speed for both resources in rad/s; overrides the movement speed when provided.",
                        ),
                    ])
                    .describe(
                      "Speed for both resources in rad/s; overrides the movement speed when provided.",
                    )
                    .default(null),
                  movement: z
                    .union([
                      z
                        .union([
                          z
                            .object({
                              joints: z.array(z.number()),
                              kind: z.literal("joints"),
                              max_joint_speed: z
                                .union([z.number(), z.null()])
                                .default(null),
                            })
                            .strict(),
                          z
                            .object({
                              kind: z.literal("pose"),
                              max_joint_speed: z
                                .union([z.number(), z.null()])
                                .default(null),
                              orientation: z
                                .union([
                                  z
                                    .object({
                                      approach: z
                                        .union([
                                          z
                                            .union([
                                              z
                                                .literal("down")
                                                .describe("World -Z."),
                                              z
                                                .literal("forward")
                                                .describe("World +X."),
                                            ])
                                            .describe(
                                              "A named approach direction.",
                                            ),
                                          z.array(z.number()).min(3).max(3),
                                        ])
                                        .describe(
                                          "The direction the jaws point along: a named one, or a nonzero world direction.",
                                        ),
                                      jaw_yaw_degrees: z.number(),
                                      kind: z.literal("grasp"),
                                    })
                                    .describe(
                                      "The approach direction and the jaw yaw about it, in degrees.",
                                    ),
                                  z
                                    .object({
                                      kind: z.literal("quaternion"),
                                      quaternion: z
                                        .array(z.number())
                                        .min(4)
                                        .max(4),
                                    })
                                    .describe(
                                      "A raw quaternion, `[x, y, z, w]`.",
                                    ),
                                ])
                                .describe(
                                  "A tool orientation as a client gives it.",
                                ),
                              position: z.array(z.number()).min(3).max(3),
                            })
                            .strict(),
                        ])
                        .describe(
                          "One movement in an action sequence. The arm settles at each target.",
                        ),
                      z.null(),
                    ])
                    .default(null),
                })
                .strict()
                .describe(
                  "A movement and gripper target in the same step start together.",
                ),
            ),
          })
          .strict()
          .describe(
            "One synchronized step; a single-arm action is a map with one entry.",
          ),
      ),
    })
    .strict()
    .describe("An ordered set of steps to execute under the caller's lease."),
  ActionSequenceStartRequest: z
    .object({
      action_sequence: z
        .object({
          expected_revisions: z
            .record(z.string(), z.number().int().gte(0))
            .describe(
              "Revisions fence every participating arm throughout the sequence.",
            )
            .default({}),
          steps: z.array(
            z
              .object({
                arms: z.record(
                  z.string(),
                  z
                    .object({
                      gripper_opening: z
                        .union([z.number(), z.null()])
                        .default(null),
                      max_joint_speed: z
                        .union([
                          z
                            .number()
                            .describe(
                              "Speed for both resources in rad/s; overrides the movement speed when provided.",
                            ),
                          z
                            .null()
                            .describe(
                              "Speed for both resources in rad/s; overrides the movement speed when provided.",
                            ),
                        ])
                        .describe(
                          "Speed for both resources in rad/s; overrides the movement speed when provided.",
                        )
                        .default(null),
                      movement: z
                        .union([
                          z
                            .union([
                              z
                                .object({
                                  joints: z.array(z.number()),
                                  kind: z.literal("joints"),
                                  max_joint_speed: z
                                    .union([z.number(), z.null()])
                                    .default(null),
                                })
                                .strict(),
                              z
                                .object({
                                  kind: z.literal("pose"),
                                  max_joint_speed: z
                                    .union([z.number(), z.null()])
                                    .default(null),
                                  orientation: z
                                    .union([
                                      z
                                        .object({
                                          approach: z
                                            .union([
                                              z
                                                .union([
                                                  z
                                                    .literal("down")
                                                    .describe("World -Z."),
                                                  z
                                                    .literal("forward")
                                                    .describe("World +X."),
                                                ])
                                                .describe(
                                                  "A named approach direction.",
                                                ),
                                              z.array(z.number()).min(3).max(3),
                                            ])
                                            .describe(
                                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                                            ),
                                          jaw_yaw_degrees: z.number(),
                                          kind: z.literal("grasp"),
                                        })
                                        .describe(
                                          "The approach direction and the jaw yaw about it, in degrees.",
                                        ),
                                      z
                                        .object({
                                          kind: z.literal("quaternion"),
                                          quaternion: z
                                            .array(z.number())
                                            .min(4)
                                            .max(4),
                                        })
                                        .describe(
                                          "A raw quaternion, `[x, y, z, w]`.",
                                        ),
                                    ])
                                    .describe(
                                      "A tool orientation as a client gives it.",
                                    ),
                                  position: z.array(z.number()).min(3).max(3),
                                })
                                .strict(),
                            ])
                            .describe(
                              "One movement in an action sequence. The arm settles at each target.",
                            ),
                          z.null(),
                        ])
                        .default(null),
                    })
                    .strict()
                    .describe(
                      "A movement and gripper target in the same step start together.",
                    ),
                ),
              })
              .strict()
              .describe(
                "One synchronized step; a single-arm action is a map with one entry.",
              ),
          ),
        })
        .strict()
        .describe(
          "An ordered set of steps to execute under the caller's lease.",
        ),
      lease_token: z.string(),
      request_id: z.string(),
      station_instance_id: z.string(),
    })
    .strict()
    .describe("Start an action sequence under an explicit station lease."),
  ActionSequenceStep: z
    .object({
      arms: z.record(
        z.string(),
        z
          .object({
            gripper_opening: z.union([z.number(), z.null()]).default(null),
            max_joint_speed: z
              .union([
                z
                  .number()
                  .describe(
                    "Speed for both resources in rad/s; overrides the movement speed when provided.",
                  ),
                z
                  .null()
                  .describe(
                    "Speed for both resources in rad/s; overrides the movement speed when provided.",
                  ),
              ])
              .describe(
                "Speed for both resources in rad/s; overrides the movement speed when provided.",
              )
              .default(null),
            movement: z
              .union([
                z
                  .union([
                    z
                      .object({
                        joints: z.array(z.number()),
                        kind: z.literal("joints"),
                        max_joint_speed: z
                          .union([z.number(), z.null()])
                          .default(null),
                      })
                      .strict(),
                    z
                      .object({
                        kind: z.literal("pose"),
                        max_joint_speed: z
                          .union([z.number(), z.null()])
                          .default(null),
                        orientation: z
                          .union([
                            z
                              .object({
                                approach: z
                                  .union([
                                    z
                                      .union([
                                        z.literal("down").describe("World -Z."),
                                        z
                                          .literal("forward")
                                          .describe("World +X."),
                                      ])
                                      .describe("A named approach direction."),
                                    z.array(z.number()).min(3).max(3),
                                  ])
                                  .describe(
                                    "The direction the jaws point along: a named one, or a nonzero world direction.",
                                  ),
                                jaw_yaw_degrees: z.number(),
                                kind: z.literal("grasp"),
                              })
                              .describe(
                                "The approach direction and the jaw yaw about it, in degrees.",
                              ),
                            z
                              .object({
                                kind: z.literal("quaternion"),
                                quaternion: z.array(z.number()).min(4).max(4),
                              })
                              .describe("A raw quaternion, `[x, y, z, w]`."),
                          ])
                          .describe("A tool orientation as a client gives it."),
                        position: z.array(z.number()).min(3).max(3),
                      })
                      .strict(),
                  ])
                  .describe(
                    "One movement in an action sequence. The arm settles at each target.",
                  ),
                z.null(),
              ])
              .default(null),
          })
          .strict()
          .describe(
            "A movement and gripper target in the same step start together.",
          ),
      ),
    })
    .strict()
    .describe(
      "One synchronized step; a single-arm action is a map with one entry.",
    ),
  Approach: z
    .union([
      z
        .union([
          z.literal("down").describe("World -Z."),
          z.literal("forward").describe("World +X."),
        ])
        .describe("A named approach direction."),
      z.array(z.number()).min(3).max(3),
    ])
    .describe(
      "The direction the jaws point along: a named one, or a nonzero world direction.",
    ),
  ArmMode: z
    .union([
      z.literal("disabled").describe("Torque off; only `enable` is accepted."),
      z.literal("holding").describe("Torque on, holding between commands."),
      z.literal("moving").describe("Following a motion or a stream."),
      z
        .literal("faulted")
        .describe(
          "Powered off after a hard fault; refuses everything until `clear_fault`.",
        ),
      z
        .literal("disconnected")
        .describe(
          "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
        ),
      z
        .literal("reconnected")
        .describe(
          "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
        ),
    ])
    .describe("Where the arm stands."),
  ArmRecord: z
    .object({
      arm: z.string(),
      record: z
        .object({
          accepted_at: z.number(),
          achieved: z
            .union([
              z
                .object({
                  joint_error_radians: z
                    .union([z.number(), z.null()])
                    .optional(),
                  joint_errors_radians: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "Each joint's absolute distance from the target joints.",
                        ),
                      z
                        .null()
                        .describe(
                          "Each joint's absolute distance from the target joints.",
                        ),
                    ])
                    .describe(
                      "Each joint's absolute distance from the target joints.",
                    )
                    .optional(),
                  joints: z.array(z.number()),
                  orientation: z
                    .union([
                      z
                        .object({
                          approach: z.array(z.number()).min(3).max(3),
                          degrees_from_down: z.number(),
                          jaw_yaw_degrees: z.number(),
                          jaw_yaw_reference: z
                            .enum(["world_x", "world_y"])
                            .describe(
                              "The world axis jaw yaw zero is projected from.",
                            ),
                          target: z
                            .union([
                              z
                                .object({
                                  approach_error_degrees: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  orientation_error_degrees: z.number(),
                                })
                                .describe(
                                  "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                ),
                              z.null(),
                            ])
                            .optional(),
                        })
                        .describe(
                          "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The tool orientation in grasp terms, against the target's when there is one.",
                    )
                    .optional(),
                  orientation_error_radians: z
                    .union([z.number(), z.null()])
                    .optional(),
                  position: z
                    .union([
                      z
                        .array(z.number())
                        .min(3)
                        .max(3)
                        .describe(
                          "The tool pose at `joints`; absent when the controller has no tool link.",
                        ),
                      z
                        .null()
                        .describe(
                          "The tool pose at `joints`; absent when the controller has no tool link.",
                        ),
                    ])
                    .describe(
                      "The tool pose at `joints`; absent when the controller has no tool link.",
                    )
                    .optional(),
                  position_error_meters: z
                    .union([z.number(), z.null()])
                    .optional(),
                  quaternion: z
                    .union([z.array(z.number()).min(4).max(4), z.null()])
                    .optional(),
                  sampled_at: z.number(),
                })
                .describe("Where the arm was, measured, when a command ended."),
              z.null(),
            ])
            .optional(),
          checkpoints: z.array(
            z
              .object({
                label: z.string(),
                measured_joints: z.array(z.number()),
                target_position: z
                  .union([z.array(z.number()).min(3).max(3), z.null()])
                  .optional(),
                target_quaternion: z
                  .union([z.array(z.number()).min(4).max(4), z.null()])
                  .optional(),
              })
              .describe(
                "Where the arm stood, as measured, when it settled at the end of a segment.",
              ),
          ),
          command_id: z.string(),
          completion: z
            .union([
              z
                .object({
                  detail: z.union([z.string(), z.null()]).optional(),
                  finished_at: z.number(),
                  gripper: z
                    .union([
                      z
                        .union([
                          z
                            .object({
                              kind: z.literal("reached"),
                              measured_angle_radians: z.number(),
                            })
                            .describe(
                              "The jaw arrived at the angle it was asked for.",
                            ),
                          z
                            .object({
                              kind: z.literal("stopped_short"),
                              measured_angle_radians: z.number(),
                              target_angle_radians: z.number(),
                            })
                            .describe(
                              "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                            ),
                        ])
                        .describe(
                          "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "How a gripper command ended; null for every other command.",
                    )
                    .optional(),
                  outcome: z
                    .enum([
                      "completed",
                      "stopped",
                      "tripped",
                      "timed_out",
                      "stopped_short",
                      "superseded",
                      "faulted",
                      "interrupted",
                      "failed",
                    ])
                    .describe("How a command ended."),
                })
                .describe(
                  "The outcome and timestamp are recorded together when a command ends.",
                ),
              z.null(),
            ])
            .optional(),
          goal_joints: z
            .union([
              z
                .array(z.number())
                .describe(
                  "The newest goal of a joint stream, after clamping to the joint limits.",
                ),
              z
                .null()
                .describe(
                  "The newest goal of a joint stream, after clamping to the joint limits.",
                ),
            ])
            .describe(
              "The newest goal of a joint stream, after clamping to the joint limits.",
            )
            .optional(),
          group_id: z
            .union([
              z
                .string()
                .describe(
                  "The grouped motion this command is one arm's part of, if it is.",
                ),
              z
                .null()
                .describe(
                  "The grouped motion this command is one arm's part of, if it is.",
                ),
            ])
            .describe(
              "The grouped motion this command is one arm's part of, if it is.",
            )
            .optional(),
          joint_speed: z
            .union([
              z
                .number()
                .describe("Speed actually used, after clamping, for motions."),
              z
                .null()
                .describe("Speed actually used, after clamping, for motions."),
            ])
            .describe("Speed actually used, after clamping, for motions.")
            .optional(),
          label: z
            .string()
            .describe(
              "What the command is: `enable`, `stream_joints`, or the label a move carried.",
            ),
          path_limited: z
            .union([
              z
                .object({ reason: z.string() })
                .describe(
                  "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                ),
              z.null(),
            ])
            .optional(),
          pose_limited: z
            .union([
              z
                .object({
                  advice: z
                    .string()
                    .describe(
                      "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                    ),
                  orientation_shortfall_radians: z
                    .number()
                    .describe(
                      "From the closest pose's orientation to the requested one.",
                    ),
                  position_shortfall_meters: z
                    .number()
                    .describe(
                      "From the closest pose's tool position to the requested one.",
                    ),
                  reason: z
                    .union([
                      z
                        .literal("at_joint_limit")
                        .describe(
                          "The best pose it found has a joint at its limit, and no step improves it.",
                        ),
                      z
                        .literal("stalled")
                        .describe(
                          "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                        ),
                      z
                        .literal("iteration_limit")
                        .describe(
                          "It was still improving when it ran out of iterations.",
                        ),
                      z
                        .literal("out_of_reach")
                        .describe(
                          "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                        ),
                    ])
                    .describe("Why a solve stopped short."),
                  waypoint: z
                    .number()
                    .int()
                    .gte(0)
                    .describe(
                      "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                    ),
                })
                .describe(
                  "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                ),
              z.null(),
            ])
            .optional(),
          target_joints: z.union([z.array(z.number()), z.null()]).optional(),
          target_position: z
            .union([z.array(z.number()).min(3).max(3), z.null()])
            .optional(),
          target_quaternion: z
            .union([z.array(z.number()).min(4).max(4), z.null()])
            .optional(),
          target_status: z
            .union([
              z
                .enum(["reached", "stopped_short", "partial", "not_started"])
                .describe("Whether the arm ended where the command was aimed."),
              z.null(),
            ])
            .optional(),
          trace_context: z
            .union([
              z
                .object({
                  traceparent: z
                    .string()
                    .describe("`00-<trace id>-<parent span id>-<flags>`."),
                  tracestate: z.union([z.string(), z.null()]).default(null),
                })
                .describe(
                  "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                ),
              z.null(),
            ])
            .describe("The trace context the command arrived with, if any.")
            .optional(),
        })
        .strict()
        .describe("One command, from acceptance to its end."),
    })
    .describe("A command record and the arm it belongs to."),
  ArmRecoveryIssue: z
    .object({
      detail: z.string(),
      motors_without_torque: z.array(z.number().int().gte(0).lte(255)),
    })
    .describe(
      "Why automatic recovery stopped, including motors found without torque.",
    ),
  ArmState: z
    .object({
      active_command: z
        .union([
          z
            .object({
              accepted_at: z.number(),
              achieved: z
                .union([
                  z
                    .object({
                      joint_error_radians: z
                        .union([z.number(), z.null()])
                        .optional(),
                      joint_errors_radians: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            ),
                          z
                            .null()
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            ),
                        ])
                        .describe(
                          "Each joint's absolute distance from the target joints.",
                        )
                        .optional(),
                      joints: z.array(z.number()),
                      orientation: z
                        .union([
                          z
                            .object({
                              approach: z.array(z.number()).min(3).max(3),
                              degrees_from_down: z.number(),
                              jaw_yaw_degrees: z.number(),
                              jaw_yaw_reference: z
                                .enum(["world_x", "world_y"])
                                .describe(
                                  "The world axis jaw yaw zero is projected from.",
                                ),
                              target: z
                                .union([
                                  z
                                    .object({
                                      approach_error_degrees: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      orientation_error_degrees: z.number(),
                                    })
                                    .describe(
                                      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                    ),
                                  z.null(),
                                ])
                                .optional(),
                            })
                            .describe(
                              "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "The tool orientation in grasp terms, against the target's when there is one.",
                        )
                        .optional(),
                      orientation_error_radians: z
                        .union([z.number(), z.null()])
                        .optional(),
                      position: z
                        .union([
                          z
                            .array(z.number())
                            .min(3)
                            .max(3)
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            ),
                          z
                            .null()
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            ),
                        ])
                        .describe(
                          "The tool pose at `joints`; absent when the controller has no tool link.",
                        )
                        .optional(),
                      position_error_meters: z
                        .union([z.number(), z.null()])
                        .optional(),
                      quaternion: z
                        .union([z.array(z.number()).min(4).max(4), z.null()])
                        .optional(),
                      sampled_at: z.number(),
                    })
                    .describe(
                      "Where the arm was, measured, when a command ended.",
                    ),
                  z.null(),
                ])
                .optional(),
              checkpoints: z.array(
                z
                  .object({
                    label: z.string(),
                    measured_joints: z.array(z.number()),
                    target_position: z
                      .union([z.array(z.number()).min(3).max(3), z.null()])
                      .optional(),
                    target_quaternion: z
                      .union([z.array(z.number()).min(4).max(4), z.null()])
                      .optional(),
                  })
                  .describe(
                    "Where the arm stood, as measured, when it settled at the end of a segment.",
                  ),
              ),
              command_id: z.string(),
              completion: z
                .union([
                  z
                    .object({
                      detail: z.union([z.string(), z.null()]).optional(),
                      finished_at: z.number(),
                      gripper: z
                        .union([
                          z
                            .union([
                              z
                                .object({
                                  kind: z.literal("reached"),
                                  measured_angle_radians: z.number(),
                                })
                                .describe(
                                  "The jaw arrived at the angle it was asked for.",
                                ),
                              z
                                .object({
                                  kind: z.literal("stopped_short"),
                                  measured_angle_radians: z.number(),
                                  target_angle_radians: z.number(),
                                })
                                .describe(
                                  "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                ),
                            ])
                            .describe(
                              "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "How a gripper command ended; null for every other command.",
                        )
                        .optional(),
                      outcome: z
                        .enum([
                          "completed",
                          "stopped",
                          "tripped",
                          "timed_out",
                          "stopped_short",
                          "superseded",
                          "faulted",
                          "interrupted",
                          "failed",
                        ])
                        .describe("How a command ended."),
                    })
                    .describe(
                      "The outcome and timestamp are recorded together when a command ends.",
                    ),
                  z.null(),
                ])
                .optional(),
              goal_joints: z
                .union([
                  z
                    .array(z.number())
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                  z
                    .null()
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                ])
                .describe(
                  "The newest goal of a joint stream, after clamping to the joint limits.",
                )
                .optional(),
              group_id: z
                .union([
                  z
                    .string()
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                  z
                    .null()
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                ])
                .describe(
                  "The grouped motion this command is one arm's part of, if it is.",
                )
                .optional(),
              joint_speed: z
                .union([
                  z
                    .number()
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                  z
                    .null()
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                ])
                .describe("Speed actually used, after clamping, for motions.")
                .optional(),
              label: z
                .string()
                .describe(
                  "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                ),
              path_limited: z
                .union([
                  z
                    .object({ reason: z.string() })
                    .describe(
                      "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                    ),
                  z.null(),
                ])
                .optional(),
              pose_limited: z
                .union([
                  z
                    .object({
                      advice: z
                        .string()
                        .describe(
                          "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                        ),
                      orientation_shortfall_radians: z
                        .number()
                        .describe(
                          "From the closest pose's orientation to the requested one.",
                        ),
                      position_shortfall_meters: z
                        .number()
                        .describe(
                          "From the closest pose's tool position to the requested one.",
                        ),
                      reason: z
                        .union([
                          z
                            .literal("at_joint_limit")
                            .describe(
                              "The best pose it found has a joint at its limit, and no step improves it.",
                            ),
                          z
                            .literal("stalled")
                            .describe(
                              "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                            ),
                          z
                            .literal("iteration_limit")
                            .describe(
                              "It was still improving when it ran out of iterations.",
                            ),
                          z
                            .literal("out_of_reach")
                            .describe(
                              "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                            ),
                        ])
                        .describe("Why a solve stopped short."),
                      waypoint: z
                        .number()
                        .int()
                        .gte(0)
                        .describe(
                          "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                        ),
                    })
                    .describe(
                      "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                    ),
                  z.null(),
                ])
                .optional(),
              target_joints: z
                .union([z.array(z.number()), z.null()])
                .optional(),
              target_position: z
                .union([z.array(z.number()).min(3).max(3), z.null()])
                .optional(),
              target_quaternion: z
                .union([z.array(z.number()).min(4).max(4), z.null()])
                .optional(),
              target_status: z
                .union([
                  z
                    .enum([
                      "reached",
                      "stopped_short",
                      "partial",
                      "not_started",
                    ])
                    .describe(
                      "Whether the arm ended where the command was aimed.",
                    ),
                  z.null(),
                ])
                .optional(),
              trace_context: z
                .union([
                  z
                    .object({
                      traceparent: z
                        .string()
                        .describe("`00-<trace id>-<parent span id>-<flags>`."),
                      tracestate: z.union([z.string(), z.null()]).default(null),
                    })
                    .describe(
                      "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                    ),
                  z.null(),
                ])
                .describe("The trace context the command arrived with, if any.")
                .optional(),
            })
            .strict()
            .describe("One command, from acceptance to its end."),
          z.null(),
        ])
        .optional(),
      active_trip: z
        .union([
          z
            .string()
            .describe(
              "The protective check currently preventing new motion, if any.",
            ),
          z
            .null()
            .describe(
              "The protective check currently preventing new motion, if any.",
            ),
        ])
        .describe(
          "The protective check currently preventing new motion, if any.",
        )
        .default(null),
      arm: z.string(),
      base_pose: z
        .union([
          z
            .object({
              position_meters: z.array(z.number()).min(3).max(3),
              yaw_radians: z.number().default(0),
            })
            .strict()
            .describe(
              "A fixed placement of an arm's base: a position, and a turn about the world's\nvertical axis.",
            ),
          z.null(),
        ])
        .describe(
          "The arm's URDF root in the station world frame. None means the world origin.",
        )
        .default(null),
      commanded_joints: z
        .union([
          z
            .array(z.number())
            .describe(
              "Joint setpoint sent in the latest sample, if the arm was commanded.",
            ),
          z
            .null()
            .describe(
              "Joint setpoint sent in the latest sample, if the arm was commanded.",
            ),
        ])
        .describe(
          "Joint setpoint sent in the latest sample, if the arm was commanded.",
        )
        .optional(),
      controller: z
        .enum(["ik", "policy"])
        .describe("Which controller turns commands into motion."),
      fault: z.union([z.string(), z.null()]).optional(),
      gripper_opening: z
        .union([
          z.number().describe("0 closed to 1 open; none without a gripper."),
          z.null().describe("0 closed to 1 open; none without a gripper."),
        ])
        .describe("0 closed to 1 open; none without a gripper.")
        .optional(),
      interlocks: z
        .array(
          z
            .union([
              z
                .object({ kind: z.literal("arm_disconnected") })
                .describe(
                  "The bus stopped answering. Clears when the arm reconnects and holds.",
                ),
              z
                .object({
                  kind: z.literal("recovery_approval_required"),
                  motors: z.array(z.string()),
                })
                .describe(
                  "The arm reconnected unpowered or failed its checks. Clears when an\noperator sends `approve_recovery` after supporting the arm.",
                ),
              z
                .object({
                  camera: z.string(),
                  kind: z.literal("required_camera_unavailable"),
                })
                .describe(
                  "A camera in the lease's `required_cameras` dropped. Clears when the\nholder reports a fresh frame, or when the lease ends.",
                ),
            ])
            .describe(
              "A condition on an arm that refuses motion until it clears. `stop`,\n`disable`, `approve_recovery` and `clear_fault` are still accepted.",
            ),
        )
        .describe("Conditions refusing motion on this arm until they clear."),
      last_command: z
        .union([
          z
            .object({
              accepted_at: z.number(),
              achieved: z
                .union([
                  z
                    .object({
                      joint_error_radians: z
                        .union([z.number(), z.null()])
                        .optional(),
                      joint_errors_radians: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            ),
                          z
                            .null()
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            ),
                        ])
                        .describe(
                          "Each joint's absolute distance from the target joints.",
                        )
                        .optional(),
                      joints: z.array(z.number()),
                      orientation: z
                        .union([
                          z
                            .object({
                              approach: z.array(z.number()).min(3).max(3),
                              degrees_from_down: z.number(),
                              jaw_yaw_degrees: z.number(),
                              jaw_yaw_reference: z
                                .enum(["world_x", "world_y"])
                                .describe(
                                  "The world axis jaw yaw zero is projected from.",
                                ),
                              target: z
                                .union([
                                  z
                                    .object({
                                      approach_error_degrees: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      orientation_error_degrees: z.number(),
                                    })
                                    .describe(
                                      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                    ),
                                  z.null(),
                                ])
                                .optional(),
                            })
                            .describe(
                              "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "The tool orientation in grasp terms, against the target's when there is one.",
                        )
                        .optional(),
                      orientation_error_radians: z
                        .union([z.number(), z.null()])
                        .optional(),
                      position: z
                        .union([
                          z
                            .array(z.number())
                            .min(3)
                            .max(3)
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            ),
                          z
                            .null()
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            ),
                        ])
                        .describe(
                          "The tool pose at `joints`; absent when the controller has no tool link.",
                        )
                        .optional(),
                      position_error_meters: z
                        .union([z.number(), z.null()])
                        .optional(),
                      quaternion: z
                        .union([z.array(z.number()).min(4).max(4), z.null()])
                        .optional(),
                      sampled_at: z.number(),
                    })
                    .describe(
                      "Where the arm was, measured, when a command ended.",
                    ),
                  z.null(),
                ])
                .optional(),
              checkpoints: z.array(
                z
                  .object({
                    label: z.string(),
                    measured_joints: z.array(z.number()),
                    target_position: z
                      .union([z.array(z.number()).min(3).max(3), z.null()])
                      .optional(),
                    target_quaternion: z
                      .union([z.array(z.number()).min(4).max(4), z.null()])
                      .optional(),
                  })
                  .describe(
                    "Where the arm stood, as measured, when it settled at the end of a segment.",
                  ),
              ),
              command_id: z.string(),
              completion: z
                .union([
                  z
                    .object({
                      detail: z.union([z.string(), z.null()]).optional(),
                      finished_at: z.number(),
                      gripper: z
                        .union([
                          z
                            .union([
                              z
                                .object({
                                  kind: z.literal("reached"),
                                  measured_angle_radians: z.number(),
                                })
                                .describe(
                                  "The jaw arrived at the angle it was asked for.",
                                ),
                              z
                                .object({
                                  kind: z.literal("stopped_short"),
                                  measured_angle_radians: z.number(),
                                  target_angle_radians: z.number(),
                                })
                                .describe(
                                  "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                ),
                            ])
                            .describe(
                              "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "How a gripper command ended; null for every other command.",
                        )
                        .optional(),
                      outcome: z
                        .enum([
                          "completed",
                          "stopped",
                          "tripped",
                          "timed_out",
                          "stopped_short",
                          "superseded",
                          "faulted",
                          "interrupted",
                          "failed",
                        ])
                        .describe("How a command ended."),
                    })
                    .describe(
                      "The outcome and timestamp are recorded together when a command ends.",
                    ),
                  z.null(),
                ])
                .optional(),
              goal_joints: z
                .union([
                  z
                    .array(z.number())
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                  z
                    .null()
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                ])
                .describe(
                  "The newest goal of a joint stream, after clamping to the joint limits.",
                )
                .optional(),
              group_id: z
                .union([
                  z
                    .string()
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                  z
                    .null()
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                ])
                .describe(
                  "The grouped motion this command is one arm's part of, if it is.",
                )
                .optional(),
              joint_speed: z
                .union([
                  z
                    .number()
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                  z
                    .null()
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                ])
                .describe("Speed actually used, after clamping, for motions.")
                .optional(),
              label: z
                .string()
                .describe(
                  "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                ),
              path_limited: z
                .union([
                  z
                    .object({ reason: z.string() })
                    .describe(
                      "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                    ),
                  z.null(),
                ])
                .optional(),
              pose_limited: z
                .union([
                  z
                    .object({
                      advice: z
                        .string()
                        .describe(
                          "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                        ),
                      orientation_shortfall_radians: z
                        .number()
                        .describe(
                          "From the closest pose's orientation to the requested one.",
                        ),
                      position_shortfall_meters: z
                        .number()
                        .describe(
                          "From the closest pose's tool position to the requested one.",
                        ),
                      reason: z
                        .union([
                          z
                            .literal("at_joint_limit")
                            .describe(
                              "The best pose it found has a joint at its limit, and no step improves it.",
                            ),
                          z
                            .literal("stalled")
                            .describe(
                              "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                            ),
                          z
                            .literal("iteration_limit")
                            .describe(
                              "It was still improving when it ran out of iterations.",
                            ),
                          z
                            .literal("out_of_reach")
                            .describe(
                              "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                            ),
                        ])
                        .describe("Why a solve stopped short."),
                      waypoint: z
                        .number()
                        .int()
                        .gte(0)
                        .describe(
                          "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                        ),
                    })
                    .describe(
                      "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                    ),
                  z.null(),
                ])
                .optional(),
              target_joints: z
                .union([z.array(z.number()), z.null()])
                .optional(),
              target_position: z
                .union([z.array(z.number()).min(3).max(3), z.null()])
                .optional(),
              target_quaternion: z
                .union([z.array(z.number()).min(4).max(4), z.null()])
                .optional(),
              target_status: z
                .union([
                  z
                    .enum([
                      "reached",
                      "stopped_short",
                      "partial",
                      "not_started",
                    ])
                    .describe(
                      "Whether the arm ended where the command was aimed.",
                    ),
                  z.null(),
                ])
                .optional(),
              trace_context: z
                .union([
                  z
                    .object({
                      traceparent: z
                        .string()
                        .describe("`00-<trace id>-<parent span id>-<flags>`."),
                      tracestate: z.union([z.string(), z.null()]).default(null),
                    })
                    .describe(
                      "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                    ),
                  z.null(),
                ])
                .describe("The trace context the command arrived with, if any.")
                .optional(),
            })
            .strict()
            .describe("One command, from acceptance to its end."),
          z.null(),
        ])
        .optional(),
      measured_joint_efforts: z
        .array(z.number())
        .describe(
          "Measured joint efforts in the hardware profile's effort units, in\n`measured_joints` order.",
        ),
      measured_joint_velocities: z
        .array(z.number())
        .describe(
          "Measured joint velocities in radians per second, in `measured_joints` order.",
        ),
      measured_joints: z.array(z.number()),
      mode: z
        .union([
          z
            .literal("disabled")
            .describe("Torque off; only `enable` is accepted."),
          z.literal("holding").describe("Torque on, holding between commands."),
          z.literal("moving").describe("Following a motion or a stream."),
          z
            .literal("faulted")
            .describe(
              "Powered off after a hard fault; refuses everything until `clear_fault`.",
            ),
          z
            .literal("disconnected")
            .describe(
              "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
            ),
          z
            .literal("reconnected")
            .describe(
              "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
            ),
        ])
        .describe("Where the arm stands."),
      recovery_issue: z
        .union([
          z
            .object({
              detail: z.string(),
              motors_without_torque: z.array(z.number().int().gte(0).lte(255)),
            })
            .describe(
              "Why automatic recovery stopped, including motors found without torque.",
            ),
          z.null(),
        ])
        .optional(),
      rest_pose: z
        .union([
          z
            .array(z.number())
            .describe(
              "The pose the arm was enabled at; shutdown returns there first.",
            ),
          z
            .null()
            .describe(
              "The pose the arm was enabled at; shutdown returns there first.",
            ),
        ])
        .describe(
          "The pose the arm was enabled at; shutdown returns there first.",
        )
        .optional(),
      revision: z
        .number()
        .int()
        .gte(0)
        .describe(
          "Bumped each time an interlock is raised; commands may carry it as\n`expected_revision`.",
        ),
      sampled_at_unix_seconds: z
        .union([
          z
            .number()
            .describe(
              "When the newest sample was taken, in Unix seconds; none before the first.",
            ),
          z
            .null()
            .describe(
              "When the newest sample was taken, in Unix seconds; none before the first.",
            ),
        ])
        .describe(
          "When the newest sample was taken, in Unix seconds; none before the first.",
        )
        .optional(),
      segment: z.union([z.string(), z.null()]).optional(),
      status: z.string().describe("Why the arm is as it is, in a few words."),
      tool: z
        .union([
          z
            .object({
              model_points: z
                .array(z.array(z.number()).min(3).max(3))
                .describe(
                  "Each joint's origin, then the tool's: the points a viewer draws the arm through.",
                ),
              orientation: z
                .object({
                  approach: z.array(z.number()).min(3).max(3),
                  degrees_from_down: z.number(),
                  jaw_yaw_degrees: z.number(),
                  jaw_yaw_reference: z
                    .enum(["world_x", "world_y"])
                    .describe("The world axis jaw yaw zero is projected from."),
                  target: z
                    .union([
                      z
                        .object({
                          approach_error_degrees: z.number(),
                          jaw_yaw_degrees: z.number(),
                          jaw_yaw_reference: z
                            .enum(["world_x", "world_y"])
                            .describe(
                              "The world axis jaw yaw zero is projected from.",
                            ),
                          orientation_error_degrees: z.number(),
                        })
                        .describe(
                          "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                })
                .describe(
                  "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                ),
              position: z.array(z.number()).min(3).max(3),
              quaternion: z.array(z.number()).min(4).max(4),
            })
            .describe(
              "The tool's pose at some joints, in the station's world frame.",
            ),
          z.null(),
        ])
        .describe(
          "The tool's pose at the measured joints, its orientation in grasp terms,\nand the joint origins a viewer sketches the arm through.",
        )
        .optional(),
    })
    .describe("One arm as clients see it."),
  BasePose: z
    .object({
      position_meters: z.array(z.number()).min(3).max(3),
      yaw_radians: z.number().default(0),
    })
    .strict()
    .describe(
      "A fixed placement of an arm's base: a position, and a turn about the world's\nvertical axis.",
    ),
  CameraListing: z
    .object({ available: z.boolean(), name: z.string() })
    .describe("A camera as the media service's `GET /cameras` lists it."),
  CameraProblem: z
    .object({
      detail: z
        .string()
        .describe(
          "The program's message, such as `camera backend: Could not open device …`.",
        ),
      last_seen_unix_seconds: z.number(),
    })
    .describe(
      "Why a camera is not producing frames: the latest error a media program printed for it.",
    ),
  Checkpoint: z
    .object({
      label: z.string(),
      measured_joints: z.array(z.number()),
      target_position: z
        .union([z.array(z.number()).min(3).max(3), z.null()])
        .optional(),
      target_quaternion: z
        .union([z.array(z.number()).min(4).max(4), z.null()])
        .optional(),
    })
    .describe(
      "Where the arm stood, as measured, when it settled at the end of a segment.",
    ),
  ClientMessage: z
    .union([
      z
        .object({
          action_sequence: z
            .object({
              expected_revisions: z
                .record(z.string(), z.number().int().gte(0))
                .describe(
                  "Revisions fence every participating arm throughout the sequence.",
                )
                .default({}),
              steps: z.array(
                z
                  .object({
                    arms: z.record(
                      z.string(),
                      z
                        .object({
                          gripper_opening: z
                            .union([z.number(), z.null()])
                            .default(null),
                          max_joint_speed: z
                            .union([
                              z
                                .number()
                                .describe(
                                  "Speed for both resources in rad/s; overrides the movement speed when provided.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Speed for both resources in rad/s; overrides the movement speed when provided.",
                                ),
                            ])
                            .describe(
                              "Speed for both resources in rad/s; overrides the movement speed when provided.",
                            )
                            .default(null),
                          movement: z
                            .union([
                              z
                                .union([
                                  z
                                    .object({
                                      joints: z.array(z.number()),
                                      kind: z.literal("joints"),
                                      max_joint_speed: z
                                        .union([z.number(), z.null()])
                                        .default(null),
                                    })
                                    .strict(),
                                  z
                                    .object({
                                      kind: z.literal("pose"),
                                      max_joint_speed: z
                                        .union([z.number(), z.null()])
                                        .default(null),
                                      orientation: z
                                        .union([
                                          z
                                            .object({
                                              approach: z
                                                .union([
                                                  z
                                                    .union([
                                                      z
                                                        .literal("down")
                                                        .describe("World -Z."),
                                                      z
                                                        .literal("forward")
                                                        .describe("World +X."),
                                                    ])
                                                    .describe(
                                                      "A named approach direction.",
                                                    ),
                                                  z
                                                    .array(z.number())
                                                    .min(3)
                                                    .max(3),
                                                ])
                                                .describe(
                                                  "The direction the jaws point along: a named one, or a nonzero world direction.",
                                                ),
                                              jaw_yaw_degrees: z.number(),
                                              kind: z.literal("grasp"),
                                            })
                                            .describe(
                                              "The approach direction and the jaw yaw about it, in degrees.",
                                            ),
                                          z
                                            .object({
                                              kind: z.literal("quaternion"),
                                              quaternion: z
                                                .array(z.number())
                                                .min(4)
                                                .max(4),
                                            })
                                            .describe(
                                              "A raw quaternion, `[x, y, z, w]`.",
                                            ),
                                        ])
                                        .describe(
                                          "A tool orientation as a client gives it.",
                                        ),
                                      position: z
                                        .array(z.number())
                                        .min(3)
                                        .max(3),
                                    })
                                    .strict(),
                                ])
                                .describe(
                                  "One movement in an action sequence. The arm settles at each target.",
                                ),
                              z.null(),
                            ])
                            .default(null),
                        })
                        .strict()
                        .describe(
                          "A movement and gripper target in the same step start together.",
                        ),
                    ),
                  })
                  .strict()
                  .describe(
                    "One synchronized step; a single-arm action is a map with one entry.",
                  ),
              ),
            })
            .strict()
            .describe(
              "An ordered set of steps to execute under the caller's lease.",
            ),
          kind: z.literal("start_action_sequence"),
          request_id: z.union([z.string(), z.null()]).optional(),
        })
        .strict()
        .describe(
          "Submit an ordered action sequence under this connection's lease.",
        ),
      z
        .object({
          action_sequence_id: z.string(),
          kind: z.literal("cancel_action_sequence"),
          request_id: z.union([z.string(), z.null()]).optional(),
        })
        .strict()
        .describe("Cancel an action sequence by its identifier."),
      z
        .object({
          kind: z.literal("hello"),
          resume_after: z
            .record(z.string(), z.number().int().gte(0))
            .default({}),
          run_after: z
            .union([
              z
                .number()
                .int()
                .gte(0)
                .describe(
                  "The last run sequence number seen; without one, the run telemetry\nstream starts at its newest record.",
                ),
              z
                .null()
                .describe(
                  "The last run sequence number seen; without one, the run telemetry\nstream starts at its newest record.",
                ),
            ])
            .describe(
              "The last run sequence number seen; without one, the run telemetry\nstream starts at its newest record.",
            )
            .default(null),
        })
        .strict()
        .describe(
          "The first message: the last sequence number seen per arm, for the arms\nto resume; the others start from a snapshot.",
        ),
      z
        .object({
          holder: z
            .union([
              z
                .literal("agent")
                .describe(
                  "A model loop, started from the terminal or the agent server.",
                ),
              z
                .literal("manual")
                .describe(
                  "A person driving the arms from the viewer's manual controls.",
                ),
            ])
            .describe("The kind of controller holding the lease."),
          kind: z.literal("acquire_lease"),
          label: z.string(),
          on_interruption: z
            .union([
              z
                .literal("wait_for_approval")
                .describe(
                  "Keep the lease and wait for an operator's `approve_recovery`.",
                ),
              z.literal("return_lease").describe("Give the lease up."),
            ])
            .describe(
              "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
            )
            .default("wait_for_approval"),
          request_id: z.union([z.string(), z.null()]).default(null),
          required_cameras: z
            .array(z.string())
            .describe("Cameras the holder cannot work without.")
            .default([]),
        })
        .strict()
        .describe(
          "Acquire the lease on every arm for this connection, answered by\n`lease`: granted when no one holds it, else the holder.",
        ),
      z
        .object({
          holder: z
            .union([
              z
                .literal("agent")
                .describe(
                  "A model loop, started from the terminal or the agent server.",
                ),
              z
                .literal("manual")
                .describe(
                  "A person driving the arms from the viewer's manual controls.",
                ),
            ])
            .describe("The kind of controller holding the lease."),
          kind: z.literal("take_lease"),
          label: z.string(),
          on_interruption: z
            .union([
              z
                .literal("wait_for_approval")
                .describe(
                  "Keep the lease and wait for an operator's `approve_recovery`.",
                ),
              z.literal("return_lease").describe("Give the lease up."),
            ])
            .describe(
              "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
            )
            .default("wait_for_approval"),
          request_id: z.union([z.string(), z.null()]).default(null),
          required_cameras: z
            .array(z.string())
            .describe("Cameras the holder cannot work without.")
            .default([]),
        })
        .strict()
        .describe(
          "Take the lease from whoever holds it, stopping every arm first.",
        ),
      z
        .object({ kind: z.literal("return_lease") })
        .strict()
        .describe(
          "Give the lease up; the arms keep doing what they were told.",
        ),
      z
        .object({
          arm: z.string(),
          command: z
            .union([
              z
                .object({ kind: z.literal("enable") })
                .strict()
                .describe(
                  "Preflight, then power the motors holding their present pose.",
                ),
              z
                .object({ kind: z.literal("approve_recovery") })
                .strict()
                .describe(
                  "Approve recovery after inspecting and supporting the arm.",
                ),
              z
                .object({ kind: z.literal("disable") })
                .strict()
                .describe(
                  "Turn torque off where the arm is; an unsupported arm will fall.",
                ),
              z
                .object({ kind: z.literal("stop") })
                .strict()
                .describe(
                  "End the active motion with outcome stopped and hold where the arm is.\nAnyone may send it, whoever holds the lease.",
                ),
              z
                .object({ kind: z.literal("clear_fault") })
                .strict()
                .describe("Acknowledge a fault; the arm is already disabled."),
              z
                .object({
                  joints: z.array(z.number()),
                  kind: z.literal("move_to_joints"),
                  max_joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Peak joint speed in rad/s; clamped to the profile's maximum.",
                        ),
                      z
                        .null()
                        .describe(
                          "Peak joint speed in rad/s; clamped to the profile's maximum.",
                        ),
                    ])
                    .describe(
                      "Peak joint speed in rad/s; clamped to the profile's maximum.",
                    )
                    .default(null),
                })
                .strict()
                .describe(
                  "Move to joint angles, one per joint in the state's order.",
                ),
              z
                .object({
                  kind: z.literal("move_to_pose"),
                  max_joint_speed: z
                    .union([z.number(), z.null()])
                    .default(null),
                  orientation: z
                    .union([
                      z
                        .object({
                          approach: z
                            .union([
                              z
                                .union([
                                  z.literal("down").describe("World -Z."),
                                  z.literal("forward").describe("World +X."),
                                ])
                                .describe("A named approach direction."),
                              z.array(z.number()).min(3).max(3),
                            ])
                            .describe(
                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                            ),
                          jaw_yaw_degrees: z.number(),
                          kind: z.literal("grasp"),
                        })
                        .describe(
                          "The approach direction and the jaw yaw about it, in degrees.",
                        ),
                      z
                        .object({
                          kind: z.literal("quaternion"),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("A raw quaternion, `[x, y, z, w]`."),
                    ])
                    .describe("A tool orientation as a client gives it."),
                  position: z.array(z.number()).min(3).max(3),
                })
                .strict()
                .describe("Move the tool to a world position and orientation."),
              z
                .object({
                  kind: z.literal("follow_path"),
                  max_joint_speed: z
                    .union([z.number(), z.null()])
                    .default(null),
                  waypoints: z.array(
                    z
                      .union([
                        z.object({
                          joints: z.array(z.number()),
                          kind: z.literal("joints"),
                        }),
                        z.object({
                          kind: z.literal("pose"),
                          orientation: z
                            .union([
                              z
                                .object({
                                  approach: z
                                    .union([
                                      z
                                        .union([
                                          z
                                            .literal("down")
                                            .describe("World -Z."),
                                          z
                                            .literal("forward")
                                            .describe("World +X."),
                                        ])
                                        .describe(
                                          "A named approach direction.",
                                        ),
                                      z.array(z.number()).min(3).max(3),
                                    ])
                                    .describe(
                                      "The direction the jaws point along: a named one, or a nonzero world direction.",
                                    ),
                                  jaw_yaw_degrees: z.number(),
                                  kind: z.literal("grasp"),
                                })
                                .describe(
                                  "The approach direction and the jaw yaw about it, in degrees.",
                                ),
                              z
                                .object({
                                  kind: z.literal("quaternion"),
                                  quaternion: z.array(z.number()).min(4).max(4),
                                })
                                .describe("A raw quaternion, `[x, y, z, w]`."),
                            ])
                            .describe(
                              "A tool orientation as a client gives it.",
                            ),
                          position: z.array(z.number()).min(3).max(3),
                        }),
                      ])
                      .describe(
                        "One corner of a move: joints, or a tool pose solved for joints.",
                      ),
                  ),
                })
                .strict()
                .describe(
                  "Waypoints are corners: the arm comes to rest and settles at each one.",
                ),
              z
                .object({
                  gripper_opening: z
                    .union([
                      z
                        .number()
                        .describe(
                          "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                        ),
                      z
                        .null()
                        .describe(
                          "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                        ),
                    ])
                    .describe(
                      "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                    )
                    .default(null),
                  joints: z.array(z.number()),
                  kind: z.literal("stream_joints"),
                  max_joint_speed: z
                    .union([z.number(), z.null()])
                    .default(null),
                  stream_command_id: z
                    .union([z.string(), z.null()])
                    .default(null),
                })
                .strict()
                .describe(
                  "Stream trajectory destinations. The first goal, without\n`stream_command_id`, starts a stream when the arm is holding; later goals\ncarry the command id it returned. Each cycle the arm moves toward the\nnewest goal at up to the first goal's speed. The stream ends stopped when\ngoals stop arriving, on stop, or on a trip.",
                ),
              z
                .object({
                  kind: z.literal("set_gripper"),
                  max_joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Peak gripper speed in rad/s; uses the arm default and maximum.",
                        ),
                      z
                        .null()
                        .describe(
                          "Peak gripper speed in rad/s; uses the arm default and maximum.",
                        ),
                    ])
                    .describe(
                      "Peak gripper speed in rad/s; uses the arm default and maximum.",
                    )
                    .default(null),
                  opening: z.number(),
                })
                .strict()
                .describe(
                  "Open or close the gripper, 0 closed to 1 open, whether the arm is holding\nor moving. It completes on arrival or on contact.",
                ),
            ])
            .describe("A command for one arm."),
          expected_revision: z
            .union([
              z
                .number()
                .int()
                .gte(0)
                .describe(
                  "Refuse the command as `stale_revision` if the arm's revision moved on.",
                ),
              z
                .null()
                .describe(
                  "Refuse the command as `stale_revision` if the arm's revision moved on.",
                ),
            ])
            .describe(
              "Refuse the command as `stale_revision` if the arm's revision moved on.",
            )
            .default(null),
          kind: z.literal("command"),
          request_id: z.union([z.string(), z.null()]).default(null),
          trace_context: z
            .union([
              z
                .object({
                  traceparent: z
                    .string()
                    .describe("`00-<trace id>-<parent span id>-<flags>`."),
                  tracestate: z.union([z.string(), z.null()]).default(null),
                })
                .describe(
                  "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                ),
              z.null(),
            ])
            .describe(
              "The caller's trace context; the command's record carries it.",
            )
            .default(null),
        })
        .strict()
        .describe(
          "A command for one arm, answered by `command_answer`. A stop can bypass\nearlier requests.",
        ),
      z
        .object({
          arm: z.string(),
          kind: z.literal("stop_while_leased"),
          request_id: z.union([z.string(), z.null()]).default(null),
        })
        .strict()
        .describe(
          "Stop one arm only while this connection still holds the lease, answered\nby `command_answer`: accepted with no records once the stop is\nsubmitted, or refused as `lease_held_elsewhere` when the lease changed.\nA plain `stop` command is accepted from anyone.",
        ),
      z
        .object({
          available: z.boolean(),
          camera: z.string(),
          kind: z.literal("report_camera"),
          request_id: z.union([z.string(), z.null()]).default(null),
        })
        .strict()
        .describe(
          "The lease holder reports whether a camera it requires gives frames,\nanswered by `command_answer` like `stop_while_leased`. A fresh frame\nclears the camera's interlock.",
        ),
      z
        .object({
          command: z
            .object({
              max_joint_speed: z.union([z.number(), z.null()]).default(null),
              targets: z.record(
                z.string(),
                z
                  .union([
                    z.object({
                      joints: z.array(z.number()),
                      kind: z.literal("joints"),
                    }),
                    z.object({
                      kind: z.literal("pose"),
                      orientation: z
                        .union([
                          z
                            .object({
                              approach: z
                                .union([
                                  z
                                    .union([
                                      z.literal("down").describe("World -Z."),
                                      z
                                        .literal("forward")
                                        .describe("World +X."),
                                    ])
                                    .describe("A named approach direction."),
                                  z.array(z.number()).min(3).max(3),
                                ])
                                .describe(
                                  "The direction the jaws point along: a named one, or a nonzero world direction.",
                                ),
                              jaw_yaw_degrees: z.number(),
                              kind: z.literal("grasp"),
                            })
                            .describe(
                              "The approach direction and the jaw yaw about it, in degrees.",
                            ),
                          z
                            .object({
                              kind: z.literal("quaternion"),
                              quaternion: z.array(z.number()).min(4).max(4),
                            })
                            .describe("A raw quaternion, `[x, y, z, w]`."),
                        ])
                        .describe("A tool orientation as a client gives it."),
                      position: z.array(z.number()).min(3).max(3),
                    }),
                  ])
                  .describe(
                    "One corner of a move: joints, or a tool pose solved for joints.",
                  ),
              ),
            })
            .strict()
            .describe(
              "Move several arms together: every target is planned before any arm moves,\none refusal refuses all, and when one arm ends without completing the others\nstop and hold. No collision checking between the arms.",
            ),
          expected_revisions: z
            .record(z.string(), z.number().int().gte(0))
            .describe(
              "Per arm: refuse the move as `stale_revision` if that arm's revision\nmoved on.",
            )
            .default({}),
          kind: z.literal("move_arms"),
          request_id: z.union([z.string(), z.null()]).default(null),
          trace_context: z
            .union([
              z
                .object({
                  traceparent: z
                    .string()
                    .describe("`00-<trace id>-<parent span id>-<flags>`."),
                  tracestate: z.union([z.string(), z.null()]).default(null),
                })
                .describe(
                  "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                ),
              z.null(),
            ])
            .describe(
              "The caller's trace context; each arm's record carries it.",
            )
            .default(null),
        })
        .strict()
        .describe("Move several arms together, answered by `command_answer`."),
      z
        .object({
          arm: z.string(),
          kind: z.literal("preview"),
          pose: z
            .object({
              orientation: z
                .union([
                  z
                    .union([
                      z
                        .object({
                          approach: z
                            .union([
                              z
                                .union([
                                  z.literal("down").describe("World -Z."),
                                  z.literal("forward").describe("World +X."),
                                ])
                                .describe("A named approach direction."),
                              z.array(z.number()).min(3).max(3),
                            ])
                            .describe(
                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                            ),
                          jaw_yaw_degrees: z.number(),
                          kind: z.literal("grasp"),
                        })
                        .describe(
                          "The approach direction and the jaw yaw about it, in degrees.",
                        ),
                      z
                        .object({
                          kind: z.literal("quaternion"),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("A raw quaternion, `[x, y, z, w]`."),
                    ])
                    .describe("A tool orientation as a client gives it."),
                  z.null(),
                ])
                .describe("None leaves the orientation to the solver.")
                .default(null),
              position: z.array(z.number()).min(3).max(3),
            })
            .strict()
            .describe(
              "Where the arm's controller would take the tool toward a pose, without moving.",
            ),
          request_id: z.union([z.string(), z.null()]).default(null),
        })
        .strict()
        .describe(
          "Where an arm would take the tool toward a pose, answered by\n`preview_answer`; nothing moves.",
        ),
      z
        .object({
          event: z
            .union([
              z
                .union([
                  z.object({
                    task: z.union([z.string(), z.null()]).default(null),
                    type: z.literal("run_started"),
                  }),
                  z.object({
                    steps: z.array(
                      z.object({
                        status: z.enum(["pending", "active", "done", "failed"]),
                        text: z.string(),
                      }),
                    ),
                    type: z.literal("plan_update"),
                  }),
                  z.object({
                    notes: z.array(z.string()),
                    type: z.literal("notes_update"),
                  }),
                  z.object({
                    type: z.literal("usage_update"),
                    usage: z
                      .object({
                        cached_input_tokens: z.number().int().gte(0),
                        input_tokens: z.number().int().gte(0),
                        output_tokens: z.number().int().gte(0),
                        reasoning_tokens: z.number().int().gte(0),
                        run_id: z.string(),
                        turns: z.number().int().gte(0),
                      })
                      .describe("A model run's token use so far."),
                  }),
                ])
                .describe("A run event the server acts on."),
              z.any(),
            ])
            .describe(
              "An event a model loop reports: one the server acts on, or any other kept\nas it came.",
            ),
          kind: z.literal("run_event"),
        })
        .strict()
        .describe("An event from a model loop, for the viewer's live run."),
      z
        .object({
          kind: z.literal("run_spans"),
          spans: z.array(
            z
              .object({
                attributes: z.record(z.string(), z.any()).default({}),
                end_time_unix_nano: z.string().regex(/^[0-9]+$/),
                name: z.string(),
                parent_span_id: z.union([z.string(), z.null()]).default(null),
                span_id: z.string(),
                start_time_unix_nano: z.string().regex(/^[0-9]+$/),
                status: z
                  .object({
                    code: z
                      .enum([
                        "STATUS_CODE_UNSET",
                        "STATUS_CODE_OK",
                        "STATUS_CODE_ERROR",
                      ])
                      .default("STATUS_CODE_UNSET"),
                    message: z.union([z.string(), z.null()]).default(null),
                  })
                  .default({ code: "STATUS_CODE_UNSET", message: null }),
                trace_id: z.string(),
              })
              .describe(
                "One span of a model loop's trace, in OpenTelemetry's terms.",
              ),
          ),
        })
        .strict()
        .describe(
          "Trace spans from a model loop: running ones as they start, then finished.",
        ),
    ])
    .describe(
      "Shared client request models used by SDK command and telemetry helpers.",
    ),
  CommandAnswer: z
    .union([
      z.object({
        records: z.array(
          z
            .object({
              arm: z.string(),
              record: z
                .object({
                  accepted_at: z.number(),
                  achieved: z
                    .union([
                      z
                        .object({
                          joint_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          joint_errors_radians: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                            ])
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            )
                            .optional(),
                          joints: z.array(z.number()),
                          orientation: z
                            .union([
                              z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  degrees_from_down: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  target: z
                                    .union([
                                      z
                                        .object({
                                          approach_error_degrees: z.number(),
                                          jaw_yaw_degrees: z.number(),
                                          jaw_yaw_reference: z
                                            .enum(["world_x", "world_y"])
                                            .describe(
                                              "The world axis jaw yaw zero is projected from.",
                                            ),
                                          orientation_error_degrees: z.number(),
                                        })
                                        .describe(
                                          "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                        ),
                                      z.null(),
                                    ])
                                    .optional(),
                                })
                                .describe(
                                  "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The tool orientation in grasp terms, against the target's when there is one.",
                            )
                            .optional(),
                          orientation_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          position: z
                            .union([
                              z
                                .array(z.number())
                                .min(3)
                                .max(3)
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                            ])
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            )
                            .optional(),
                          position_error_meters: z
                            .union([z.number(), z.null()])
                            .optional(),
                          quaternion: z
                            .union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ])
                            .optional(),
                          sampled_at: z.number(),
                        })
                        .describe(
                          "Where the arm was, measured, when a command ended.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  checkpoints: z.array(
                    z
                      .object({
                        label: z.string(),
                        measured_joints: z.array(z.number()),
                        target_position: z
                          .union([z.array(z.number()).min(3).max(3), z.null()])
                          .optional(),
                        target_quaternion: z
                          .union([z.array(z.number()).min(4).max(4), z.null()])
                          .optional(),
                      })
                      .describe(
                        "Where the arm stood, as measured, when it settled at the end of a segment.",
                      ),
                  ),
                  command_id: z.string(),
                  completion: z
                    .union([
                      z
                        .object({
                          detail: z.union([z.string(), z.null()]).optional(),
                          finished_at: z.number(),
                          gripper: z
                            .union([
                              z
                                .union([
                                  z
                                    .object({
                                      kind: z.literal("reached"),
                                      measured_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw arrived at the angle it was asked for.",
                                    ),
                                  z
                                    .object({
                                      kind: z.literal("stopped_short"),
                                      measured_angle_radians: z.number(),
                                      target_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                    ),
                                ])
                                .describe(
                                  "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "How a gripper command ended; null for every other command.",
                            )
                            .optional(),
                          outcome: z
                            .enum([
                              "completed",
                              "stopped",
                              "tripped",
                              "timed_out",
                              "stopped_short",
                              "superseded",
                              "faulted",
                              "interrupted",
                              "failed",
                            ])
                            .describe("How a command ended."),
                        })
                        .describe(
                          "The outcome and timestamp are recorded together when a command ends.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  goal_joints: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                      z
                        .null()
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                    ])
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    )
                    .optional(),
                  group_id: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                      z
                        .null()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                    ])
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    )
                    .optional(),
                  joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                      z
                        .null()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                    ])
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    )
                    .optional(),
                  label: z
                    .string()
                    .describe(
                      "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                    ),
                  path_limited: z
                    .union([
                      z
                        .object({ reason: z.string() })
                        .describe(
                          "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  pose_limited: z
                    .union([
                      z
                        .object({
                          advice: z
                            .string()
                            .describe(
                              "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                            ),
                          orientation_shortfall_radians: z
                            .number()
                            .describe(
                              "From the closest pose's orientation to the requested one.",
                            ),
                          position_shortfall_meters: z
                            .number()
                            .describe(
                              "From the closest pose's tool position to the requested one.",
                            ),
                          reason: z
                            .union([
                              z
                                .literal("at_joint_limit")
                                .describe(
                                  "The best pose it found has a joint at its limit, and no step improves it.",
                                ),
                              z
                                .literal("stalled")
                                .describe(
                                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                ),
                              z
                                .literal("iteration_limit")
                                .describe(
                                  "It was still improving when it ran out of iterations.",
                                ),
                              z
                                .literal("out_of_reach")
                                .describe(
                                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                ),
                            ])
                            .describe("Why a solve stopped short."),
                          waypoint: z
                            .number()
                            .int()
                            .gte(0)
                            .describe(
                              "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                            ),
                        })
                        .describe(
                          "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  target_joints: z
                    .union([z.array(z.number()), z.null()])
                    .optional(),
                  target_position: z
                    .union([z.array(z.number()).min(3).max(3), z.null()])
                    .optional(),
                  target_quaternion: z
                    .union([z.array(z.number()).min(4).max(4), z.null()])
                    .optional(),
                  target_status: z
                    .union([
                      z
                        .enum([
                          "reached",
                          "stopped_short",
                          "partial",
                          "not_started",
                        ])
                        .describe(
                          "Whether the arm ended where the command was aimed.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  trace_context: z
                    .union([
                      z
                        .object({
                          traceparent: z
                            .string()
                            .describe(
                              "`00-<trace id>-<parent span id>-<flags>`.",
                            ),
                          tracestate: z
                            .union([z.string(), z.null()])
                            .default(null),
                        })
                        .describe(
                          "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The trace context the command arrived with, if any.",
                    )
                    .optional(),
                })
                .strict()
                .describe("One command, from acceptance to its end."),
            })
            .describe("A command record and the arm it belongs to."),
        ),
        status: z.literal("accepted"),
      }),
      z.object({
        detail: z.string(),
        reason: z
          .enum([
            "arm_busy",
            "not_enabled",
            "stale_revision",
            "interlocked",
            "faulted",
            "invalid_target",
            "stream_ended",
            "unreachable",
            "lease_held_elsewhere",
            "malformed",
          ])
          .describe("Why a command was not accepted."),
        status: z.literal("refused"),
      }),
    ])
    .describe(
      "How a command was answered: accepted with its records (one per arm for\n`move_arms`), or refused, in which case nothing moved.",
    ),
  CommandCancellationRequest: z
    .object({
      arms: z.array(z.string()),
      lease_token: z.string(),
      request_id: z.string(),
      station_instance_id: z.string(),
    })
    .strict()
    .describe(
      "Cancel a submission and stop its affected arms while the lease is current.",
    ),
  CommandCompletion: z
    .object({
      detail: z.union([z.string(), z.null()]).optional(),
      finished_at: z.number(),
      gripper: z
        .union([
          z
            .union([
              z
                .object({
                  kind: z.literal("reached"),
                  measured_angle_radians: z.number(),
                })
                .describe("The jaw arrived at the angle it was asked for."),
              z
                .object({
                  kind: z.literal("stopped_short"),
                  measured_angle_radians: z.number(),
                  target_angle_radians: z.number(),
                })
                .describe(
                  "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                ),
            ])
            .describe(
              "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
            ),
          z.null(),
        ])
        .describe("How a gripper command ended; null for every other command.")
        .optional(),
      outcome: z
        .enum([
          "completed",
          "stopped",
          "tripped",
          "timed_out",
          "stopped_short",
          "superseded",
          "faulted",
          "interrupted",
          "failed",
        ])
        .describe("How a command ended."),
    })
    .describe(
      "The outcome and timestamp are recorded together when a command ends.",
    ),
  CommandOutcome: z
    .enum([
      "completed",
      "stopped",
      "tripped",
      "timed_out",
      "stopped_short",
      "superseded",
      "faulted",
      "interrupted",
      "failed",
    ])
    .describe("How a command ended."),
  CommandRecord: z
    .object({
      accepted_at: z.number(),
      achieved: z.union([
        z
          .object({
            joint_error_radians: z.union([z.number(), z.null()]),
            joint_errors_radians: z
              .union([
                z
                  .array(z.number())
                  .describe(
                    "Each joint's absolute distance from the target joints.",
                  ),
                z
                  .null()
                  .describe(
                    "Each joint's absolute distance from the target joints.",
                  ),
              ])
              .describe(
                "Each joint's absolute distance from the target joints.",
              ),
            joints: z.array(z.number()),
            orientation: z
              .union([
                z
                  .object({
                    approach: z.array(z.number()).min(3).max(3),
                    degrees_from_down: z.number(),
                    jaw_yaw_degrees: z.number(),
                    jaw_yaw_reference: z
                      .enum(["world_x", "world_y"])
                      .describe(
                        "The world axis jaw yaw zero is projected from.",
                      ),
                    target: z.union([
                      z
                        .object({
                          approach_error_degrees: z.number(),
                          jaw_yaw_degrees: z.number(),
                          jaw_yaw_reference: z
                            .enum(["world_x", "world_y"])
                            .describe(
                              "The world axis jaw yaw zero is projected from.",
                            ),
                          orientation_error_degrees: z.number(),
                        })
                        .describe(
                          "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                        ),
                      z.null(),
                    ]),
                  })
                  .describe(
                    "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                  ),
                z.null(),
              ])
              .describe(
                "The tool orientation in grasp terms, against the target's when there is one.",
              ),
            orientation_error_radians: z.union([z.number(), z.null()]),
            position: z
              .union([
                z
                  .array(z.number())
                  .min(3)
                  .max(3)
                  .describe(
                    "The tool pose at `joints`; absent when the controller has no tool link.",
                  ),
                z
                  .null()
                  .describe(
                    "The tool pose at `joints`; absent when the controller has no tool link.",
                  ),
              ])
              .describe(
                "The tool pose at `joints`; absent when the controller has no tool link.",
              ),
            position_error_meters: z.union([z.number(), z.null()]),
            quaternion: z.union([z.array(z.number()).min(4).max(4), z.null()]),
            sampled_at: z.number(),
          })
          .describe("Where the arm was, measured, when a command ended."),
        z.null(),
      ]),
      checkpoints: z.array(
        z
          .object({
            label: z.string(),
            measured_joints: z.array(z.number()),
            target_position: z.union([
              z.array(z.number()).min(3).max(3),
              z.null(),
            ]),
            target_quaternion: z.union([
              z.array(z.number()).min(4).max(4),
              z.null(),
            ]),
          })
          .describe(
            "Where the arm stood, as measured, when it settled at the end of a segment.",
          ),
      ),
      command_id: z.string(),
      completion: z.union([
        z
          .object({
            detail: z.union([z.string(), z.null()]),
            finished_at: z.number(),
            gripper: z
              .union([
                z
                  .union([
                    z
                      .object({
                        kind: z.literal("reached"),
                        measured_angle_radians: z.number(),
                      })
                      .describe(
                        "The jaw arrived at the angle it was asked for.",
                      ),
                    z
                      .object({
                        kind: z.literal("stopped_short"),
                        measured_angle_radians: z.number(),
                        target_angle_radians: z.number(),
                      })
                      .describe(
                        "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                      ),
                  ])
                  .describe(
                    "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                  ),
                z.null(),
              ])
              .describe(
                "How a gripper command ended; null for every other command.",
              ),
            outcome: z
              .enum([
                "completed",
                "stopped",
                "tripped",
                "timed_out",
                "stopped_short",
                "superseded",
                "faulted",
                "interrupted",
                "failed",
              ])
              .describe("How a command ended."),
          })
          .describe(
            "The outcome and timestamp are recorded together when a command ends.",
          ),
        z.null(),
      ]),
      goal_joints: z
        .union([
          z
            .array(z.number())
            .describe(
              "The newest goal of a joint stream, after clamping to the joint limits.",
            ),
          z
            .null()
            .describe(
              "The newest goal of a joint stream, after clamping to the joint limits.",
            ),
        ])
        .describe(
          "The newest goal of a joint stream, after clamping to the joint limits.",
        ),
      group_id: z
        .union([
          z
            .string()
            .describe(
              "The grouped motion this command is one arm's part of, if it is.",
            ),
          z
            .null()
            .describe(
              "The grouped motion this command is one arm's part of, if it is.",
            ),
        ])
        .describe(
          "The grouped motion this command is one arm's part of, if it is.",
        ),
      joint_speed: z
        .union([
          z
            .number()
            .describe("Speed actually used, after clamping, for motions."),
          z
            .null()
            .describe("Speed actually used, after clamping, for motions."),
        ])
        .describe("Speed actually used, after clamping, for motions."),
      label: z
        .string()
        .describe(
          "What the command is: `enable`, `stream_joints`, or the label a move carried.",
        ),
      path_limited: z.union([
        z
          .object({ reason: z.string() })
          .describe(
            "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
          ),
        z.null(),
      ]),
      pose_limited: z.union([
        z
          .object({
            advice: z
              .string()
              .describe(
                "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
              ),
            orientation_shortfall_radians: z
              .number()
              .describe(
                "From the closest pose's orientation to the requested one.",
              ),
            position_shortfall_meters: z
              .number()
              .describe(
                "From the closest pose's tool position to the requested one.",
              ),
            reason: z
              .union([
                z
                  .literal("at_joint_limit")
                  .describe(
                    "The best pose it found has a joint at its limit, and no step improves it.",
                  ),
                z
                  .literal("stalled")
                  .describe(
                    "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                  ),
                z
                  .literal("iteration_limit")
                  .describe(
                    "It was still improving when it ran out of iterations.",
                  ),
                z
                  .literal("out_of_reach")
                  .describe(
                    "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                  ),
              ])
              .describe("Why a solve stopped short."),
            waypoint: z
              .number()
              .int()
              .gte(0)
              .describe(
                "The waypoint the move stops at, counted from 0; later waypoints are left out.",
              ),
          })
          .describe(
            "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
          ),
        z.null(),
      ]),
      target_joints: z.union([z.array(z.number()), z.null()]),
      target_position: z.union([z.array(z.number()).min(3).max(3), z.null()]),
      target_quaternion: z.union([z.array(z.number()).min(4).max(4), z.null()]),
      target_status: z.union([
        z
          .enum(["reached", "stopped_short", "partial", "not_started"])
          .describe("Whether the arm ended where the command was aimed."),
        z.null(),
      ]),
      trace_context: z
        .union([
          z
            .object({
              traceparent: z
                .string()
                .describe("`00-<trace id>-<parent span id>-<flags>`."),
              tracestate: z.union([z.string(), z.null()]).default(null),
            })
            .describe(
              "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
            ),
          z.null(),
        ])
        .describe("The trace context the command arrived with, if any."),
    })
    .strict()
    .describe("One command, from acceptance to its end."),
  CommandSubmissionRequest: z
    .object({
      arm: z.string(),
      command: z
        .union([
          z
            .object({ kind: z.literal("enable") })
            .strict()
            .describe(
              "Preflight, then power the motors holding their present pose.",
            ),
          z
            .object({ kind: z.literal("approve_recovery") })
            .strict()
            .describe(
              "Approve recovery after inspecting and supporting the arm.",
            ),
          z
            .object({ kind: z.literal("disable") })
            .strict()
            .describe(
              "Turn torque off where the arm is; an unsupported arm will fall.",
            ),
          z
            .object({ kind: z.literal("stop") })
            .strict()
            .describe(
              "End the active motion with outcome stopped and hold where the arm is.\nAnyone may send it, whoever holds the lease.",
            ),
          z
            .object({ kind: z.literal("clear_fault") })
            .strict()
            .describe("Acknowledge a fault; the arm is already disabled."),
          z
            .object({
              joints: z.array(z.number()),
              kind: z.literal("move_to_joints"),
              max_joint_speed: z
                .union([
                  z
                    .number()
                    .describe(
                      "Peak joint speed in rad/s; clamped to the profile's maximum.",
                    ),
                  z
                    .null()
                    .describe(
                      "Peak joint speed in rad/s; clamped to the profile's maximum.",
                    ),
                ])
                .describe(
                  "Peak joint speed in rad/s; clamped to the profile's maximum.",
                )
                .default(null),
            })
            .strict()
            .describe(
              "Move to joint angles, one per joint in the state's order.",
            ),
          z
            .object({
              kind: z.literal("move_to_pose"),
              max_joint_speed: z.union([z.number(), z.null()]).default(null),
              orientation: z
                .union([
                  z
                    .object({
                      approach: z
                        .union([
                          z
                            .union([
                              z.literal("down").describe("World -Z."),
                              z.literal("forward").describe("World +X."),
                            ])
                            .describe("A named approach direction."),
                          z.array(z.number()).min(3).max(3),
                        ])
                        .describe(
                          "The direction the jaws point along: a named one, or a nonzero world direction.",
                        ),
                      jaw_yaw_degrees: z.number(),
                      kind: z.literal("grasp"),
                    })
                    .describe(
                      "The approach direction and the jaw yaw about it, in degrees.",
                    ),
                  z
                    .object({
                      kind: z.literal("quaternion"),
                      quaternion: z.array(z.number()).min(4).max(4),
                    })
                    .describe("A raw quaternion, `[x, y, z, w]`."),
                ])
                .describe("A tool orientation as a client gives it."),
              position: z.array(z.number()).min(3).max(3),
            })
            .strict()
            .describe("Move the tool to a world position and orientation."),
          z
            .object({
              kind: z.literal("follow_path"),
              max_joint_speed: z.union([z.number(), z.null()]).default(null),
              waypoints: z.array(
                z
                  .union([
                    z.object({
                      joints: z.array(z.number()),
                      kind: z.literal("joints"),
                    }),
                    z.object({
                      kind: z.literal("pose"),
                      orientation: z
                        .union([
                          z
                            .object({
                              approach: z
                                .union([
                                  z
                                    .union([
                                      z.literal("down").describe("World -Z."),
                                      z
                                        .literal("forward")
                                        .describe("World +X."),
                                    ])
                                    .describe("A named approach direction."),
                                  z.array(z.number()).min(3).max(3),
                                ])
                                .describe(
                                  "The direction the jaws point along: a named one, or a nonzero world direction.",
                                ),
                              jaw_yaw_degrees: z.number(),
                              kind: z.literal("grasp"),
                            })
                            .describe(
                              "The approach direction and the jaw yaw about it, in degrees.",
                            ),
                          z
                            .object({
                              kind: z.literal("quaternion"),
                              quaternion: z.array(z.number()).min(4).max(4),
                            })
                            .describe("A raw quaternion, `[x, y, z, w]`."),
                        ])
                        .describe("A tool orientation as a client gives it."),
                      position: z.array(z.number()).min(3).max(3),
                    }),
                  ])
                  .describe(
                    "One corner of a move: joints, or a tool pose solved for joints.",
                  ),
              ),
            })
            .strict()
            .describe(
              "Waypoints are corners: the arm comes to rest and settles at each one.",
            ),
          z
            .object({
              gripper_opening: z
                .union([
                  z
                    .number()
                    .describe(
                      "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                    ),
                  z
                    .null()
                    .describe(
                      "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                    ),
                ])
                .describe(
                  "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                )
                .default(null),
              joints: z.array(z.number()),
              kind: z.literal("stream_joints"),
              max_joint_speed: z.union([z.number(), z.null()]).default(null),
              stream_command_id: z.union([z.string(), z.null()]).default(null),
            })
            .strict()
            .describe(
              "Stream trajectory destinations. The first goal, without\n`stream_command_id`, starts a stream when the arm is holding; later goals\ncarry the command id it returned. Each cycle the arm moves toward the\nnewest goal at up to the first goal's speed. The stream ends stopped when\ngoals stop arriving, on stop, or on a trip.",
            ),
          z
            .object({
              kind: z.literal("set_gripper"),
              max_joint_speed: z
                .union([
                  z
                    .number()
                    .describe(
                      "Peak gripper speed in rad/s; uses the arm default and maximum.",
                    ),
                  z
                    .null()
                    .describe(
                      "Peak gripper speed in rad/s; uses the arm default and maximum.",
                    ),
                ])
                .describe(
                  "Peak gripper speed in rad/s; uses the arm default and maximum.",
                )
                .default(null),
              opening: z.number(),
            })
            .strict()
            .describe(
              "Open or close the gripper, 0 closed to 1 open, whether the arm is holding\nor moving. It completes on arrival or on contact.",
            ),
        ])
        .describe("A command for one arm."),
      expected_revision: z
        .union([z.number().int().gte(0), z.null()])
        .default(null),
      lease_token: z
        .union([
          z
            .string()
            .describe(
              "The active lease token, when one is required for this command.",
            ),
          z
            .null()
            .describe(
              "The active lease token, when one is required for this command.",
            ),
        ])
        .describe(
          "The active lease token, when one is required for this command.",
        )
        .default(null),
      request_id: z
        .string()
        .describe("A caller-generated UUID v4 identifying this submission."),
      station_instance_id: z
        .string()
        .describe(
          "The server instance the caller observed when creating this request.",
        ),
      trace_context: z
        .union([
          z
            .object({
              traceparent: z
                .string()
                .describe("`00-<trace id>-<parent span id>-<flags>`."),
              tracestate: z.union([z.string(), z.null()]).default(null),
            })
            .describe(
              "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
            ),
          z.null(),
        ])
        .default(null),
    })
    .strict()
    .describe(
      "Submit one arm command with a client-generated request identifier.\nThe station uses the identifier to recognize retries of the same submission.",
    ),
  CompilationArtifact: z
    .object({
      compilation_id: z.string(),
      compiled: z.object({
        arms: z.array(z.string()),
        reused_operations: z.number().int().gte(0),
        steps: z.array(
          z.record(
            z.string(),
            z
              .object({
                action: z.union([
                  z
                    .object({
                      gripper_opening: z
                        .union([z.number(), z.null()])
                        .optional(),
                      kind: z.literal("move"),
                      max_joint_speed: z
                        .union([z.number(), z.null()])
                        .optional(),
                      movement: z.union([
                        z
                          .object({
                            joints: z.array(z.number()),
                            kind: z.literal("joints"),
                          })
                          .strict(),
                        z
                          .object({
                            kind: z.literal("pose"),
                            orientation: z
                              .union([
                                z
                                  .object({
                                    approach: z
                                      .union([
                                        z
                                          .union([
                                            z
                                              .literal("down")
                                              .describe("World -Z."),
                                            z
                                              .literal("forward")
                                              .describe("World +X."),
                                          ])
                                          .describe(
                                            "A named approach direction.",
                                          ),
                                        z.array(z.number()).min(3).max(3),
                                      ])
                                      .describe(
                                        "The direction the jaws point along: a named one, or a nonzero world direction.",
                                      ),
                                    jaw_yaw_degrees: z.number(),
                                    kind: z.literal("grasp"),
                                  })
                                  .describe(
                                    "The approach direction and the jaw yaw about it, in degrees.",
                                  ),
                                z
                                  .object({
                                    kind: z.literal("quaternion"),
                                    quaternion: z
                                      .array(z.number())
                                      .min(4)
                                      .max(4),
                                  })
                                  .describe(
                                    "A raw quaternion, `[x, y, z, w]`.",
                                  ),
                              ])
                              .describe(
                                "A tool orientation as a client gives it.",
                              ),
                            position: z.array(z.number()).min(3).max(3),
                          })
                          .strict(),
                      ]),
                    })
                    .strict(),
                  z
                    .object({
                      kind: z.literal("gripper"),
                      max_joint_speed: z
                        .union([z.number(), z.null()])
                        .optional(),
                      opening: z.number(),
                    })
                    .strict(),
                ]),
                source_operation_id: z.string(),
              })
              .describe(
                "A source identity remains stable while this step, arm and requested action are unchanged.",
              ),
          ),
        ),
      }),
      compiler_version: z.string(),
      diagnostics: z.array(
        z
          .object({
            arm: z.union([z.string(), z.null()]).optional(),
            detail: z.string(),
            step: z.union([z.number().int().gte(0), z.null()]).optional(),
          })
          .describe("A structural diagnostic located in the submitted source."),
      ),
      schema_version: z.number().int().gte(0),
      source: z
        .object({
          expected_revisions: z
            .record(z.string(), z.number().int().gte(0))
            .describe(
              "Revisions fence every participating arm throughout the sequence.",
            )
            .default({}),
          steps: z.array(
            z
              .object({
                arms: z.record(
                  z.string(),
                  z
                    .object({
                      gripper_opening: z
                        .union([z.number(), z.null()])
                        .default(null),
                      max_joint_speed: z
                        .union([
                          z
                            .number()
                            .describe(
                              "Speed for both resources in rad/s; overrides the movement speed when provided.",
                            ),
                          z
                            .null()
                            .describe(
                              "Speed for both resources in rad/s; overrides the movement speed when provided.",
                            ),
                        ])
                        .describe(
                          "Speed for both resources in rad/s; overrides the movement speed when provided.",
                        )
                        .default(null),
                      movement: z
                        .union([
                          z
                            .union([
                              z
                                .object({
                                  joints: z.array(z.number()),
                                  kind: z.literal("joints"),
                                  max_joint_speed: z
                                    .union([z.number(), z.null()])
                                    .default(null),
                                })
                                .strict(),
                              z
                                .object({
                                  kind: z.literal("pose"),
                                  max_joint_speed: z
                                    .union([z.number(), z.null()])
                                    .default(null),
                                  orientation: z
                                    .union([
                                      z
                                        .object({
                                          approach: z
                                            .union([
                                              z
                                                .union([
                                                  z
                                                    .literal("down")
                                                    .describe("World -Z."),
                                                  z
                                                    .literal("forward")
                                                    .describe("World +X."),
                                                ])
                                                .describe(
                                                  "A named approach direction.",
                                                ),
                                              z.array(z.number()).min(3).max(3),
                                            ])
                                            .describe(
                                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                                            ),
                                          jaw_yaw_degrees: z.number(),
                                          kind: z.literal("grasp"),
                                        })
                                        .describe(
                                          "The approach direction and the jaw yaw about it, in degrees.",
                                        ),
                                      z
                                        .object({
                                          kind: z.literal("quaternion"),
                                          quaternion: z
                                            .array(z.number())
                                            .min(4)
                                            .max(4),
                                        })
                                        .describe(
                                          "A raw quaternion, `[x, y, z, w]`.",
                                        ),
                                    ])
                                    .describe(
                                      "A tool orientation as a client gives it.",
                                    ),
                                  position: z.array(z.number()).min(3).max(3),
                                })
                                .strict(),
                            ])
                            .describe(
                              "One movement in an action sequence. The arm settles at each target.",
                            ),
                          z.null(),
                        ])
                        .default(null),
                    })
                    .strict()
                    .describe(
                      "A movement and gripper target in the same step start together.",
                    ),
                ),
              })
              .strict()
              .describe(
                "One synchronized step; a single-arm action is a map with one entry.",
              ),
          ),
        })
        .strict()
        .describe(
          "An ordered set of steps to execute under the caller's lease.",
        ),
      station_instance_id: z.string(),
      structural_compilation_seconds: z.number(),
      targets: z.record(
        z.string(),
        z.object({
          configuration_identity: z.string(),
          has_gripper: z.boolean(),
          jaw_closing_axis: z
            .enum(["x", "y"])
            .describe("The tool-frame axis the gripper's jaws close along."),
          joint_count: z.number().int().gte(0),
          supports_joints: z.boolean(),
          supports_pose: z.boolean(),
        }),
      ),
    })
    .describe(
      "Structural operations still require preparation from live measured state.",
    ),
  CompilationDiagnostic: z
    .object({
      arm: z.union([z.string(), z.null()]).optional(),
      detail: z.string(),
      step: z.union([z.number().int().gte(0), z.null()]).optional(),
    })
    .describe("A structural diagnostic located in the submitted source."),
  CompilationOutcome: z
    .union([
      z.object({
        artifact: z
          .object({
            compilation_id: z.string(),
            compiled: z.object({
              arms: z.array(z.string()),
              reused_operations: z.number().int().gte(0),
              steps: z.array(
                z.record(
                  z.string(),
                  z
                    .object({
                      action: z.union([
                        z
                          .object({
                            gripper_opening: z
                              .union([z.number(), z.null()])
                              .optional(),
                            kind: z.literal("move"),
                            max_joint_speed: z
                              .union([z.number(), z.null()])
                              .optional(),
                            movement: z.union([
                              z
                                .object({
                                  joints: z.array(z.number()),
                                  kind: z.literal("joints"),
                                })
                                .strict(),
                              z
                                .object({
                                  kind: z.literal("pose"),
                                  orientation: z
                                    .union([
                                      z
                                        .object({
                                          approach: z
                                            .union([
                                              z
                                                .union([
                                                  z
                                                    .literal("down")
                                                    .describe("World -Z."),
                                                  z
                                                    .literal("forward")
                                                    .describe("World +X."),
                                                ])
                                                .describe(
                                                  "A named approach direction.",
                                                ),
                                              z.array(z.number()).min(3).max(3),
                                            ])
                                            .describe(
                                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                                            ),
                                          jaw_yaw_degrees: z.number(),
                                          kind: z.literal("grasp"),
                                        })
                                        .describe(
                                          "The approach direction and the jaw yaw about it, in degrees.",
                                        ),
                                      z
                                        .object({
                                          kind: z.literal("quaternion"),
                                          quaternion: z
                                            .array(z.number())
                                            .min(4)
                                            .max(4),
                                        })
                                        .describe(
                                          "A raw quaternion, `[x, y, z, w]`.",
                                        ),
                                    ])
                                    .describe(
                                      "A tool orientation as a client gives it.",
                                    ),
                                  position: z.array(z.number()).min(3).max(3),
                                })
                                .strict(),
                            ]),
                          })
                          .strict(),
                        z
                          .object({
                            kind: z.literal("gripper"),
                            max_joint_speed: z
                              .union([z.number(), z.null()])
                              .optional(),
                            opening: z.number(),
                          })
                          .strict(),
                      ]),
                      source_operation_id: z.string(),
                    })
                    .describe(
                      "A source identity remains stable while this step, arm and requested action are unchanged.",
                    ),
                ),
              ),
            }),
            compiler_version: z.string(),
            diagnostics: z.array(
              z
                .object({
                  arm: z.union([z.string(), z.null()]).optional(),
                  detail: z.string(),
                  step: z.union([z.number().int().gte(0), z.null()]).optional(),
                })
                .describe(
                  "A structural diagnostic located in the submitted source.",
                ),
            ),
            schema_version: z.number().int().gte(0),
            source: z
              .object({
                expected_revisions: z
                  .record(z.string(), z.number().int().gte(0))
                  .describe(
                    "Revisions fence every participating arm throughout the sequence.",
                  )
                  .default({}),
                steps: z.array(
                  z
                    .object({
                      arms: z.record(
                        z.string(),
                        z
                          .object({
                            gripper_opening: z
                              .union([z.number(), z.null()])
                              .default(null),
                            max_joint_speed: z
                              .union([
                                z
                                  .number()
                                  .describe(
                                    "Speed for both resources in rad/s; overrides the movement speed when provided.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Speed for both resources in rad/s; overrides the movement speed when provided.",
                                  ),
                              ])
                              .describe(
                                "Speed for both resources in rad/s; overrides the movement speed when provided.",
                              )
                              .default(null),
                            movement: z
                              .union([
                                z
                                  .union([
                                    z
                                      .object({
                                        joints: z.array(z.number()),
                                        kind: z.literal("joints"),
                                        max_joint_speed: z
                                          .union([z.number(), z.null()])
                                          .default(null),
                                      })
                                      .strict(),
                                    z
                                      .object({
                                        kind: z.literal("pose"),
                                        max_joint_speed: z
                                          .union([z.number(), z.null()])
                                          .default(null),
                                        orientation: z
                                          .union([
                                            z
                                              .object({
                                                approach: z
                                                  .union([
                                                    z
                                                      .union([
                                                        z
                                                          .literal("down")
                                                          .describe(
                                                            "World -Z.",
                                                          ),
                                                        z
                                                          .literal("forward")
                                                          .describe(
                                                            "World +X.",
                                                          ),
                                                      ])
                                                      .describe(
                                                        "A named approach direction.",
                                                      ),
                                                    z
                                                      .array(z.number())
                                                      .min(3)
                                                      .max(3),
                                                  ])
                                                  .describe(
                                                    "The direction the jaws point along: a named one, or a nonzero world direction.",
                                                  ),
                                                jaw_yaw_degrees: z.number(),
                                                kind: z.literal("grasp"),
                                              })
                                              .describe(
                                                "The approach direction and the jaw yaw about it, in degrees.",
                                              ),
                                            z
                                              .object({
                                                kind: z.literal("quaternion"),
                                                quaternion: z
                                                  .array(z.number())
                                                  .min(4)
                                                  .max(4),
                                              })
                                              .describe(
                                                "A raw quaternion, `[x, y, z, w]`.",
                                              ),
                                          ])
                                          .describe(
                                            "A tool orientation as a client gives it.",
                                          ),
                                        position: z
                                          .array(z.number())
                                          .min(3)
                                          .max(3),
                                      })
                                      .strict(),
                                  ])
                                  .describe(
                                    "One movement in an action sequence. The arm settles at each target.",
                                  ),
                                z.null(),
                              ])
                              .default(null),
                          })
                          .strict()
                          .describe(
                            "A movement and gripper target in the same step start together.",
                          ),
                      ),
                    })
                    .strict()
                    .describe(
                      "One synchronized step; a single-arm action is a map with one entry.",
                    ),
                ),
              })
              .strict()
              .describe(
                "An ordered set of steps to execute under the caller's lease.",
              ),
            station_instance_id: z.string(),
            structural_compilation_seconds: z.number(),
            targets: z.record(
              z.string(),
              z.object({
                configuration_identity: z.string(),
                has_gripper: z.boolean(),
                jaw_closing_axis: z
                  .enum(["x", "y"])
                  .describe(
                    "The tool-frame axis the gripper's jaws close along.",
                  ),
                joint_count: z.number().int().gte(0),
                supports_joints: z.boolean(),
                supports_pose: z.boolean(),
              }),
            ),
          })
          .describe(
            "Structural operations still require preparation from live measured state.",
          ),
        kind: z.literal("compiled"),
      }),
      z.object({
        diagnostics: z.array(
          z
            .object({
              arm: z.union([z.string(), z.null()]).optional(),
              detail: z.string(),
              step: z.union([z.number().int().gte(0), z.null()]).optional(),
            })
            .describe(
              "A structural diagnostic located in the submitted source.",
            ),
        ),
        kind: z.literal("rejected"),
      }),
    ])
    .describe("Compilation does not acquire a lease or command hardware."),
  CompilationTarget: z.object({
    configuration_identity: z.string(),
    has_gripper: z.boolean(),
    jaw_closing_axis: z
      .enum(["x", "y"])
      .describe("The tool-frame axis the gripper's jaws close along."),
    joint_count: z.number().int().gte(0),
    supports_joints: z.boolean(),
    supports_pose: z.boolean(),
  }),
  CompiledArmAction: z.union([
    z
      .object({
        gripper_opening: z.union([z.number(), z.null()]).optional(),
        kind: z.literal("move"),
        max_joint_speed: z.union([z.number(), z.null()]).optional(),
        movement: z.union([
          z
            .object({ joints: z.array(z.number()), kind: z.literal("joints") })
            .strict(),
          z
            .object({
              kind: z.literal("pose"),
              orientation: z
                .union([
                  z
                    .object({
                      approach: z
                        .union([
                          z
                            .union([
                              z.literal("down").describe("World -Z."),
                              z.literal("forward").describe("World +X."),
                            ])
                            .describe("A named approach direction."),
                          z.array(z.number()).min(3).max(3),
                        ])
                        .describe(
                          "The direction the jaws point along: a named one, or a nonzero world direction.",
                        ),
                      jaw_yaw_degrees: z.number(),
                      kind: z.literal("grasp"),
                    })
                    .describe(
                      "The approach direction and the jaw yaw about it, in degrees.",
                    ),
                  z
                    .object({
                      kind: z.literal("quaternion"),
                      quaternion: z.array(z.number()).min(4).max(4),
                    })
                    .describe("A raw quaternion, `[x, y, z, w]`."),
                ])
                .describe("A tool orientation as a client gives it."),
              position: z.array(z.number()).min(3).max(3),
            })
            .strict(),
        ]),
      })
      .strict(),
    z
      .object({
        kind: z.literal("gripper"),
        max_joint_speed: z.union([z.number(), z.null()]).optional(),
        opening: z.number(),
      })
      .strict(),
  ]),
  CompiledMovement: z.union([
    z
      .object({ joints: z.array(z.number()), kind: z.literal("joints") })
      .strict(),
    z
      .object({
        kind: z.literal("pose"),
        orientation: z
          .union([
            z
              .object({
                approach: z
                  .union([
                    z
                      .union([
                        z.literal("down").describe("World -Z."),
                        z.literal("forward").describe("World +X."),
                      ])
                      .describe("A named approach direction."),
                    z.array(z.number()).min(3).max(3),
                  ])
                  .describe(
                    "The direction the jaws point along: a named one, or a nonzero world direction.",
                  ),
                jaw_yaw_degrees: z.number(),
                kind: z.literal("grasp"),
              })
              .describe(
                "The approach direction and the jaw yaw about it, in degrees.",
              ),
            z
              .object({
                kind: z.literal("quaternion"),
                quaternion: z.array(z.number()).min(4).max(4),
              })
              .describe("A raw quaternion, `[x, y, z, w]`."),
          ])
          .describe("A tool orientation as a client gives it."),
        position: z.array(z.number()).min(3).max(3),
      })
      .strict(),
  ]),
  CompiledOperation: z
    .object({
      action: z.union([
        z
          .object({
            gripper_opening: z.union([z.number(), z.null()]).optional(),
            kind: z.literal("move"),
            max_joint_speed: z.union([z.number(), z.null()]).optional(),
            movement: z.union([
              z
                .object({
                  joints: z.array(z.number()),
                  kind: z.literal("joints"),
                })
                .strict(),
              z
                .object({
                  kind: z.literal("pose"),
                  orientation: z
                    .union([
                      z
                        .object({
                          approach: z
                            .union([
                              z
                                .union([
                                  z.literal("down").describe("World -Z."),
                                  z.literal("forward").describe("World +X."),
                                ])
                                .describe("A named approach direction."),
                              z.array(z.number()).min(3).max(3),
                            ])
                            .describe(
                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                            ),
                          jaw_yaw_degrees: z.number(),
                          kind: z.literal("grasp"),
                        })
                        .describe(
                          "The approach direction and the jaw yaw about it, in degrees.",
                        ),
                      z
                        .object({
                          kind: z.literal("quaternion"),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("A raw quaternion, `[x, y, z, w]`."),
                    ])
                    .describe("A tool orientation as a client gives it."),
                  position: z.array(z.number()).min(3).max(3),
                })
                .strict(),
            ]),
          })
          .strict(),
        z
          .object({
            kind: z.literal("gripper"),
            max_joint_speed: z.union([z.number(), z.null()]).optional(),
            opening: z.number(),
          })
          .strict(),
      ]),
      source_operation_id: z.string(),
    })
    .describe(
      "A source identity remains stable while this step, arm and requested action are unchanged.",
    ),
  CompiledProgram: z.object({
    arms: z.array(z.string()),
    reused_operations: z.number().int().gte(0),
    steps: z.array(
      z.record(
        z.string(),
        z
          .object({
            action: z.union([
              z
                .object({
                  gripper_opening: z.union([z.number(), z.null()]).optional(),
                  kind: z.literal("move"),
                  max_joint_speed: z.union([z.number(), z.null()]).optional(),
                  movement: z.union([
                    z
                      .object({
                        joints: z.array(z.number()),
                        kind: z.literal("joints"),
                      })
                      .strict(),
                    z
                      .object({
                        kind: z.literal("pose"),
                        orientation: z
                          .union([
                            z
                              .object({
                                approach: z
                                  .union([
                                    z
                                      .union([
                                        z.literal("down").describe("World -Z."),
                                        z
                                          .literal("forward")
                                          .describe("World +X."),
                                      ])
                                      .describe("A named approach direction."),
                                    z.array(z.number()).min(3).max(3),
                                  ])
                                  .describe(
                                    "The direction the jaws point along: a named one, or a nonzero world direction.",
                                  ),
                                jaw_yaw_degrees: z.number(),
                                kind: z.literal("grasp"),
                              })
                              .describe(
                                "The approach direction and the jaw yaw about it, in degrees.",
                              ),
                            z
                              .object({
                                kind: z.literal("quaternion"),
                                quaternion: z.array(z.number()).min(4).max(4),
                              })
                              .describe("A raw quaternion, `[x, y, z, w]`."),
                          ])
                          .describe("A tool orientation as a client gives it."),
                        position: z.array(z.number()).min(3).max(3),
                      })
                      .strict(),
                  ]),
                })
                .strict(),
              z
                .object({
                  kind: z.literal("gripper"),
                  max_joint_speed: z.union([z.number(), z.null()]).optional(),
                  opening: z.number(),
                })
                .strict(),
            ]),
            source_operation_id: z.string(),
          })
          .describe(
            "A source identity remains stable while this step, arm and requested action are unchanged.",
          ),
      ),
    ),
  }),
  ControllerKind: z.enum(["ik", "policy"]),
  ControllerStatus: z.union([
    z.object({ phase: z.literal("unmanaged") }),
    z.object({ phase: z.literal("starting") }),
    z.object({ phase: z.literal("online") }),
    z.object({ phase: z.literal("stopping") }),
    z.object({
      guard_released: z.boolean(),
      phase: z.literal("unavailable"),
      reason: z.string(),
      reports: z.array(z.string()),
    }),
    z.object({
      phase: z.literal("waiting_for_guard"),
      reason: z.string(),
      reports: z.array(z.string()),
    }),
  ]),
  CornerPlan: z.object({
    peak_speed: z.number(),
    segments: z.array(
      z
        .object({
          corners: z.array(z.array(z.number())),
          label: z.string(),
          minimum_duration_seconds: z
            .union([
              z
                .number()
                .describe(
                  "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                ),
              z
                .null()
                .describe(
                  "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                ),
            ])
            .describe(
              "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
            )
            .default(null),
          target_position: z
            .union([z.array(z.number()).min(3).max(3), z.null()])
            .default(null),
          target_quaternion: z
            .union([z.array(z.number()).min(4).max(4), z.null()])
            .default(null),
        })
        .describe(
          "One straight run between corners, with where it is meant to end.",
        ),
    ),
  }),
  Event: z
    .union([
      z
        .object({
          active_command: z
            .union([
              z
                .string()
                .describe("The command being carried out, when one is."),
              z.null().describe("The command being carried out, when one is."),
            ])
            .describe("The command being carried out, when one is.")
            .optional(),
          active_trip: z
            .union([
              z
                .string()
                .describe(
                  "The protective check currently preventing new motion, if any.",
                ),
              z
                .null()
                .describe(
                  "The protective check currently preventing new motion, if any.",
                ),
            ])
            .describe(
              "The protective check currently preventing new motion, if any.",
            )
            .default(null),
          commanded_positions: z
            .union([
              z
                .array(z.number())
                .describe(
                  "Joint setpoint successfully sent this cycle, if any.",
                ),
              z
                .null()
                .describe(
                  "Joint setpoint successfully sent this cycle, if any.",
                ),
            ])
            .describe("Joint setpoint successfully sent this cycle, if any.")
            .optional(),
          cycle: z.number().int().gte(0),
          cycle_seconds: z.number(),
          efforts: z
            .array(z.number())
            .describe(
              "Each arm joint's load, in percent of the servo's rated load.",
            ),
          fault: z
            .union([
              z
                .string()
                .describe(
                  "A fault the hardware reports this cycle, tripped or not.",
                ),
              z
                .null()
                .describe(
                  "A fault the hardware reports this cycle, tripped or not.",
                ),
            ])
            .describe(
              "A fault the hardware reports this cycle, tripped or not.",
            )
            .optional(),
          gripper_angle: z.union([z.number(), z.null()]).optional(),
          hold_pose: z
            .union([
              z
                .array(z.number())
                .describe(
                  "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                ),
              z
                .null()
                .describe(
                  "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                ),
            ])
            .describe(
              "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
            )
            .optional(),
          mode: z
            .union([
              z
                .literal("disabled")
                .describe("Torque off; only `enable` is accepted."),
              z
                .literal("holding")
                .describe("Torque on, holding between commands."),
              z.literal("moving").describe("Following a motion or a stream."),
              z
                .literal("faulted")
                .describe(
                  "Powered off after a hard fault; refuses everything until `clear_fault`.",
                ),
              z
                .literal("disconnected")
                .describe(
                  "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                ),
              z
                .literal("reconnected")
                .describe(
                  "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                ),
            ])
            .describe("Where the arm stands."),
          positions: z.array(z.number()),
          recovery_issue: z
            .union([
              z
                .object({
                  detail: z.string(),
                  motors_without_torque: z.array(
                    z.number().int().gte(0).lte(255),
                  ),
                })
                .describe(
                  "Why automatic recovery stopped, including motors found without torque.",
                ),
              z.null(),
            ])
            .optional(),
          rest_pose: z
            .union([
              z
                .array(z.number())
                .describe(
                  "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                ),
              z
                .null()
                .describe(
                  "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                ),
            ])
            .describe(
              "Where the arm rested unpowered when it was enabled; shutdown returns there.",
            )
            .optional(),
          segment: z
            .union([
              z
                .string()
                .describe("The segment of the active motion being followed."),
              z
                .null()
                .describe("The segment of the active motion being followed."),
            ])
            .describe("The segment of the active motion being followed.")
            .optional(),
          temperatures_celsius: z
            .array(z.number())
            .describe(
              "The last reading of each driven motor, the gripper's last; empty in\nsimulation. Sent while tripped too, so a holding arm that heats shows.",
            ),
          velocities: z.array(z.number()),
          voltages_volts: z
            .array(z.number())
            .describe(
              "The supply voltage each driven motor last reported, as the temperatures.",
            ),
          kind: z.literal("state"),
        })
        .describe(
          "One control cycle's state; a sample, so a slow client may miss some.",
        ),
      z
        .object({
          accepted: z.boolean(),
          description: z
            .union([
              z
                .object({
                  model_points: z
                    .array(z.array(z.number()).min(3).max(3))
                    .describe(
                      "Each joint's origin, then the tool's: the points a viewer draws the arm through.",
                    ),
                  orientation: z
                    .object({
                      approach: z.array(z.number()).min(3).max(3),
                      degrees_from_down: z.number(),
                      jaw_yaw_degrees: z.number(),
                      jaw_yaw_reference: z
                        .enum(["world_x", "world_y"])
                        .describe(
                          "The world axis jaw yaw zero is projected from.",
                        ),
                      target: z
                        .union([
                          z
                            .object({
                              approach_error_degrees: z.number(),
                              jaw_yaw_degrees: z.number(),
                              jaw_yaw_reference: z
                                .enum(["world_x", "world_y"])
                                .describe(
                                  "The world axis jaw yaw zero is projected from.",
                                ),
                              orientation_error_degrees: z.number(),
                            })
                            .describe(
                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                    })
                    .describe(
                      "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                    ),
                  position: z.array(z.number()).min(3).max(3),
                  quaternion: z.array(z.number()).min(4).max(4),
                })
                .describe(
                  "The tool's pose at some joints, in the station's world frame.",
                ),
              z.null(),
            ])
            .describe("The answer to a `describe_pose`.")
            .optional(),
          detail: z.union([z.string(), z.null()]).optional(),
          ik: z
            .union([
              z
                .union([
                  z
                    .object({
                      joints: z.array(z.number()),
                      orientation_error_radians: z
                        .number()
                        .describe(
                          "From the required orientation, or from the seed's when it was free.",
                        ),
                      position_error_meters: z.number(),
                      tool_position: z.array(z.number()).min(3).max(3),
                      tool_quaternion: z.array(z.number()).min(4).max(4),
                      outcome: z.literal("solved"),
                    })
                    .describe("The tool is within tolerance of the target."),
                  z
                    .object({
                      best: z
                        .object({
                          joints: z.array(z.number()),
                          orientation_error_radians: z
                            .number()
                            .describe(
                              "From the required orientation, or from the seed's when it was free.",
                            ),
                          position_error_meters: z.number(),
                          tool_position: z.array(z.number()).min(3).max(3),
                          tool_quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("Where a solve left the arm."),
                      outcome: z.literal("failed"),
                      reason: z
                        .union([
                          z
                            .literal("at_joint_limit")
                            .describe(
                              "The best pose it found has a joint at its limit, and no step improves it.",
                            ),
                          z
                            .literal("stalled")
                            .describe(
                              "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                            ),
                          z
                            .literal("iteration_limit")
                            .describe(
                              "It was still improving when it ran out of iterations.",
                            ),
                          z
                            .literal("out_of_reach")
                            .describe(
                              "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                            ),
                        ])
                        .describe("Why a solve stopped short."),
                    })
                    .describe(
                      "It is not; `best` is the pose with the smallest position error found,\nalways inside the joint limits.",
                    ),
                ])
                .describe("A solve's outcome."),
              z.null(),
            ])
            .describe("How a `solve_pose` ended.")
            .optional(),
          kind: z.literal("reply"),
          lease: z
            .union([
              z
                .union([
                  z
                    .object({
                      lease_seconds: z.number(),
                      lease_token: z.string(),
                      status: z
                        .object({
                          expires_in_seconds: z
                            .number()
                            .describe(
                              "The lease lapses this long after its grant or last explicit renewal.",
                            ),
                          holder: z
                            .union([
                              z
                                .literal("agent")
                                .describe(
                                  "A model loop, started from the terminal or the agent server.",
                                ),
                              z
                                .literal("manual")
                                .describe(
                                  "A person driving the arms from the viewer's manual controls.",
                                ),
                            ])
                            .describe(
                              "The kind of controller holding the lease.",
                            ),
                          label: z.string(),
                          on_interruption: z
                            .union([
                              z
                                .literal("wait_for_approval")
                                .describe(
                                  "Keep the lease and wait for an operator's `approve_recovery`.",
                                ),
                              z
                                .literal("return_lease")
                                .describe("Give the lease up."),
                            ])
                            .describe(
                              "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                            ),
                          required_cameras: z
                            .array(z.string())
                            .describe(
                              "Cameras the holder cannot work without.",
                            ),
                        })
                        .describe("The current lease, as everyone may see it."),
                      kind: z.literal("granted"),
                    })
                    .describe("The lease is yours."),
                  z
                    .object({
                      kind: z.literal("holder"),
                      status: z
                        .union([
                          z
                            .object({
                              expires_in_seconds: z
                                .number()
                                .describe(
                                  "The lease lapses this long after its grant or last explicit renewal.",
                                ),
                              holder: z
                                .union([
                                  z
                                    .literal("agent")
                                    .describe(
                                      "A model loop, started from the terminal or the agent server.",
                                    ),
                                  z
                                    .literal("manual")
                                    .describe(
                                      "A person driving the arms from the viewer's manual controls.",
                                    ),
                                ])
                                .describe(
                                  "The kind of controller holding the lease.",
                                ),
                              label: z.string(),
                              on_interruption: z
                                .union([
                                  z
                                    .literal("wait_for_approval")
                                    .describe(
                                      "Keep the lease and wait for an operator's `approve_recovery`.",
                                    ),
                                  z
                                    .literal("return_lease")
                                    .describe("Give the lease up."),
                                ])
                                .describe(
                                  "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                                ),
                              required_cameras: z
                                .array(z.string())
                                .describe(
                                  "Cameras the holder cannot work without.",
                                ),
                            })
                            .describe(
                              "The current lease, as everyone may see it.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                    })
                    .describe("Who holds the lease, if anyone."),
                ])
                .describe("What a request about the lease answers."),
              z.null(),
            ])
            .describe("The answer to a request about the lease.")
            .optional(),
          plan: z
            .union([
              z
                .object({
                  path_limited: z
                    .union([
                      z
                        .object({ reason: z.string() })
                        .describe(
                          "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  plan: z.object({
                    peak_speed: z.number(),
                    segments: z.array(
                      z
                        .object({
                          corners: z.array(z.array(z.number())),
                          label: z.string(),
                          minimum_duration_seconds: z
                            .union([
                              z
                                .number()
                                .describe(
                                  "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                ),
                            ])
                            .describe(
                              "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                            )
                            .default(null),
                          target_position: z
                            .union([
                              z.array(z.number()).min(3).max(3),
                              z.null(),
                            ])
                            .default(null),
                          target_quaternion: z
                            .union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ])
                            .default(null),
                        })
                        .describe(
                          "One straight run between corners, with where it is meant to end.",
                        ),
                    ),
                  }),
                  pose_limited: z
                    .union([
                      z
                        .object({
                          advice: z
                            .string()
                            .describe(
                              "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                            ),
                          orientation_shortfall_radians: z
                            .number()
                            .describe(
                              "From the closest pose's orientation to the requested one.",
                            ),
                          position_shortfall_meters: z
                            .number()
                            .describe(
                              "From the closest pose's tool position to the requested one.",
                            ),
                          reason: z
                            .union([
                              z
                                .literal("at_joint_limit")
                                .describe(
                                  "The best pose it found has a joint at its limit, and no step improves it.",
                                ),
                              z
                                .literal("stalled")
                                .describe(
                                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                ),
                              z
                                .literal("iteration_limit")
                                .describe(
                                  "It was still improving when it ran out of iterations.",
                                ),
                              z
                                .literal("out_of_reach")
                                .describe(
                                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                ),
                            ])
                            .describe("Why a solve stopped short."),
                          waypoint: z
                            .number()
                            .int()
                            .gte(0)
                            .describe(
                              "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                            ),
                        })
                        .describe(
                          "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                })
                .describe(
                  "What the arm is asked to run, and why it stops short of the target if it does.",
                ),
              z.null(),
            ])
            .describe("The plan a `plan_motion` made.")
            .optional(),
          projection: z
            .union([
              z
                .object({
                  calculation_milliseconds: z.number(),
                  jaw_yaw_ranges: z
                    .array(
                      z
                        .object({
                          from_degrees: z.number(),
                          reachable: z.boolean(),
                          reason: z.union([z.string(), z.null()]).optional(),
                          to_degrees: z.number(),
                        })
                        .describe(
                          "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
                        ),
                    )
                    .describe(
                      "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
                    ),
                  outcome: z
                    .union([
                      z
                        .object({
                          kind: z.literal("projected"),
                          pose: z
                            .object({
                              joints: z.array(z.number()),
                              orientation: z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  jaw_yaw_degrees: z.number(),
                                })
                                .describe(
                                  "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                                ),
                              orientation_error_radians: z
                                .union([
                                  z
                                    .number()
                                    .describe(
                                      "From the requested orientation; none when the solver chose it.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "From the requested orientation; none when the solver chose it.",
                                    ),
                                ])
                                .describe(
                                  "From the requested orientation; none when the solver chose it.",
                                )
                                .optional(),
                              position: z.array(z.number()).min(3).max(3),
                              position_error_meters: z.number(),
                              quaternion: z.array(z.number()).min(4).max(4),
                            })
                            .describe("Where the arm would put the tool."),
                          reason: z
                            .union([
                              z
                                .string()
                                .describe(
                                  "Why the target was not met; none when it was met within tolerance.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Why the target was not met; none when it was met within tolerance.",
                                ),
                            ])
                            .describe(
                              "Why the target was not met; none when it was met within tolerance.",
                            )
                            .optional(),
                          within_tolerance: z
                            .boolean()
                            .describe(
                              "The solver or controller met the target without a path constraint.",
                            ),
                        })
                        .describe(
                          "The best pose found, including an approximation when the target was not met.",
                        ),
                      z
                        .object({
                          kind: z.literal("unavailable"),
                          reason: z.string(),
                        })
                        .describe(
                          "The preview could not produce a usable pose.",
                        ),
                    ])
                    .describe(
                      "A usable projected pose, or why the preview could not produce one.",
                    ),
                  requested: z
                    .union([
                      z
                        .object({
                          orientation: z
                            .object({
                              approach: z.array(z.number()).min(3).max(3),
                              jaw_yaw_degrees: z.number(),
                            })
                            .describe(
                              "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                            ),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe(
                          "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "None for a position-only target the solver chose the orientation for.",
                    )
                    .optional(),
                })
                .describe("The answer to a projection."),
              z.null(),
            ])
            .describe("The answer to a `project_pose`.")
            .optional(),
          reason: z
            .union([
              z
                .enum([
                  "arm_busy",
                  "not_enabled",
                  "stale_revision",
                  "interlocked",
                  "faulted",
                  "invalid_target",
                  "stream_ended",
                  "unreachable",
                  "lease_held_elsewhere",
                  "malformed",
                ])
                .describe("Why a command was not accepted."),
              z.null(),
            ])
            .optional(),
          record: z
            .union([
              z
                .object({
                  accepted_at: z.number(),
                  achieved: z
                    .union([
                      z
                        .object({
                          joint_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          joint_errors_radians: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                            ])
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            )
                            .optional(),
                          joints: z.array(z.number()),
                          orientation: z
                            .union([
                              z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  degrees_from_down: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  target: z
                                    .union([
                                      z
                                        .object({
                                          approach_error_degrees: z.number(),
                                          jaw_yaw_degrees: z.number(),
                                          jaw_yaw_reference: z
                                            .enum(["world_x", "world_y"])
                                            .describe(
                                              "The world axis jaw yaw zero is projected from.",
                                            ),
                                          orientation_error_degrees: z.number(),
                                        })
                                        .describe(
                                          "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                        ),
                                      z.null(),
                                    ])
                                    .optional(),
                                })
                                .describe(
                                  "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The tool orientation in grasp terms, against the target's when there is one.",
                            )
                            .optional(),
                          orientation_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          position: z
                            .union([
                              z
                                .array(z.number())
                                .min(3)
                                .max(3)
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                            ])
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            )
                            .optional(),
                          position_error_meters: z
                            .union([z.number(), z.null()])
                            .optional(),
                          quaternion: z
                            .union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ])
                            .optional(),
                          sampled_at: z.number(),
                        })
                        .describe(
                          "Where the arm was, measured, when a command ended.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  checkpoints: z.array(
                    z
                      .object({
                        label: z.string(),
                        measured_joints: z.array(z.number()),
                        target_position: z
                          .union([z.array(z.number()).min(3).max(3), z.null()])
                          .optional(),
                        target_quaternion: z
                          .union([z.array(z.number()).min(4).max(4), z.null()])
                          .optional(),
                      })
                      .describe(
                        "Where the arm stood, as measured, when it settled at the end of a segment.",
                      ),
                  ),
                  command_id: z.string(),
                  completion: z
                    .union([
                      z
                        .object({
                          detail: z.union([z.string(), z.null()]).optional(),
                          finished_at: z.number(),
                          gripper: z
                            .union([
                              z
                                .union([
                                  z
                                    .object({
                                      kind: z.literal("reached"),
                                      measured_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw arrived at the angle it was asked for.",
                                    ),
                                  z
                                    .object({
                                      kind: z.literal("stopped_short"),
                                      measured_angle_radians: z.number(),
                                      target_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                    ),
                                ])
                                .describe(
                                  "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "How a gripper command ended; null for every other command.",
                            )
                            .optional(),
                          outcome: z
                            .enum([
                              "completed",
                              "stopped",
                              "tripped",
                              "timed_out",
                              "stopped_short",
                              "superseded",
                              "faulted",
                              "interrupted",
                              "failed",
                            ])
                            .describe("How a command ended."),
                        })
                        .describe(
                          "The outcome and timestamp are recorded together when a command ends.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  goal_joints: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                      z
                        .null()
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                    ])
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    )
                    .optional(),
                  group_id: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                      z
                        .null()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                    ])
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    )
                    .optional(),
                  joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                      z
                        .null()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                    ])
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    )
                    .optional(),
                  label: z
                    .string()
                    .describe(
                      "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                    ),
                  path_limited: z
                    .union([
                      z
                        .object({ reason: z.string() })
                        .describe(
                          "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  pose_limited: z
                    .union([
                      z
                        .object({
                          advice: z
                            .string()
                            .describe(
                              "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                            ),
                          orientation_shortfall_radians: z
                            .number()
                            .describe(
                              "From the closest pose's orientation to the requested one.",
                            ),
                          position_shortfall_meters: z
                            .number()
                            .describe(
                              "From the closest pose's tool position to the requested one.",
                            ),
                          reason: z
                            .union([
                              z
                                .literal("at_joint_limit")
                                .describe(
                                  "The best pose it found has a joint at its limit, and no step improves it.",
                                ),
                              z
                                .literal("stalled")
                                .describe(
                                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                ),
                              z
                                .literal("iteration_limit")
                                .describe(
                                  "It was still improving when it ran out of iterations.",
                                ),
                              z
                                .literal("out_of_reach")
                                .describe(
                                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                ),
                            ])
                            .describe("Why a solve stopped short."),
                          waypoint: z
                            .number()
                            .int()
                            .gte(0)
                            .describe(
                              "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                            ),
                        })
                        .describe(
                          "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  target_joints: z
                    .union([z.array(z.number()), z.null()])
                    .optional(),
                  target_position: z
                    .union([z.array(z.number()).min(3).max(3), z.null()])
                    .optional(),
                  target_quaternion: z
                    .union([z.array(z.number()).min(4).max(4), z.null()])
                    .optional(),
                  target_status: z
                    .union([
                      z
                        .enum([
                          "reached",
                          "stopped_short",
                          "partial",
                          "not_started",
                        ])
                        .describe(
                          "Whether the arm ended where the command was aimed.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  trace_context: z
                    .union([
                      z
                        .object({
                          traceparent: z
                            .string()
                            .describe(
                              "`00-<trace id>-<parent span id>-<flags>`.",
                            ),
                          tracestate: z
                            .union([z.string(), z.null()])
                            .default(null),
                        })
                        .describe(
                          "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The trace context the command arrived with, if any.",
                    )
                    .optional(),
                })
                .strict()
                .describe("One command, from acceptance to its end."),
              z.null(),
            ])
            .optional(),
          records: z
            .array(
              z
                .object({
                  accepted_at: z.number(),
                  achieved: z
                    .union([
                      z
                        .object({
                          joint_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          joint_errors_radians: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                            ])
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            )
                            .optional(),
                          joints: z.array(z.number()),
                          orientation: z
                            .union([
                              z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  degrees_from_down: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  target: z
                                    .union([
                                      z
                                        .object({
                                          approach_error_degrees: z.number(),
                                          jaw_yaw_degrees: z.number(),
                                          jaw_yaw_reference: z
                                            .enum(["world_x", "world_y"])
                                            .describe(
                                              "The world axis jaw yaw zero is projected from.",
                                            ),
                                          orientation_error_degrees: z.number(),
                                        })
                                        .describe(
                                          "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                        ),
                                      z.null(),
                                    ])
                                    .optional(),
                                })
                                .describe(
                                  "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The tool orientation in grasp terms, against the target's when there is one.",
                            )
                            .optional(),
                          orientation_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          position: z
                            .union([
                              z
                                .array(z.number())
                                .min(3)
                                .max(3)
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                            ])
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            )
                            .optional(),
                          position_error_meters: z
                            .union([z.number(), z.null()])
                            .optional(),
                          quaternion: z
                            .union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ])
                            .optional(),
                          sampled_at: z.number(),
                        })
                        .describe(
                          "Where the arm was, measured, when a command ended.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  checkpoints: z.array(
                    z
                      .object({
                        label: z.string(),
                        measured_joints: z.array(z.number()),
                        target_position: z
                          .union([z.array(z.number()).min(3).max(3), z.null()])
                          .optional(),
                        target_quaternion: z
                          .union([z.array(z.number()).min(4).max(4), z.null()])
                          .optional(),
                      })
                      .describe(
                        "Where the arm stood, as measured, when it settled at the end of a segment.",
                      ),
                  ),
                  command_id: z.string(),
                  completion: z
                    .union([
                      z
                        .object({
                          detail: z.union([z.string(), z.null()]).optional(),
                          finished_at: z.number(),
                          gripper: z
                            .union([
                              z
                                .union([
                                  z
                                    .object({
                                      kind: z.literal("reached"),
                                      measured_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw arrived at the angle it was asked for.",
                                    ),
                                  z
                                    .object({
                                      kind: z.literal("stopped_short"),
                                      measured_angle_radians: z.number(),
                                      target_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                    ),
                                ])
                                .describe(
                                  "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "How a gripper command ended; null for every other command.",
                            )
                            .optional(),
                          outcome: z
                            .enum([
                              "completed",
                              "stopped",
                              "tripped",
                              "timed_out",
                              "stopped_short",
                              "superseded",
                              "faulted",
                              "interrupted",
                              "failed",
                            ])
                            .describe("How a command ended."),
                        })
                        .describe(
                          "The outcome and timestamp are recorded together when a command ends.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  goal_joints: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                      z
                        .null()
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                    ])
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    )
                    .optional(),
                  group_id: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                      z
                        .null()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                    ])
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    )
                    .optional(),
                  joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                      z
                        .null()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                    ])
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    )
                    .optional(),
                  label: z
                    .string()
                    .describe(
                      "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                    ),
                  path_limited: z
                    .union([
                      z
                        .object({ reason: z.string() })
                        .describe(
                          "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  pose_limited: z
                    .union([
                      z
                        .object({
                          advice: z
                            .string()
                            .describe(
                              "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                            ),
                          orientation_shortfall_radians: z
                            .number()
                            .describe(
                              "From the closest pose's orientation to the requested one.",
                            ),
                          position_shortfall_meters: z
                            .number()
                            .describe(
                              "From the closest pose's tool position to the requested one.",
                            ),
                          reason: z
                            .union([
                              z
                                .literal("at_joint_limit")
                                .describe(
                                  "The best pose it found has a joint at its limit, and no step improves it.",
                                ),
                              z
                                .literal("stalled")
                                .describe(
                                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                ),
                              z
                                .literal("iteration_limit")
                                .describe(
                                  "It was still improving when it ran out of iterations.",
                                ),
                              z
                                .literal("out_of_reach")
                                .describe(
                                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                ),
                            ])
                            .describe("Why a solve stopped short."),
                          waypoint: z
                            .number()
                            .int()
                            .gte(0)
                            .describe(
                              "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                            ),
                        })
                        .describe(
                          "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  target_joints: z
                    .union([z.array(z.number()), z.null()])
                    .optional(),
                  target_position: z
                    .union([z.array(z.number()).min(3).max(3), z.null()])
                    .optional(),
                  target_quaternion: z
                    .union([z.array(z.number()).min(4).max(4), z.null()])
                    .optional(),
                  target_status: z
                    .union([
                      z
                        .enum([
                          "reached",
                          "stopped_short",
                          "partial",
                          "not_started",
                        ])
                        .describe(
                          "Whether the arm ended where the command was aimed.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  trace_context: z
                    .union([
                      z
                        .object({
                          traceparent: z
                            .string()
                            .describe(
                              "`00-<trace id>-<parent span id>-<flags>`.",
                            ),
                          tracestate: z
                            .union([z.string(), z.null()])
                            .default(null),
                        })
                        .describe(
                          "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The trace context the command arrived with, if any.",
                    )
                    .optional(),
                })
                .strict()
                .describe("One command, from acceptance to its end."),
            )
            .describe(
              "The records of a grouped move, one per arm in the order asked; empty\nfor any other request.",
            )
            .default([]),
          request_id: z.string(),
        })
        .describe(
          "The one answer to a request that carried a request id: the command's\nrecord when it was accepted, or the reason it was not.",
        ),
      z
        .object({
          fault: z.union([z.string(), z.null()]).optional(),
          kind: z.literal("snapshot"),
          log: z
            .array(
              z
                .object({
                  emitted_unix_nano: z
                    .string()
                    .regex(/^[0-9]+$/)
                    .describe(
                      "When the controller's writer made the frame; a replayed log event keeps\nthe time it was first emitted.",
                    ),
                  event: z.any(),
                  seq: z
                    .union([
                      z
                        .number()
                        .int()
                        .gte(0)
                        .describe(
                          "Present on log events only; numbers rise by one with no gaps.",
                        ),
                      z
                        .null()
                        .describe(
                          "Present on log events only; numbers rise by one with no gaps.",
                        ),
                    ])
                    .describe(
                      "Present on log events only; numbers rise by one with no gaps.",
                    )
                    .optional(),
                })
                .describe(
                  "One line from the controller: a log event with its sequence number, or an\nunsequenced sample or reply.",
                ),
            )
            .describe("The retained log, oldest first."),
          mode: z
            .union([
              z
                .literal("disabled")
                .describe("Torque off; only `enable` is accepted."),
              z
                .literal("holding")
                .describe("Torque on, holding between commands."),
              z.literal("moving").describe("Following a motion or a stream."),
              z
                .literal("faulted")
                .describe(
                  "Powered off after a hard fault; refuses everything until `clear_fault`.",
                ),
              z
                .literal("disconnected")
                .describe(
                  "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                ),
              z
                .literal("reconnected")
                .describe(
                  "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                ),
            ])
            .describe("Where the arm stands."),
          records: z
            .array(
              z
                .object({
                  accepted_at: z.number(),
                  achieved: z
                    .union([
                      z
                        .object({
                          joint_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          joint_errors_radians: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                ),
                            ])
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            )
                            .optional(),
                          joints: z.array(z.number()),
                          orientation: z
                            .union([
                              z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  degrees_from_down: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  target: z
                                    .union([
                                      z
                                        .object({
                                          approach_error_degrees: z.number(),
                                          jaw_yaw_degrees: z.number(),
                                          jaw_yaw_reference: z
                                            .enum(["world_x", "world_y"])
                                            .describe(
                                              "The world axis jaw yaw zero is projected from.",
                                            ),
                                          orientation_error_degrees: z.number(),
                                        })
                                        .describe(
                                          "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                        ),
                                      z.null(),
                                    ])
                                    .optional(),
                                })
                                .describe(
                                  "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The tool orientation in grasp terms, against the target's when there is one.",
                            )
                            .optional(),
                          orientation_error_radians: z
                            .union([z.number(), z.null()])
                            .optional(),
                          position: z
                            .union([
                              z
                                .array(z.number())
                                .min(3)
                                .max(3)
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                ),
                            ])
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            )
                            .optional(),
                          position_error_meters: z
                            .union([z.number(), z.null()])
                            .optional(),
                          quaternion: z
                            .union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ])
                            .optional(),
                          sampled_at: z.number(),
                        })
                        .describe(
                          "Where the arm was, measured, when a command ended.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  checkpoints: z.array(
                    z
                      .object({
                        label: z.string(),
                        measured_joints: z.array(z.number()),
                        target_position: z
                          .union([z.array(z.number()).min(3).max(3), z.null()])
                          .optional(),
                        target_quaternion: z
                          .union([z.array(z.number()).min(4).max(4), z.null()])
                          .optional(),
                      })
                      .describe(
                        "Where the arm stood, as measured, when it settled at the end of a segment.",
                      ),
                  ),
                  command_id: z.string(),
                  completion: z
                    .union([
                      z
                        .object({
                          detail: z.union([z.string(), z.null()]).optional(),
                          finished_at: z.number(),
                          gripper: z
                            .union([
                              z
                                .union([
                                  z
                                    .object({
                                      kind: z.literal("reached"),
                                      measured_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw arrived at the angle it was asked for.",
                                    ),
                                  z
                                    .object({
                                      kind: z.literal("stopped_short"),
                                      measured_angle_radians: z.number(),
                                      target_angle_radians: z.number(),
                                    })
                                    .describe(
                                      "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                    ),
                                ])
                                .describe(
                                  "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "How a gripper command ended; null for every other command.",
                            )
                            .optional(),
                          outcome: z
                            .enum([
                              "completed",
                              "stopped",
                              "tripped",
                              "timed_out",
                              "stopped_short",
                              "superseded",
                              "faulted",
                              "interrupted",
                              "failed",
                            ])
                            .describe("How a command ended."),
                        })
                        .describe(
                          "The outcome and timestamp are recorded together when a command ends.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  goal_joints: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                      z
                        .null()
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                    ])
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    )
                    .optional(),
                  group_id: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                      z
                        .null()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                    ])
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    )
                    .optional(),
                  joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                      z
                        .null()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                    ])
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    )
                    .optional(),
                  label: z
                    .string()
                    .describe(
                      "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                    ),
                  path_limited: z
                    .union([
                      z
                        .object({ reason: z.string() })
                        .describe(
                          "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  pose_limited: z
                    .union([
                      z
                        .object({
                          advice: z
                            .string()
                            .describe(
                              "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                            ),
                          orientation_shortfall_radians: z
                            .number()
                            .describe(
                              "From the closest pose's orientation to the requested one.",
                            ),
                          position_shortfall_meters: z
                            .number()
                            .describe(
                              "From the closest pose's tool position to the requested one.",
                            ),
                          reason: z
                            .union([
                              z
                                .literal("at_joint_limit")
                                .describe(
                                  "The best pose it found has a joint at its limit, and no step improves it.",
                                ),
                              z
                                .literal("stalled")
                                .describe(
                                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                ),
                              z
                                .literal("iteration_limit")
                                .describe(
                                  "It was still improving when it ran out of iterations.",
                                ),
                              z
                                .literal("out_of_reach")
                                .describe(
                                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                ),
                            ])
                            .describe("Why a solve stopped short."),
                          waypoint: z
                            .number()
                            .int()
                            .gte(0)
                            .describe(
                              "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                            ),
                        })
                        .describe(
                          "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  target_joints: z
                    .union([z.array(z.number()), z.null()])
                    .optional(),
                  target_position: z
                    .union([z.array(z.number()).min(3).max(3), z.null()])
                    .optional(),
                  target_quaternion: z
                    .union([z.array(z.number()).min(4).max(4), z.null()])
                    .optional(),
                  target_status: z
                    .union([
                      z
                        .enum([
                          "reached",
                          "stopped_short",
                          "partial",
                          "not_started",
                        ])
                        .describe(
                          "Whether the arm ended where the command was aimed.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  trace_context: z
                    .union([
                      z
                        .object({
                          traceparent: z
                            .string()
                            .describe(
                              "`00-<trace id>-<parent span id>-<flags>`.",
                            ),
                          tracestate: z
                            .union([z.string(), z.null()])
                            .default(null),
                        })
                        .describe(
                          "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The trace context the command arrived with, if any.",
                    )
                    .optional(),
                })
                .strict()
                .describe("One command, from acceptance to its end."),
            )
            .describe(
              "The commands the arm still remembers, oldest first, as they stand.",
            ),
          sample: z
            .union([
              z
                .object({
                  active_command: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The command being carried out, when one is.",
                        ),
                      z
                        .null()
                        .describe(
                          "The command being carried out, when one is.",
                        ),
                    ])
                    .describe("The command being carried out, when one is.")
                    .optional(),
                  active_trip: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The protective check currently preventing new motion, if any.",
                        ),
                      z
                        .null()
                        .describe(
                          "The protective check currently preventing new motion, if any.",
                        ),
                    ])
                    .describe(
                      "The protective check currently preventing new motion, if any.",
                    )
                    .default(null),
                  commanded_positions: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "Joint setpoint successfully sent this cycle, if any.",
                        ),
                      z
                        .null()
                        .describe(
                          "Joint setpoint successfully sent this cycle, if any.",
                        ),
                    ])
                    .describe(
                      "Joint setpoint successfully sent this cycle, if any.",
                    )
                    .optional(),
                  cycle: z.number().int().gte(0),
                  cycle_seconds: z.number(),
                  efforts: z
                    .array(z.number())
                    .describe(
                      "Each arm joint's load, in percent of the servo's rated load.",
                    ),
                  fault: z
                    .union([
                      z
                        .string()
                        .describe(
                          "A fault the hardware reports this cycle, tripped or not.",
                        ),
                      z
                        .null()
                        .describe(
                          "A fault the hardware reports this cycle, tripped or not.",
                        ),
                    ])
                    .describe(
                      "A fault the hardware reports this cycle, tripped or not.",
                    )
                    .optional(),
                  gripper_angle: z.union([z.number(), z.null()]).optional(),
                  hold_pose: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                        ),
                      z
                        .null()
                        .describe(
                          "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                        ),
                    ])
                    .describe(
                      "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                    )
                    .optional(),
                  mode: z
                    .union([
                      z
                        .literal("disabled")
                        .describe("Torque off; only `enable` is accepted."),
                      z
                        .literal("holding")
                        .describe("Torque on, holding between commands."),
                      z
                        .literal("moving")
                        .describe("Following a motion or a stream."),
                      z
                        .literal("faulted")
                        .describe(
                          "Powered off after a hard fault; refuses everything until `clear_fault`.",
                        ),
                      z
                        .literal("disconnected")
                        .describe(
                          "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                        ),
                      z
                        .literal("reconnected")
                        .describe(
                          "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                        ),
                    ])
                    .describe("Where the arm stands."),
                  positions: z.array(z.number()),
                  recovery_issue: z
                    .union([
                      z
                        .object({
                          detail: z.string(),
                          motors_without_torque: z.array(
                            z.number().int().gte(0).lte(255),
                          ),
                        })
                        .describe(
                          "Why automatic recovery stopped, including motors found without torque.",
                        ),
                      z.null(),
                    ])
                    .optional(),
                  rest_pose: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                        ),
                      z
                        .null()
                        .describe(
                          "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                        ),
                    ])
                    .describe(
                      "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                    )
                    .optional(),
                  segment: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The segment of the active motion being followed.",
                        ),
                      z
                        .null()
                        .describe(
                          "The segment of the active motion being followed.",
                        ),
                    ])
                    .describe(
                      "The segment of the active motion being followed.",
                    )
                    .optional(),
                  temperatures_celsius: z
                    .array(z.number())
                    .describe(
                      "The last reading of each driven motor, the gripper's last; empty in\nsimulation. Sent while tripped too, so a holding arm that heats shows.",
                    ),
                  velocities: z.array(z.number()),
                  voltages_volts: z
                    .array(z.number())
                    .describe(
                      "The supply voltage each driven motor last reported, as the temperatures.",
                    ),
                })
                .describe(
                  "One control cycle's state; a sample, so a slow client may miss some.",
                ),
              z.null(),
            ])
            .optional(),
          seq: z
            .number()
            .int()
            .gte(0)
            .describe(
              "The newest sequence number issued; subscribing after it misses nothing.",
            ),
        })
        .describe(
          "The answer to `snapshot`, and to `subscribe` when events were lost.",
        ),
      z
        .object({
          after: z.number().int().gte(0),
          kind: z.literal("resync"),
          newest: z.number().int().gte(0),
          oldest_retained: z.number().int().gte(0),
        })
        .describe(
          "A resume point the retained log no longer reaches, or that is ahead of\nit: events between `after` and `oldest_retained` are lost. A snapshot\nfollows.",
        ),
      z
        .object({
          accepted_at: z.number(),
          achieved: z
            .union([
              z
                .object({
                  joint_error_radians: z
                    .union([z.number(), z.null()])
                    .optional(),
                  joint_errors_radians: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "Each joint's absolute distance from the target joints.",
                        ),
                      z
                        .null()
                        .describe(
                          "Each joint's absolute distance from the target joints.",
                        ),
                    ])
                    .describe(
                      "Each joint's absolute distance from the target joints.",
                    )
                    .optional(),
                  joints: z.array(z.number()),
                  orientation: z
                    .union([
                      z
                        .object({
                          approach: z.array(z.number()).min(3).max(3),
                          degrees_from_down: z.number(),
                          jaw_yaw_degrees: z.number(),
                          jaw_yaw_reference: z
                            .enum(["world_x", "world_y"])
                            .describe(
                              "The world axis jaw yaw zero is projected from.",
                            ),
                          target: z
                            .union([
                              z
                                .object({
                                  approach_error_degrees: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  orientation_error_degrees: z.number(),
                                })
                                .describe(
                                  "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                ),
                              z.null(),
                            ])
                            .optional(),
                        })
                        .describe(
                          "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The tool orientation in grasp terms, against the target's when there is one.",
                    )
                    .optional(),
                  orientation_error_radians: z
                    .union([z.number(), z.null()])
                    .optional(),
                  position: z
                    .union([
                      z
                        .array(z.number())
                        .min(3)
                        .max(3)
                        .describe(
                          "The tool pose at `joints`; absent when the controller has no tool link.",
                        ),
                      z
                        .null()
                        .describe(
                          "The tool pose at `joints`; absent when the controller has no tool link.",
                        ),
                    ])
                    .describe(
                      "The tool pose at `joints`; absent when the controller has no tool link.",
                    )
                    .optional(),
                  position_error_meters: z
                    .union([z.number(), z.null()])
                    .optional(),
                  quaternion: z
                    .union([z.array(z.number()).min(4).max(4), z.null()])
                    .optional(),
                  sampled_at: z.number(),
                })
                .describe("Where the arm was, measured, when a command ended."),
              z.null(),
            ])
            .optional(),
          checkpoints: z.array(
            z
              .object({
                label: z.string(),
                measured_joints: z.array(z.number()),
                target_position: z
                  .union([z.array(z.number()).min(3).max(3), z.null()])
                  .optional(),
                target_quaternion: z
                  .union([z.array(z.number()).min(4).max(4), z.null()])
                  .optional(),
              })
              .describe(
                "Where the arm stood, as measured, when it settled at the end of a segment.",
              ),
          ),
          command_id: z.string(),
          completion: z
            .union([
              z
                .object({
                  detail: z.union([z.string(), z.null()]).optional(),
                  finished_at: z.number(),
                  gripper: z
                    .union([
                      z
                        .union([
                          z
                            .object({
                              kind: z.literal("reached"),
                              measured_angle_radians: z.number(),
                            })
                            .describe(
                              "The jaw arrived at the angle it was asked for.",
                            ),
                          z
                            .object({
                              kind: z.literal("stopped_short"),
                              measured_angle_radians: z.number(),
                              target_angle_radians: z.number(),
                            })
                            .describe(
                              "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                            ),
                        ])
                        .describe(
                          "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "How a gripper command ended; null for every other command.",
                    )
                    .optional(),
                  outcome: z
                    .enum([
                      "completed",
                      "stopped",
                      "tripped",
                      "timed_out",
                      "stopped_short",
                      "superseded",
                      "faulted",
                      "interrupted",
                      "failed",
                    ])
                    .describe("How a command ended."),
                })
                .describe(
                  "The outcome and timestamp are recorded together when a command ends.",
                ),
              z.null(),
            ])
            .optional(),
          goal_joints: z
            .union([
              z
                .array(z.number())
                .describe(
                  "The newest goal of a joint stream, after clamping to the joint limits.",
                ),
              z
                .null()
                .describe(
                  "The newest goal of a joint stream, after clamping to the joint limits.",
                ),
            ])
            .describe(
              "The newest goal of a joint stream, after clamping to the joint limits.",
            )
            .optional(),
          group_id: z
            .union([
              z
                .string()
                .describe(
                  "The grouped motion this command is one arm's part of, if it is.",
                ),
              z
                .null()
                .describe(
                  "The grouped motion this command is one arm's part of, if it is.",
                ),
            ])
            .describe(
              "The grouped motion this command is one arm's part of, if it is.",
            )
            .optional(),
          joint_speed: z
            .union([
              z
                .number()
                .describe("Speed actually used, after clamping, for motions."),
              z
                .null()
                .describe("Speed actually used, after clamping, for motions."),
            ])
            .describe("Speed actually used, after clamping, for motions.")
            .optional(),
          kind: z.literal("record"),
          label: z
            .string()
            .describe(
              "What the command is: `enable`, `stream_joints`, or the label a move carried.",
            ),
          path_limited: z
            .union([
              z
                .object({ reason: z.string() })
                .describe(
                  "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                ),
              z.null(),
            ])
            .optional(),
          pose_limited: z
            .union([
              z
                .object({
                  advice: z
                    .string()
                    .describe(
                      "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                    ),
                  orientation_shortfall_radians: z
                    .number()
                    .describe(
                      "From the closest pose's orientation to the requested one.",
                    ),
                  position_shortfall_meters: z
                    .number()
                    .describe(
                      "From the closest pose's tool position to the requested one.",
                    ),
                  reason: z
                    .union([
                      z
                        .literal("at_joint_limit")
                        .describe(
                          "The best pose it found has a joint at its limit, and no step improves it.",
                        ),
                      z
                        .literal("stalled")
                        .describe(
                          "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                        ),
                      z
                        .literal("iteration_limit")
                        .describe(
                          "It was still improving when it ran out of iterations.",
                        ),
                      z
                        .literal("out_of_reach")
                        .describe(
                          "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                        ),
                    ])
                    .describe("Why a solve stopped short."),
                  waypoint: z
                    .number()
                    .int()
                    .gte(0)
                    .describe(
                      "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                    ),
                })
                .describe(
                  "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                ),
              z.null(),
            ])
            .optional(),
          target_joints: z.union([z.array(z.number()), z.null()]).optional(),
          target_position: z
            .union([z.array(z.number()).min(3).max(3), z.null()])
            .optional(),
          target_quaternion: z
            .union([z.array(z.number()).min(4).max(4), z.null()])
            .optional(),
          target_status: z
            .union([
              z
                .enum(["reached", "stopped_short", "partial", "not_started"])
                .describe("Whether the arm ended where the command was aimed."),
              z.null(),
            ])
            .optional(),
          trace_context: z
            .union([
              z
                .object({
                  traceparent: z
                    .string()
                    .describe("`00-<trace id>-<parent span id>-<flags>`."),
                  tracestate: z.union([z.string(), z.null()]).default(null),
                })
                .describe(
                  "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                ),
              z.null(),
            ])
            .describe("The trace context the command arrived with, if any.")
            .optional(),
        })
        .strict()
        .describe(
          "A command's record changed: accepted, started, a checkpoint, ended.",
        ),
      z
        .object({
          fault: z
            .union([
              z.string().describe("Why, after a fault."),
              z.null().describe("Why, after a fault."),
            ])
            .describe("Why, after a fault.")
            .optional(),
          kind: z.literal("mode"),
          mode: z
            .union([
              z
                .literal("disabled")
                .describe("Torque off; only `enable` is accepted."),
              z
                .literal("holding")
                .describe("Torque on, holding between commands."),
              z.literal("moving").describe("Following a motion or a stream."),
              z
                .literal("faulted")
                .describe(
                  "Powered off after a hard fault; refuses everything until `clear_fault`.",
                ),
              z
                .literal("disconnected")
                .describe(
                  "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                ),
              z
                .literal("reconnected")
                .describe(
                  "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                ),
            ])
            .describe("Where the arm stands."),
        })
        .describe("The arm changed mode."),
      z
        .object({ kind: z.literal("rejected"), reason: z.string() })
        .describe(
          "A request that was not acted on, when it carried no request id to reply to.",
        ),
      z
        .object({ kind: z.literal("tripped"), reason: z.string() })
        .describe(
          "A safety check stopped the arm, which holds where it is. Reported once\neach time a check starts failing.",
        ),
      z
        .object({
          kind: z.literal("controller_unavailable"),
          reason: z.string(),
        })
        .describe(
          "The station lost its motor-controller process; the arm's torque is unknown.",
        ),
      z
        .object({ kind: z.literal("controller_available") })
        .describe(
          "A fresh controller passed startup checks and the arm is disabled.",
        ),
    ])
    .describe("Controller to client."),
  ExecutionFailure: z
    .union([
      z.object({
        arm: z.string(),
        detail: z.string(),
        reason: z
          .enum([
            "arm_busy",
            "not_enabled",
            "stale_revision",
            "interlocked",
            "faulted",
            "invalid_target",
            "stream_ended",
            "unreachable",
            "lease_held_elsewhere",
            "malformed",
          ])
          .describe("Why a command was not accepted."),
        stage: z.literal("preparation"),
        step: z.number().int().gte(0),
      }),
      z.object({
        detail: z.string(),
        reason: z
          .enum([
            "arm_busy",
            "not_enabled",
            "stale_revision",
            "interlocked",
            "faulted",
            "invalid_target",
            "stream_ended",
            "unreachable",
            "lease_held_elsewhere",
            "malformed",
          ])
          .describe("Why a command was not accepted."),
        stage: z.literal("admission"),
        step: z.number().int().gte(0),
      }),
      z.object({
        detail: z.string(),
        stage: z.literal("admission_unknown"),
        step: z.number().int().gte(0),
      }),
      z.object({
        detail: z.string(),
        stage: z.literal("execution"),
        step: z.number().int().gte(0),
      }),
      z.object({
        detail: z.string(),
        stage: z.literal("cancellation"),
        step: z.number().int().gte(0),
      }),
      z.object({
        detail: z.string(),
        stage: z.literal("authority"),
        step: z.number().int().gte(0),
      }),
      z.object({
        detail: z.string(),
        stage: z.literal("runtime"),
        step: z.number().int().gte(0),
      }),
    ])
    .describe(
      "The boundary where an execution stopped; uncertain admission may already have acted.",
    ),
  Frame: z
    .object({
      emitted_unix_nano: z
        .string()
        .regex(/^[0-9]+$/)
        .describe(
          "When the controller's writer made the frame; a replayed log event keeps\nthe time it was first emitted.",
        ),
      event: z
        .union([
          z
            .object({
              active_command: z
                .union([
                  z
                    .string()
                    .describe("The command being carried out, when one is."),
                  z
                    .null()
                    .describe("The command being carried out, when one is."),
                ])
                .describe("The command being carried out, when one is.")
                .optional(),
              active_trip: z
                .union([
                  z
                    .string()
                    .describe(
                      "The protective check currently preventing new motion, if any.",
                    ),
                  z
                    .null()
                    .describe(
                      "The protective check currently preventing new motion, if any.",
                    ),
                ])
                .describe(
                  "The protective check currently preventing new motion, if any.",
                )
                .default(null),
              commanded_positions: z
                .union([
                  z
                    .array(z.number())
                    .describe(
                      "Joint setpoint successfully sent this cycle, if any.",
                    ),
                  z
                    .null()
                    .describe(
                      "Joint setpoint successfully sent this cycle, if any.",
                    ),
                ])
                .describe(
                  "Joint setpoint successfully sent this cycle, if any.",
                )
                .optional(),
              cycle: z.number().int().gte(0),
              cycle_seconds: z.number(),
              efforts: z
                .array(z.number())
                .describe(
                  "Each arm joint's load, in percent of the servo's rated load.",
                ),
              fault: z
                .union([
                  z
                    .string()
                    .describe(
                      "A fault the hardware reports this cycle, tripped or not.",
                    ),
                  z
                    .null()
                    .describe(
                      "A fault the hardware reports this cycle, tripped or not.",
                    ),
                ])
                .describe(
                  "A fault the hardware reports this cycle, tripped or not.",
                )
                .optional(),
              gripper_angle: z.union([z.number(), z.null()]).optional(),
              hold_pose: z
                .union([
                  z
                    .array(z.number())
                    .describe(
                      "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                    ),
                  z
                    .null()
                    .describe(
                      "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                    ),
                ])
                .describe(
                  "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                )
                .optional(),
              mode: z
                .union([
                  z
                    .literal("disabled")
                    .describe("Torque off; only `enable` is accepted."),
                  z
                    .literal("holding")
                    .describe("Torque on, holding between commands."),
                  z
                    .literal("moving")
                    .describe("Following a motion or a stream."),
                  z
                    .literal("faulted")
                    .describe(
                      "Powered off after a hard fault; refuses everything until `clear_fault`.",
                    ),
                  z
                    .literal("disconnected")
                    .describe(
                      "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                    ),
                  z
                    .literal("reconnected")
                    .describe(
                      "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                    ),
                ])
                .describe("Where the arm stands."),
              positions: z.array(z.number()),
              recovery_issue: z
                .union([
                  z
                    .object({
                      detail: z.string(),
                      motors_without_torque: z.array(
                        z.number().int().gte(0).lte(255),
                      ),
                    })
                    .describe(
                      "Why automatic recovery stopped, including motors found without torque.",
                    ),
                  z.null(),
                ])
                .optional(),
              rest_pose: z
                .union([
                  z
                    .array(z.number())
                    .describe(
                      "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                    ),
                  z
                    .null()
                    .describe(
                      "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                    ),
                ])
                .describe(
                  "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                )
                .optional(),
              segment: z
                .union([
                  z
                    .string()
                    .describe(
                      "The segment of the active motion being followed.",
                    ),
                  z
                    .null()
                    .describe(
                      "The segment of the active motion being followed.",
                    ),
                ])
                .describe("The segment of the active motion being followed.")
                .optional(),
              temperatures_celsius: z
                .array(z.number())
                .describe(
                  "The last reading of each driven motor, the gripper's last; empty in\nsimulation. Sent while tripped too, so a holding arm that heats shows.",
                ),
              velocities: z.array(z.number()),
              voltages_volts: z
                .array(z.number())
                .describe(
                  "The supply voltage each driven motor last reported, as the temperatures.",
                ),
              kind: z.literal("state"),
            })
            .describe(
              "One control cycle's state; a sample, so a slow client may miss some.",
            ),
          z
            .object({
              accepted: z.boolean(),
              description: z
                .union([
                  z
                    .object({
                      model_points: z
                        .array(z.array(z.number()).min(3).max(3))
                        .describe(
                          "Each joint's origin, then the tool's: the points a viewer draws the arm through.",
                        ),
                      orientation: z
                        .object({
                          approach: z.array(z.number()).min(3).max(3),
                          degrees_from_down: z.number(),
                          jaw_yaw_degrees: z.number(),
                          jaw_yaw_reference: z
                            .enum(["world_x", "world_y"])
                            .describe(
                              "The world axis jaw yaw zero is projected from.",
                            ),
                          target: z
                            .union([
                              z
                                .object({
                                  approach_error_degrees: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  orientation_error_degrees: z.number(),
                                })
                                .describe(
                                  "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                ),
                              z.null(),
                            ])
                            .optional(),
                        })
                        .describe(
                          "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                        ),
                      position: z.array(z.number()).min(3).max(3),
                      quaternion: z.array(z.number()).min(4).max(4),
                    })
                    .describe(
                      "The tool's pose at some joints, in the station's world frame.",
                    ),
                  z.null(),
                ])
                .describe("The answer to a `describe_pose`.")
                .optional(),
              detail: z.union([z.string(), z.null()]).optional(),
              ik: z
                .union([
                  z
                    .union([
                      z
                        .object({
                          joints: z.array(z.number()),
                          orientation_error_radians: z
                            .number()
                            .describe(
                              "From the required orientation, or from the seed's when it was free.",
                            ),
                          position_error_meters: z.number(),
                          tool_position: z.array(z.number()).min(3).max(3),
                          tool_quaternion: z.array(z.number()).min(4).max(4),
                          outcome: z.literal("solved"),
                        })
                        .describe(
                          "The tool is within tolerance of the target.",
                        ),
                      z
                        .object({
                          best: z
                            .object({
                              joints: z.array(z.number()),
                              orientation_error_radians: z
                                .number()
                                .describe(
                                  "From the required orientation, or from the seed's when it was free.",
                                ),
                              position_error_meters: z.number(),
                              tool_position: z.array(z.number()).min(3).max(3),
                              tool_quaternion: z
                                .array(z.number())
                                .min(4)
                                .max(4),
                            })
                            .describe("Where a solve left the arm."),
                          outcome: z.literal("failed"),
                          reason: z
                            .union([
                              z
                                .literal("at_joint_limit")
                                .describe(
                                  "The best pose it found has a joint at its limit, and no step improves it.",
                                ),
                              z
                                .literal("stalled")
                                .describe(
                                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                ),
                              z
                                .literal("iteration_limit")
                                .describe(
                                  "It was still improving when it ran out of iterations.",
                                ),
                              z
                                .literal("out_of_reach")
                                .describe(
                                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                ),
                            ])
                            .describe("Why a solve stopped short."),
                        })
                        .describe(
                          "It is not; `best` is the pose with the smallest position error found,\nalways inside the joint limits.",
                        ),
                    ])
                    .describe("A solve's outcome."),
                  z.null(),
                ])
                .describe("How a `solve_pose` ended.")
                .optional(),
              kind: z.literal("reply"),
              lease: z
                .union([
                  z
                    .union([
                      z
                        .object({
                          lease_seconds: z.number(),
                          lease_token: z.string(),
                          status: z
                            .object({
                              expires_in_seconds: z
                                .number()
                                .describe(
                                  "The lease lapses this long after its grant or last explicit renewal.",
                                ),
                              holder: z
                                .union([
                                  z
                                    .literal("agent")
                                    .describe(
                                      "A model loop, started from the terminal or the agent server.",
                                    ),
                                  z
                                    .literal("manual")
                                    .describe(
                                      "A person driving the arms from the viewer's manual controls.",
                                    ),
                                ])
                                .describe(
                                  "The kind of controller holding the lease.",
                                ),
                              label: z.string(),
                              on_interruption: z
                                .union([
                                  z
                                    .literal("wait_for_approval")
                                    .describe(
                                      "Keep the lease and wait for an operator's `approve_recovery`.",
                                    ),
                                  z
                                    .literal("return_lease")
                                    .describe("Give the lease up."),
                                ])
                                .describe(
                                  "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                                ),
                              required_cameras: z
                                .array(z.string())
                                .describe(
                                  "Cameras the holder cannot work without.",
                                ),
                            })
                            .describe(
                              "The current lease, as everyone may see it.",
                            ),
                          kind: z.literal("granted"),
                        })
                        .describe("The lease is yours."),
                      z
                        .object({
                          kind: z.literal("holder"),
                          status: z
                            .union([
                              z
                                .object({
                                  expires_in_seconds: z
                                    .number()
                                    .describe(
                                      "The lease lapses this long after its grant or last explicit renewal.",
                                    ),
                                  holder: z
                                    .union([
                                      z
                                        .literal("agent")
                                        .describe(
                                          "A model loop, started from the terminal or the agent server.",
                                        ),
                                      z
                                        .literal("manual")
                                        .describe(
                                          "A person driving the arms from the viewer's manual controls.",
                                        ),
                                    ])
                                    .describe(
                                      "The kind of controller holding the lease.",
                                    ),
                                  label: z.string(),
                                  on_interruption: z
                                    .union([
                                      z
                                        .literal("wait_for_approval")
                                        .describe(
                                          "Keep the lease and wait for an operator's `approve_recovery`.",
                                        ),
                                      z
                                        .literal("return_lease")
                                        .describe("Give the lease up."),
                                    ])
                                    .describe(
                                      "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                                    ),
                                  required_cameras: z
                                    .array(z.string())
                                    .describe(
                                      "Cameras the holder cannot work without.",
                                    ),
                                })
                                .describe(
                                  "The current lease, as everyone may see it.",
                                ),
                              z.null(),
                            ])
                            .optional(),
                        })
                        .describe("Who holds the lease, if anyone."),
                    ])
                    .describe("What a request about the lease answers."),
                  z.null(),
                ])
                .describe("The answer to a request about the lease.")
                .optional(),
              plan: z
                .union([
                  z
                    .object({
                      path_limited: z
                        .union([
                          z
                            .object({ reason: z.string() })
                            .describe(
                              "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      plan: z.object({
                        peak_speed: z.number(),
                        segments: z.array(
                          z
                            .object({
                              corners: z.array(z.array(z.number())),
                              label: z.string(),
                              minimum_duration_seconds: z
                                .union([
                                  z
                                    .number()
                                    .describe(
                                      "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                    ),
                                ])
                                .describe(
                                  "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                )
                                .default(null),
                              target_position: z
                                .union([
                                  z.array(z.number()).min(3).max(3),
                                  z.null(),
                                ])
                                .default(null),
                              target_quaternion: z
                                .union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ])
                                .default(null),
                            })
                            .describe(
                              "One straight run between corners, with where it is meant to end.",
                            ),
                        ),
                      }),
                      pose_limited: z
                        .union([
                          z
                            .object({
                              advice: z
                                .string()
                                .describe(
                                  "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                ),
                              orientation_shortfall_radians: z
                                .number()
                                .describe(
                                  "From the closest pose's orientation to the requested one.",
                                ),
                              position_shortfall_meters: z
                                .number()
                                .describe(
                                  "From the closest pose's tool position to the requested one.",
                                ),
                              reason: z
                                .union([
                                  z
                                    .literal("at_joint_limit")
                                    .describe(
                                      "The best pose it found has a joint at its limit, and no step improves it.",
                                    ),
                                  z
                                    .literal("stalled")
                                    .describe(
                                      "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                    ),
                                  z
                                    .literal("iteration_limit")
                                    .describe(
                                      "It was still improving when it ran out of iterations.",
                                    ),
                                  z
                                    .literal("out_of_reach")
                                    .describe(
                                      "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                    ),
                                ])
                                .describe("Why a solve stopped short."),
                              waypoint: z
                                .number()
                                .int()
                                .gte(0)
                                .describe(
                                  "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                ),
                            })
                            .describe(
                              "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                    })
                    .describe(
                      "What the arm is asked to run, and why it stops short of the target if it does.",
                    ),
                  z.null(),
                ])
                .describe("The plan a `plan_motion` made.")
                .optional(),
              projection: z
                .union([
                  z
                    .object({
                      calculation_milliseconds: z.number(),
                      jaw_yaw_ranges: z
                        .array(
                          z
                            .object({
                              from_degrees: z.number(),
                              reachable: z.boolean(),
                              reason: z
                                .union([z.string(), z.null()])
                                .optional(),
                              to_degrees: z.number(),
                            })
                            .describe(
                              "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
                            ),
                        )
                        .describe(
                          "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
                        ),
                      outcome: z
                        .union([
                          z
                            .object({
                              kind: z.literal("projected"),
                              pose: z
                                .object({
                                  joints: z.array(z.number()),
                                  orientation: z
                                    .object({
                                      approach: z
                                        .array(z.number())
                                        .min(3)
                                        .max(3),
                                      jaw_yaw_degrees: z.number(),
                                    })
                                    .describe(
                                      "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                                    ),
                                  orientation_error_radians: z
                                    .union([
                                      z
                                        .number()
                                        .describe(
                                          "From the requested orientation; none when the solver chose it.",
                                        ),
                                      z
                                        .null()
                                        .describe(
                                          "From the requested orientation; none when the solver chose it.",
                                        ),
                                    ])
                                    .describe(
                                      "From the requested orientation; none when the solver chose it.",
                                    )
                                    .optional(),
                                  position: z.array(z.number()).min(3).max(3),
                                  position_error_meters: z.number(),
                                  quaternion: z.array(z.number()).min(4).max(4),
                                })
                                .describe("Where the arm would put the tool."),
                              reason: z
                                .union([
                                  z
                                    .string()
                                    .describe(
                                      "Why the target was not met; none when it was met within tolerance.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Why the target was not met; none when it was met within tolerance.",
                                    ),
                                ])
                                .describe(
                                  "Why the target was not met; none when it was met within tolerance.",
                                )
                                .optional(),
                              within_tolerance: z
                                .boolean()
                                .describe(
                                  "The solver or controller met the target without a path constraint.",
                                ),
                            })
                            .describe(
                              "The best pose found, including an approximation when the target was not met.",
                            ),
                          z
                            .object({
                              kind: z.literal("unavailable"),
                              reason: z.string(),
                            })
                            .describe(
                              "The preview could not produce a usable pose.",
                            ),
                        ])
                        .describe(
                          "A usable projected pose, or why the preview could not produce one.",
                        ),
                      requested: z
                        .union([
                          z
                            .object({
                              orientation: z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  jaw_yaw_degrees: z.number(),
                                })
                                .describe(
                                  "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                                ),
                              quaternion: z.array(z.number()).min(4).max(4),
                            })
                            .describe(
                              "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "None for a position-only target the solver chose the orientation for.",
                        )
                        .optional(),
                    })
                    .describe("The answer to a projection."),
                  z.null(),
                ])
                .describe("The answer to a `project_pose`.")
                .optional(),
              reason: z
                .union([
                  z
                    .enum([
                      "arm_busy",
                      "not_enabled",
                      "stale_revision",
                      "interlocked",
                      "faulted",
                      "invalid_target",
                      "stream_ended",
                      "unreachable",
                      "lease_held_elsewhere",
                      "malformed",
                    ])
                    .describe("Why a command was not accepted."),
                  z.null(),
                ])
                .optional(),
              record: z
                .union([
                  z
                    .object({
                      accepted_at: z.number(),
                      achieved: z
                        .union([
                          z
                            .object({
                              joint_error_radians: z
                                .union([z.number(), z.null()])
                                .optional(),
                              joint_errors_radians: z
                                .union([
                                  z
                                    .array(z.number())
                                    .describe(
                                      "Each joint's absolute distance from the target joints.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Each joint's absolute distance from the target joints.",
                                    ),
                                ])
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                )
                                .optional(),
                              joints: z.array(z.number()),
                              orientation: z
                                .union([
                                  z
                                    .object({
                                      approach: z
                                        .array(z.number())
                                        .min(3)
                                        .max(3),
                                      degrees_from_down: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      target: z
                                        .union([
                                          z
                                            .object({
                                              approach_error_degrees:
                                                z.number(),
                                              jaw_yaw_degrees: z.number(),
                                              jaw_yaw_reference: z
                                                .enum(["world_x", "world_y"])
                                                .describe(
                                                  "The world axis jaw yaw zero is projected from.",
                                                ),
                                              orientation_error_degrees:
                                                z.number(),
                                            })
                                            .describe(
                                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                            ),
                                          z.null(),
                                        ])
                                        .optional(),
                                    })
                                    .describe(
                                      "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "The tool orientation in grasp terms, against the target's when there is one.",
                                )
                                .optional(),
                              orientation_error_radians: z
                                .union([z.number(), z.null()])
                                .optional(),
                              position: z
                                .union([
                                  z
                                    .array(z.number())
                                    .min(3)
                                    .max(3)
                                    .describe(
                                      "The tool pose at `joints`; absent when the controller has no tool link.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The tool pose at `joints`; absent when the controller has no tool link.",
                                    ),
                                ])
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                )
                                .optional(),
                              position_error_meters: z
                                .union([z.number(), z.null()])
                                .optional(),
                              quaternion: z
                                .union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ])
                                .optional(),
                              sampled_at: z.number(),
                            })
                            .describe(
                              "Where the arm was, measured, when a command ended.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      checkpoints: z.array(
                        z
                          .object({
                            label: z.string(),
                            measured_joints: z.array(z.number()),
                            target_position: z
                              .union([
                                z.array(z.number()).min(3).max(3),
                                z.null(),
                              ])
                              .optional(),
                            target_quaternion: z
                              .union([
                                z.array(z.number()).min(4).max(4),
                                z.null(),
                              ])
                              .optional(),
                          })
                          .describe(
                            "Where the arm stood, as measured, when it settled at the end of a segment.",
                          ),
                      ),
                      command_id: z.string(),
                      completion: z
                        .union([
                          z
                            .object({
                              detail: z
                                .union([z.string(), z.null()])
                                .optional(),
                              finished_at: z.number(),
                              gripper: z
                                .union([
                                  z
                                    .union([
                                      z
                                        .object({
                                          kind: z.literal("reached"),
                                          measured_angle_radians: z.number(),
                                        })
                                        .describe(
                                          "The jaw arrived at the angle it was asked for.",
                                        ),
                                      z
                                        .object({
                                          kind: z.literal("stopped_short"),
                                          measured_angle_radians: z.number(),
                                          target_angle_radians: z.number(),
                                        })
                                        .describe(
                                          "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                        ),
                                    ])
                                    .describe(
                                      "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "How a gripper command ended; null for every other command.",
                                )
                                .optional(),
                              outcome: z
                                .enum([
                                  "completed",
                                  "stopped",
                                  "tripped",
                                  "timed_out",
                                  "stopped_short",
                                  "superseded",
                                  "faulted",
                                  "interrupted",
                                  "failed",
                                ])
                                .describe("How a command ended."),
                            })
                            .describe(
                              "The outcome and timestamp are recorded together when a command ends.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      goal_joints: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                          z
                            .null()
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                        ])
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        )
                        .optional(),
                      group_id: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                          z
                            .null()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                        ])
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        )
                        .optional(),
                      joint_speed: z
                        .union([
                          z
                            .number()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                          z
                            .null()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                        ])
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        )
                        .optional(),
                      label: z
                        .string()
                        .describe(
                          "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                        ),
                      path_limited: z
                        .union([
                          z
                            .object({ reason: z.string() })
                            .describe(
                              "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      pose_limited: z
                        .union([
                          z
                            .object({
                              advice: z
                                .string()
                                .describe(
                                  "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                ),
                              orientation_shortfall_radians: z
                                .number()
                                .describe(
                                  "From the closest pose's orientation to the requested one.",
                                ),
                              position_shortfall_meters: z
                                .number()
                                .describe(
                                  "From the closest pose's tool position to the requested one.",
                                ),
                              reason: z
                                .union([
                                  z
                                    .literal("at_joint_limit")
                                    .describe(
                                      "The best pose it found has a joint at its limit, and no step improves it.",
                                    ),
                                  z
                                    .literal("stalled")
                                    .describe(
                                      "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                    ),
                                  z
                                    .literal("iteration_limit")
                                    .describe(
                                      "It was still improving when it ran out of iterations.",
                                    ),
                                  z
                                    .literal("out_of_reach")
                                    .describe(
                                      "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                    ),
                                ])
                                .describe("Why a solve stopped short."),
                              waypoint: z
                                .number()
                                .int()
                                .gte(0)
                                .describe(
                                  "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                ),
                            })
                            .describe(
                              "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      target_joints: z
                        .union([z.array(z.number()), z.null()])
                        .optional(),
                      target_position: z
                        .union([z.array(z.number()).min(3).max(3), z.null()])
                        .optional(),
                      target_quaternion: z
                        .union([z.array(z.number()).min(4).max(4), z.null()])
                        .optional(),
                      target_status: z
                        .union([
                          z
                            .enum([
                              "reached",
                              "stopped_short",
                              "partial",
                              "not_started",
                            ])
                            .describe(
                              "Whether the arm ended where the command was aimed.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      trace_context: z
                        .union([
                          z
                            .object({
                              traceparent: z
                                .string()
                                .describe(
                                  "`00-<trace id>-<parent span id>-<flags>`.",
                                ),
                              tracestate: z
                                .union([z.string(), z.null()])
                                .default(null),
                            })
                            .describe(
                              "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "The trace context the command arrived with, if any.",
                        )
                        .optional(),
                    })
                    .strict()
                    .describe("One command, from acceptance to its end."),
                  z.null(),
                ])
                .optional(),
              records: z
                .array(
                  z
                    .object({
                      accepted_at: z.number(),
                      achieved: z
                        .union([
                          z
                            .object({
                              joint_error_radians: z
                                .union([z.number(), z.null()])
                                .optional(),
                              joint_errors_radians: z
                                .union([
                                  z
                                    .array(z.number())
                                    .describe(
                                      "Each joint's absolute distance from the target joints.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Each joint's absolute distance from the target joints.",
                                    ),
                                ])
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                )
                                .optional(),
                              joints: z.array(z.number()),
                              orientation: z
                                .union([
                                  z
                                    .object({
                                      approach: z
                                        .array(z.number())
                                        .min(3)
                                        .max(3),
                                      degrees_from_down: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      target: z
                                        .union([
                                          z
                                            .object({
                                              approach_error_degrees:
                                                z.number(),
                                              jaw_yaw_degrees: z.number(),
                                              jaw_yaw_reference: z
                                                .enum(["world_x", "world_y"])
                                                .describe(
                                                  "The world axis jaw yaw zero is projected from.",
                                                ),
                                              orientation_error_degrees:
                                                z.number(),
                                            })
                                            .describe(
                                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                            ),
                                          z.null(),
                                        ])
                                        .optional(),
                                    })
                                    .describe(
                                      "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "The tool orientation in grasp terms, against the target's when there is one.",
                                )
                                .optional(),
                              orientation_error_radians: z
                                .union([z.number(), z.null()])
                                .optional(),
                              position: z
                                .union([
                                  z
                                    .array(z.number())
                                    .min(3)
                                    .max(3)
                                    .describe(
                                      "The tool pose at `joints`; absent when the controller has no tool link.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The tool pose at `joints`; absent when the controller has no tool link.",
                                    ),
                                ])
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                )
                                .optional(),
                              position_error_meters: z
                                .union([z.number(), z.null()])
                                .optional(),
                              quaternion: z
                                .union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ])
                                .optional(),
                              sampled_at: z.number(),
                            })
                            .describe(
                              "Where the arm was, measured, when a command ended.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      checkpoints: z.array(
                        z
                          .object({
                            label: z.string(),
                            measured_joints: z.array(z.number()),
                            target_position: z
                              .union([
                                z.array(z.number()).min(3).max(3),
                                z.null(),
                              ])
                              .optional(),
                            target_quaternion: z
                              .union([
                                z.array(z.number()).min(4).max(4),
                                z.null(),
                              ])
                              .optional(),
                          })
                          .describe(
                            "Where the arm stood, as measured, when it settled at the end of a segment.",
                          ),
                      ),
                      command_id: z.string(),
                      completion: z
                        .union([
                          z
                            .object({
                              detail: z
                                .union([z.string(), z.null()])
                                .optional(),
                              finished_at: z.number(),
                              gripper: z
                                .union([
                                  z
                                    .union([
                                      z
                                        .object({
                                          kind: z.literal("reached"),
                                          measured_angle_radians: z.number(),
                                        })
                                        .describe(
                                          "The jaw arrived at the angle it was asked for.",
                                        ),
                                      z
                                        .object({
                                          kind: z.literal("stopped_short"),
                                          measured_angle_radians: z.number(),
                                          target_angle_radians: z.number(),
                                        })
                                        .describe(
                                          "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                        ),
                                    ])
                                    .describe(
                                      "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "How a gripper command ended; null for every other command.",
                                )
                                .optional(),
                              outcome: z
                                .enum([
                                  "completed",
                                  "stopped",
                                  "tripped",
                                  "timed_out",
                                  "stopped_short",
                                  "superseded",
                                  "faulted",
                                  "interrupted",
                                  "failed",
                                ])
                                .describe("How a command ended."),
                            })
                            .describe(
                              "The outcome and timestamp are recorded together when a command ends.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      goal_joints: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                          z
                            .null()
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                        ])
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        )
                        .optional(),
                      group_id: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                          z
                            .null()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                        ])
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        )
                        .optional(),
                      joint_speed: z
                        .union([
                          z
                            .number()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                          z
                            .null()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                        ])
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        )
                        .optional(),
                      label: z
                        .string()
                        .describe(
                          "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                        ),
                      path_limited: z
                        .union([
                          z
                            .object({ reason: z.string() })
                            .describe(
                              "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      pose_limited: z
                        .union([
                          z
                            .object({
                              advice: z
                                .string()
                                .describe(
                                  "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                ),
                              orientation_shortfall_radians: z
                                .number()
                                .describe(
                                  "From the closest pose's orientation to the requested one.",
                                ),
                              position_shortfall_meters: z
                                .number()
                                .describe(
                                  "From the closest pose's tool position to the requested one.",
                                ),
                              reason: z
                                .union([
                                  z
                                    .literal("at_joint_limit")
                                    .describe(
                                      "The best pose it found has a joint at its limit, and no step improves it.",
                                    ),
                                  z
                                    .literal("stalled")
                                    .describe(
                                      "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                    ),
                                  z
                                    .literal("iteration_limit")
                                    .describe(
                                      "It was still improving when it ran out of iterations.",
                                    ),
                                  z
                                    .literal("out_of_reach")
                                    .describe(
                                      "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                    ),
                                ])
                                .describe("Why a solve stopped short."),
                              waypoint: z
                                .number()
                                .int()
                                .gte(0)
                                .describe(
                                  "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                ),
                            })
                            .describe(
                              "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      target_joints: z
                        .union([z.array(z.number()), z.null()])
                        .optional(),
                      target_position: z
                        .union([z.array(z.number()).min(3).max(3), z.null()])
                        .optional(),
                      target_quaternion: z
                        .union([z.array(z.number()).min(4).max(4), z.null()])
                        .optional(),
                      target_status: z
                        .union([
                          z
                            .enum([
                              "reached",
                              "stopped_short",
                              "partial",
                              "not_started",
                            ])
                            .describe(
                              "Whether the arm ended where the command was aimed.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      trace_context: z
                        .union([
                          z
                            .object({
                              traceparent: z
                                .string()
                                .describe(
                                  "`00-<trace id>-<parent span id>-<flags>`.",
                                ),
                              tracestate: z
                                .union([z.string(), z.null()])
                                .default(null),
                            })
                            .describe(
                              "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "The trace context the command arrived with, if any.",
                        )
                        .optional(),
                    })
                    .strict()
                    .describe("One command, from acceptance to its end."),
                )
                .describe(
                  "The records of a grouped move, one per arm in the order asked; empty\nfor any other request.",
                )
                .default([]),
              request_id: z.string(),
            })
            .describe(
              "The one answer to a request that carried a request id: the command's\nrecord when it was accepted, or the reason it was not.",
            ),
          z
            .object({
              fault: z.union([z.string(), z.null()]).optional(),
              kind: z.literal("snapshot"),
              log: z.array(z.any()).describe("The retained log, oldest first."),
              mode: z
                .union([
                  z
                    .literal("disabled")
                    .describe("Torque off; only `enable` is accepted."),
                  z
                    .literal("holding")
                    .describe("Torque on, holding between commands."),
                  z
                    .literal("moving")
                    .describe("Following a motion or a stream."),
                  z
                    .literal("faulted")
                    .describe(
                      "Powered off after a hard fault; refuses everything until `clear_fault`.",
                    ),
                  z
                    .literal("disconnected")
                    .describe(
                      "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                    ),
                  z
                    .literal("reconnected")
                    .describe(
                      "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                    ),
                ])
                .describe("Where the arm stands."),
              records: z
                .array(
                  z
                    .object({
                      accepted_at: z.number(),
                      achieved: z
                        .union([
                          z
                            .object({
                              joint_error_radians: z
                                .union([z.number(), z.null()])
                                .optional(),
                              joint_errors_radians: z
                                .union([
                                  z
                                    .array(z.number())
                                    .describe(
                                      "Each joint's absolute distance from the target joints.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Each joint's absolute distance from the target joints.",
                                    ),
                                ])
                                .describe(
                                  "Each joint's absolute distance from the target joints.",
                                )
                                .optional(),
                              joints: z.array(z.number()),
                              orientation: z
                                .union([
                                  z
                                    .object({
                                      approach: z
                                        .array(z.number())
                                        .min(3)
                                        .max(3),
                                      degrees_from_down: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      target: z
                                        .union([
                                          z
                                            .object({
                                              approach_error_degrees:
                                                z.number(),
                                              jaw_yaw_degrees: z.number(),
                                              jaw_yaw_reference: z
                                                .enum(["world_x", "world_y"])
                                                .describe(
                                                  "The world axis jaw yaw zero is projected from.",
                                                ),
                                              orientation_error_degrees:
                                                z.number(),
                                            })
                                            .describe(
                                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                            ),
                                          z.null(),
                                        ])
                                        .optional(),
                                    })
                                    .describe(
                                      "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "The tool orientation in grasp terms, against the target's when there is one.",
                                )
                                .optional(),
                              orientation_error_radians: z
                                .union([z.number(), z.null()])
                                .optional(),
                              position: z
                                .union([
                                  z
                                    .array(z.number())
                                    .min(3)
                                    .max(3)
                                    .describe(
                                      "The tool pose at `joints`; absent when the controller has no tool link.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The tool pose at `joints`; absent when the controller has no tool link.",
                                    ),
                                ])
                                .describe(
                                  "The tool pose at `joints`; absent when the controller has no tool link.",
                                )
                                .optional(),
                              position_error_meters: z
                                .union([z.number(), z.null()])
                                .optional(),
                              quaternion: z
                                .union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ])
                                .optional(),
                              sampled_at: z.number(),
                            })
                            .describe(
                              "Where the arm was, measured, when a command ended.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      checkpoints: z.array(
                        z
                          .object({
                            label: z.string(),
                            measured_joints: z.array(z.number()),
                            target_position: z
                              .union([
                                z.array(z.number()).min(3).max(3),
                                z.null(),
                              ])
                              .optional(),
                            target_quaternion: z
                              .union([
                                z.array(z.number()).min(4).max(4),
                                z.null(),
                              ])
                              .optional(),
                          })
                          .describe(
                            "Where the arm stood, as measured, when it settled at the end of a segment.",
                          ),
                      ),
                      command_id: z.string(),
                      completion: z
                        .union([
                          z
                            .object({
                              detail: z
                                .union([z.string(), z.null()])
                                .optional(),
                              finished_at: z.number(),
                              gripper: z
                                .union([
                                  z
                                    .union([
                                      z
                                        .object({
                                          kind: z.literal("reached"),
                                          measured_angle_radians: z.number(),
                                        })
                                        .describe(
                                          "The jaw arrived at the angle it was asked for.",
                                        ),
                                      z
                                        .object({
                                          kind: z.literal("stopped_short"),
                                          measured_angle_radians: z.number(),
                                          target_angle_radians: z.number(),
                                        })
                                        .describe(
                                          "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                        ),
                                    ])
                                    .describe(
                                      "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "How a gripper command ended; null for every other command.",
                                )
                                .optional(),
                              outcome: z
                                .enum([
                                  "completed",
                                  "stopped",
                                  "tripped",
                                  "timed_out",
                                  "stopped_short",
                                  "superseded",
                                  "faulted",
                                  "interrupted",
                                  "failed",
                                ])
                                .describe("How a command ended."),
                            })
                            .describe(
                              "The outcome and timestamp are recorded together when a command ends.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      goal_joints: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                          z
                            .null()
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                        ])
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        )
                        .optional(),
                      group_id: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                          z
                            .null()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                        ])
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        )
                        .optional(),
                      joint_speed: z
                        .union([
                          z
                            .number()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                          z
                            .null()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                        ])
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        )
                        .optional(),
                      label: z
                        .string()
                        .describe(
                          "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                        ),
                      path_limited: z
                        .union([
                          z
                            .object({ reason: z.string() })
                            .describe(
                              "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      pose_limited: z
                        .union([
                          z
                            .object({
                              advice: z
                                .string()
                                .describe(
                                  "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                ),
                              orientation_shortfall_radians: z
                                .number()
                                .describe(
                                  "From the closest pose's orientation to the requested one.",
                                ),
                              position_shortfall_meters: z
                                .number()
                                .describe(
                                  "From the closest pose's tool position to the requested one.",
                                ),
                              reason: z
                                .union([
                                  z
                                    .literal("at_joint_limit")
                                    .describe(
                                      "The best pose it found has a joint at its limit, and no step improves it.",
                                    ),
                                  z
                                    .literal("stalled")
                                    .describe(
                                      "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                    ),
                                  z
                                    .literal("iteration_limit")
                                    .describe(
                                      "It was still improving when it ran out of iterations.",
                                    ),
                                  z
                                    .literal("out_of_reach")
                                    .describe(
                                      "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                    ),
                                ])
                                .describe("Why a solve stopped short."),
                              waypoint: z
                                .number()
                                .int()
                                .gte(0)
                                .describe(
                                  "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                ),
                            })
                            .describe(
                              "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      target_joints: z
                        .union([z.array(z.number()), z.null()])
                        .optional(),
                      target_position: z
                        .union([z.array(z.number()).min(3).max(3), z.null()])
                        .optional(),
                      target_quaternion: z
                        .union([z.array(z.number()).min(4).max(4), z.null()])
                        .optional(),
                      target_status: z
                        .union([
                          z
                            .enum([
                              "reached",
                              "stopped_short",
                              "partial",
                              "not_started",
                            ])
                            .describe(
                              "Whether the arm ended where the command was aimed.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      trace_context: z
                        .union([
                          z
                            .object({
                              traceparent: z
                                .string()
                                .describe(
                                  "`00-<trace id>-<parent span id>-<flags>`.",
                                ),
                              tracestate: z
                                .union([z.string(), z.null()])
                                .default(null),
                            })
                            .describe(
                              "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "The trace context the command arrived with, if any.",
                        )
                        .optional(),
                    })
                    .strict()
                    .describe("One command, from acceptance to its end."),
                )
                .describe(
                  "The commands the arm still remembers, oldest first, as they stand.",
                ),
              sample: z
                .union([
                  z
                    .object({
                      active_command: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The command being carried out, when one is.",
                            ),
                          z
                            .null()
                            .describe(
                              "The command being carried out, when one is.",
                            ),
                        ])
                        .describe("The command being carried out, when one is.")
                        .optional(),
                      active_trip: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The protective check currently preventing new motion, if any.",
                            ),
                          z
                            .null()
                            .describe(
                              "The protective check currently preventing new motion, if any.",
                            ),
                        ])
                        .describe(
                          "The protective check currently preventing new motion, if any.",
                        )
                        .default(null),
                      commanded_positions: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Joint setpoint successfully sent this cycle, if any.",
                            ),
                          z
                            .null()
                            .describe(
                              "Joint setpoint successfully sent this cycle, if any.",
                            ),
                        ])
                        .describe(
                          "Joint setpoint successfully sent this cycle, if any.",
                        )
                        .optional(),
                      cycle: z.number().int().gte(0),
                      cycle_seconds: z.number(),
                      efforts: z
                        .array(z.number())
                        .describe(
                          "Each arm joint's load, in percent of the servo's rated load.",
                        ),
                      fault: z
                        .union([
                          z
                            .string()
                            .describe(
                              "A fault the hardware reports this cycle, tripped or not.",
                            ),
                          z
                            .null()
                            .describe(
                              "A fault the hardware reports this cycle, tripped or not.",
                            ),
                        ])
                        .describe(
                          "A fault the hardware reports this cycle, tripped or not.",
                        )
                        .optional(),
                      gripper_angle: z.union([z.number(), z.null()]).optional(),
                      hold_pose: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                            ),
                          z
                            .null()
                            .describe(
                              "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                            ),
                        ])
                        .describe(
                          "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                        )
                        .optional(),
                      mode: z
                        .union([
                          z
                            .literal("disabled")
                            .describe("Torque off; only `enable` is accepted."),
                          z
                            .literal("holding")
                            .describe("Torque on, holding between commands."),
                          z
                            .literal("moving")
                            .describe("Following a motion or a stream."),
                          z
                            .literal("faulted")
                            .describe(
                              "Powered off after a hard fault; refuses everything until `clear_fault`.",
                            ),
                          z
                            .literal("disconnected")
                            .describe(
                              "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                            ),
                          z
                            .literal("reconnected")
                            .describe(
                              "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                            ),
                        ])
                        .describe("Where the arm stands."),
                      positions: z.array(z.number()),
                      recovery_issue: z
                        .union([
                          z
                            .object({
                              detail: z.string(),
                              motors_without_torque: z.array(
                                z.number().int().gte(0).lte(255),
                              ),
                            })
                            .describe(
                              "Why automatic recovery stopped, including motors found without torque.",
                            ),
                          z.null(),
                        ])
                        .optional(),
                      rest_pose: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                            ),
                          z
                            .null()
                            .describe(
                              "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                            ),
                        ])
                        .describe(
                          "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                        )
                        .optional(),
                      segment: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The segment of the active motion being followed.",
                            ),
                          z
                            .null()
                            .describe(
                              "The segment of the active motion being followed.",
                            ),
                        ])
                        .describe(
                          "The segment of the active motion being followed.",
                        )
                        .optional(),
                      temperatures_celsius: z
                        .array(z.number())
                        .describe(
                          "The last reading of each driven motor, the gripper's last; empty in\nsimulation. Sent while tripped too, so a holding arm that heats shows.",
                        ),
                      velocities: z.array(z.number()),
                      voltages_volts: z
                        .array(z.number())
                        .describe(
                          "The supply voltage each driven motor last reported, as the temperatures.",
                        ),
                    })
                    .describe(
                      "One control cycle's state; a sample, so a slow client may miss some.",
                    ),
                  z.null(),
                ])
                .optional(),
              seq: z
                .number()
                .int()
                .gte(0)
                .describe(
                  "The newest sequence number issued; subscribing after it misses nothing.",
                ),
            })
            .describe(
              "The answer to `snapshot`, and to `subscribe` when events were lost.",
            ),
          z
            .object({
              after: z.number().int().gte(0),
              kind: z.literal("resync"),
              newest: z.number().int().gte(0),
              oldest_retained: z.number().int().gte(0),
            })
            .describe(
              "A resume point the retained log no longer reaches, or that is ahead of\nit: events between `after` and `oldest_retained` are lost. A snapshot\nfollows.",
            ),
          z
            .object({
              accepted_at: z.number(),
              achieved: z
                .union([
                  z
                    .object({
                      joint_error_radians: z
                        .union([z.number(), z.null()])
                        .optional(),
                      joint_errors_radians: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            ),
                          z
                            .null()
                            .describe(
                              "Each joint's absolute distance from the target joints.",
                            ),
                        ])
                        .describe(
                          "Each joint's absolute distance from the target joints.",
                        )
                        .optional(),
                      joints: z.array(z.number()),
                      orientation: z
                        .union([
                          z
                            .object({
                              approach: z.array(z.number()).min(3).max(3),
                              degrees_from_down: z.number(),
                              jaw_yaw_degrees: z.number(),
                              jaw_yaw_reference: z
                                .enum(["world_x", "world_y"])
                                .describe(
                                  "The world axis jaw yaw zero is projected from.",
                                ),
                              target: z
                                .union([
                                  z
                                    .object({
                                      approach_error_degrees: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      orientation_error_degrees: z.number(),
                                    })
                                    .describe(
                                      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                    ),
                                  z.null(),
                                ])
                                .optional(),
                            })
                            .describe(
                              "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "The tool orientation in grasp terms, against the target's when there is one.",
                        )
                        .optional(),
                      orientation_error_radians: z
                        .union([z.number(), z.null()])
                        .optional(),
                      position: z
                        .union([
                          z
                            .array(z.number())
                            .min(3)
                            .max(3)
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            ),
                          z
                            .null()
                            .describe(
                              "The tool pose at `joints`; absent when the controller has no tool link.",
                            ),
                        ])
                        .describe(
                          "The tool pose at `joints`; absent when the controller has no tool link.",
                        )
                        .optional(),
                      position_error_meters: z
                        .union([z.number(), z.null()])
                        .optional(),
                      quaternion: z
                        .union([z.array(z.number()).min(4).max(4), z.null()])
                        .optional(),
                      sampled_at: z.number(),
                    })
                    .describe(
                      "Where the arm was, measured, when a command ended.",
                    ),
                  z.null(),
                ])
                .optional(),
              checkpoints: z.array(
                z
                  .object({
                    label: z.string(),
                    measured_joints: z.array(z.number()),
                    target_position: z
                      .union([z.array(z.number()).min(3).max(3), z.null()])
                      .optional(),
                    target_quaternion: z
                      .union([z.array(z.number()).min(4).max(4), z.null()])
                      .optional(),
                  })
                  .describe(
                    "Where the arm stood, as measured, when it settled at the end of a segment.",
                  ),
              ),
              command_id: z.string(),
              completion: z
                .union([
                  z
                    .object({
                      detail: z.union([z.string(), z.null()]).optional(),
                      finished_at: z.number(),
                      gripper: z
                        .union([
                          z
                            .union([
                              z
                                .object({
                                  kind: z.literal("reached"),
                                  measured_angle_radians: z.number(),
                                })
                                .describe(
                                  "The jaw arrived at the angle it was asked for.",
                                ),
                              z
                                .object({
                                  kind: z.literal("stopped_short"),
                                  measured_angle_radians: z.number(),
                                  target_angle_radians: z.number(),
                                })
                                .describe(
                                  "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                ),
                            ])
                            .describe(
                              "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "How a gripper command ended; null for every other command.",
                        )
                        .optional(),
                      outcome: z
                        .enum([
                          "completed",
                          "stopped",
                          "tripped",
                          "timed_out",
                          "stopped_short",
                          "superseded",
                          "faulted",
                          "interrupted",
                          "failed",
                        ])
                        .describe("How a command ended."),
                    })
                    .describe(
                      "The outcome and timestamp are recorded together when a command ends.",
                    ),
                  z.null(),
                ])
                .optional(),
              goal_joints: z
                .union([
                  z
                    .array(z.number())
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                  z
                    .null()
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                ])
                .describe(
                  "The newest goal of a joint stream, after clamping to the joint limits.",
                )
                .optional(),
              group_id: z
                .union([
                  z
                    .string()
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                  z
                    .null()
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                ])
                .describe(
                  "The grouped motion this command is one arm's part of, if it is.",
                )
                .optional(),
              joint_speed: z
                .union([
                  z
                    .number()
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                  z
                    .null()
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                ])
                .describe("Speed actually used, after clamping, for motions.")
                .optional(),
              kind: z.literal("record"),
              label: z
                .string()
                .describe(
                  "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                ),
              path_limited: z
                .union([
                  z
                    .object({ reason: z.string() })
                    .describe(
                      "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                    ),
                  z.null(),
                ])
                .optional(),
              pose_limited: z
                .union([
                  z
                    .object({
                      advice: z
                        .string()
                        .describe(
                          "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                        ),
                      orientation_shortfall_radians: z
                        .number()
                        .describe(
                          "From the closest pose's orientation to the requested one.",
                        ),
                      position_shortfall_meters: z
                        .number()
                        .describe(
                          "From the closest pose's tool position to the requested one.",
                        ),
                      reason: z
                        .union([
                          z
                            .literal("at_joint_limit")
                            .describe(
                              "The best pose it found has a joint at its limit, and no step improves it.",
                            ),
                          z
                            .literal("stalled")
                            .describe(
                              "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                            ),
                          z
                            .literal("iteration_limit")
                            .describe(
                              "It was still improving when it ran out of iterations.",
                            ),
                          z
                            .literal("out_of_reach")
                            .describe(
                              "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                            ),
                        ])
                        .describe("Why a solve stopped short."),
                      waypoint: z
                        .number()
                        .int()
                        .gte(0)
                        .describe(
                          "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                        ),
                    })
                    .describe(
                      "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                    ),
                  z.null(),
                ])
                .optional(),
              target_joints: z
                .union([z.array(z.number()), z.null()])
                .optional(),
              target_position: z
                .union([z.array(z.number()).min(3).max(3), z.null()])
                .optional(),
              target_quaternion: z
                .union([z.array(z.number()).min(4).max(4), z.null()])
                .optional(),
              target_status: z
                .union([
                  z
                    .enum([
                      "reached",
                      "stopped_short",
                      "partial",
                      "not_started",
                    ])
                    .describe(
                      "Whether the arm ended where the command was aimed.",
                    ),
                  z.null(),
                ])
                .optional(),
              trace_context: z
                .union([
                  z
                    .object({
                      traceparent: z
                        .string()
                        .describe("`00-<trace id>-<parent span id>-<flags>`."),
                      tracestate: z.union([z.string(), z.null()]).default(null),
                    })
                    .describe(
                      "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                    ),
                  z.null(),
                ])
                .describe("The trace context the command arrived with, if any.")
                .optional(),
            })
            .strict()
            .describe(
              "A command's record changed: accepted, started, a checkpoint, ended.",
            ),
          z
            .object({
              fault: z
                .union([
                  z.string().describe("Why, after a fault."),
                  z.null().describe("Why, after a fault."),
                ])
                .describe("Why, after a fault.")
                .optional(),
              kind: z.literal("mode"),
              mode: z
                .union([
                  z
                    .literal("disabled")
                    .describe("Torque off; only `enable` is accepted."),
                  z
                    .literal("holding")
                    .describe("Torque on, holding between commands."),
                  z
                    .literal("moving")
                    .describe("Following a motion or a stream."),
                  z
                    .literal("faulted")
                    .describe(
                      "Powered off after a hard fault; refuses everything until `clear_fault`.",
                    ),
                  z
                    .literal("disconnected")
                    .describe(
                      "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                    ),
                  z
                    .literal("reconnected")
                    .describe(
                      "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                    ),
                ])
                .describe("Where the arm stands."),
            })
            .describe("The arm changed mode."),
          z
            .object({ kind: z.literal("rejected"), reason: z.string() })
            .describe(
              "A request that was not acted on, when it carried no request id to reply to.",
            ),
          z
            .object({ kind: z.literal("tripped"), reason: z.string() })
            .describe(
              "A safety check stopped the arm, which holds where it is. Reported once\neach time a check starts failing.",
            ),
          z
            .object({
              kind: z.literal("controller_unavailable"),
              reason: z.string(),
            })
            .describe(
              "The station lost its motor-controller process; the arm's torque is unknown.",
            ),
          z
            .object({ kind: z.literal("controller_available") })
            .describe(
              "A fresh controller passed startup checks and the arm is disabled.",
            ),
        ])
        .describe("Controller to client."),
      seq: z
        .union([
          z
            .number()
            .int()
            .gte(0)
            .describe(
              "Present on log events only; numbers rise by one with no gaps.",
            ),
          z
            .null()
            .describe(
              "Present on log events only; numbers rise by one with no gaps.",
            ),
        ])
        .describe(
          "Present on log events only; numbers rise by one with no gaps.",
        )
        .optional(),
    })
    .describe(
      "One line from the controller: a log event with its sequence number, or an\nunsequenced sample or reply.",
    ),
  GripperResult: z
    .union([
      z
        .object({
          kind: z.literal("reached"),
          measured_angle_radians: z.number(),
        })
        .describe("The jaw arrived at the angle it was asked for."),
      z
        .object({
          kind: z.literal("stopped_short"),
          measured_angle_radians: z.number(),
          target_angle_radians: z.number(),
        })
        .describe(
          "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
        ),
    ])
    .describe(
      "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
    ),
  GroupCommandSubmissionRequest: z
    .object({
      command: z
        .object({
          max_joint_speed: z.union([z.number(), z.null()]).default(null),
          targets: z.record(
            z.string(),
            z
              .union([
                z.object({
                  joints: z.array(z.number()),
                  kind: z.literal("joints"),
                }),
                z.object({
                  kind: z.literal("pose"),
                  orientation: z
                    .union([
                      z
                        .object({
                          approach: z
                            .union([
                              z
                                .union([
                                  z.literal("down").describe("World -Z."),
                                  z.literal("forward").describe("World +X."),
                                ])
                                .describe("A named approach direction."),
                              z.array(z.number()).min(3).max(3),
                            ])
                            .describe(
                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                            ),
                          jaw_yaw_degrees: z.number(),
                          kind: z.literal("grasp"),
                        })
                        .describe(
                          "The approach direction and the jaw yaw about it, in degrees.",
                        ),
                      z
                        .object({
                          kind: z.literal("quaternion"),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("A raw quaternion, `[x, y, z, w]`."),
                    ])
                    .describe("A tool orientation as a client gives it."),
                  position: z.array(z.number()).min(3).max(3),
                }),
              ])
              .describe(
                "One corner of a move: joints, or a tool pose solved for joints.",
              ),
          ),
        })
        .strict()
        .describe(
          "Move several arms together: every target is planned before any arm moves,\none refusal refuses all, and when one arm ends without completing the others\nstop and hold. No collision checking between the arms.",
        ),
      expected_revisions: z
        .record(z.string(), z.number().int().gte(0))
        .default({}),
      lease_token: z.union([z.string(), z.null()]).default(null),
      request_id: z
        .string()
        .describe("A caller-generated UUID v4 identifying this submission."),
      station_instance_id: z.string(),
      trace_context: z
        .union([
          z
            .object({
              traceparent: z
                .string()
                .describe("`00-<trace id>-<parent span id>-<flags>`."),
              tracestate: z.union([z.string(), z.null()]).default(null),
            })
            .describe(
              "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
            ),
          z.null(),
        ])
        .default(null),
    })
    .strict()
    .describe(
      "Submit a coordinated multi-arm move with retry-safe request identity.",
    ),
  IkFailure: z
    .union([
      z
        .literal("at_joint_limit")
        .describe(
          "The best pose it found has a joint at its limit, and no step improves it.",
        ),
      z
        .literal("stalled")
        .describe(
          "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
        ),
      z
        .literal("iteration_limit")
        .describe("It was still improving when it ran out of iterations."),
      z
        .literal("out_of_reach")
        .describe(
          "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
        ),
    ])
    .describe("Why a solve stopped short."),
  IkPose: z
    .object({
      joints: z.array(z.number()),
      orientation_error_radians: z
        .number()
        .describe(
          "From the required orientation, or from the seed's when it was free.",
        ),
      position_error_meters: z.number(),
      tool_position: z.array(z.number()).min(3).max(3),
      tool_quaternion: z.array(z.number()).min(4).max(4),
    })
    .describe("Where a solve left the arm."),
  IkResult: z
    .union([
      z
        .object({
          joints: z.array(z.number()),
          orientation_error_radians: z
            .number()
            .describe(
              "From the required orientation, or from the seed's when it was free.",
            ),
          position_error_meters: z.number(),
          tool_position: z.array(z.number()).min(3).max(3),
          tool_quaternion: z.array(z.number()).min(4).max(4),
          outcome: z.literal("solved"),
        })
        .describe("The tool is within tolerance of the target."),
      z
        .object({
          best: z
            .object({
              joints: z.array(z.number()),
              orientation_error_radians: z
                .number()
                .describe(
                  "From the required orientation, or from the seed's when it was free.",
                ),
              position_error_meters: z.number(),
              tool_position: z.array(z.number()).min(3).max(3),
              tool_quaternion: z.array(z.number()).min(4).max(4),
            })
            .describe("Where a solve left the arm."),
          outcome: z.literal("failed"),
          reason: z
            .union([
              z
                .literal("at_joint_limit")
                .describe(
                  "The best pose it found has a joint at its limit, and no step improves it.",
                ),
              z
                .literal("stalled")
                .describe(
                  "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                ),
              z
                .literal("iteration_limit")
                .describe(
                  "It was still improving when it ran out of iterations.",
                ),
              z
                .literal("out_of_reach")
                .describe(
                  "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                ),
            ])
            .describe("Why a solve stopped short."),
        })
        .describe(
          "It is not; `best` is the pose with the smallest position error found,\nalways inside the joint limits.",
        ),
    ])
    .describe("A solve's outcome."),
  Interlock: z
    .union([
      z
        .object({ kind: z.literal("arm_disconnected") })
        .describe(
          "The bus stopped answering. Clears when the arm reconnects and holds.",
        ),
      z
        .object({
          kind: z.literal("recovery_approval_required"),
          motors: z.array(z.string()),
        })
        .describe(
          "The arm reconnected unpowered or failed its checks. Clears when an\noperator sends `approve_recovery` after supporting the arm.",
        ),
      z
        .object({
          camera: z.string(),
          kind: z.literal("required_camera_unavailable"),
        })
        .describe(
          "A camera in the lease's `required_cameras` dropped. Clears when the\nholder reports a fresh frame, or when the lease ends.",
        ),
    ])
    .describe(
      "A condition on an arm that refuses motion until it clears. `stop`,\n`disable`, `approve_recovery` and `clear_fault` are still accepted.",
    ),
  JawClosingAxis: z
    .enum(["x", "y"])
    .describe("The tool-frame axis the gripper's jaws close along."),
  JawYawRange: z
    .object({
      from_degrees: z.number(),
      reachable: z.boolean(),
      reason: z.union([z.string(), z.null()]).optional(),
      to_degrees: z.number(),
    })
    .describe(
      "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
    ),
  JawYawReference: z
    .enum(["world_x", "world_y"])
    .describe("The world axis jaw yaw zero is projected from."),
  KnownRunEvent: z
    .union([
      z.object({
        task: z.union([z.string(), z.null()]).default(null),
        type: z.literal("run_started"),
      }),
      z.object({
        steps: z.array(
          z.object({
            status: z.enum(["pending", "active", "done", "failed"]),
            text: z.string(),
          }),
        ),
        type: z.literal("plan_update"),
      }),
      z.object({ notes: z.array(z.string()), type: z.literal("notes_update") }),
      z.object({
        type: z.literal("usage_update"),
        usage: z
          .object({
            cached_input_tokens: z.number().int().gte(0),
            input_tokens: z.number().int().gte(0),
            output_tokens: z.number().int().gte(0),
            reasoning_tokens: z.number().int().gte(0),
            run_id: z.string(),
            turns: z.number().int().gte(0),
          })
          .describe("A model run's token use so far."),
      }),
    ])
    .describe("A run event the server acts on."),
  LeaseAcquireRequest: z
    .object({
      request: z
        .object({
          holder: z
            .union([
              z
                .literal("agent")
                .describe(
                  "A model loop, started from the terminal or the agent server.",
                ),
              z
                .literal("manual")
                .describe(
                  "A person driving the arms from the viewer's manual controls.",
                ),
            ])
            .describe("The kind of controller holding the lease."),
          label: z.string(),
          on_interruption: z
            .union([
              z
                .literal("wait_for_approval")
                .describe(
                  "Keep the lease and wait for an operator's `approve_recovery`.",
                ),
              z.literal("return_lease").describe("Give the lease up."),
            ])
            .describe(
              "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
            )
            .default("wait_for_approval"),
          required_cameras: z.array(z.string()).default([]),
        })
        .strict()
        .describe(
          "The requested holder and interruption policy for a station lease.",
        ),
      request_id: z
        .string()
        .describe(
          "Caller-generated UUID v4, reused when retrying this acquisition.",
        ),
      station_instance_id: z
        .string()
        .describe(
          "The station process observed before this acquisition was created.",
        ),
      take: z.boolean().default(false),
    })
    .strict()
    .describe(
      "Acquire a station lease, optionally taking it from the current holder.",
    ),
  LeaseAnswer: z
    .union([
      z
        .object({
          lease_seconds: z.number(),
          lease_token: z.string(),
          status: z
            .object({
              expires_in_seconds: z
                .number()
                .describe(
                  "The lease lapses this long after its grant or last explicit renewal.",
                ),
              holder: z
                .union([
                  z
                    .literal("agent")
                    .describe(
                      "A model loop, started from the terminal or the agent server.",
                    ),
                  z
                    .literal("manual")
                    .describe(
                      "A person driving the arms from the viewer's manual controls.",
                    ),
                ])
                .describe("The kind of controller holding the lease."),
              label: z.string(),
              on_interruption: z
                .union([
                  z
                    .literal("wait_for_approval")
                    .describe(
                      "Keep the lease and wait for an operator's `approve_recovery`.",
                    ),
                  z.literal("return_lease").describe("Give the lease up."),
                ])
                .describe(
                  "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                ),
              required_cameras: z
                .array(z.string())
                .describe("Cameras the holder cannot work without."),
            })
            .describe("The current lease, as everyone may see it."),
          kind: z.literal("granted"),
        })
        .describe("The lease is yours."),
      z
        .object({
          kind: z.literal("holder"),
          status: z
            .union([
              z
                .object({
                  expires_in_seconds: z
                    .number()
                    .describe(
                      "The lease lapses this long after its grant or last explicit renewal.",
                    ),
                  holder: z
                    .union([
                      z
                        .literal("agent")
                        .describe(
                          "A model loop, started from the terminal or the agent server.",
                        ),
                      z
                        .literal("manual")
                        .describe(
                          "A person driving the arms from the viewer's manual controls.",
                        ),
                    ])
                    .describe("The kind of controller holding the lease."),
                  label: z.string(),
                  on_interruption: z
                    .union([
                      z
                        .literal("wait_for_approval")
                        .describe(
                          "Keep the lease and wait for an operator's `approve_recovery`.",
                        ),
                      z.literal("return_lease").describe("Give the lease up."),
                    ])
                    .describe(
                      "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                    ),
                  required_cameras: z
                    .array(z.string())
                    .describe("Cameras the holder cannot work without."),
                })
                .describe("The current lease, as everyone may see it."),
              z.null(),
            ])
            .optional(),
        })
        .describe("Who holds the lease, if anyone."),
    ])
    .describe("What a request about the lease answers."),
  LeaseCameraReport: z
    .object({
      available: z.boolean(),
      camera: z.string(),
      lease_token: z.string(),
    })
    .strict()
    .describe(
      "Report a required camera's availability under the current lease.",
    ),
  LeaseGrant: z
    .object({
      lease_seconds: z.number(),
      lease_token: z.string(),
      status: z
        .object({
          expires_in_seconds: z
            .number()
            .describe(
              "The lease lapses this long after its grant or last explicit renewal.",
            ),
          holder: z
            .union([
              z
                .literal("agent")
                .describe(
                  "A model loop, started from the terminal or the agent server.",
                ),
              z
                .literal("manual")
                .describe(
                  "A person driving the arms from the viewer's manual controls.",
                ),
            ])
            .describe("The kind of controller holding the lease."),
          label: z.string(),
          on_interruption: z
            .union([
              z
                .literal("wait_for_approval")
                .describe(
                  "Keep the lease and wait for an operator's `approve_recovery`.",
                ),
              z.literal("return_lease").describe("Give the lease up."),
            ])
            .describe(
              "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
            ),
          required_cameras: z
            .array(z.string())
            .describe("Cameras the holder cannot work without."),
        })
        .describe("The current lease, as everyone may see it."),
    })
    .describe("Proof of the lease: send the token with every command."),
  LeaseHolder: z
    .union([
      z
        .literal("agent")
        .describe(
          "A model loop, started from the terminal or the agent server.",
        ),
      z
        .literal("manual")
        .describe(
          "A person driving the arms from the viewer's manual controls.",
        ),
    ])
    .describe("The kind of controller holding the lease."),
  LeaseRequest: z
    .object({
      holder: z
        .union([
          z
            .literal("agent")
            .describe(
              "A model loop, started from the terminal or the agent server.",
            ),
          z
            .literal("manual")
            .describe(
              "A person driving the arms from the viewer's manual controls.",
            ),
        ])
        .describe("The kind of controller holding the lease."),
      label: z.string(),
      on_interruption: z
        .union([
          z
            .literal("wait_for_approval")
            .describe(
              "Keep the lease and wait for an operator's `approve_recovery`.",
            ),
          z.literal("return_lease").describe("Give the lease up."),
        ])
        .describe(
          "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
        )
        .default("wait_for_approval"),
      required_cameras: z.array(z.string()).default([]),
    })
    .strict()
    .describe(
      "The requested holder and interruption policy for a station lease.",
    ),
  LeaseStatus: z
    .object({
      expires_in_seconds: z
        .number()
        .describe(
          "The lease lapses this long after its grant or last explicit renewal.",
        ),
      holder: z
        .union([
          z
            .literal("agent")
            .describe(
              "A model loop, started from the terminal or the agent server.",
            ),
          z
            .literal("manual")
            .describe(
              "A person driving the arms from the viewer's manual controls.",
            ),
        ])
        .describe("The kind of controller holding the lease."),
      label: z.string(),
      on_interruption: z
        .union([
          z
            .literal("wait_for_approval")
            .describe(
              "Keep the lease and wait for an operator's `approve_recovery`.",
            ),
          z.literal("return_lease").describe("Give the lease up."),
        ])
        .describe(
          "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
        ),
      required_cameras: z
        .array(z.string())
        .describe("Cameras the holder cannot work without."),
    })
    .describe("The current lease, as everyone may see it."),
  LeaseToken: z
    .object({ lease_token: z.string() })
    .strict()
    .describe("A lease token, for routes that act only while it is current."),
  LiveRun: z
    .object({
      memory: z
        .object({
          notes: z.array(z.string()),
          plan: z.union([
            z.array(
              z.object({
                status: z.enum(["pending", "active", "done", "failed"]),
                text: z.string(),
              }),
            ),
            z.null(),
          ]),
          task: z.union([z.string(), z.null()]),
        })
        .describe(
          "What the model keeps in mind: its task, its plan and its notes.",
        ),
      revision: z.number().int().gte(0),
      spans: z.array(
        z
          .object({
            attributes: z.record(z.string(), z.any()).default({}),
            end_time_unix_nano: z.string().regex(/^[0-9]+$/),
            name: z.string(),
            parent_span_id: z.union([z.string(), z.null()]).default(null),
            span_id: z.string(),
            start_time_unix_nano: z.string().regex(/^[0-9]+$/),
            status: z
              .object({
                code: z
                  .enum([
                    "STATUS_CODE_UNSET",
                    "STATUS_CODE_OK",
                    "STATUS_CODE_ERROR",
                  ])
                  .default("STATUS_CODE_UNSET"),
                message: z.union([z.string(), z.null()]).default(null),
              })
              .default({ code: "STATUS_CODE_UNSET", message: null }),
            trace_id: z.string(),
          })
          .describe(
            "One span of a model loop's trace, in OpenTelemetry's terms.",
          ),
      ),
      usage: z.union([
        z
          .object({
            cached_input_tokens: z.number().int().gte(0),
            input_tokens: z.number().int().gte(0),
            output_tokens: z.number().int().gte(0),
            reasoning_tokens: z.number().int().gte(0),
            run_id: z.string(),
            turns: z.number().int().gte(0),
          })
          .describe("A model run's token use so far."),
        z.null(),
      ]),
    })
    .describe("The model loop's live telemetry, as the viewer draws it."),
  MediaState: z.union([
    z.object({ state: z.literal("running") }),
    z
      .object({ state: z.literal("not_configured") })
      .describe(
        "Not asked for: `publish` without `LiveKit` credentials or USB cameras.",
      ),
    z
      .object({ last_exit: z.string(), state: z.literal("restarting") })
      .describe("It exited and will be started again."),
    z
      .object({ reason: z.string(), state: z.literal("unavailable") })
      .describe(
        "It cannot be started, for instance because the binary is missing.",
      ),
  ]),
  MediaStatusReport: z
    .object({
      cameras: z.record(
        z.string(),
        z
          .object({
            detail: z
              .string()
              .describe(
                "The program's message, such as `camera backend: Could not open device …`.",
              ),
            last_seen_unix_seconds: z.number(),
          })
          .describe(
            "Why a camera is not producing frames: the latest error a media program printed for it.",
          ),
      ),
      programs: z.record(
        z.string(),
        z.union([
          z.object({ state: z.literal("running") }),
          z
            .object({ state: z.literal("not_configured") })
            .describe(
              "Not asked for: `publish` without `LiveKit` credentials or USB cameras.",
            ),
          z
            .object({ last_exit: z.string(), state: z.literal("restarting") })
            .describe("It exited and will be started again."),
          z
            .object({ reason: z.string(), state: z.literal("unavailable") })
            .describe(
              "It cannot be started, for instance because the binary is missing.",
            ),
        ]),
      ),
    })
    .describe(
      "`GET /viewer/api/media-status`: each program's state and each camera's latest problem.\nA problem stays until a newer one replaces it, so read it only for a camera that is\nunavailable.",
    ),
  MoveArmsCommand: z
    .object({
      max_joint_speed: z.union([z.number(), z.null()]).default(null),
      targets: z.record(
        z.string(),
        z
          .union([
            z.object({
              joints: z.array(z.number()),
              kind: z.literal("joints"),
            }),
            z.object({
              kind: z.literal("pose"),
              orientation: z
                .union([
                  z
                    .object({
                      approach: z
                        .union([
                          z
                            .union([
                              z.literal("down").describe("World -Z."),
                              z.literal("forward").describe("World +X."),
                            ])
                            .describe("A named approach direction."),
                          z.array(z.number()).min(3).max(3),
                        ])
                        .describe(
                          "The direction the jaws point along: a named one, or a nonzero world direction.",
                        ),
                      jaw_yaw_degrees: z.number(),
                      kind: z.literal("grasp"),
                    })
                    .describe(
                      "The approach direction and the jaw yaw about it, in degrees.",
                    ),
                  z
                    .object({
                      kind: z.literal("quaternion"),
                      quaternion: z.array(z.number()).min(4).max(4),
                    })
                    .describe("A raw quaternion, `[x, y, z, w]`."),
                ])
                .describe("A tool orientation as a client gives it."),
              position: z.array(z.number()).min(3).max(3),
            }),
          ])
          .describe(
            "One corner of a move: joints, or a tool pose solved for joints.",
          ),
      ),
    })
    .strict()
    .describe(
      "Move several arms together: every target is planned before any arm moves,\none refusal refuses all, and when one arm ends without completing the others\nstop and hold. No collision checking between the arms.",
    ),
  NamedApproach: z
    .union([
      z.literal("down").describe("World -Z."),
      z.literal("forward").describe("World +X."),
    ])
    .describe("A named approach direction."),
  OnInterruption: z
    .union([
      z
        .literal("wait_for_approval")
        .describe(
          "Keep the lease and wait for an operator's `approve_recovery`.",
        ),
      z.literal("return_lease").describe("Give the lease up."),
    ])
    .describe(
      "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
    ),
  OrientationSummary: z
    .object({
      approach: z.array(z.number()).min(3).max(3),
      degrees_from_down: z.number(),
      jaw_yaw_degrees: z.number(),
      jaw_yaw_reference: z
        .enum(["world_x", "world_y"])
        .describe("The world axis jaw yaw zero is projected from."),
      target: z
        .union([
          z
            .object({
              approach_error_degrees: z.number(),
              jaw_yaw_degrees: z.number(),
              jaw_yaw_reference: z
                .enum(["world_x", "world_y"])
                .describe("The world axis jaw yaw zero is projected from."),
              orientation_error_degrees: z.number(),
            })
            .describe(
              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
            ),
          z.null(),
        ])
        .optional(),
    })
    .describe(
      "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
    ),
  PathLimited: z
    .object({ reason: z.string() })
    .describe(
      "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
    ),
  PhoneInvite: z
    .object({
      camera_name: z.string(),
      qr_code_image: z.string(),
      url: z.string(),
    })
    .describe(
      "A phone invite: scan `url` to stream the phone as `camera_name`.",
    ),
  PhoneInviteRequest: z
    .object({
      label: z.string().describe("The phone streams as `phone_<label>`."),
    })
    .describe("`POST /viewer/api/phone-invites`."),
  PlanStep: z.object({
    status: z.enum(["pending", "active", "done", "failed"]),
    text: z.string(),
  }),
  PlanStepStatus: z.enum(["pending", "active", "done", "failed"]),
  PlannedMotion: z
    .object({
      path_limited: z
        .union([
          z
            .object({ reason: z.string() })
            .describe(
              "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
            ),
          z.null(),
        ])
        .optional(),
      plan: z.object({
        peak_speed: z.number(),
        segments: z.array(
          z
            .object({
              corners: z.array(z.array(z.number())),
              label: z.string(),
              minimum_duration_seconds: z
                .union([
                  z
                    .number()
                    .describe(
                      "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                    ),
                  z
                    .null()
                    .describe(
                      "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                    ),
                ])
                .describe(
                  "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                )
                .default(null),
              target_position: z
                .union([z.array(z.number()).min(3).max(3), z.null()])
                .default(null),
              target_quaternion: z
                .union([z.array(z.number()).min(4).max(4), z.null()])
                .default(null),
            })
            .describe(
              "One straight run between corners, with where it is meant to end.",
            ),
        ),
      }),
      pose_limited: z
        .union([
          z
            .object({
              advice: z
                .string()
                .describe(
                  "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                ),
              orientation_shortfall_radians: z
                .number()
                .describe(
                  "From the closest pose's orientation to the requested one.",
                ),
              position_shortfall_meters: z
                .number()
                .describe(
                  "From the closest pose's tool position to the requested one.",
                ),
              reason: z
                .union([
                  z
                    .literal("at_joint_limit")
                    .describe(
                      "The best pose it found has a joint at its limit, and no step improves it.",
                    ),
                  z
                    .literal("stalled")
                    .describe(
                      "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                    ),
                  z
                    .literal("iteration_limit")
                    .describe(
                      "It was still improving when it ran out of iterations.",
                    ),
                  z
                    .literal("out_of_reach")
                    .describe(
                      "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                    ),
                ])
                .describe("Why a solve stopped short."),
              waypoint: z
                .number()
                .int()
                .gte(0)
                .describe(
                  "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                ),
            })
            .describe(
              "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
            ),
          z.null(),
        ])
        .optional(),
    })
    .describe(
      "What the arm is asked to run, and why it stops short of the target if it does.",
    ),
  PoseDescription: z
    .object({
      model_points: z
        .array(z.array(z.number()).min(3).max(3))
        .describe(
          "Each joint's origin, then the tool's: the points a viewer draws the arm through.",
        ),
      orientation: z
        .object({
          approach: z.array(z.number()).min(3).max(3),
          degrees_from_down: z.number(),
          jaw_yaw_degrees: z.number(),
          jaw_yaw_reference: z
            .enum(["world_x", "world_y"])
            .describe("The world axis jaw yaw zero is projected from."),
          target: z
            .union([
              z
                .object({
                  approach_error_degrees: z.number(),
                  jaw_yaw_degrees: z.number(),
                  jaw_yaw_reference: z
                    .enum(["world_x", "world_y"])
                    .describe("The world axis jaw yaw zero is projected from."),
                  orientation_error_degrees: z.number(),
                })
                .describe(
                  "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                ),
              z.null(),
            ])
            .optional(),
        })
        .describe(
          "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
        ),
      position: z.array(z.number()).min(3).max(3),
      quaternion: z.array(z.number()).min(4).max(4),
    })
    .describe("The tool's pose at some joints, in the station's world frame."),
  PoseLimited: z
    .object({
      advice: z
        .string()
        .describe(
          "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
        ),
      orientation_shortfall_radians: z
        .number()
        .describe("From the closest pose's orientation to the requested one."),
      position_shortfall_meters: z
        .number()
        .describe(
          "From the closest pose's tool position to the requested one.",
        ),
      reason: z
        .union([
          z
            .literal("at_joint_limit")
            .describe(
              "The best pose it found has a joint at its limit, and no step improves it.",
            ),
          z
            .literal("stalled")
            .describe(
              "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
            ),
          z
            .literal("iteration_limit")
            .describe("It was still improving when it ran out of iterations."),
          z
            .literal("out_of_reach")
            .describe(
              "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
            ),
        ])
        .describe("Why a solve stopped short."),
      waypoint: z
        .number()
        .int()
        .gte(0)
        .describe(
          "The waypoint the move stops at, counted from 0; later waypoints are left out.",
        ),
    })
    .describe(
      "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
    ),
  PoseOrientation: z
    .union([
      z
        .object({
          approach: z
            .union([
              z
                .union([
                  z.literal("down").describe("World -Z."),
                  z.literal("forward").describe("World +X."),
                ])
                .describe("A named approach direction."),
              z.array(z.number()).min(3).max(3),
            ])
            .describe(
              "The direction the jaws point along: a named one, or a nonzero world direction.",
            ),
          jaw_yaw_degrees: z.number(),
          kind: z.literal("grasp"),
        })
        .describe(
          "The approach direction and the jaw yaw about it, in degrees.",
        ),
      z
        .object({
          kind: z.literal("quaternion"),
          quaternion: z.array(z.number()).min(4).max(4),
        })
        .describe("A raw quaternion, `[x, y, z, w]`."),
    ])
    .describe("A tool orientation as a client gives it."),
  PosePreviewBatchRequest: z
    .object({
      arm: z.string(),
      poses: z
        .array(
          z
            .object({
              id: z
                .string()
                .describe(
                  "A caller-generated UUID v4, unique within the batch.",
                ),
              label: z
                .union([
                  z
                    .string()
                    .describe(
                      "Optional human-readable context, echoed with the answer.",
                    ),
                  z
                    .null()
                    .describe(
                      "Optional human-readable context, echoed with the answer.",
                    ),
                ])
                .describe(
                  "Optional human-readable context, echoed with the answer.",
                )
                .default(null),
              pose: z
                .object({
                  orientation: z
                    .union([
                      z
                        .union([
                          z
                            .object({
                              approach: z
                                .union([
                                  z
                                    .union([
                                      z.literal("down").describe("World -Z."),
                                      z
                                        .literal("forward")
                                        .describe("World +X."),
                                    ])
                                    .describe("A named approach direction."),
                                  z.array(z.number()).min(3).max(3),
                                ])
                                .describe(
                                  "The direction the jaws point along: a named one, or a nonzero world direction.",
                                ),
                              jaw_yaw_degrees: z.number(),
                              kind: z.literal("grasp"),
                            })
                            .describe(
                              "The approach direction and the jaw yaw about it, in degrees.",
                            ),
                          z
                            .object({
                              kind: z.literal("quaternion"),
                              quaternion: z.array(z.number()).min(4).max(4),
                            })
                            .describe("A raw quaternion, `[x, y, z, w]`."),
                        ])
                        .describe("A tool orientation as a client gives it."),
                      z.null(),
                    ])
                    .describe("None leaves the orientation to the solver.")
                    .default(null),
                  position: z.array(z.number()).min(3).max(3),
                })
                .strict()
                .describe(
                  "Where the arm's controller would take the tool toward a pose, without moving.",
                ),
            })
            .strict(),
        )
        .min(1)
        .describe("At least one independent candidate."),
    })
    .strict()
    .describe(
      "Independent pose candidates for one arm; identifiers are supplied by the caller.",
    ),
  PosePreviewBatchResponse: z.object({
    results: z
      .array(
        z.object({
          answer: z
            .union([
              z.object({
                projection: z
                  .object({
                    calculation_milliseconds: z.number(),
                    jaw_yaw_ranges: z
                      .array(
                        z
                          .object({
                            from_degrees: z.number(),
                            reachable: z.boolean(),
                            reason: z.union([z.string(), z.null()]).optional(),
                            to_degrees: z.number(),
                          })
                          .describe(
                            "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
                          ),
                      )
                      .describe(
                        "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
                      ),
                    outcome: z
                      .union([
                        z
                          .object({
                            kind: z.literal("projected"),
                            pose: z
                              .object({
                                joints: z.array(z.number()),
                                orientation: z
                                  .object({
                                    approach: z.array(z.number()).min(3).max(3),
                                    jaw_yaw_degrees: z.number(),
                                  })
                                  .describe(
                                    "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                                  ),
                                orientation_error_radians: z
                                  .union([
                                    z
                                      .number()
                                      .describe(
                                        "From the requested orientation; none when the solver chose it.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "From the requested orientation; none when the solver chose it.",
                                      ),
                                  ])
                                  .describe(
                                    "From the requested orientation; none when the solver chose it.",
                                  )
                                  .optional(),
                                position: z.array(z.number()).min(3).max(3),
                                position_error_meters: z.number(),
                                quaternion: z.array(z.number()).min(4).max(4),
                              })
                              .describe("Where the arm would put the tool."),
                            reason: z
                              .union([
                                z
                                  .string()
                                  .describe(
                                    "Why the target was not met; none when it was met within tolerance.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Why the target was not met; none when it was met within tolerance.",
                                  ),
                              ])
                              .describe(
                                "Why the target was not met; none when it was met within tolerance.",
                              )
                              .optional(),
                            within_tolerance: z
                              .boolean()
                              .describe(
                                "The solver or controller met the target without a path constraint.",
                              ),
                          })
                          .describe(
                            "The best pose found, including an approximation when the target was not met.",
                          ),
                        z
                          .object({
                            kind: z.literal("unavailable"),
                            reason: z.string(),
                          })
                          .describe(
                            "The preview could not produce a usable pose.",
                          ),
                      ])
                      .describe(
                        "A usable projected pose, or why the preview could not produce one.",
                      ),
                    requested: z
                      .union([
                        z
                          .object({
                            orientation: z
                              .object({
                                approach: z.array(z.number()).min(3).max(3),
                                jaw_yaw_degrees: z.number(),
                              })
                              .describe(
                                "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                              ),
                            quaternion: z.array(z.number()).min(4).max(4),
                          })
                          .describe(
                            "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
                          ),
                        z.null(),
                      ])
                      .describe(
                        "None for a position-only target the solver chose the orientation for.",
                      )
                      .optional(),
                  })
                  .describe("The answer to a projection."),
                status: z.literal("projected"),
              }),
              z.object({
                detail: z.string(),
                reason: z
                  .enum([
                    "arm_busy",
                    "not_enabled",
                    "stale_revision",
                    "interlocked",
                    "faulted",
                    "invalid_target",
                    "stream_ended",
                    "unreachable",
                    "lease_held_elsewhere",
                    "malformed",
                  ])
                  .describe("Why a command was not accepted."),
                status: z.literal("refused"),
              }),
            ])
            .describe("How a preview was answered."),
          id: z.string(),
          label: z.union([z.string(), z.null()]).optional(),
        }),
      )
      .describe("One answer per candidate, in request order."),
  }),
  PosePreviewCandidate: z
    .object({
      id: z
        .string()
        .describe("A caller-generated UUID v4, unique within the batch."),
      label: z
        .union([
          z
            .string()
            .describe(
              "Optional human-readable context, echoed with the answer.",
            ),
          z
            .null()
            .describe(
              "Optional human-readable context, echoed with the answer.",
            ),
        ])
        .describe("Optional human-readable context, echoed with the answer.")
        .default(null),
      pose: z
        .object({
          orientation: z
            .union([
              z
                .union([
                  z
                    .object({
                      approach: z
                        .union([
                          z
                            .union([
                              z.literal("down").describe("World -Z."),
                              z.literal("forward").describe("World +X."),
                            ])
                            .describe("A named approach direction."),
                          z.array(z.number()).min(3).max(3),
                        ])
                        .describe(
                          "The direction the jaws point along: a named one, or a nonzero world direction.",
                        ),
                      jaw_yaw_degrees: z.number(),
                      kind: z.literal("grasp"),
                    })
                    .describe(
                      "The approach direction and the jaw yaw about it, in degrees.",
                    ),
                  z
                    .object({
                      kind: z.literal("quaternion"),
                      quaternion: z.array(z.number()).min(4).max(4),
                    })
                    .describe("A raw quaternion, `[x, y, z, w]`."),
                ])
                .describe("A tool orientation as a client gives it."),
              z.null(),
            ])
            .describe("None leaves the orientation to the solver.")
            .default(null),
          position: z.array(z.number()).min(3).max(3),
        })
        .strict()
        .describe(
          "Where the arm's controller would take the tool toward a pose, without moving.",
        ),
    })
    .strict(),
  PosePreviewCandidateResult: z.object({
    answer: z
      .union([
        z.object({
          projection: z
            .object({
              calculation_milliseconds: z.number(),
              jaw_yaw_ranges: z
                .array(
                  z
                    .object({
                      from_degrees: z.number(),
                      reachable: z.boolean(),
                      reason: z.union([z.string(), z.null()]).optional(),
                      to_degrees: z.number(),
                    })
                    .describe(
                      "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
                    ),
                )
                .describe(
                  "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
                ),
              outcome: z
                .union([
                  z
                    .object({
                      kind: z.literal("projected"),
                      pose: z
                        .object({
                          joints: z.array(z.number()),
                          orientation: z
                            .object({
                              approach: z.array(z.number()).min(3).max(3),
                              jaw_yaw_degrees: z.number(),
                            })
                            .describe(
                              "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                            ),
                          orientation_error_radians: z
                            .union([
                              z
                                .number()
                                .describe(
                                  "From the requested orientation; none when the solver chose it.",
                                ),
                              z
                                .null()
                                .describe(
                                  "From the requested orientation; none when the solver chose it.",
                                ),
                            ])
                            .describe(
                              "From the requested orientation; none when the solver chose it.",
                            )
                            .optional(),
                          position: z.array(z.number()).min(3).max(3),
                          position_error_meters: z.number(),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("Where the arm would put the tool."),
                      reason: z
                        .union([
                          z
                            .string()
                            .describe(
                              "Why the target was not met; none when it was met within tolerance.",
                            ),
                          z
                            .null()
                            .describe(
                              "Why the target was not met; none when it was met within tolerance.",
                            ),
                        ])
                        .describe(
                          "Why the target was not met; none when it was met within tolerance.",
                        )
                        .optional(),
                      within_tolerance: z
                        .boolean()
                        .describe(
                          "The solver or controller met the target without a path constraint.",
                        ),
                    })
                    .describe(
                      "The best pose found, including an approximation when the target was not met.",
                    ),
                  z
                    .object({
                      kind: z.literal("unavailable"),
                      reason: z.string(),
                    })
                    .describe("The preview could not produce a usable pose."),
                ])
                .describe(
                  "A usable projected pose, or why the preview could not produce one.",
                ),
              requested: z
                .union([
                  z
                    .object({
                      orientation: z
                        .object({
                          approach: z.array(z.number()).min(3).max(3),
                          jaw_yaw_degrees: z.number(),
                        })
                        .describe(
                          "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                        ),
                      quaternion: z.array(z.number()).min(4).max(4),
                    })
                    .describe(
                      "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
                    ),
                  z.null(),
                ])
                .describe(
                  "None for a position-only target the solver chose the orientation for.",
                )
                .optional(),
            })
            .describe("The answer to a projection."),
          status: z.literal("projected"),
        }),
        z.object({
          detail: z.string(),
          reason: z
            .enum([
              "arm_busy",
              "not_enabled",
              "stale_revision",
              "interlocked",
              "faulted",
              "invalid_target",
              "stream_ended",
              "unreachable",
              "lease_held_elsewhere",
              "malformed",
            ])
            .describe("Why a command was not accepted."),
          status: z.literal("refused"),
        }),
      ])
      .describe("How a preview was answered."),
    id: z.string(),
    label: z.union([z.string(), z.null()]).optional(),
  }),
  PoseProjection: z
    .object({
      calculation_milliseconds: z.number(),
      jaw_yaw_ranges: z
        .array(
          z
            .object({
              from_degrees: z.number(),
              reachable: z.boolean(),
              reason: z.union([z.string(), z.null()]).optional(),
              to_degrees: z.number(),
            })
            .describe(
              "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
            ),
        )
        .describe(
          "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
        ),
      outcome: z
        .union([
          z
            .object({
              kind: z.literal("projected"),
              pose: z
                .object({
                  joints: z.array(z.number()),
                  orientation: z
                    .object({
                      approach: z.array(z.number()).min(3).max(3),
                      jaw_yaw_degrees: z.number(),
                    })
                    .describe(
                      "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                    ),
                  orientation_error_radians: z
                    .union([
                      z
                        .number()
                        .describe(
                          "From the requested orientation; none when the solver chose it.",
                        ),
                      z
                        .null()
                        .describe(
                          "From the requested orientation; none when the solver chose it.",
                        ),
                    ])
                    .describe(
                      "From the requested orientation; none when the solver chose it.",
                    )
                    .optional(),
                  position: z.array(z.number()).min(3).max(3),
                  position_error_meters: z.number(),
                  quaternion: z.array(z.number()).min(4).max(4),
                })
                .describe("Where the arm would put the tool."),
              reason: z
                .union([
                  z
                    .string()
                    .describe(
                      "Why the target was not met; none when it was met within tolerance.",
                    ),
                  z
                    .null()
                    .describe(
                      "Why the target was not met; none when it was met within tolerance.",
                    ),
                ])
                .describe(
                  "Why the target was not met; none when it was met within tolerance.",
                )
                .optional(),
              within_tolerance: z
                .boolean()
                .describe(
                  "The solver or controller met the target without a path constraint.",
                ),
            })
            .describe(
              "The best pose found, including an approximation when the target was not met.",
            ),
          z
            .object({ kind: z.literal("unavailable"), reason: z.string() })
            .describe("The preview could not produce a usable pose."),
        ])
        .describe(
          "A usable projected pose, or why the preview could not produce one.",
        ),
      requested: z
        .union([
          z
            .object({
              orientation: z
                .object({
                  approach: z.array(z.number()).min(3).max(3),
                  jaw_yaw_degrees: z.number(),
                })
                .describe(
                  "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                ),
              quaternion: z.array(z.number()).min(4).max(4),
            })
            .describe(
              "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
            ),
          z.null(),
        ])
        .describe(
          "None for a position-only target the solver chose the orientation for.",
        )
        .optional(),
    })
    .describe("The answer to a projection."),
  PreviewAnswer: z
    .union([
      z.object({
        projection: z
          .object({
            calculation_milliseconds: z.number(),
            jaw_yaw_ranges: z
              .array(
                z
                  .object({
                    from_degrees: z.number(),
                    reachable: z.boolean(),
                    reason: z.union([z.string(), z.null()]).optional(),
                    to_degrees: z.number(),
                  })
                  .describe(
                    "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
                  ),
              )
              .describe(
                "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
              ),
            outcome: z
              .union([
                z
                  .object({
                    kind: z.literal("projected"),
                    pose: z
                      .object({
                        joints: z.array(z.number()),
                        orientation: z
                          .object({
                            approach: z.array(z.number()).min(3).max(3),
                            jaw_yaw_degrees: z.number(),
                          })
                          .describe(
                            "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                          ),
                        orientation_error_radians: z
                          .union([
                            z
                              .number()
                              .describe(
                                "From the requested orientation; none when the solver chose it.",
                              ),
                            z
                              .null()
                              .describe(
                                "From the requested orientation; none when the solver chose it.",
                              ),
                          ])
                          .describe(
                            "From the requested orientation; none when the solver chose it.",
                          )
                          .optional(),
                        position: z.array(z.number()).min(3).max(3),
                        position_error_meters: z.number(),
                        quaternion: z.array(z.number()).min(4).max(4),
                      })
                      .describe("Where the arm would put the tool."),
                    reason: z
                      .union([
                        z
                          .string()
                          .describe(
                            "Why the target was not met; none when it was met within tolerance.",
                          ),
                        z
                          .null()
                          .describe(
                            "Why the target was not met; none when it was met within tolerance.",
                          ),
                      ])
                      .describe(
                        "Why the target was not met; none when it was met within tolerance.",
                      )
                      .optional(),
                    within_tolerance: z
                      .boolean()
                      .describe(
                        "The solver or controller met the target without a path constraint.",
                      ),
                  })
                  .describe(
                    "The best pose found, including an approximation when the target was not met.",
                  ),
                z
                  .object({
                    kind: z.literal("unavailable"),
                    reason: z.string(),
                  })
                  .describe("The preview could not produce a usable pose."),
              ])
              .describe(
                "A usable projected pose, or why the preview could not produce one.",
              ),
            requested: z
              .union([
                z
                  .object({
                    orientation: z
                      .object({
                        approach: z.array(z.number()).min(3).max(3),
                        jaw_yaw_degrees: z.number(),
                      })
                      .describe(
                        "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                      ),
                    quaternion: z.array(z.number()).min(4).max(4),
                  })
                  .describe(
                    "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
                  ),
                z.null(),
              ])
              .describe(
                "None for a position-only target the solver chose the orientation for.",
              )
              .optional(),
          })
          .describe("The answer to a projection."),
        status: z.literal("projected"),
      }),
      z.object({
        detail: z.string(),
        reason: z
          .enum([
            "arm_busy",
            "not_enabled",
            "stale_revision",
            "interlocked",
            "faulted",
            "invalid_target",
            "stream_ended",
            "unreachable",
            "lease_held_elsewhere",
            "malformed",
          ])
          .describe("Why a command was not accepted."),
        status: z.literal("refused"),
      }),
    ])
    .describe("How a preview was answered."),
  PreviewPose: z
    .object({
      orientation: z
        .union([
          z
            .union([
              z
                .object({
                  approach: z
                    .union([
                      z
                        .union([
                          z.literal("down").describe("World -Z."),
                          z.literal("forward").describe("World +X."),
                        ])
                        .describe("A named approach direction."),
                      z.array(z.number()).min(3).max(3),
                    ])
                    .describe(
                      "The direction the jaws point along: a named one, or a nonzero world direction.",
                    ),
                  jaw_yaw_degrees: z.number(),
                  kind: z.literal("grasp"),
                })
                .describe(
                  "The approach direction and the jaw yaw about it, in degrees.",
                ),
              z
                .object({
                  kind: z.literal("quaternion"),
                  quaternion: z.array(z.number()).min(4).max(4),
                })
                .describe("A raw quaternion, `[x, y, z, w]`."),
            ])
            .describe("A tool orientation as a client gives it."),
          z.null(),
        ])
        .describe("None leaves the orientation to the solver.")
        .default(null),
      position: z.array(z.number()).min(3).max(3),
    })
    .strict()
    .describe(
      "Where the arm's controller would take the tool toward a pose, without moving.",
    ),
  ProgramContinuation: z
    .object({
      next_step: z.number().int().gte(0),
      prior_execution_id: z.string(),
    })
    .strict()
    .describe(
      "Explicitly select the unattempted suffix of a finished execution.",
    ),
  ProjectedPose: z
    .object({
      joints: z.array(z.number()),
      orientation: z
        .object({
          approach: z.array(z.number()).min(3).max(3),
          jaw_yaw_degrees: z.number(),
        })
        .describe(
          "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
        ),
      orientation_error_radians: z
        .union([
          z
            .number()
            .describe(
              "From the requested orientation; none when the solver chose it.",
            ),
          z
            .null()
            .describe(
              "From the requested orientation; none when the solver chose it.",
            ),
        ])
        .describe(
          "From the requested orientation; none when the solver chose it.",
        )
        .optional(),
      position: z.array(z.number()).min(3).max(3),
      position_error_meters: z.number(),
      quaternion: z.array(z.number()).min(4).max(4),
    })
    .describe("Where the arm would put the tool."),
  ProjectionOutcome: z
    .union([
      z
        .object({
          kind: z.literal("projected"),
          pose: z
            .object({
              joints: z.array(z.number()),
              orientation: z
                .object({
                  approach: z.array(z.number()).min(3).max(3),
                  jaw_yaw_degrees: z.number(),
                })
                .describe(
                  "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                ),
              orientation_error_radians: z
                .union([
                  z
                    .number()
                    .describe(
                      "From the requested orientation; none when the solver chose it.",
                    ),
                  z
                    .null()
                    .describe(
                      "From the requested orientation; none when the solver chose it.",
                    ),
                ])
                .describe(
                  "From the requested orientation; none when the solver chose it.",
                )
                .optional(),
              position: z.array(z.number()).min(3).max(3),
              position_error_meters: z.number(),
              quaternion: z.array(z.number()).min(4).max(4),
            })
            .describe("Where the arm would put the tool."),
          reason: z
            .union([
              z
                .string()
                .describe(
                  "Why the target was not met; none when it was met within tolerance.",
                ),
              z
                .null()
                .describe(
                  "Why the target was not met; none when it was met within tolerance.",
                ),
            ])
            .describe(
              "Why the target was not met; none when it was met within tolerance.",
            )
            .optional(),
          within_tolerance: z
            .boolean()
            .describe(
              "The solver or controller met the target without a path constraint.",
            ),
        })
        .describe(
          "The best pose found, including an approximation when the target was not met.",
        ),
      z
        .object({ kind: z.literal("unavailable"), reason: z.string() })
        .describe("The preview could not produce a usable pose."),
    ])
    .describe(
      "A usable projected pose, or why the preview could not produce one.",
    ),
  ReconnectRequest: z.object({ supported: z.boolean() }),
  Recording: z
    .object({
      mcapPath: z.union([z.string(), z.null()]).optional(),
      name: z.string(),
      sizeBytes: z.number().int().gte(0),
    })
    .describe(
      "One recording directory. Its data, video included, is `recording.mcap`.",
    ),
  RecordingStatus: z
    .union([
      z.object({ state: z.literal("idle") }),
      z.object({ name: z.string(), state: z.literal("recording") }),
      z.object({ name: z.string(), state: z.literal("stopping") }),
      z.object({ name: z.string(), state: z.literal("finished") }),
      z.object({
        detail: z.string(),
        name: z.string(),
        state: z.literal("failed"),
      }),
    ])
    .describe(
      "Where the station's recorder is. Recording starts and stops only on request,\nindependent of power and the lease.",
    ),
  Recordings: z.object({
    active: z.union([z.string(), z.null()]).optional(),
    recordings: z.array(
      z
        .object({
          mcapPath: z.union([z.string(), z.null()]).optional(),
          name: z.string(),
          sizeBytes: z.number().int().gte(0),
        })
        .describe(
          "One recording directory. Its data, video included, is `recording.mcap`.",
        ),
    ),
  }),
  RejectionReason: z
    .enum([
      "arm_busy",
      "not_enabled",
      "stale_revision",
      "interlocked",
      "faulted",
      "invalid_target",
      "stream_ended",
      "unreachable",
      "lease_held_elsewhere",
      "malformed",
    ])
    .describe("Why a command was not accepted."),
  RequestedOrientation: z
    .object({
      orientation: z
        .object({
          approach: z.array(z.number()).min(3).max(3),
          jaw_yaw_degrees: z.number(),
        })
        .describe(
          "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
        ),
      quaternion: z.array(z.number()).min(4).max(4),
    })
    .describe(
      "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
    ),
  RoomAccess: z.object({ server_url: z.string(), token: z.string() }),
  RunCompiledProgramRequest: z
    .object({
      compilation_id: z.string(),
      continuation: z
        .union([
          z
            .object({
              next_step: z.number().int().gte(0),
              prior_execution_id: z.string(),
            })
            .strict()
            .describe(
              "Explicitly select the unattempted suffix of a finished execution.",
            ),
          z.null(),
        ])
        .default(null),
      expected_revisions: z
        .record(z.string(), z.number().int().gte(0))
        .default({}),
      lease_token: z.string(),
      request_id: z.string(),
      station_instance_id: z.string(),
    })
    .strict()
    .describe(
      "Submit a station-owned compilation under current execution authority.",
    ),
  RunEvent: z
    .union([
      z
        .union([
          z.object({
            task: z.union([z.string(), z.null()]).default(null),
            type: z.literal("run_started"),
          }),
          z.object({
            steps: z.array(
              z.object({
                status: z.enum(["pending", "active", "done", "failed"]),
                text: z.string(),
              }),
            ),
            type: z.literal("plan_update"),
          }),
          z.object({
            notes: z.array(z.string()),
            type: z.literal("notes_update"),
          }),
          z.object({
            type: z.literal("usage_update"),
            usage: z
              .object({
                cached_input_tokens: z.number().int().gte(0),
                input_tokens: z.number().int().gte(0),
                output_tokens: z.number().int().gte(0),
                reasoning_tokens: z.number().int().gte(0),
                run_id: z.string(),
                turns: z.number().int().gte(0),
              })
              .describe("A model run's token use so far."),
          }),
        ])
        .describe("A run event the server acts on."),
      z.any(),
    ])
    .describe(
      "An event a model loop reports: one the server acts on, or any other kept\nas it came.",
    ),
  RunMemory: z
    .object({
      notes: z.array(z.string()),
      plan: z
        .union([
          z.array(
            z.object({
              status: z.enum(["pending", "active", "done", "failed"]),
              text: z.string(),
            }),
          ),
          z.null(),
        ])
        .optional(),
      task: z.union([z.string(), z.null()]).optional(),
    })
    .describe(
      "What the model keeps in mind: its task, its plan and its notes.",
    ),
  RunSpansRequest: z
    .object({
      spans: z.array(
        z
          .object({
            attributes: z.record(z.string(), z.any()).default({}),
            end_time_unix_nano: z.string().regex(/^[0-9]+$/),
            name: z.string(),
            parent_span_id: z.union([z.string(), z.null()]).default(null),
            span_id: z.string(),
            start_time_unix_nano: z.string().regex(/^[0-9]+$/),
            status: z
              .object({
                code: z
                  .enum([
                    "STATUS_CODE_UNSET",
                    "STATUS_CODE_OK",
                    "STATUS_CODE_ERROR",
                  ])
                  .default("STATUS_CODE_UNSET"),
                message: z.union([z.string(), z.null()]).default(null),
              })
              .default({ code: "STATUS_CODE_UNSET", message: null }),
            trace_id: z.string(),
          })
          .describe(
            "One span of a model loop's trace, in OpenTelemetry's terms.",
          ),
      ),
    })
    .strict()
    .describe("A set of model-loop trace span updates."),
  RunUsage: z
    .object({
      cached_input_tokens: z.number().int().gte(0),
      input_tokens: z.number().int().gte(0),
      output_tokens: z.number().int().gte(0),
      reasoning_tokens: z.number().int().gte(0),
      run_id: z.string(),
      turns: z.number().int().gte(0),
    })
    .describe("A model run's token use so far."),
  Sample: z
    .object({
      active_command: z
        .union([
          z.string().describe("The command being carried out, when one is."),
          z.null().describe("The command being carried out, when one is."),
        ])
        .describe("The command being carried out, when one is.")
        .optional(),
      active_trip: z
        .union([
          z
            .string()
            .describe(
              "The protective check currently preventing new motion, if any.",
            ),
          z
            .null()
            .describe(
              "The protective check currently preventing new motion, if any.",
            ),
        ])
        .describe(
          "The protective check currently preventing new motion, if any.",
        )
        .default(null),
      commanded_positions: z
        .union([
          z
            .array(z.number())
            .describe("Joint setpoint successfully sent this cycle, if any."),
          z
            .null()
            .describe("Joint setpoint successfully sent this cycle, if any."),
        ])
        .describe("Joint setpoint successfully sent this cycle, if any.")
        .optional(),
      cycle: z.number().int().gte(0),
      cycle_seconds: z.number(),
      efforts: z
        .array(z.number())
        .describe(
          "Each arm joint's load, in percent of the servo's rated load.",
        ),
      fault: z
        .union([
          z
            .string()
            .describe(
              "A fault the hardware reports this cycle, tripped or not.",
            ),
          z
            .null()
            .describe(
              "A fault the hardware reports this cycle, tripped or not.",
            ),
        ])
        .describe("A fault the hardware reports this cycle, tripped or not.")
        .optional(),
      gripper_angle: z.union([z.number(), z.null()]).optional(),
      hold_pose: z
        .union([
          z
            .array(z.number())
            .describe(
              "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
            ),
          z
            .null()
            .describe(
              "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
            ),
        ])
        .describe(
          "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
        )
        .optional(),
      mode: z
        .union([
          z
            .literal("disabled")
            .describe("Torque off; only `enable` is accepted."),
          z.literal("holding").describe("Torque on, holding between commands."),
          z.literal("moving").describe("Following a motion or a stream."),
          z
            .literal("faulted")
            .describe(
              "Powered off after a hard fault; refuses everything until `clear_fault`.",
            ),
          z
            .literal("disconnected")
            .describe(
              "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
            ),
          z
            .literal("reconnected")
            .describe(
              "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
            ),
        ])
        .describe("Where the arm stands."),
      positions: z.array(z.number()),
      recovery_issue: z
        .union([
          z
            .object({
              detail: z.string(),
              motors_without_torque: z.array(z.number().int().gte(0).lte(255)),
            })
            .describe(
              "Why automatic recovery stopped, including motors found without torque.",
            ),
          z.null(),
        ])
        .optional(),
      rest_pose: z
        .union([
          z
            .array(z.number())
            .describe(
              "Where the arm rested unpowered when it was enabled; shutdown returns there.",
            ),
          z
            .null()
            .describe(
              "Where the arm rested unpowered when it was enabled; shutdown returns there.",
            ),
        ])
        .describe(
          "Where the arm rested unpowered when it was enabled; shutdown returns there.",
        )
        .optional(),
      segment: z
        .union([
          z
            .string()
            .describe("The segment of the active motion being followed."),
          z.null().describe("The segment of the active motion being followed."),
        ])
        .describe("The segment of the active motion being followed.")
        .optional(),
      temperatures_celsius: z
        .array(z.number())
        .describe(
          "The last reading of each driven motor, the gripper's last; empty in\nsimulation. Sent while tripped too, so a holding arm that heats shows.",
        ),
      velocities: z.array(z.number()),
      voltages_volts: z
        .array(z.number())
        .describe(
          "The supply voltage each driven motor last reported, as the temperatures.",
        ),
    })
    .describe(
      "One control cycle's state; a sample, so a slow client may miss some.",
    ),
  Segment: z
    .object({
      corners: z.array(z.array(z.number())),
      label: z.string(),
      minimum_duration_seconds: z
        .union([
          z
            .number()
            .describe(
              "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
            ),
          z
            .null()
            .describe(
              "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
            ),
        ])
        .describe(
          "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
        )
        .default(null),
      target_position: z
        .union([z.array(z.number()).min(3).max(3), z.null()])
        .default(null),
      target_quaternion: z
        .union([z.array(z.number()).min(4).max(4), z.null()])
        .default(null),
    })
    .describe(
      "One straight run between corners, with where it is meant to end.",
    ),
  ServerMessage: z
    .union([
      z.object({
        action_sequence_id: z.string(),
        kind: z.literal("action_sequence_accepted"),
        request_id: z.union([z.string(), z.null()]),
      }),
      z
        .object({ arms: z.array(z.string()), kind: z.literal("hello") })
        .describe("The first message: the arms this controller serves."),
      z
        .object({
          arm: z.string(),
          frame: z
            .object({
              emitted_unix_nano: z
                .string()
                .regex(/^[0-9]+$/)
                .describe(
                  "When the controller's writer made the frame; a replayed log event keeps\nthe time it was first emitted.",
                ),
              event: z
                .union([
                  z
                    .object({
                      active_command: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The command being carried out, when one is.",
                            ),
                          z
                            .null()
                            .describe(
                              "The command being carried out, when one is.",
                            ),
                        ])
                        .describe(
                          "The command being carried out, when one is.",
                        ),
                      active_trip: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The protective check currently preventing new motion, if any.",
                            ),
                          z
                            .null()
                            .describe(
                              "The protective check currently preventing new motion, if any.",
                            ),
                        ])
                        .describe(
                          "The protective check currently preventing new motion, if any.",
                        )
                        .default(null),
                      commanded_positions: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Joint setpoint successfully sent this cycle, if any.",
                            ),
                          z
                            .null()
                            .describe(
                              "Joint setpoint successfully sent this cycle, if any.",
                            ),
                        ])
                        .describe(
                          "Joint setpoint successfully sent this cycle, if any.",
                        ),
                      cycle: z.number().int().gte(0),
                      cycle_seconds: z.number(),
                      efforts: z
                        .array(z.number())
                        .describe(
                          "Each arm joint's load, in percent of the servo's rated load.",
                        ),
                      fault: z
                        .union([
                          z
                            .string()
                            .describe(
                              "A fault the hardware reports this cycle, tripped or not.",
                            ),
                          z
                            .null()
                            .describe(
                              "A fault the hardware reports this cycle, tripped or not.",
                            ),
                        ])
                        .describe(
                          "A fault the hardware reports this cycle, tripped or not.",
                        ),
                      gripper_angle: z.union([z.number(), z.null()]),
                      hold_pose: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                            ),
                          z
                            .null()
                            .describe(
                              "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                            ),
                        ])
                        .describe(
                          "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                        ),
                      mode: z
                        .union([
                          z
                            .literal("disabled")
                            .describe("Torque off; only `enable` is accepted."),
                          z
                            .literal("holding")
                            .describe("Torque on, holding between commands."),
                          z
                            .literal("moving")
                            .describe("Following a motion or a stream."),
                          z
                            .literal("faulted")
                            .describe(
                              "Powered off after a hard fault; refuses everything until `clear_fault`.",
                            ),
                          z
                            .literal("disconnected")
                            .describe(
                              "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                            ),
                          z
                            .literal("reconnected")
                            .describe(
                              "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                            ),
                        ])
                        .describe("Where the arm stands."),
                      positions: z.array(z.number()),
                      recovery_issue: z.union([
                        z
                          .object({
                            detail: z.string(),
                            motors_without_torque: z.array(
                              z.number().int().gte(0).lte(255),
                            ),
                          })
                          .describe(
                            "Why automatic recovery stopped, including motors found without torque.",
                          ),
                        z.null(),
                      ]),
                      rest_pose: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                            ),
                          z
                            .null()
                            .describe(
                              "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                            ),
                        ])
                        .describe(
                          "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                        ),
                      segment: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The segment of the active motion being followed.",
                            ),
                          z
                            .null()
                            .describe(
                              "The segment of the active motion being followed.",
                            ),
                        ])
                        .describe(
                          "The segment of the active motion being followed.",
                        ),
                      temperatures_celsius: z
                        .array(z.number())
                        .describe(
                          "The last reading of each driven motor, the gripper's last; empty in\nsimulation. Sent while tripped too, so a holding arm that heats shows.",
                        ),
                      velocities: z.array(z.number()),
                      voltages_volts: z
                        .array(z.number())
                        .describe(
                          "The supply voltage each driven motor last reported, as the temperatures.",
                        ),
                      kind: z.literal("state"),
                    })
                    .describe(
                      "One control cycle's state; a sample, so a slow client may miss some.",
                    ),
                  z
                    .object({
                      accepted: z.boolean(),
                      description: z
                        .union([
                          z
                            .object({
                              model_points: z
                                .array(z.array(z.number()).min(3).max(3))
                                .describe(
                                  "Each joint's origin, then the tool's: the points a viewer draws the arm through.",
                                ),
                              orientation: z
                                .object({
                                  approach: z.array(z.number()).min(3).max(3),
                                  degrees_from_down: z.number(),
                                  jaw_yaw_degrees: z.number(),
                                  jaw_yaw_reference: z
                                    .enum(["world_x", "world_y"])
                                    .describe(
                                      "The world axis jaw yaw zero is projected from.",
                                    ),
                                  target: z.union([
                                    z
                                      .object({
                                        approach_error_degrees: z.number(),
                                        jaw_yaw_degrees: z.number(),
                                        jaw_yaw_reference: z
                                          .enum(["world_x", "world_y"])
                                          .describe(
                                            "The world axis jaw yaw zero is projected from.",
                                          ),
                                        orientation_error_degrees: z.number(),
                                      })
                                      .describe(
                                        "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                      ),
                                    z.null(),
                                  ]),
                                })
                                .describe(
                                  "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                ),
                              position: z.array(z.number()).min(3).max(3),
                              quaternion: z.array(z.number()).min(4).max(4),
                            })
                            .describe(
                              "The tool's pose at some joints, in the station's world frame.",
                            ),
                          z.null(),
                        ])
                        .describe("The answer to a `describe_pose`."),
                      detail: z.union([z.string(), z.null()]),
                      ik: z
                        .union([
                          z
                            .union([
                              z
                                .object({
                                  joints: z.array(z.number()),
                                  orientation_error_radians: z
                                    .number()
                                    .describe(
                                      "From the required orientation, or from the seed's when it was free.",
                                    ),
                                  position_error_meters: z.number(),
                                  tool_position: z
                                    .array(z.number())
                                    .min(3)
                                    .max(3),
                                  tool_quaternion: z
                                    .array(z.number())
                                    .min(4)
                                    .max(4),
                                  outcome: z.literal("solved"),
                                })
                                .describe(
                                  "The tool is within tolerance of the target.",
                                ),
                              z
                                .object({
                                  best: z
                                    .object({
                                      joints: z.array(z.number()),
                                      orientation_error_radians: z
                                        .number()
                                        .describe(
                                          "From the required orientation, or from the seed's when it was free.",
                                        ),
                                      position_error_meters: z.number(),
                                      tool_position: z
                                        .array(z.number())
                                        .min(3)
                                        .max(3),
                                      tool_quaternion: z
                                        .array(z.number())
                                        .min(4)
                                        .max(4),
                                    })
                                    .describe("Where a solve left the arm."),
                                  outcome: z.literal("failed"),
                                  reason: z
                                    .union([
                                      z
                                        .literal("at_joint_limit")
                                        .describe(
                                          "The best pose it found has a joint at its limit, and no step improves it.",
                                        ),
                                      z
                                        .literal("stalled")
                                        .describe(
                                          "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                        ),
                                      z
                                        .literal("iteration_limit")
                                        .describe(
                                          "It was still improving when it ran out of iterations.",
                                        ),
                                      z
                                        .literal("out_of_reach")
                                        .describe(
                                          "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                        ),
                                    ])
                                    .describe("Why a solve stopped short."),
                                })
                                .describe(
                                  "It is not; `best` is the pose with the smallest position error found,\nalways inside the joint limits.",
                                ),
                            ])
                            .describe("A solve's outcome."),
                          z.null(),
                        ])
                        .describe("How a `solve_pose` ended."),
                      kind: z.literal("reply"),
                      lease: z
                        .union([
                          z
                            .union([
                              z
                                .object({
                                  lease_seconds: z.number(),
                                  lease_token: z.string(),
                                  status: z
                                    .object({
                                      expires_in_seconds: z
                                        .number()
                                        .describe(
                                          "The lease lapses this long after its grant or last explicit renewal.",
                                        ),
                                      holder: z
                                        .union([
                                          z
                                            .literal("agent")
                                            .describe(
                                              "A model loop, started from the terminal or the agent server.",
                                            ),
                                          z
                                            .literal("manual")
                                            .describe(
                                              "A person driving the arms from the viewer's manual controls.",
                                            ),
                                        ])
                                        .describe(
                                          "The kind of controller holding the lease.",
                                        ),
                                      label: z.string(),
                                      on_interruption: z
                                        .union([
                                          z
                                            .literal("wait_for_approval")
                                            .describe(
                                              "Keep the lease and wait for an operator's `approve_recovery`.",
                                            ),
                                          z
                                            .literal("return_lease")
                                            .describe("Give the lease up."),
                                        ])
                                        .describe(
                                          "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                                        ),
                                      required_cameras: z
                                        .array(z.string())
                                        .describe(
                                          "Cameras the holder cannot work without.",
                                        ),
                                    })
                                    .describe(
                                      "The current lease, as everyone may see it.",
                                    ),
                                  kind: z.literal("granted"),
                                })
                                .describe("The lease is yours."),
                              z
                                .object({
                                  kind: z.literal("holder"),
                                  status: z.union([
                                    z
                                      .object({
                                        expires_in_seconds: z
                                          .number()
                                          .describe(
                                            "The lease lapses this long after its grant or last explicit renewal.",
                                          ),
                                        holder: z
                                          .union([
                                            z
                                              .literal("agent")
                                              .describe(
                                                "A model loop, started from the terminal or the agent server.",
                                              ),
                                            z
                                              .literal("manual")
                                              .describe(
                                                "A person driving the arms from the viewer's manual controls.",
                                              ),
                                          ])
                                          .describe(
                                            "The kind of controller holding the lease.",
                                          ),
                                        label: z.string(),
                                        on_interruption: z
                                          .union([
                                            z
                                              .literal("wait_for_approval")
                                              .describe(
                                                "Keep the lease and wait for an operator's `approve_recovery`.",
                                              ),
                                            z
                                              .literal("return_lease")
                                              .describe("Give the lease up."),
                                          ])
                                          .describe(
                                            "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                                          ),
                                        required_cameras: z
                                          .array(z.string())
                                          .describe(
                                            "Cameras the holder cannot work without.",
                                          ),
                                      })
                                      .describe(
                                        "The current lease, as everyone may see it.",
                                      ),
                                    z.null(),
                                  ]),
                                })
                                .describe("Who holds the lease, if anyone."),
                            ])
                            .describe(
                              "What a request about the lease answers.",
                            ),
                          z.null(),
                        ])
                        .describe("The answer to a request about the lease."),
                      plan: z
                        .union([
                          z
                            .object({
                              path_limited: z.union([
                                z
                                  .object({ reason: z.string() })
                                  .describe(
                                    "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                              plan: z.object({
                                peak_speed: z.number(),
                                segments: z.array(
                                  z
                                    .object({
                                      corners: z.array(z.array(z.number())),
                                      label: z.string(),
                                      minimum_duration_seconds: z
                                        .union([
                                          z
                                            .number()
                                            .describe(
                                              "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                            ),
                                          z
                                            .null()
                                            .describe(
                                              "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                            ),
                                        ])
                                        .describe(
                                          "Slows the section to last at least this long, so arms moving together\ncan arrive together. Only for a single straight section.",
                                        )
                                        .default(null),
                                      target_position: z
                                        .union([
                                          z.array(z.number()).min(3).max(3),
                                          z.null(),
                                        ])
                                        .default(null),
                                      target_quaternion: z
                                        .union([
                                          z.array(z.number()).min(4).max(4),
                                          z.null(),
                                        ])
                                        .default(null),
                                    })
                                    .describe(
                                      "One straight run between corners, with where it is meant to end.",
                                    ),
                                ),
                              }),
                              pose_limited: z.union([
                                z
                                  .object({
                                    advice: z
                                      .string()
                                      .describe(
                                        "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                      ),
                                    orientation_shortfall_radians: z
                                      .number()
                                      .describe(
                                        "From the closest pose's orientation to the requested one.",
                                      ),
                                    position_shortfall_meters: z
                                      .number()
                                      .describe(
                                        "From the closest pose's tool position to the requested one.",
                                      ),
                                    reason: z
                                      .union([
                                        z
                                          .literal("at_joint_limit")
                                          .describe(
                                            "The best pose it found has a joint at its limit, and no step improves it.",
                                          ),
                                        z
                                          .literal("stalled")
                                          .describe(
                                            "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                          ),
                                        z
                                          .literal("iteration_limit")
                                          .describe(
                                            "It was still improving when it ran out of iterations.",
                                          ),
                                        z
                                          .literal("out_of_reach")
                                          .describe(
                                            "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                          ),
                                      ])
                                      .describe("Why a solve stopped short."),
                                    waypoint: z
                                      .number()
                                      .int()
                                      .gte(0)
                                      .describe(
                                        "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                      ),
                                  })
                                  .describe(
                                    "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                            })
                            .describe(
                              "What the arm is asked to run, and why it stops short of the target if it does.",
                            ),
                          z.null(),
                        ])
                        .describe("The plan a `plan_motion` made."),
                      projection: z
                        .union([
                          z
                            .object({
                              calculation_milliseconds: z.number(),
                              jaw_yaw_ranges: z
                                .array(
                                  z
                                    .object({
                                      from_degrees: z.number(),
                                      reachable: z.boolean(),
                                      reason: z.union([z.string(), z.null()]),
                                      to_degrees: z.number(),
                                    })
                                    .describe(
                                      "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
                                    ),
                                )
                                .describe(
                                  "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
                                ),
                              outcome: z
                                .union([
                                  z
                                    .object({
                                      kind: z.literal("projected"),
                                      pose: z
                                        .object({
                                          joints: z.array(z.number()),
                                          orientation: z
                                            .object({
                                              approach: z
                                                .array(z.number())
                                                .min(3)
                                                .max(3),
                                              jaw_yaw_degrees: z.number(),
                                            })
                                            .describe(
                                              "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                                            ),
                                          orientation_error_radians: z
                                            .union([
                                              z
                                                .number()
                                                .describe(
                                                  "From the requested orientation; none when the solver chose it.",
                                                ),
                                              z
                                                .null()
                                                .describe(
                                                  "From the requested orientation; none when the solver chose it.",
                                                ),
                                            ])
                                            .describe(
                                              "From the requested orientation; none when the solver chose it.",
                                            ),
                                          position: z
                                            .array(z.number())
                                            .min(3)
                                            .max(3),
                                          position_error_meters: z.number(),
                                          quaternion: z
                                            .array(z.number())
                                            .min(4)
                                            .max(4),
                                        })
                                        .describe(
                                          "Where the arm would put the tool.",
                                        ),
                                      reason: z
                                        .union([
                                          z
                                            .string()
                                            .describe(
                                              "Why the target was not met; none when it was met within tolerance.",
                                            ),
                                          z
                                            .null()
                                            .describe(
                                              "Why the target was not met; none when it was met within tolerance.",
                                            ),
                                        ])
                                        .describe(
                                          "Why the target was not met; none when it was met within tolerance.",
                                        ),
                                      within_tolerance: z
                                        .boolean()
                                        .describe(
                                          "The solver or controller met the target without a path constraint.",
                                        ),
                                    })
                                    .describe(
                                      "The best pose found, including an approximation when the target was not met.",
                                    ),
                                  z
                                    .object({
                                      kind: z.literal("unavailable"),
                                      reason: z.string(),
                                    })
                                    .describe(
                                      "The preview could not produce a usable pose.",
                                    ),
                                ])
                                .describe(
                                  "A usable projected pose, or why the preview could not produce one.",
                                ),
                              requested: z
                                .union([
                                  z
                                    .object({
                                      orientation: z
                                        .object({
                                          approach: z
                                            .array(z.number())
                                            .min(3)
                                            .max(3),
                                          jaw_yaw_degrees: z.number(),
                                        })
                                        .describe(
                                          "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                                        ),
                                      quaternion: z
                                        .array(z.number())
                                        .min(4)
                                        .max(4),
                                    })
                                    .describe(
                                      "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "None for a position-only target the solver chose the orientation for.",
                                ),
                            })
                            .describe("The answer to a projection."),
                          z.null(),
                        ])
                        .describe("The answer to a `project_pose`."),
                      reason: z.union([
                        z
                          .enum([
                            "arm_busy",
                            "not_enabled",
                            "stale_revision",
                            "interlocked",
                            "faulted",
                            "invalid_target",
                            "stream_ended",
                            "unreachable",
                            "lease_held_elsewhere",
                            "malformed",
                          ])
                          .describe("Why a command was not accepted."),
                        z.null(),
                      ]),
                      record: z.union([
                        z
                          .object({
                            accepted_at: z.number(),
                            achieved: z.union([
                              z
                                .object({
                                  joint_error_radians: z.union([
                                    z.number(),
                                    z.null(),
                                  ]),
                                  joint_errors_radians: z
                                    .union([
                                      z
                                        .array(z.number())
                                        .describe(
                                          "Each joint's absolute distance from the target joints.",
                                        ),
                                      z
                                        .null()
                                        .describe(
                                          "Each joint's absolute distance from the target joints.",
                                        ),
                                    ])
                                    .describe(
                                      "Each joint's absolute distance from the target joints.",
                                    ),
                                  joints: z.array(z.number()),
                                  orientation: z
                                    .union([
                                      z
                                        .object({
                                          approach: z
                                            .array(z.number())
                                            .min(3)
                                            .max(3),
                                          degrees_from_down: z.number(),
                                          jaw_yaw_degrees: z.number(),
                                          jaw_yaw_reference: z
                                            .enum(["world_x", "world_y"])
                                            .describe(
                                              "The world axis jaw yaw zero is projected from.",
                                            ),
                                          target: z.union([
                                            z
                                              .object({
                                                approach_error_degrees:
                                                  z.number(),
                                                jaw_yaw_degrees: z.number(),
                                                jaw_yaw_reference: z
                                                  .enum(["world_x", "world_y"])
                                                  .describe(
                                                    "The world axis jaw yaw zero is projected from.",
                                                  ),
                                                orientation_error_degrees:
                                                  z.number(),
                                              })
                                              .describe(
                                                "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                              ),
                                            z.null(),
                                          ]),
                                        })
                                        .describe(
                                          "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                        ),
                                      z.null(),
                                    ])
                                    .describe(
                                      "The tool orientation in grasp terms, against the target's when there is one.",
                                    ),
                                  orientation_error_radians: z.union([
                                    z.number(),
                                    z.null(),
                                  ]),
                                  position: z
                                    .union([
                                      z
                                        .array(z.number())
                                        .min(3)
                                        .max(3)
                                        .describe(
                                          "The tool pose at `joints`; absent when the controller has no tool link.",
                                        ),
                                      z
                                        .null()
                                        .describe(
                                          "The tool pose at `joints`; absent when the controller has no tool link.",
                                        ),
                                    ])
                                    .describe(
                                      "The tool pose at `joints`; absent when the controller has no tool link.",
                                    ),
                                  position_error_meters: z.union([
                                    z.number(),
                                    z.null(),
                                  ]),
                                  quaternion: z.union([
                                    z.array(z.number()).min(4).max(4),
                                    z.null(),
                                  ]),
                                  sampled_at: z.number(),
                                })
                                .describe(
                                  "Where the arm was, measured, when a command ended.",
                                ),
                              z.null(),
                            ]),
                            checkpoints: z.array(
                              z
                                .object({
                                  label: z.string(),
                                  measured_joints: z.array(z.number()),
                                  target_position: z.union([
                                    z.array(z.number()).min(3).max(3),
                                    z.null(),
                                  ]),
                                  target_quaternion: z.union([
                                    z.array(z.number()).min(4).max(4),
                                    z.null(),
                                  ]),
                                })
                                .describe(
                                  "Where the arm stood, as measured, when it settled at the end of a segment.",
                                ),
                            ),
                            command_id: z.string(),
                            completion: z.union([
                              z
                                .object({
                                  detail: z.union([z.string(), z.null()]),
                                  finished_at: z.number(),
                                  gripper: z
                                    .union([
                                      z
                                        .union([
                                          z
                                            .object({
                                              kind: z.literal("reached"),
                                              measured_angle_radians:
                                                z.number(),
                                            })
                                            .describe(
                                              "The jaw arrived at the angle it was asked for.",
                                            ),
                                          z
                                            .object({
                                              kind: z.literal("stopped_short"),
                                              measured_angle_radians:
                                                z.number(),
                                              target_angle_radians: z.number(),
                                            })
                                            .describe(
                                              "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                            ),
                                        ])
                                        .describe(
                                          "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                        ),
                                      z.null(),
                                    ])
                                    .describe(
                                      "How a gripper command ended; null for every other command.",
                                    ),
                                  outcome: z
                                    .enum([
                                      "completed",
                                      "stopped",
                                      "tripped",
                                      "timed_out",
                                      "stopped_short",
                                      "superseded",
                                      "faulted",
                                      "interrupted",
                                      "failed",
                                    ])
                                    .describe("How a command ended."),
                                })
                                .describe(
                                  "The outcome and timestamp are recorded together when a command ends.",
                                ),
                              z.null(),
                            ]),
                            goal_joints: z
                              .union([
                                z
                                  .array(z.number())
                                  .describe(
                                    "The newest goal of a joint stream, after clamping to the joint limits.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "The newest goal of a joint stream, after clamping to the joint limits.",
                                  ),
                              ])
                              .describe(
                                "The newest goal of a joint stream, after clamping to the joint limits.",
                              ),
                            group_id: z
                              .union([
                                z
                                  .string()
                                  .describe(
                                    "The grouped motion this command is one arm's part of, if it is.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "The grouped motion this command is one arm's part of, if it is.",
                                  ),
                              ])
                              .describe(
                                "The grouped motion this command is one arm's part of, if it is.",
                              ),
                            joint_speed: z
                              .union([
                                z
                                  .number()
                                  .describe(
                                    "Speed actually used, after clamping, for motions.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Speed actually used, after clamping, for motions.",
                                  ),
                              ])
                              .describe(
                                "Speed actually used, after clamping, for motions.",
                              ),
                            label: z
                              .string()
                              .describe(
                                "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                              ),
                            path_limited: z.union([
                              z
                                .object({ reason: z.string() })
                                .describe(
                                  "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                                ),
                              z.null(),
                            ]),
                            pose_limited: z.union([
                              z
                                .object({
                                  advice: z
                                    .string()
                                    .describe(
                                      "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                    ),
                                  orientation_shortfall_radians: z
                                    .number()
                                    .describe(
                                      "From the closest pose's orientation to the requested one.",
                                    ),
                                  position_shortfall_meters: z
                                    .number()
                                    .describe(
                                      "From the closest pose's tool position to the requested one.",
                                    ),
                                  reason: z
                                    .union([
                                      z
                                        .literal("at_joint_limit")
                                        .describe(
                                          "The best pose it found has a joint at its limit, and no step improves it.",
                                        ),
                                      z
                                        .literal("stalled")
                                        .describe(
                                          "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                        ),
                                      z
                                        .literal("iteration_limit")
                                        .describe(
                                          "It was still improving when it ran out of iterations.",
                                        ),
                                      z
                                        .literal("out_of_reach")
                                        .describe(
                                          "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                        ),
                                    ])
                                    .describe("Why a solve stopped short."),
                                  waypoint: z
                                    .number()
                                    .int()
                                    .gte(0)
                                    .describe(
                                      "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                    ),
                                })
                                .describe(
                                  "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                                ),
                              z.null(),
                            ]),
                            target_joints: z.union([
                              z.array(z.number()),
                              z.null(),
                            ]),
                            target_position: z.union([
                              z.array(z.number()).min(3).max(3),
                              z.null(),
                            ]),
                            target_quaternion: z.union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ]),
                            target_status: z.union([
                              z
                                .enum([
                                  "reached",
                                  "stopped_short",
                                  "partial",
                                  "not_started",
                                ])
                                .describe(
                                  "Whether the arm ended where the command was aimed.",
                                ),
                              z.null(),
                            ]),
                            trace_context: z
                              .union([
                                z
                                  .object({
                                    traceparent: z
                                      .string()
                                      .describe(
                                        "`00-<trace id>-<parent span id>-<flags>`.",
                                      ),
                                    tracestate: z
                                      .union([z.string(), z.null()])
                                      .default(null),
                                  })
                                  .describe(
                                    "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                                  ),
                                z.null(),
                              ])
                              .describe(
                                "The trace context the command arrived with, if any.",
                              ),
                          })
                          .strict()
                          .describe("One command, from acceptance to its end."),
                        z.null(),
                      ]),
                      records: z
                        .array(
                          z
                            .object({
                              accepted_at: z.number(),
                              achieved: z.union([
                                z
                                  .object({
                                    joint_error_radians: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    joint_errors_radians: z
                                      .union([
                                        z
                                          .array(z.number())
                                          .describe(
                                            "Each joint's absolute distance from the target joints.",
                                          ),
                                        z
                                          .null()
                                          .describe(
                                            "Each joint's absolute distance from the target joints.",
                                          ),
                                      ])
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                    joints: z.array(z.number()),
                                    orientation: z
                                      .union([
                                        z
                                          .object({
                                            approach: z
                                              .array(z.number())
                                              .min(3)
                                              .max(3),
                                            degrees_from_down: z.number(),
                                            jaw_yaw_degrees: z.number(),
                                            jaw_yaw_reference: z
                                              .enum(["world_x", "world_y"])
                                              .describe(
                                                "The world axis jaw yaw zero is projected from.",
                                              ),
                                            target: z.union([
                                              z
                                                .object({
                                                  approach_error_degrees:
                                                    z.number(),
                                                  jaw_yaw_degrees: z.number(),
                                                  jaw_yaw_reference: z
                                                    .enum([
                                                      "world_x",
                                                      "world_y",
                                                    ])
                                                    .describe(
                                                      "The world axis jaw yaw zero is projected from.",
                                                    ),
                                                  orientation_error_degrees:
                                                    z.number(),
                                                })
                                                .describe(
                                                  "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                                ),
                                              z.null(),
                                            ]),
                                          })
                                          .describe(
                                            "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                          ),
                                        z.null(),
                                      ])
                                      .describe(
                                        "The tool orientation in grasp terms, against the target's when there is one.",
                                      ),
                                    orientation_error_radians: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    position: z
                                      .union([
                                        z
                                          .array(z.number())
                                          .min(3)
                                          .max(3)
                                          .describe(
                                            "The tool pose at `joints`; absent when the controller has no tool link.",
                                          ),
                                        z
                                          .null()
                                          .describe(
                                            "The tool pose at `joints`; absent when the controller has no tool link.",
                                          ),
                                      ])
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                    position_error_meters: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    quaternion: z.union([
                                      z.array(z.number()).min(4).max(4),
                                      z.null(),
                                    ]),
                                    sampled_at: z.number(),
                                  })
                                  .describe(
                                    "Where the arm was, measured, when a command ended.",
                                  ),
                                z.null(),
                              ]),
                              checkpoints: z.array(
                                z
                                  .object({
                                    label: z.string(),
                                    measured_joints: z.array(z.number()),
                                    target_position: z.union([
                                      z.array(z.number()).min(3).max(3),
                                      z.null(),
                                    ]),
                                    target_quaternion: z.union([
                                      z.array(z.number()).min(4).max(4),
                                      z.null(),
                                    ]),
                                  })
                                  .describe(
                                    "Where the arm stood, as measured, when it settled at the end of a segment.",
                                  ),
                              ),
                              command_id: z.string(),
                              completion: z.union([
                                z
                                  .object({
                                    detail: z.union([z.string(), z.null()]),
                                    finished_at: z.number(),
                                    gripper: z
                                      .union([
                                        z
                                          .union([
                                            z
                                              .object({
                                                kind: z.literal("reached"),
                                                measured_angle_radians:
                                                  z.number(),
                                              })
                                              .describe(
                                                "The jaw arrived at the angle it was asked for.",
                                              ),
                                            z
                                              .object({
                                                kind: z.literal(
                                                  "stopped_short",
                                                ),
                                                measured_angle_radians:
                                                  z.number(),
                                                target_angle_radians:
                                                  z.number(),
                                              })
                                              .describe(
                                                "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                              ),
                                          ])
                                          .describe(
                                            "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                          ),
                                        z.null(),
                                      ])
                                      .describe(
                                        "How a gripper command ended; null for every other command.",
                                      ),
                                    outcome: z
                                      .enum([
                                        "completed",
                                        "stopped",
                                        "tripped",
                                        "timed_out",
                                        "stopped_short",
                                        "superseded",
                                        "faulted",
                                        "interrupted",
                                        "failed",
                                      ])
                                      .describe("How a command ended."),
                                  })
                                  .describe(
                                    "The outcome and timestamp are recorded together when a command ends.",
                                  ),
                                z.null(),
                              ]),
                              goal_joints: z
                                .union([
                                  z
                                    .array(z.number())
                                    .describe(
                                      "The newest goal of a joint stream, after clamping to the joint limits.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The newest goal of a joint stream, after clamping to the joint limits.",
                                    ),
                                ])
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                              group_id: z
                                .union([
                                  z
                                    .string()
                                    .describe(
                                      "The grouped motion this command is one arm's part of, if it is.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The grouped motion this command is one arm's part of, if it is.",
                                    ),
                                ])
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                              joint_speed: z
                                .union([
                                  z
                                    .number()
                                    .describe(
                                      "Speed actually used, after clamping, for motions.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Speed actually used, after clamping, for motions.",
                                    ),
                                ])
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                              label: z
                                .string()
                                .describe(
                                  "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                                ),
                              path_limited: z.union([
                                z
                                  .object({ reason: z.string() })
                                  .describe(
                                    "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                              pose_limited: z.union([
                                z
                                  .object({
                                    advice: z
                                      .string()
                                      .describe(
                                        "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                      ),
                                    orientation_shortfall_radians: z
                                      .number()
                                      .describe(
                                        "From the closest pose's orientation to the requested one.",
                                      ),
                                    position_shortfall_meters: z
                                      .number()
                                      .describe(
                                        "From the closest pose's tool position to the requested one.",
                                      ),
                                    reason: z
                                      .union([
                                        z
                                          .literal("at_joint_limit")
                                          .describe(
                                            "The best pose it found has a joint at its limit, and no step improves it.",
                                          ),
                                        z
                                          .literal("stalled")
                                          .describe(
                                            "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                          ),
                                        z
                                          .literal("iteration_limit")
                                          .describe(
                                            "It was still improving when it ran out of iterations.",
                                          ),
                                        z
                                          .literal("out_of_reach")
                                          .describe(
                                            "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                          ),
                                      ])
                                      .describe("Why a solve stopped short."),
                                    waypoint: z
                                      .number()
                                      .int()
                                      .gte(0)
                                      .describe(
                                        "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                      ),
                                  })
                                  .describe(
                                    "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                              target_joints: z.union([
                                z.array(z.number()),
                                z.null(),
                              ]),
                              target_position: z.union([
                                z.array(z.number()).min(3).max(3),
                                z.null(),
                              ]),
                              target_quaternion: z.union([
                                z.array(z.number()).min(4).max(4),
                                z.null(),
                              ]),
                              target_status: z.union([
                                z
                                  .enum([
                                    "reached",
                                    "stopped_short",
                                    "partial",
                                    "not_started",
                                  ])
                                  .describe(
                                    "Whether the arm ended where the command was aimed.",
                                  ),
                                z.null(),
                              ]),
                              trace_context: z
                                .union([
                                  z
                                    .object({
                                      traceparent: z
                                        .string()
                                        .describe(
                                          "`00-<trace id>-<parent span id>-<flags>`.",
                                        ),
                                      tracestate: z
                                        .union([z.string(), z.null()])
                                        .default(null),
                                    })
                                    .describe(
                                      "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "The trace context the command arrived with, if any.",
                                ),
                            })
                            .strict()
                            .describe(
                              "One command, from acceptance to its end.",
                            ),
                        )
                        .describe(
                          "The records of a grouped move, one per arm in the order asked; empty\nfor any other request.",
                        )
                        .default([]),
                      request_id: z.string(),
                    })
                    .describe(
                      "The one answer to a request that carried a request id: the command's\nrecord when it was accepted, or the reason it was not.",
                    ),
                  z
                    .object({
                      fault: z.union([z.string(), z.null()]),
                      kind: z.literal("snapshot"),
                      log: z
                        .array(z.any())
                        .describe("The retained log, oldest first."),
                      mode: z
                        .union([
                          z
                            .literal("disabled")
                            .describe("Torque off; only `enable` is accepted."),
                          z
                            .literal("holding")
                            .describe("Torque on, holding between commands."),
                          z
                            .literal("moving")
                            .describe("Following a motion or a stream."),
                          z
                            .literal("faulted")
                            .describe(
                              "Powered off after a hard fault; refuses everything until `clear_fault`.",
                            ),
                          z
                            .literal("disconnected")
                            .describe(
                              "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                            ),
                          z
                            .literal("reconnected")
                            .describe(
                              "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                            ),
                        ])
                        .describe("Where the arm stands."),
                      records: z
                        .array(
                          z
                            .object({
                              accepted_at: z.number(),
                              achieved: z.union([
                                z
                                  .object({
                                    joint_error_radians: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    joint_errors_radians: z
                                      .union([
                                        z
                                          .array(z.number())
                                          .describe(
                                            "Each joint's absolute distance from the target joints.",
                                          ),
                                        z
                                          .null()
                                          .describe(
                                            "Each joint's absolute distance from the target joints.",
                                          ),
                                      ])
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                    joints: z.array(z.number()),
                                    orientation: z
                                      .union([
                                        z
                                          .object({
                                            approach: z
                                              .array(z.number())
                                              .min(3)
                                              .max(3),
                                            degrees_from_down: z.number(),
                                            jaw_yaw_degrees: z.number(),
                                            jaw_yaw_reference: z
                                              .enum(["world_x", "world_y"])
                                              .describe(
                                                "The world axis jaw yaw zero is projected from.",
                                              ),
                                            target: z.union([
                                              z
                                                .object({
                                                  approach_error_degrees:
                                                    z.number(),
                                                  jaw_yaw_degrees: z.number(),
                                                  jaw_yaw_reference: z
                                                    .enum([
                                                      "world_x",
                                                      "world_y",
                                                    ])
                                                    .describe(
                                                      "The world axis jaw yaw zero is projected from.",
                                                    ),
                                                  orientation_error_degrees:
                                                    z.number(),
                                                })
                                                .describe(
                                                  "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                                ),
                                              z.null(),
                                            ]),
                                          })
                                          .describe(
                                            "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                          ),
                                        z.null(),
                                      ])
                                      .describe(
                                        "The tool orientation in grasp terms, against the target's when there is one.",
                                      ),
                                    orientation_error_radians: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    position: z
                                      .union([
                                        z
                                          .array(z.number())
                                          .min(3)
                                          .max(3)
                                          .describe(
                                            "The tool pose at `joints`; absent when the controller has no tool link.",
                                          ),
                                        z
                                          .null()
                                          .describe(
                                            "The tool pose at `joints`; absent when the controller has no tool link.",
                                          ),
                                      ])
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                    position_error_meters: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    quaternion: z.union([
                                      z.array(z.number()).min(4).max(4),
                                      z.null(),
                                    ]),
                                    sampled_at: z.number(),
                                  })
                                  .describe(
                                    "Where the arm was, measured, when a command ended.",
                                  ),
                                z.null(),
                              ]),
                              checkpoints: z.array(
                                z
                                  .object({
                                    label: z.string(),
                                    measured_joints: z.array(z.number()),
                                    target_position: z.union([
                                      z.array(z.number()).min(3).max(3),
                                      z.null(),
                                    ]),
                                    target_quaternion: z.union([
                                      z.array(z.number()).min(4).max(4),
                                      z.null(),
                                    ]),
                                  })
                                  .describe(
                                    "Where the arm stood, as measured, when it settled at the end of a segment.",
                                  ),
                              ),
                              command_id: z.string(),
                              completion: z.union([
                                z
                                  .object({
                                    detail: z.union([z.string(), z.null()]),
                                    finished_at: z.number(),
                                    gripper: z
                                      .union([
                                        z
                                          .union([
                                            z
                                              .object({
                                                kind: z.literal("reached"),
                                                measured_angle_radians:
                                                  z.number(),
                                              })
                                              .describe(
                                                "The jaw arrived at the angle it was asked for.",
                                              ),
                                            z
                                              .object({
                                                kind: z.literal(
                                                  "stopped_short",
                                                ),
                                                measured_angle_radians:
                                                  z.number(),
                                                target_angle_radians:
                                                  z.number(),
                                              })
                                              .describe(
                                                "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                              ),
                                          ])
                                          .describe(
                                            "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                          ),
                                        z.null(),
                                      ])
                                      .describe(
                                        "How a gripper command ended; null for every other command.",
                                      ),
                                    outcome: z
                                      .enum([
                                        "completed",
                                        "stopped",
                                        "tripped",
                                        "timed_out",
                                        "stopped_short",
                                        "superseded",
                                        "faulted",
                                        "interrupted",
                                        "failed",
                                      ])
                                      .describe("How a command ended."),
                                  })
                                  .describe(
                                    "The outcome and timestamp are recorded together when a command ends.",
                                  ),
                                z.null(),
                              ]),
                              goal_joints: z
                                .union([
                                  z
                                    .array(z.number())
                                    .describe(
                                      "The newest goal of a joint stream, after clamping to the joint limits.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The newest goal of a joint stream, after clamping to the joint limits.",
                                    ),
                                ])
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                              group_id: z
                                .union([
                                  z
                                    .string()
                                    .describe(
                                      "The grouped motion this command is one arm's part of, if it is.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The grouped motion this command is one arm's part of, if it is.",
                                    ),
                                ])
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                              joint_speed: z
                                .union([
                                  z
                                    .number()
                                    .describe(
                                      "Speed actually used, after clamping, for motions.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Speed actually used, after clamping, for motions.",
                                    ),
                                ])
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                              label: z
                                .string()
                                .describe(
                                  "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                                ),
                              path_limited: z.union([
                                z
                                  .object({ reason: z.string() })
                                  .describe(
                                    "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                              pose_limited: z.union([
                                z
                                  .object({
                                    advice: z
                                      .string()
                                      .describe(
                                        "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                      ),
                                    orientation_shortfall_radians: z
                                      .number()
                                      .describe(
                                        "From the closest pose's orientation to the requested one.",
                                      ),
                                    position_shortfall_meters: z
                                      .number()
                                      .describe(
                                        "From the closest pose's tool position to the requested one.",
                                      ),
                                    reason: z
                                      .union([
                                        z
                                          .literal("at_joint_limit")
                                          .describe(
                                            "The best pose it found has a joint at its limit, and no step improves it.",
                                          ),
                                        z
                                          .literal("stalled")
                                          .describe(
                                            "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                          ),
                                        z
                                          .literal("iteration_limit")
                                          .describe(
                                            "It was still improving when it ran out of iterations.",
                                          ),
                                        z
                                          .literal("out_of_reach")
                                          .describe(
                                            "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                          ),
                                      ])
                                      .describe("Why a solve stopped short."),
                                    waypoint: z
                                      .number()
                                      .int()
                                      .gte(0)
                                      .describe(
                                        "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                      ),
                                  })
                                  .describe(
                                    "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                              target_joints: z.union([
                                z.array(z.number()),
                                z.null(),
                              ]),
                              target_position: z.union([
                                z.array(z.number()).min(3).max(3),
                                z.null(),
                              ]),
                              target_quaternion: z.union([
                                z.array(z.number()).min(4).max(4),
                                z.null(),
                              ]),
                              target_status: z.union([
                                z
                                  .enum([
                                    "reached",
                                    "stopped_short",
                                    "partial",
                                    "not_started",
                                  ])
                                  .describe(
                                    "Whether the arm ended where the command was aimed.",
                                  ),
                                z.null(),
                              ]),
                              trace_context: z
                                .union([
                                  z
                                    .object({
                                      traceparent: z
                                        .string()
                                        .describe(
                                          "`00-<trace id>-<parent span id>-<flags>`.",
                                        ),
                                      tracestate: z
                                        .union([z.string(), z.null()])
                                        .default(null),
                                    })
                                    .describe(
                                      "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "The trace context the command arrived with, if any.",
                                ),
                            })
                            .strict()
                            .describe(
                              "One command, from acceptance to its end.",
                            ),
                        )
                        .describe(
                          "The commands the arm still remembers, oldest first, as they stand.",
                        ),
                      sample: z.union([
                        z
                          .object({
                            active_command: z
                              .union([
                                z
                                  .string()
                                  .describe(
                                    "The command being carried out, when one is.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "The command being carried out, when one is.",
                                  ),
                              ])
                              .describe(
                                "The command being carried out, when one is.",
                              ),
                            active_trip: z
                              .union([
                                z
                                  .string()
                                  .describe(
                                    "The protective check currently preventing new motion, if any.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "The protective check currently preventing new motion, if any.",
                                  ),
                              ])
                              .describe(
                                "The protective check currently preventing new motion, if any.",
                              )
                              .default(null),
                            commanded_positions: z
                              .union([
                                z
                                  .array(z.number())
                                  .describe(
                                    "Joint setpoint successfully sent this cycle, if any.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Joint setpoint successfully sent this cycle, if any.",
                                  ),
                              ])
                              .describe(
                                "Joint setpoint successfully sent this cycle, if any.",
                              ),
                            cycle: z.number().int().gte(0),
                            cycle_seconds: z.number(),
                            efforts: z
                              .array(z.number())
                              .describe(
                                "Each arm joint's load, in percent of the servo's rated load.",
                              ),
                            fault: z
                              .union([
                                z
                                  .string()
                                  .describe(
                                    "A fault the hardware reports this cycle, tripped or not.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "A fault the hardware reports this cycle, tripped or not.",
                                  ),
                              ])
                              .describe(
                                "A fault the hardware reports this cycle, tripped or not.",
                              ),
                            gripper_angle: z.union([z.number(), z.null()]),
                            hold_pose: z
                              .union([
                                z
                                  .array(z.number())
                                  .describe(
                                    "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                                  ),
                              ])
                              .describe(
                                "Where the arm holds between commands, which a plan must start from; only\nwhile the motors are enabled.",
                              ),
                            mode: z
                              .union([
                                z
                                  .literal("disabled")
                                  .describe(
                                    "Torque off; only `enable` is accepted.",
                                  ),
                                z
                                  .literal("holding")
                                  .describe(
                                    "Torque on, holding between commands.",
                                  ),
                                z
                                  .literal("moving")
                                  .describe("Following a motion or a stream."),
                                z
                                  .literal("faulted")
                                  .describe(
                                    "Powered off after a hard fault; refuses everything until `clear_fault`.",
                                  ),
                                z
                                  .literal("disconnected")
                                  .describe(
                                    "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                                  ),
                                z
                                  .literal("reconnected")
                                  .describe(
                                    "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                                  ),
                              ])
                              .describe("Where the arm stands."),
                            positions: z.array(z.number()),
                            recovery_issue: z.union([
                              z
                                .object({
                                  detail: z.string(),
                                  motors_without_torque: z.array(
                                    z.number().int().gte(0).lte(255),
                                  ),
                                })
                                .describe(
                                  "Why automatic recovery stopped, including motors found without torque.",
                                ),
                              z.null(),
                            ]),
                            rest_pose: z
                              .union([
                                z
                                  .array(z.number())
                                  .describe(
                                    "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                                  ),
                              ])
                              .describe(
                                "Where the arm rested unpowered when it was enabled; shutdown returns there.",
                              ),
                            segment: z
                              .union([
                                z
                                  .string()
                                  .describe(
                                    "The segment of the active motion being followed.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "The segment of the active motion being followed.",
                                  ),
                              ])
                              .describe(
                                "The segment of the active motion being followed.",
                              ),
                            temperatures_celsius: z
                              .array(z.number())
                              .describe(
                                "The last reading of each driven motor, the gripper's last; empty in\nsimulation. Sent while tripped too, so a holding arm that heats shows.",
                              ),
                            velocities: z.array(z.number()),
                            voltages_volts: z
                              .array(z.number())
                              .describe(
                                "The supply voltage each driven motor last reported, as the temperatures.",
                              ),
                          })
                          .describe(
                            "One control cycle's state; a sample, so a slow client may miss some.",
                          ),
                        z.null(),
                      ]),
                      seq: z
                        .number()
                        .int()
                        .gte(0)
                        .describe(
                          "The newest sequence number issued; subscribing after it misses nothing.",
                        ),
                    })
                    .describe(
                      "The answer to `snapshot`, and to `subscribe` when events were lost.",
                    ),
                  z
                    .object({
                      after: z.number().int().gte(0),
                      kind: z.literal("resync"),
                      newest: z.number().int().gte(0),
                      oldest_retained: z.number().int().gte(0),
                    })
                    .describe(
                      "A resume point the retained log no longer reaches, or that is ahead of\nit: events between `after` and `oldest_retained` are lost. A snapshot\nfollows.",
                    ),
                  z
                    .object({
                      accepted_at: z.number(),
                      achieved: z.union([
                        z
                          .object({
                            joint_error_radians: z.union([
                              z.number(),
                              z.null(),
                            ]),
                            joint_errors_radians: z
                              .union([
                                z
                                  .array(z.number())
                                  .describe(
                                    "Each joint's absolute distance from the target joints.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Each joint's absolute distance from the target joints.",
                                  ),
                              ])
                              .describe(
                                "Each joint's absolute distance from the target joints.",
                              ),
                            joints: z.array(z.number()),
                            orientation: z
                              .union([
                                z
                                  .object({
                                    approach: z.array(z.number()).min(3).max(3),
                                    degrees_from_down: z.number(),
                                    jaw_yaw_degrees: z.number(),
                                    jaw_yaw_reference: z
                                      .enum(["world_x", "world_y"])
                                      .describe(
                                        "The world axis jaw yaw zero is projected from.",
                                      ),
                                    target: z.union([
                                      z
                                        .object({
                                          approach_error_degrees: z.number(),
                                          jaw_yaw_degrees: z.number(),
                                          jaw_yaw_reference: z
                                            .enum(["world_x", "world_y"])
                                            .describe(
                                              "The world axis jaw yaw zero is projected from.",
                                            ),
                                          orientation_error_degrees: z.number(),
                                        })
                                        .describe(
                                          "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                        ),
                                      z.null(),
                                    ]),
                                  })
                                  .describe(
                                    "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                  ),
                                z.null(),
                              ])
                              .describe(
                                "The tool orientation in grasp terms, against the target's when there is one.",
                              ),
                            orientation_error_radians: z.union([
                              z.number(),
                              z.null(),
                            ]),
                            position: z
                              .union([
                                z
                                  .array(z.number())
                                  .min(3)
                                  .max(3)
                                  .describe(
                                    "The tool pose at `joints`; absent when the controller has no tool link.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "The tool pose at `joints`; absent when the controller has no tool link.",
                                  ),
                              ])
                              .describe(
                                "The tool pose at `joints`; absent when the controller has no tool link.",
                              ),
                            position_error_meters: z.union([
                              z.number(),
                              z.null(),
                            ]),
                            quaternion: z.union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ]),
                            sampled_at: z.number(),
                          })
                          .describe(
                            "Where the arm was, measured, when a command ended.",
                          ),
                        z.null(),
                      ]),
                      checkpoints: z.array(
                        z
                          .object({
                            label: z.string(),
                            measured_joints: z.array(z.number()),
                            target_position: z.union([
                              z.array(z.number()).min(3).max(3),
                              z.null(),
                            ]),
                            target_quaternion: z.union([
                              z.array(z.number()).min(4).max(4),
                              z.null(),
                            ]),
                          })
                          .describe(
                            "Where the arm stood, as measured, when it settled at the end of a segment.",
                          ),
                      ),
                      command_id: z.string(),
                      completion: z.union([
                        z
                          .object({
                            detail: z.union([z.string(), z.null()]),
                            finished_at: z.number(),
                            gripper: z
                              .union([
                                z
                                  .union([
                                    z
                                      .object({
                                        kind: z.literal("reached"),
                                        measured_angle_radians: z.number(),
                                      })
                                      .describe(
                                        "The jaw arrived at the angle it was asked for.",
                                      ),
                                    z
                                      .object({
                                        kind: z.literal("stopped_short"),
                                        measured_angle_radians: z.number(),
                                        target_angle_radians: z.number(),
                                      })
                                      .describe(
                                        "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                      ),
                                  ])
                                  .describe(
                                    "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                  ),
                                z.null(),
                              ])
                              .describe(
                                "How a gripper command ended; null for every other command.",
                              ),
                            outcome: z
                              .enum([
                                "completed",
                                "stopped",
                                "tripped",
                                "timed_out",
                                "stopped_short",
                                "superseded",
                                "faulted",
                                "interrupted",
                                "failed",
                              ])
                              .describe("How a command ended."),
                          })
                          .describe(
                            "The outcome and timestamp are recorded together when a command ends.",
                          ),
                        z.null(),
                      ]),
                      goal_joints: z
                        .union([
                          z
                            .array(z.number())
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                          z
                            .null()
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                        ])
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                      group_id: z
                        .union([
                          z
                            .string()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                          z
                            .null()
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                        ])
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                      joint_speed: z
                        .union([
                          z
                            .number()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                          z
                            .null()
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                        ])
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                      kind: z.literal("record"),
                      label: z
                        .string()
                        .describe(
                          "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                        ),
                      path_limited: z.union([
                        z
                          .object({ reason: z.string() })
                          .describe(
                            "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                          ),
                        z.null(),
                      ]),
                      pose_limited: z.union([
                        z
                          .object({
                            advice: z
                              .string()
                              .describe(
                                "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                              ),
                            orientation_shortfall_radians: z
                              .number()
                              .describe(
                                "From the closest pose's orientation to the requested one.",
                              ),
                            position_shortfall_meters: z
                              .number()
                              .describe(
                                "From the closest pose's tool position to the requested one.",
                              ),
                            reason: z
                              .union([
                                z
                                  .literal("at_joint_limit")
                                  .describe(
                                    "The best pose it found has a joint at its limit, and no step improves it.",
                                  ),
                                z
                                  .literal("stalled")
                                  .describe(
                                    "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                  ),
                                z
                                  .literal("iteration_limit")
                                  .describe(
                                    "It was still improving when it ran out of iterations.",
                                  ),
                                z
                                  .literal("out_of_reach")
                                  .describe(
                                    "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                  ),
                              ])
                              .describe("Why a solve stopped short."),
                            waypoint: z
                              .number()
                              .int()
                              .gte(0)
                              .describe(
                                "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                              ),
                          })
                          .describe(
                            "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                          ),
                        z.null(),
                      ]),
                      target_joints: z.union([z.array(z.number()), z.null()]),
                      target_position: z.union([
                        z.array(z.number()).min(3).max(3),
                        z.null(),
                      ]),
                      target_quaternion: z.union([
                        z.array(z.number()).min(4).max(4),
                        z.null(),
                      ]),
                      target_status: z.union([
                        z
                          .enum([
                            "reached",
                            "stopped_short",
                            "partial",
                            "not_started",
                          ])
                          .describe(
                            "Whether the arm ended where the command was aimed.",
                          ),
                        z.null(),
                      ]),
                      trace_context: z
                        .union([
                          z
                            .object({
                              traceparent: z
                                .string()
                                .describe(
                                  "`00-<trace id>-<parent span id>-<flags>`.",
                                ),
                              tracestate: z
                                .union([z.string(), z.null()])
                                .default(null),
                            })
                            .describe(
                              "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                            ),
                          z.null(),
                        ])
                        .describe(
                          "The trace context the command arrived with, if any.",
                        ),
                    })
                    .strict()
                    .describe(
                      "A command's record changed: accepted, started, a checkpoint, ended.",
                    ),
                  z
                    .object({
                      fault: z
                        .union([
                          z.string().describe("Why, after a fault."),
                          z.null().describe("Why, after a fault."),
                        ])
                        .describe("Why, after a fault."),
                      kind: z.literal("mode"),
                      mode: z
                        .union([
                          z
                            .literal("disabled")
                            .describe("Torque off; only `enable` is accepted."),
                          z
                            .literal("holding")
                            .describe("Torque on, holding between commands."),
                          z
                            .literal("moving")
                            .describe("Following a motion or a stream."),
                          z
                            .literal("faulted")
                            .describe(
                              "Powered off after a hard fault; refuses everything until `clear_fault`.",
                            ),
                          z
                            .literal("disconnected")
                            .describe(
                              "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                            ),
                          z
                            .literal("reconnected")
                            .describe(
                              "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                            ),
                        ])
                        .describe("Where the arm stands."),
                    })
                    .describe("The arm changed mode."),
                  z
                    .object({ kind: z.literal("rejected"), reason: z.string() })
                    .describe(
                      "A request that was not acted on, when it carried no request id to reply to.",
                    ),
                  z
                    .object({ kind: z.literal("tripped"), reason: z.string() })
                    .describe(
                      "A safety check stopped the arm, which holds where it is. Reported once\neach time a check starts failing.",
                    ),
                  z
                    .object({
                      kind: z.literal("controller_unavailable"),
                      reason: z.string(),
                    })
                    .describe(
                      "The station lost its motor-controller process; the arm's torque is unknown.",
                    ),
                  z
                    .object({ kind: z.literal("controller_available") })
                    .describe(
                      "A fresh controller passed startup checks and the arm is disabled.",
                    ),
                ])
                .describe("Controller to client."),
              seq: z
                .union([
                  z
                    .number()
                    .int()
                    .gte(0)
                    .describe(
                      "Present on log events only; numbers rise by one with no gaps.",
                    ),
                  z
                    .null()
                    .describe(
                      "Present on log events only; numbers rise by one with no gaps.",
                    ),
                ])
                .describe(
                  "Present on log events only; numbers rise by one with no gaps.",
                ),
            })
            .describe(
              "One line from the controller: a log event with its sequence number, or an\nunsequenced sample or reply.",
            ),
          kind: z.literal("frame"),
        })
        .describe("One frame from one arm's link."),
      z
        .object({
          answer: z
            .union([
              z
                .object({
                  lease_seconds: z.number(),
                  lease_token: z.string(),
                  status: z
                    .object({
                      expires_in_seconds: z
                        .number()
                        .describe(
                          "The lease lapses this long after its grant or last explicit renewal.",
                        ),
                      holder: z
                        .union([
                          z
                            .literal("agent")
                            .describe(
                              "A model loop, started from the terminal or the agent server.",
                            ),
                          z
                            .literal("manual")
                            .describe(
                              "A person driving the arms from the viewer's manual controls.",
                            ),
                        ])
                        .describe("The kind of controller holding the lease."),
                      label: z.string(),
                      on_interruption: z
                        .union([
                          z
                            .literal("wait_for_approval")
                            .describe(
                              "Keep the lease and wait for an operator's `approve_recovery`.",
                            ),
                          z
                            .literal("return_lease")
                            .describe("Give the lease up."),
                        ])
                        .describe(
                          "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                        ),
                      required_cameras: z
                        .array(z.string())
                        .describe("Cameras the holder cannot work without."),
                    })
                    .describe("The current lease, as everyone may see it."),
                  kind: z.literal("granted"),
                })
                .describe("The lease is yours."),
              z
                .object({
                  kind: z.literal("holder"),
                  status: z.union([
                    z
                      .object({
                        expires_in_seconds: z
                          .number()
                          .describe(
                            "The lease lapses this long after its grant or last explicit renewal.",
                          ),
                        holder: z
                          .union([
                            z
                              .literal("agent")
                              .describe(
                                "A model loop, started from the terminal or the agent server.",
                              ),
                            z
                              .literal("manual")
                              .describe(
                                "A person driving the arms from the viewer's manual controls.",
                              ),
                          ])
                          .describe(
                            "The kind of controller holding the lease.",
                          ),
                        label: z.string(),
                        on_interruption: z
                          .union([
                            z
                              .literal("wait_for_approval")
                              .describe(
                                "Keep the lease and wait for an operator's `approve_recovery`.",
                              ),
                            z
                              .literal("return_lease")
                              .describe("Give the lease up."),
                          ])
                          .describe(
                            "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                          ),
                        required_cameras: z
                          .array(z.string())
                          .describe("Cameras the holder cannot work without."),
                      })
                      .describe("The current lease, as everyone may see it."),
                    z.null(),
                  ]),
                })
                .describe("Who holds the lease, if anyone."),
            ])
            .describe("What a request about the lease answers."),
          kind: z.literal("lease"),
          request_id: z.union([z.string(), z.null()]),
        })
        .describe("The answer to `acquire_lease` and `take_lease`."),
      z
        .object({
          answer: z
            .union([
              z.object({
                records: z.array(
                  z
                    .object({
                      arm: z.string(),
                      record: z
                        .object({
                          accepted_at: z.number(),
                          achieved: z.union([
                            z
                              .object({
                                joint_error_radians: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                joint_errors_radians: z
                                  .union([
                                    z
                                      .array(z.number())
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                  ])
                                  .describe(
                                    "Each joint's absolute distance from the target joints.",
                                  ),
                                joints: z.array(z.number()),
                                orientation: z
                                  .union([
                                    z
                                      .object({
                                        approach: z
                                          .array(z.number())
                                          .min(3)
                                          .max(3),
                                        degrees_from_down: z.number(),
                                        jaw_yaw_degrees: z.number(),
                                        jaw_yaw_reference: z
                                          .enum(["world_x", "world_y"])
                                          .describe(
                                            "The world axis jaw yaw zero is projected from.",
                                          ),
                                        target: z.union([
                                          z
                                            .object({
                                              approach_error_degrees:
                                                z.number(),
                                              jaw_yaw_degrees: z.number(),
                                              jaw_yaw_reference: z
                                                .enum(["world_x", "world_y"])
                                                .describe(
                                                  "The world axis jaw yaw zero is projected from.",
                                                ),
                                              orientation_error_degrees:
                                                z.number(),
                                            })
                                            .describe(
                                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                            ),
                                          z.null(),
                                        ]),
                                      })
                                      .describe(
                                        "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                      ),
                                    z.null(),
                                  ])
                                  .describe(
                                    "The tool orientation in grasp terms, against the target's when there is one.",
                                  ),
                                orientation_error_radians: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                position: z
                                  .union([
                                    z
                                      .array(z.number())
                                      .min(3)
                                      .max(3)
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                  ])
                                  .describe(
                                    "The tool pose at `joints`; absent when the controller has no tool link.",
                                  ),
                                position_error_meters: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                quaternion: z.union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ]),
                                sampled_at: z.number(),
                              })
                              .describe(
                                "Where the arm was, measured, when a command ended.",
                              ),
                            z.null(),
                          ]),
                          checkpoints: z.array(
                            z
                              .object({
                                label: z.string(),
                                measured_joints: z.array(z.number()),
                                target_position: z.union([
                                  z.array(z.number()).min(3).max(3),
                                  z.null(),
                                ]),
                                target_quaternion: z.union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ]),
                              })
                              .describe(
                                "Where the arm stood, as measured, when it settled at the end of a segment.",
                              ),
                          ),
                          command_id: z.string(),
                          completion: z.union([
                            z
                              .object({
                                detail: z.union([z.string(), z.null()]),
                                finished_at: z.number(),
                                gripper: z
                                  .union([
                                    z
                                      .union([
                                        z
                                          .object({
                                            kind: z.literal("reached"),
                                            measured_angle_radians: z.number(),
                                          })
                                          .describe(
                                            "The jaw arrived at the angle it was asked for.",
                                          ),
                                        z
                                          .object({
                                            kind: z.literal("stopped_short"),
                                            measured_angle_radians: z.number(),
                                            target_angle_radians: z.number(),
                                          })
                                          .describe(
                                            "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                          ),
                                      ])
                                      .describe(
                                        "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                      ),
                                    z.null(),
                                  ])
                                  .describe(
                                    "How a gripper command ended; null for every other command.",
                                  ),
                                outcome: z
                                  .enum([
                                    "completed",
                                    "stopped",
                                    "tripped",
                                    "timed_out",
                                    "stopped_short",
                                    "superseded",
                                    "faulted",
                                    "interrupted",
                                    "failed",
                                  ])
                                  .describe("How a command ended."),
                              })
                              .describe(
                                "The outcome and timestamp are recorded together when a command ends.",
                              ),
                            z.null(),
                          ]),
                          goal_joints: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                            ])
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                          group_id: z
                            .union([
                              z
                                .string()
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                            ])
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                          joint_speed: z
                            .union([
                              z
                                .number()
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                            ])
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                          label: z
                            .string()
                            .describe(
                              "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                            ),
                          path_limited: z.union([
                            z
                              .object({ reason: z.string() })
                              .describe(
                                "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                              ),
                            z.null(),
                          ]),
                          pose_limited: z.union([
                            z
                              .object({
                                advice: z
                                  .string()
                                  .describe(
                                    "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                  ),
                                orientation_shortfall_radians: z
                                  .number()
                                  .describe(
                                    "From the closest pose's orientation to the requested one.",
                                  ),
                                position_shortfall_meters: z
                                  .number()
                                  .describe(
                                    "From the closest pose's tool position to the requested one.",
                                  ),
                                reason: z
                                  .union([
                                    z
                                      .literal("at_joint_limit")
                                      .describe(
                                        "The best pose it found has a joint at its limit, and no step improves it.",
                                      ),
                                    z
                                      .literal("stalled")
                                      .describe(
                                        "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                      ),
                                    z
                                      .literal("iteration_limit")
                                      .describe(
                                        "It was still improving when it ran out of iterations.",
                                      ),
                                    z
                                      .literal("out_of_reach")
                                      .describe(
                                        "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                      ),
                                  ])
                                  .describe("Why a solve stopped short."),
                                waypoint: z
                                  .number()
                                  .int()
                                  .gte(0)
                                  .describe(
                                    "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                  ),
                              })
                              .describe(
                                "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                              ),
                            z.null(),
                          ]),
                          target_joints: z.union([
                            z.array(z.number()),
                            z.null(),
                          ]),
                          target_position: z.union([
                            z.array(z.number()).min(3).max(3),
                            z.null(),
                          ]),
                          target_quaternion: z.union([
                            z.array(z.number()).min(4).max(4),
                            z.null(),
                          ]),
                          target_status: z.union([
                            z
                              .enum([
                                "reached",
                                "stopped_short",
                                "partial",
                                "not_started",
                              ])
                              .describe(
                                "Whether the arm ended where the command was aimed.",
                              ),
                            z.null(),
                          ]),
                          trace_context: z
                            .union([
                              z
                                .object({
                                  traceparent: z
                                    .string()
                                    .describe(
                                      "`00-<trace id>-<parent span id>-<flags>`.",
                                    ),
                                  tracestate: z
                                    .union([z.string(), z.null()])
                                    .default(null),
                                })
                                .describe(
                                  "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The trace context the command arrived with, if any.",
                            ),
                        })
                        .strict()
                        .describe("One command, from acceptance to its end."),
                    })
                    .describe("A command record and the arm it belongs to."),
                ),
                status: z.literal("accepted"),
              }),
              z.object({
                detail: z.string(),
                reason: z
                  .enum([
                    "arm_busy",
                    "not_enabled",
                    "stale_revision",
                    "interlocked",
                    "faulted",
                    "invalid_target",
                    "stream_ended",
                    "unreachable",
                    "lease_held_elsewhere",
                    "malformed",
                  ])
                  .describe("Why a command was not accepted."),
                status: z.literal("refused"),
              }),
            ])
            .describe(
              "How a command was answered: accepted with its records (one per arm for\n`move_arms`), or refused, in which case nothing moved.",
            ),
          kind: z.literal("command_answer"),
          request_id: z.union([z.string(), z.null()]),
        })
        .describe("The answer to `command` or `move_arms`."),
      z
        .object({
          answer: z
            .union([
              z.object({
                projection: z
                  .object({
                    calculation_milliseconds: z.number(),
                    jaw_yaw_ranges: z
                      .array(
                        z
                          .object({
                            from_degrees: z.number(),
                            reachable: z.boolean(),
                            reason: z.union([z.string(), z.null()]),
                            to_degrees: z.number(),
                          })
                          .describe(
                            "A run of jaw yaws the arm can or cannot take at a position and approach.\nDegrees follow the jaw yaw convention, from 0 up to 360, in order.",
                          ),
                      )
                      .describe(
                        "The jaw yaws the arm can take at the projected position and approach;\nempty when nothing was projected or the arm has no wrist roll.",
                      ),
                    outcome: z
                      .union([
                        z
                          .object({
                            kind: z.literal("projected"),
                            pose: z
                              .object({
                                joints: z.array(z.number()),
                                orientation: z
                                  .object({
                                    approach: z.array(z.number()).min(3).max(3),
                                    jaw_yaw_degrees: z.number(),
                                  })
                                  .describe(
                                    "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                                  ),
                                orientation_error_radians: z
                                  .union([
                                    z
                                      .number()
                                      .describe(
                                        "From the requested orientation; none when the solver chose it.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "From the requested orientation; none when the solver chose it.",
                                      ),
                                  ])
                                  .describe(
                                    "From the requested orientation; none when the solver chose it.",
                                  ),
                                position: z.array(z.number()).min(3).max(3),
                                position_error_meters: z.number(),
                                quaternion: z.array(z.number()).min(4).max(4),
                              })
                              .describe("Where the arm would put the tool."),
                            reason: z
                              .union([
                                z
                                  .string()
                                  .describe(
                                    "Why the target was not met; none when it was met within tolerance.",
                                  ),
                                z
                                  .null()
                                  .describe(
                                    "Why the target was not met; none when it was met within tolerance.",
                                  ),
                              ])
                              .describe(
                                "Why the target was not met; none when it was met within tolerance.",
                              ),
                            within_tolerance: z
                              .boolean()
                              .describe(
                                "The solver or controller met the target without a path constraint.",
                              ),
                          })
                          .describe(
                            "The best pose found, including an approximation when the target was not met.",
                          ),
                        z
                          .object({
                            kind: z.literal("unavailable"),
                            reason: z.string(),
                          })
                          .describe(
                            "The preview could not produce a usable pose.",
                          ),
                      ])
                      .describe(
                        "A usable projected pose, or why the preview could not produce one.",
                      ),
                    requested: z
                      .union([
                        z
                          .object({
                            orientation: z
                              .object({
                                approach: z.array(z.number()).min(3).max(3),
                                jaw_yaw_degrees: z.number(),
                              })
                              .describe(
                                "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
                              ),
                            quaternion: z.array(z.number()).min(4).max(4),
                          })
                          .describe(
                            "The orientation the target asked for, so a caller that sent a quaternion\nlearns the approach and jaw yaw it stands for, and one that sent those gets\nthe quaternion to send instead.",
                          ),
                        z.null(),
                      ])
                      .describe(
                        "None for a position-only target the solver chose the orientation for.",
                      ),
                  })
                  .describe("The answer to a projection."),
                status: z.literal("projected"),
              }),
              z.object({
                detail: z.string(),
                reason: z
                  .enum([
                    "arm_busy",
                    "not_enabled",
                    "stale_revision",
                    "interlocked",
                    "faulted",
                    "invalid_target",
                    "stream_ended",
                    "unreachable",
                    "lease_held_elsewhere",
                    "malformed",
                  ])
                  .describe("Why a command was not accepted."),
                status: z.literal("refused"),
              }),
            ])
            .describe("How a preview was answered."),
          kind: z.literal("preview_answer"),
          request_id: z.union([z.string(), z.null()]),
        })
        .describe("The answer to `preview`."),
      z
        .object({ detail: z.string(), kind: z.literal("lease_lost") })
        .describe(
          "This connection no longer holds the lease: taken, revoked, returned\nfor an interruption, or lapsed.",
        ),
      z
        .object({
          kind: z.literal("state"),
          state: z
            .object({
              arms: z.array(
                z
                  .object({
                    active_command: z.union([
                      z
                        .object({
                          accepted_at: z.number(),
                          achieved: z.union([
                            z
                              .object({
                                joint_error_radians: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                joint_errors_radians: z
                                  .union([
                                    z
                                      .array(z.number())
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                  ])
                                  .describe(
                                    "Each joint's absolute distance from the target joints.",
                                  ),
                                joints: z.array(z.number()),
                                orientation: z
                                  .union([
                                    z
                                      .object({
                                        approach: z
                                          .array(z.number())
                                          .min(3)
                                          .max(3),
                                        degrees_from_down: z.number(),
                                        jaw_yaw_degrees: z.number(),
                                        jaw_yaw_reference: z
                                          .enum(["world_x", "world_y"])
                                          .describe(
                                            "The world axis jaw yaw zero is projected from.",
                                          ),
                                        target: z.union([
                                          z
                                            .object({
                                              approach_error_degrees:
                                                z.number(),
                                              jaw_yaw_degrees: z.number(),
                                              jaw_yaw_reference: z
                                                .enum(["world_x", "world_y"])
                                                .describe(
                                                  "The world axis jaw yaw zero is projected from.",
                                                ),
                                              orientation_error_degrees:
                                                z.number(),
                                            })
                                            .describe(
                                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                            ),
                                          z.null(),
                                        ]),
                                      })
                                      .describe(
                                        "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                      ),
                                    z.null(),
                                  ])
                                  .describe(
                                    "The tool orientation in grasp terms, against the target's when there is one.",
                                  ),
                                orientation_error_radians: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                position: z
                                  .union([
                                    z
                                      .array(z.number())
                                      .min(3)
                                      .max(3)
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                  ])
                                  .describe(
                                    "The tool pose at `joints`; absent when the controller has no tool link.",
                                  ),
                                position_error_meters: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                quaternion: z.union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ]),
                                sampled_at: z.number(),
                              })
                              .describe(
                                "Where the arm was, measured, when a command ended.",
                              ),
                            z.null(),
                          ]),
                          checkpoints: z.array(
                            z
                              .object({
                                label: z.string(),
                                measured_joints: z.array(z.number()),
                                target_position: z.union([
                                  z.array(z.number()).min(3).max(3),
                                  z.null(),
                                ]),
                                target_quaternion: z.union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ]),
                              })
                              .describe(
                                "Where the arm stood, as measured, when it settled at the end of a segment.",
                              ),
                          ),
                          command_id: z.string(),
                          completion: z.union([
                            z
                              .object({
                                detail: z.union([z.string(), z.null()]),
                                finished_at: z.number(),
                                gripper: z
                                  .union([
                                    z
                                      .union([
                                        z
                                          .object({
                                            kind: z.literal("reached"),
                                            measured_angle_radians: z.number(),
                                          })
                                          .describe(
                                            "The jaw arrived at the angle it was asked for.",
                                          ),
                                        z
                                          .object({
                                            kind: z.literal("stopped_short"),
                                            measured_angle_radians: z.number(),
                                            target_angle_radians: z.number(),
                                          })
                                          .describe(
                                            "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                          ),
                                      ])
                                      .describe(
                                        "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                      ),
                                    z.null(),
                                  ])
                                  .describe(
                                    "How a gripper command ended; null for every other command.",
                                  ),
                                outcome: z
                                  .enum([
                                    "completed",
                                    "stopped",
                                    "tripped",
                                    "timed_out",
                                    "stopped_short",
                                    "superseded",
                                    "faulted",
                                    "interrupted",
                                    "failed",
                                  ])
                                  .describe("How a command ended."),
                              })
                              .describe(
                                "The outcome and timestamp are recorded together when a command ends.",
                              ),
                            z.null(),
                          ]),
                          goal_joints: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                            ])
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                          group_id: z
                            .union([
                              z
                                .string()
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                            ])
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                          joint_speed: z
                            .union([
                              z
                                .number()
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                            ])
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                          label: z
                            .string()
                            .describe(
                              "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                            ),
                          path_limited: z.union([
                            z
                              .object({ reason: z.string() })
                              .describe(
                                "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                              ),
                            z.null(),
                          ]),
                          pose_limited: z.union([
                            z
                              .object({
                                advice: z
                                  .string()
                                  .describe(
                                    "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                  ),
                                orientation_shortfall_radians: z
                                  .number()
                                  .describe(
                                    "From the closest pose's orientation to the requested one.",
                                  ),
                                position_shortfall_meters: z
                                  .number()
                                  .describe(
                                    "From the closest pose's tool position to the requested one.",
                                  ),
                                reason: z
                                  .union([
                                    z
                                      .literal("at_joint_limit")
                                      .describe(
                                        "The best pose it found has a joint at its limit, and no step improves it.",
                                      ),
                                    z
                                      .literal("stalled")
                                      .describe(
                                        "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                      ),
                                    z
                                      .literal("iteration_limit")
                                      .describe(
                                        "It was still improving when it ran out of iterations.",
                                      ),
                                    z
                                      .literal("out_of_reach")
                                      .describe(
                                        "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                      ),
                                  ])
                                  .describe("Why a solve stopped short."),
                                waypoint: z
                                  .number()
                                  .int()
                                  .gte(0)
                                  .describe(
                                    "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                  ),
                              })
                              .describe(
                                "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                              ),
                            z.null(),
                          ]),
                          target_joints: z.union([
                            z.array(z.number()),
                            z.null(),
                          ]),
                          target_position: z.union([
                            z.array(z.number()).min(3).max(3),
                            z.null(),
                          ]),
                          target_quaternion: z.union([
                            z.array(z.number()).min(4).max(4),
                            z.null(),
                          ]),
                          target_status: z.union([
                            z
                              .enum([
                                "reached",
                                "stopped_short",
                                "partial",
                                "not_started",
                              ])
                              .describe(
                                "Whether the arm ended where the command was aimed.",
                              ),
                            z.null(),
                          ]),
                          trace_context: z
                            .union([
                              z
                                .object({
                                  traceparent: z
                                    .string()
                                    .describe(
                                      "`00-<trace id>-<parent span id>-<flags>`.",
                                    ),
                                  tracestate: z
                                    .union([z.string(), z.null()])
                                    .default(null),
                                })
                                .describe(
                                  "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The trace context the command arrived with, if any.",
                            ),
                        })
                        .strict()
                        .describe("One command, from acceptance to its end."),
                      z.null(),
                    ]),
                    active_trip: z
                      .union([
                        z
                          .string()
                          .describe(
                            "The protective check currently preventing new motion, if any.",
                          ),
                        z
                          .null()
                          .describe(
                            "The protective check currently preventing new motion, if any.",
                          ),
                      ])
                      .describe(
                        "The protective check currently preventing new motion, if any.",
                      )
                      .default(null),
                    arm: z.string(),
                    base_pose: z
                      .union([
                        z
                          .object({
                            position_meters: z.array(z.number()).min(3).max(3),
                            yaw_radians: z.number().default(0),
                          })
                          .strict()
                          .describe(
                            "A fixed placement of an arm's base: a position, and a turn about the world's\nvertical axis.",
                          ),
                        z.null(),
                      ])
                      .describe(
                        "The arm's URDF root in the station world frame. None means the world origin.",
                      )
                      .default(null),
                    commanded_joints: z
                      .union([
                        z
                          .array(z.number())
                          .describe(
                            "Joint setpoint sent in the latest sample, if the arm was commanded.",
                          ),
                        z
                          .null()
                          .describe(
                            "Joint setpoint sent in the latest sample, if the arm was commanded.",
                          ),
                      ])
                      .describe(
                        "Joint setpoint sent in the latest sample, if the arm was commanded.",
                      ),
                    controller: z
                      .enum(["ik", "policy"])
                      .describe("Which controller turns commands into motion."),
                    fault: z.union([z.string(), z.null()]),
                    gripper_opening: z
                      .union([
                        z
                          .number()
                          .describe(
                            "0 closed to 1 open; none without a gripper.",
                          ),
                        z
                          .null()
                          .describe(
                            "0 closed to 1 open; none without a gripper.",
                          ),
                      ])
                      .describe("0 closed to 1 open; none without a gripper."),
                    interlocks: z
                      .array(
                        z
                          .union([
                            z
                              .object({ kind: z.literal("arm_disconnected") })
                              .describe(
                                "The bus stopped answering. Clears when the arm reconnects and holds.",
                              ),
                            z
                              .object({
                                kind: z.literal("recovery_approval_required"),
                                motors: z.array(z.string()),
                              })
                              .describe(
                                "The arm reconnected unpowered or failed its checks. Clears when an\noperator sends `approve_recovery` after supporting the arm.",
                              ),
                            z
                              .object({
                                camera: z.string(),
                                kind: z.literal("required_camera_unavailable"),
                              })
                              .describe(
                                "A camera in the lease's `required_cameras` dropped. Clears when the\nholder reports a fresh frame, or when the lease ends.",
                              ),
                          ])
                          .describe(
                            "A condition on an arm that refuses motion until it clears. `stop`,\n`disable`, `approve_recovery` and `clear_fault` are still accepted.",
                          ),
                      )
                      .describe(
                        "Conditions refusing motion on this arm until they clear.",
                      ),
                    last_command: z.union([
                      z
                        .object({
                          accepted_at: z.number(),
                          achieved: z.union([
                            z
                              .object({
                                joint_error_radians: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                joint_errors_radians: z
                                  .union([
                                    z
                                      .array(z.number())
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                  ])
                                  .describe(
                                    "Each joint's absolute distance from the target joints.",
                                  ),
                                joints: z.array(z.number()),
                                orientation: z
                                  .union([
                                    z
                                      .object({
                                        approach: z
                                          .array(z.number())
                                          .min(3)
                                          .max(3),
                                        degrees_from_down: z.number(),
                                        jaw_yaw_degrees: z.number(),
                                        jaw_yaw_reference: z
                                          .enum(["world_x", "world_y"])
                                          .describe(
                                            "The world axis jaw yaw zero is projected from.",
                                          ),
                                        target: z.union([
                                          z
                                            .object({
                                              approach_error_degrees:
                                                z.number(),
                                              jaw_yaw_degrees: z.number(),
                                              jaw_yaw_reference: z
                                                .enum(["world_x", "world_y"])
                                                .describe(
                                                  "The world axis jaw yaw zero is projected from.",
                                                ),
                                              orientation_error_degrees:
                                                z.number(),
                                            })
                                            .describe(
                                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                            ),
                                          z.null(),
                                        ]),
                                      })
                                      .describe(
                                        "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                      ),
                                    z.null(),
                                  ])
                                  .describe(
                                    "The tool orientation in grasp terms, against the target's when there is one.",
                                  ),
                                orientation_error_radians: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                position: z
                                  .union([
                                    z
                                      .array(z.number())
                                      .min(3)
                                      .max(3)
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                    z
                                      .null()
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                  ])
                                  .describe(
                                    "The tool pose at `joints`; absent when the controller has no tool link.",
                                  ),
                                position_error_meters: z.union([
                                  z.number(),
                                  z.null(),
                                ]),
                                quaternion: z.union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ]),
                                sampled_at: z.number(),
                              })
                              .describe(
                                "Where the arm was, measured, when a command ended.",
                              ),
                            z.null(),
                          ]),
                          checkpoints: z.array(
                            z
                              .object({
                                label: z.string(),
                                measured_joints: z.array(z.number()),
                                target_position: z.union([
                                  z.array(z.number()).min(3).max(3),
                                  z.null(),
                                ]),
                                target_quaternion: z.union([
                                  z.array(z.number()).min(4).max(4),
                                  z.null(),
                                ]),
                              })
                              .describe(
                                "Where the arm stood, as measured, when it settled at the end of a segment.",
                              ),
                          ),
                          command_id: z.string(),
                          completion: z.union([
                            z
                              .object({
                                detail: z.union([z.string(), z.null()]),
                                finished_at: z.number(),
                                gripper: z
                                  .union([
                                    z
                                      .union([
                                        z
                                          .object({
                                            kind: z.literal("reached"),
                                            measured_angle_radians: z.number(),
                                          })
                                          .describe(
                                            "The jaw arrived at the angle it was asked for.",
                                          ),
                                        z
                                          .object({
                                            kind: z.literal("stopped_short"),
                                            measured_angle_radians: z.number(),
                                            target_angle_radians: z.number(),
                                          })
                                          .describe(
                                            "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                          ),
                                      ])
                                      .describe(
                                        "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                      ),
                                    z.null(),
                                  ])
                                  .describe(
                                    "How a gripper command ended; null for every other command.",
                                  ),
                                outcome: z
                                  .enum([
                                    "completed",
                                    "stopped",
                                    "tripped",
                                    "timed_out",
                                    "stopped_short",
                                    "superseded",
                                    "faulted",
                                    "interrupted",
                                    "failed",
                                  ])
                                  .describe("How a command ended."),
                              })
                              .describe(
                                "The outcome and timestamp are recorded together when a command ends.",
                              ),
                            z.null(),
                          ]),
                          goal_joints: z
                            .union([
                              z
                                .array(z.number())
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                            ])
                            .describe(
                              "The newest goal of a joint stream, after clamping to the joint limits.",
                            ),
                          group_id: z
                            .union([
                              z
                                .string()
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                              z
                                .null()
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                            ])
                            .describe(
                              "The grouped motion this command is one arm's part of, if it is.",
                            ),
                          joint_speed: z
                            .union([
                              z
                                .number()
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                              z
                                .null()
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                            ])
                            .describe(
                              "Speed actually used, after clamping, for motions.",
                            ),
                          label: z
                            .string()
                            .describe(
                              "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                            ),
                          path_limited: z.union([
                            z
                              .object({ reason: z.string() })
                              .describe(
                                "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                              ),
                            z.null(),
                          ]),
                          pose_limited: z.union([
                            z
                              .object({
                                advice: z
                                  .string()
                                  .describe(
                                    "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                  ),
                                orientation_shortfall_radians: z
                                  .number()
                                  .describe(
                                    "From the closest pose's orientation to the requested one.",
                                  ),
                                position_shortfall_meters: z
                                  .number()
                                  .describe(
                                    "From the closest pose's tool position to the requested one.",
                                  ),
                                reason: z
                                  .union([
                                    z
                                      .literal("at_joint_limit")
                                      .describe(
                                        "The best pose it found has a joint at its limit, and no step improves it.",
                                      ),
                                    z
                                      .literal("stalled")
                                      .describe(
                                        "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                      ),
                                    z
                                      .literal("iteration_limit")
                                      .describe(
                                        "It was still improving when it ran out of iterations.",
                                      ),
                                    z
                                      .literal("out_of_reach")
                                      .describe(
                                        "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                      ),
                                  ])
                                  .describe("Why a solve stopped short."),
                                waypoint: z
                                  .number()
                                  .int()
                                  .gte(0)
                                  .describe(
                                    "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                  ),
                              })
                              .describe(
                                "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                              ),
                            z.null(),
                          ]),
                          target_joints: z.union([
                            z.array(z.number()),
                            z.null(),
                          ]),
                          target_position: z.union([
                            z.array(z.number()).min(3).max(3),
                            z.null(),
                          ]),
                          target_quaternion: z.union([
                            z.array(z.number()).min(4).max(4),
                            z.null(),
                          ]),
                          target_status: z.union([
                            z
                              .enum([
                                "reached",
                                "stopped_short",
                                "partial",
                                "not_started",
                              ])
                              .describe(
                                "Whether the arm ended where the command was aimed.",
                              ),
                            z.null(),
                          ]),
                          trace_context: z
                            .union([
                              z
                                .object({
                                  traceparent: z
                                    .string()
                                    .describe(
                                      "`00-<trace id>-<parent span id>-<flags>`.",
                                    ),
                                  tracestate: z
                                    .union([z.string(), z.null()])
                                    .default(null),
                                })
                                .describe(
                                  "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                                ),
                              z.null(),
                            ])
                            .describe(
                              "The trace context the command arrived with, if any.",
                            ),
                        })
                        .strict()
                        .describe("One command, from acceptance to its end."),
                      z.null(),
                    ]),
                    measured_joint_efforts: z
                      .array(z.number())
                      .describe(
                        "Measured joint efforts in the hardware profile's effort units, in\n`measured_joints` order.",
                      ),
                    measured_joint_velocities: z
                      .array(z.number())
                      .describe(
                        "Measured joint velocities in radians per second, in `measured_joints` order.",
                      ),
                    measured_joints: z.array(z.number()),
                    mode: z
                      .union([
                        z
                          .literal("disabled")
                          .describe("Torque off; only `enable` is accepted."),
                        z
                          .literal("holding")
                          .describe("Torque on, holding between commands."),
                        z
                          .literal("moving")
                          .describe("Following a motion or a stream."),
                        z
                          .literal("faulted")
                          .describe(
                            "Powered off after a hard fault; refuses everything until `clear_fault`.",
                          ),
                        z
                          .literal("disconnected")
                          .describe(
                            "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                          ),
                        z
                          .literal("reconnected")
                          .describe(
                            "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                          ),
                      ])
                      .describe("Where the arm stands."),
                    recovery_issue: z.union([
                      z
                        .object({
                          detail: z.string(),
                          motors_without_torque: z.array(
                            z.number().int().gte(0).lte(255),
                          ),
                        })
                        .describe(
                          "Why automatic recovery stopped, including motors found without torque.",
                        ),
                      z.null(),
                    ]),
                    rest_pose: z
                      .union([
                        z
                          .array(z.number())
                          .describe(
                            "The pose the arm was enabled at; shutdown returns there first.",
                          ),
                        z
                          .null()
                          .describe(
                            "The pose the arm was enabled at; shutdown returns there first.",
                          ),
                      ])
                      .describe(
                        "The pose the arm was enabled at; shutdown returns there first.",
                      ),
                    revision: z
                      .number()
                      .int()
                      .gte(0)
                      .describe(
                        "Bumped each time an interlock is raised; commands may carry it as\n`expected_revision`.",
                      ),
                    sampled_at_unix_seconds: z
                      .union([
                        z
                          .number()
                          .describe(
                            "When the newest sample was taken, in Unix seconds; none before the first.",
                          ),
                        z
                          .null()
                          .describe(
                            "When the newest sample was taken, in Unix seconds; none before the first.",
                          ),
                      ])
                      .describe(
                        "When the newest sample was taken, in Unix seconds; none before the first.",
                      ),
                    segment: z.union([z.string(), z.null()]),
                    status: z
                      .string()
                      .describe("Why the arm is as it is, in a few words."),
                    tool: z
                      .union([
                        z
                          .object({
                            model_points: z
                              .array(z.array(z.number()).min(3).max(3))
                              .describe(
                                "Each joint's origin, then the tool's: the points a viewer draws the arm through.",
                              ),
                            orientation: z
                              .object({
                                approach: z.array(z.number()).min(3).max(3),
                                degrees_from_down: z.number(),
                                jaw_yaw_degrees: z.number(),
                                jaw_yaw_reference: z
                                  .enum(["world_x", "world_y"])
                                  .describe(
                                    "The world axis jaw yaw zero is projected from.",
                                  ),
                                target: z.union([
                                  z
                                    .object({
                                      approach_error_degrees: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      orientation_error_degrees: z.number(),
                                    })
                                    .describe(
                                      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                    ),
                                  z.null(),
                                ]),
                              })
                              .describe(
                                "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                              ),
                            position: z.array(z.number()).min(3).max(3),
                            quaternion: z.array(z.number()).min(4).max(4),
                          })
                          .describe(
                            "The tool's pose at some joints, in the station's world frame.",
                          ),
                        z.null(),
                      ])
                      .describe(
                        "The tool's pose at the measured joints, its orientation in grasp terms,\nand the joint origins a viewer sketches the arm through.",
                      ),
                  })
                  .describe("One arm as clients see it."),
              ),
              description: z
                .union([
                  z
                    .string()
                    .describe(
                      "What a model is told about this robot: layout, frames, reach.",
                    ),
                  z
                    .null()
                    .describe(
                      "What a model is told about this robot: layout, frames, reach.",
                    ),
                ])
                .describe(
                  "What a model is told about this robot: layout, frames, reach.",
                ),
              jaw_closing_axis: z
                .enum(["x", "y"])
                .describe("The tool-frame axis the jaws close along."),
              joint_names: z.array(z.string()),
              lease: z
                .union([
                  z
                    .object({
                      expires_in_seconds: z
                        .number()
                        .describe(
                          "The lease lapses this long after its grant or last explicit renewal.",
                        ),
                      holder: z
                        .union([
                          z
                            .literal("agent")
                            .describe(
                              "A model loop, started from the terminal or the agent server.",
                            ),
                          z
                            .literal("manual")
                            .describe(
                              "A person driving the arms from the viewer's manual controls.",
                            ),
                        ])
                        .describe("The kind of controller holding the lease."),
                      label: z.string(),
                      on_interruption: z
                        .union([
                          z
                            .literal("wait_for_approval")
                            .describe(
                              "Keep the lease and wait for an operator's `approve_recovery`.",
                            ),
                          z
                            .literal("return_lease")
                            .describe("Give the lease up."),
                        ])
                        .describe(
                          "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                        ),
                      required_cameras: z
                        .array(z.string())
                        .describe("Cameras the holder cannot work without."),
                    })
                    .describe("The current lease, as everyone may see it."),
                  z.null(),
                ])
                .describe(
                  "None while no one holds the lease: anyone may command.",
                ),
              max_joint_speed: z.number(),
              robot: z
                .union([
                  z
                    .string()
                    .describe(
                      "The profile name, absent for a bare controller configuration.",
                    ),
                  z
                    .null()
                    .describe(
                      "The profile name, absent for a bare controller configuration.",
                    ),
                ])
                .describe(
                  "The profile name, absent for a bare controller configuration.",
                ),
              surface_guard: z
                .union([
                  z
                    .object({
                      leeway_meters: z.number(),
                      surface_height_meters: z.number(),
                    })
                    .strict(),
                  z.null(),
                ])
                .describe("None when the profile turns the table guard off."),
            })
            .describe(
              "The whole station, as `GET /state` and the event stream provide it.",
            ),
        })
        .describe("The station, ten times a second."),
      z
        .object({
          action_sequence: z.union([
            z
              .object({
                arms: z.array(z.string()),
                compilation_id: z.string(),
                continuation: z.union([
                  z
                    .object({
                      next_step: z.number().int().gte(0),
                      prior_execution_id: z.string(),
                    })
                    .strict()
                    .describe(
                      "Explicitly select the unattempted suffix of a finished execution.",
                    ),
                  z.null(),
                ]),
                id: z.string(),
                phase: z
                  .union([
                    z.object({
                      state: z.literal("running"),
                      step: z.number().int().gte(0),
                    }),
                    z.object({ state: z.literal("completed") }),
                    z.object({
                      failure: z
                        .union([
                          z.object({
                            arm: z.string(),
                            detail: z.string(),
                            reason: z
                              .enum([
                                "arm_busy",
                                "not_enabled",
                                "stale_revision",
                                "interlocked",
                                "faulted",
                                "invalid_target",
                                "stream_ended",
                                "unreachable",
                                "lease_held_elsewhere",
                                "malformed",
                              ])
                              .describe("Why a command was not accepted."),
                            stage: z.literal("preparation"),
                            step: z.number().int().gte(0),
                          }),
                          z.object({
                            detail: z.string(),
                            reason: z
                              .enum([
                                "arm_busy",
                                "not_enabled",
                                "stale_revision",
                                "interlocked",
                                "faulted",
                                "invalid_target",
                                "stream_ended",
                                "unreachable",
                                "lease_held_elsewhere",
                                "malformed",
                              ])
                              .describe("Why a command was not accepted."),
                            stage: z.literal("admission"),
                            step: z.number().int().gte(0),
                          }),
                          z.object({
                            detail: z.string(),
                            stage: z.literal("admission_unknown"),
                            step: z.number().int().gte(0),
                          }),
                          z.object({
                            detail: z.string(),
                            stage: z.literal("execution"),
                            step: z.number().int().gte(0),
                          }),
                          z.object({
                            detail: z.string(),
                            stage: z.literal("cancellation"),
                            step: z.number().int().gte(0),
                          }),
                          z.object({
                            detail: z.string(),
                            stage: z.literal("authority"),
                            step: z.number().int().gte(0),
                          }),
                          z.object({
                            detail: z.string(),
                            stage: z.literal("runtime"),
                            step: z.number().int().gte(0),
                          }),
                        ])
                        .describe(
                          "The boundary where an execution stopped; uncertain admission may already have acted.",
                        ),
                      state: z.literal("stopped"),
                    }),
                  ])
                  .describe(
                    "Progress of an action sequence, including the records of each finished step.",
                  ),
                requested_steps: z
                  .array(
                    z
                      .object({
                        arms: z.record(
                          z.string(),
                          z
                            .object({
                              gripper_opening: z
                                .union([z.number(), z.null()])
                                .default(null),
                              max_joint_speed: z
                                .union([
                                  z
                                    .number()
                                    .describe(
                                      "Speed for both resources in rad/s; overrides the movement speed when provided.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Speed for both resources in rad/s; overrides the movement speed when provided.",
                                    ),
                                ])
                                .describe(
                                  "Speed for both resources in rad/s; overrides the movement speed when provided.",
                                )
                                .default(null),
                              movement: z
                                .union([
                                  z
                                    .union([
                                      z
                                        .object({
                                          joints: z.array(z.number()),
                                          kind: z.literal("joints"),
                                          max_joint_speed: z
                                            .union([z.number(), z.null()])
                                            .default(null),
                                        })
                                        .strict(),
                                      z
                                        .object({
                                          kind: z.literal("pose"),
                                          max_joint_speed: z
                                            .union([z.number(), z.null()])
                                            .default(null),
                                          orientation: z
                                            .union([
                                              z
                                                .object({
                                                  approach: z
                                                    .union([
                                                      z
                                                        .union([
                                                          z
                                                            .literal("down")
                                                            .describe(
                                                              "World -Z.",
                                                            ),
                                                          z
                                                            .literal("forward")
                                                            .describe(
                                                              "World +X.",
                                                            ),
                                                        ])
                                                        .describe(
                                                          "A named approach direction.",
                                                        ),
                                                      z
                                                        .array(z.number())
                                                        .min(3)
                                                        .max(3),
                                                    ])
                                                    .describe(
                                                      "The direction the jaws point along: a named one, or a nonzero world direction.",
                                                    ),
                                                  jaw_yaw_degrees: z.number(),
                                                  kind: z.literal("grasp"),
                                                })
                                                .describe(
                                                  "The approach direction and the jaw yaw about it, in degrees.",
                                                ),
                                              z
                                                .object({
                                                  kind: z.literal("quaternion"),
                                                  quaternion: z
                                                    .array(z.number())
                                                    .min(4)
                                                    .max(4),
                                                })
                                                .describe(
                                                  "A raw quaternion, `[x, y, z, w]`.",
                                                ),
                                            ])
                                            .describe(
                                              "A tool orientation as a client gives it.",
                                            ),
                                          position: z
                                            .array(z.number())
                                            .min(3)
                                            .max(3),
                                        })
                                        .strict(),
                                    ])
                                    .describe(
                                      "One movement in an action sequence. The arm settles at each target.",
                                    ),
                                  z.null(),
                                ])
                                .default(null),
                            })
                            .strict()
                            .describe(
                              "A movement and gripper target in the same step start together.",
                            ),
                        ),
                      })
                      .strict()
                      .describe(
                        "One synchronized step; a single-arm action is a map with one entry.",
                      ),
                  )
                  .describe(
                    "The submitted steps, including those not yet executed.",
                  ),
                steps: z
                  .array(
                    z.array(
                      z
                        .object({
                          arm: z.string(),
                          record: z
                            .object({
                              accepted_at: z.number(),
                              achieved: z.union([
                                z
                                  .object({
                                    joint_error_radians: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    joint_errors_radians: z
                                      .union([
                                        z
                                          .array(z.number())
                                          .describe(
                                            "Each joint's absolute distance from the target joints.",
                                          ),
                                        z
                                          .null()
                                          .describe(
                                            "Each joint's absolute distance from the target joints.",
                                          ),
                                      ])
                                      .describe(
                                        "Each joint's absolute distance from the target joints.",
                                      ),
                                    joints: z.array(z.number()),
                                    orientation: z
                                      .union([
                                        z
                                          .object({
                                            approach: z
                                              .array(z.number())
                                              .min(3)
                                              .max(3),
                                            degrees_from_down: z.number(),
                                            jaw_yaw_degrees: z.number(),
                                            jaw_yaw_reference: z
                                              .enum(["world_x", "world_y"])
                                              .describe(
                                                "The world axis jaw yaw zero is projected from.",
                                              ),
                                            target: z.union([
                                              z
                                                .object({
                                                  approach_error_degrees:
                                                    z.number(),
                                                  jaw_yaw_degrees: z.number(),
                                                  jaw_yaw_reference: z
                                                    .enum([
                                                      "world_x",
                                                      "world_y",
                                                    ])
                                                    .describe(
                                                      "The world axis jaw yaw zero is projected from.",
                                                    ),
                                                  orientation_error_degrees:
                                                    z.number(),
                                                })
                                                .describe(
                                                  "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                                ),
                                              z.null(),
                                            ]),
                                          })
                                          .describe(
                                            "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                                          ),
                                        z.null(),
                                      ])
                                      .describe(
                                        "The tool orientation in grasp terms, against the target's when there is one.",
                                      ),
                                    orientation_error_radians: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    position: z
                                      .union([
                                        z
                                          .array(z.number())
                                          .min(3)
                                          .max(3)
                                          .describe(
                                            "The tool pose at `joints`; absent when the controller has no tool link.",
                                          ),
                                        z
                                          .null()
                                          .describe(
                                            "The tool pose at `joints`; absent when the controller has no tool link.",
                                          ),
                                      ])
                                      .describe(
                                        "The tool pose at `joints`; absent when the controller has no tool link.",
                                      ),
                                    position_error_meters: z.union([
                                      z.number(),
                                      z.null(),
                                    ]),
                                    quaternion: z.union([
                                      z.array(z.number()).min(4).max(4),
                                      z.null(),
                                    ]),
                                    sampled_at: z.number(),
                                  })
                                  .describe(
                                    "Where the arm was, measured, when a command ended.",
                                  ),
                                z.null(),
                              ]),
                              checkpoints: z.array(
                                z
                                  .object({
                                    label: z.string(),
                                    measured_joints: z.array(z.number()),
                                    target_position: z.union([
                                      z.array(z.number()).min(3).max(3),
                                      z.null(),
                                    ]),
                                    target_quaternion: z.union([
                                      z.array(z.number()).min(4).max(4),
                                      z.null(),
                                    ]),
                                  })
                                  .describe(
                                    "Where the arm stood, as measured, when it settled at the end of a segment.",
                                  ),
                              ),
                              command_id: z.string(),
                              completion: z.union([
                                z
                                  .object({
                                    detail: z.union([z.string(), z.null()]),
                                    finished_at: z.number(),
                                    gripper: z
                                      .union([
                                        z
                                          .union([
                                            z
                                              .object({
                                                kind: z.literal("reached"),
                                                measured_angle_radians:
                                                  z.number(),
                                              })
                                              .describe(
                                                "The jaw arrived at the angle it was asked for.",
                                              ),
                                            z
                                              .object({
                                                kind: z.literal(
                                                  "stopped_short",
                                                ),
                                                measured_angle_radians:
                                                  z.number(),
                                                target_angle_radians:
                                                  z.number(),
                                              })
                                              .describe(
                                                "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                              ),
                                          ])
                                          .describe(
                                            "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                                          ),
                                        z.null(),
                                      ])
                                      .describe(
                                        "How a gripper command ended; null for every other command.",
                                      ),
                                    outcome: z
                                      .enum([
                                        "completed",
                                        "stopped",
                                        "tripped",
                                        "timed_out",
                                        "stopped_short",
                                        "superseded",
                                        "faulted",
                                        "interrupted",
                                        "failed",
                                      ])
                                      .describe("How a command ended."),
                                  })
                                  .describe(
                                    "The outcome and timestamp are recorded together when a command ends.",
                                  ),
                                z.null(),
                              ]),
                              goal_joints: z
                                .union([
                                  z
                                    .array(z.number())
                                    .describe(
                                      "The newest goal of a joint stream, after clamping to the joint limits.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The newest goal of a joint stream, after clamping to the joint limits.",
                                    ),
                                ])
                                .describe(
                                  "The newest goal of a joint stream, after clamping to the joint limits.",
                                ),
                              group_id: z
                                .union([
                                  z
                                    .string()
                                    .describe(
                                      "The grouped motion this command is one arm's part of, if it is.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "The grouped motion this command is one arm's part of, if it is.",
                                    ),
                                ])
                                .describe(
                                  "The grouped motion this command is one arm's part of, if it is.",
                                ),
                              joint_speed: z
                                .union([
                                  z
                                    .number()
                                    .describe(
                                      "Speed actually used, after clamping, for motions.",
                                    ),
                                  z
                                    .null()
                                    .describe(
                                      "Speed actually used, after clamping, for motions.",
                                    ),
                                ])
                                .describe(
                                  "Speed actually used, after clamping, for motions.",
                                ),
                              label: z
                                .string()
                                .describe(
                                  "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                                ),
                              path_limited: z.union([
                                z
                                  .object({ reason: z.string() })
                                  .describe(
                                    "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                              pose_limited: z.union([
                                z
                                  .object({
                                    advice: z
                                      .string()
                                      .describe(
                                        "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                                      ),
                                    orientation_shortfall_radians: z
                                      .number()
                                      .describe(
                                        "From the closest pose's orientation to the requested one.",
                                      ),
                                    position_shortfall_meters: z
                                      .number()
                                      .describe(
                                        "From the closest pose's tool position to the requested one.",
                                      ),
                                    reason: z
                                      .union([
                                        z
                                          .literal("at_joint_limit")
                                          .describe(
                                            "The best pose it found has a joint at its limit, and no step improves it.",
                                          ),
                                        z
                                          .literal("stalled")
                                          .describe(
                                            "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                                          ),
                                        z
                                          .literal("iteration_limit")
                                          .describe(
                                            "It was still improving when it ran out of iterations.",
                                          ),
                                        z
                                          .literal("out_of_reach")
                                          .describe(
                                            "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                                          ),
                                      ])
                                      .describe("Why a solve stopped short."),
                                    waypoint: z
                                      .number()
                                      .int()
                                      .gte(0)
                                      .describe(
                                        "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                                      ),
                                  })
                                  .describe(
                                    "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                                  ),
                                z.null(),
                              ]),
                              target_joints: z.union([
                                z.array(z.number()),
                                z.null(),
                              ]),
                              target_position: z.union([
                                z.array(z.number()).min(3).max(3),
                                z.null(),
                              ]),
                              target_quaternion: z.union([
                                z.array(z.number()).min(4).max(4),
                                z.null(),
                              ]),
                              target_status: z.union([
                                z
                                  .enum([
                                    "reached",
                                    "stopped_short",
                                    "partial",
                                    "not_started",
                                  ])
                                  .describe(
                                    "Whether the arm ended where the command was aimed.",
                                  ),
                                z.null(),
                              ]),
                              trace_context: z
                                .union([
                                  z
                                    .object({
                                      traceparent: z
                                        .string()
                                        .describe(
                                          "`00-<trace id>-<parent span id>-<flags>`.",
                                        ),
                                      tracestate: z
                                        .union([z.string(), z.null()])
                                        .default(null),
                                    })
                                    .describe(
                                      "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                                    ),
                                  z.null(),
                                ])
                                .describe(
                                  "The trace context the command arrived with, if any.",
                                ),
                            })
                            .strict()
                            .describe(
                              "One command, from acceptance to its end.",
                            ),
                        })
                        .describe(
                          "A command record and the arm it belongs to.",
                        ),
                    ),
                  )
                  .describe(
                    "Command records for steps that have finished or been interrupted.",
                  ),
              })
              .describe("The station's retained action sequence state."),
            z.null(),
          ]),
          kind: z.literal("action_sequence_state"),
        })
        .describe(
          "Current or most recent action sequence, sent on connection and when it changes.",
        ),
      z
        .object({
          event: z
            .union([
              z
                .union([
                  z.object({
                    task: z.union([z.string(), z.null()]).default(null),
                    type: z.literal("run_started"),
                  }),
                  z.object({
                    steps: z.array(
                      z.object({
                        status: z.enum(["pending", "active", "done", "failed"]),
                        text: z.string(),
                      }),
                    ),
                    type: z.literal("plan_update"),
                  }),
                  z.object({
                    notes: z.array(z.string()),
                    type: z.literal("notes_update"),
                  }),
                  z.object({
                    type: z.literal("usage_update"),
                    usage: z
                      .object({
                        cached_input_tokens: z.number().int().gte(0),
                        input_tokens: z.number().int().gte(0),
                        output_tokens: z.number().int().gte(0),
                        reasoning_tokens: z.number().int().gte(0),
                        run_id: z.string(),
                        turns: z.number().int().gte(0),
                      })
                      .describe("A model run's token use so far."),
                  }),
                ])
                .describe("A run event the server acts on."),
              z.any(),
            ])
            .describe(
              "An event a model loop reports: one the server acts on, or any other kept\nas it came.",
            ),
          kind: z.literal("run_event"),
          received_unix_nano: z.string().regex(/^[0-9]+$/),
          seq: z.number().int().gte(0),
        })
        .describe("An event a model loop reported, as the server received it."),
      z
        .object({
          kind: z.literal("run_span"),
          received_unix_nano: z.string().regex(/^[0-9]+$/),
          seq: z.number().int().gte(0),
          span: z
            .object({
              attributes: z.record(z.string(), z.any()).default({}),
              end_time_unix_nano: z.string().regex(/^[0-9]+$/),
              name: z.string(),
              parent_span_id: z.union([z.string(), z.null()]).default(null),
              span_id: z.string(),
              start_time_unix_nano: z.string().regex(/^[0-9]+$/),
              status: z
                .object({
                  code: z
                    .enum([
                      "STATUS_CODE_UNSET",
                      "STATUS_CODE_OK",
                      "STATUS_CODE_ERROR",
                    ])
                    .default("STATUS_CODE_UNSET"),
                  message: z.union([z.string(), z.null()]).default(null),
                })
                .default({ code: "STATUS_CODE_UNSET", message: null }),
              trace_id: z.string(),
            })
            .describe(
              "One span of a model loop's trace, in OpenTelemetry's terms.",
            ),
        })
        .describe(
          "A span as a model loop reported it; every revision arrives, unlike the\nretained spans in `run`.",
        ),
      z
        .object({
          kind: z.literal("run_resync"),
          newest: z.number().int().gte(0),
          oldest_retained: z.number().int().gte(0),
        })
        .describe(
          "The run cursor fell out of the retained records, or is ahead of them:\nrecords after it up to `oldest_retained` are lost. The stream continues\nafter `newest`.",
        ),
      z
        .object({
          kind: z.literal("run"),
          run: z
            .object({
              memory: z
                .object({
                  notes: z.array(z.string()),
                  plan: z.union([
                    z.array(
                      z.object({
                        status: z.enum(["pending", "active", "done", "failed"]),
                        text: z.string(),
                      }),
                    ),
                    z.null(),
                  ]),
                  task: z.union([z.string(), z.null()]),
                })
                .describe(
                  "What the model keeps in mind: its task, its plan and its notes.",
                ),
              revision: z.number().int().gte(0),
              spans: z.array(
                z
                  .object({
                    attributes: z.record(z.string(), z.any()).default({}),
                    end_time_unix_nano: z.string().regex(/^[0-9]+$/),
                    name: z.string(),
                    parent_span_id: z
                      .union([z.string(), z.null()])
                      .default(null),
                    span_id: z.string(),
                    start_time_unix_nano: z.string().regex(/^[0-9]+$/),
                    status: z
                      .object({
                        code: z
                          .enum([
                            "STATUS_CODE_UNSET",
                            "STATUS_CODE_OK",
                            "STATUS_CODE_ERROR",
                          ])
                          .default("STATUS_CODE_UNSET"),
                        message: z.union([z.string(), z.null()]).default(null),
                      })
                      .default({ code: "STATUS_CODE_UNSET", message: null }),
                    trace_id: z.string(),
                  })
                  .describe(
                    "One span of a model loop's trace, in OpenTelemetry's terms.",
                  ),
              ),
              usage: z.union([
                z
                  .object({
                    cached_input_tokens: z.number().int().gte(0),
                    input_tokens: z.number().int().gte(0),
                    output_tokens: z.number().int().gte(0),
                    reasoning_tokens: z.number().int().gte(0),
                    run_id: z.string(),
                    turns: z.number().int().gte(0),
                  })
                  .describe("A model run's token use so far."),
                z.null(),
              ]),
            })
            .describe(
              "The model loop's live telemetry, as the viewer draws it.",
            ),
        })
        .describe("The model loop's telemetry, whenever it changed."),
      z
        .object({
          detail: z.string(),
          kind: z.literal("refused"),
          request_id: z.union([z.string(), z.null()]),
        })
        .describe(
          "A message the server could not act on; the connection stays open.",
        ),
    ])
    .describe("Station event and result models shared with SDKs."),
  SpanStatus: z.object({
    code: z
      .enum(["STATUS_CODE_UNSET", "STATUS_CODE_OK", "STATUS_CODE_ERROR"])
      .default("STATUS_CODE_UNSET"),
    message: z.union([z.string(), z.null()]).default(null),
  }),
  SpanStatusCode: z.enum([
    "STATUS_CODE_UNSET",
    "STATUS_CODE_OK",
    "STATUS_CODE_ERROR",
  ]),
  StationCommand: z
    .union([
      z
        .object({ kind: z.literal("enable") })
        .strict()
        .describe(
          "Preflight, then power the motors holding their present pose.",
        ),
      z
        .object({ kind: z.literal("approve_recovery") })
        .strict()
        .describe("Approve recovery after inspecting and supporting the arm."),
      z
        .object({ kind: z.literal("disable") })
        .strict()
        .describe(
          "Turn torque off where the arm is; an unsupported arm will fall.",
        ),
      z
        .object({ kind: z.literal("stop") })
        .strict()
        .describe(
          "End the active motion with outcome stopped and hold where the arm is.\nAnyone may send it, whoever holds the lease.",
        ),
      z
        .object({ kind: z.literal("clear_fault") })
        .strict()
        .describe("Acknowledge a fault; the arm is already disabled."),
      z
        .object({
          joints: z.array(z.number()),
          kind: z.literal("move_to_joints"),
          max_joint_speed: z
            .union([
              z
                .number()
                .describe(
                  "Peak joint speed in rad/s; clamped to the profile's maximum.",
                ),
              z
                .null()
                .describe(
                  "Peak joint speed in rad/s; clamped to the profile's maximum.",
                ),
            ])
            .describe(
              "Peak joint speed in rad/s; clamped to the profile's maximum.",
            )
            .default(null),
        })
        .strict()
        .describe("Move to joint angles, one per joint in the state's order."),
      z
        .object({
          kind: z.literal("move_to_pose"),
          max_joint_speed: z.union([z.number(), z.null()]).default(null),
          orientation: z
            .union([
              z
                .object({
                  approach: z
                    .union([
                      z
                        .union([
                          z.literal("down").describe("World -Z."),
                          z.literal("forward").describe("World +X."),
                        ])
                        .describe("A named approach direction."),
                      z.array(z.number()).min(3).max(3),
                    ])
                    .describe(
                      "The direction the jaws point along: a named one, or a nonzero world direction.",
                    ),
                  jaw_yaw_degrees: z.number(),
                  kind: z.literal("grasp"),
                })
                .describe(
                  "The approach direction and the jaw yaw about it, in degrees.",
                ),
              z
                .object({
                  kind: z.literal("quaternion"),
                  quaternion: z.array(z.number()).min(4).max(4),
                })
                .describe("A raw quaternion, `[x, y, z, w]`."),
            ])
            .describe("A tool orientation as a client gives it."),
          position: z.array(z.number()).min(3).max(3),
        })
        .strict()
        .describe("Move the tool to a world position and orientation."),
      z
        .object({
          kind: z.literal("follow_path"),
          max_joint_speed: z.union([z.number(), z.null()]).default(null),
          waypoints: z.array(
            z
              .union([
                z.object({
                  joints: z.array(z.number()),
                  kind: z.literal("joints"),
                }),
                z.object({
                  kind: z.literal("pose"),
                  orientation: z
                    .union([
                      z
                        .object({
                          approach: z
                            .union([
                              z
                                .union([
                                  z.literal("down").describe("World -Z."),
                                  z.literal("forward").describe("World +X."),
                                ])
                                .describe("A named approach direction."),
                              z.array(z.number()).min(3).max(3),
                            ])
                            .describe(
                              "The direction the jaws point along: a named one, or a nonzero world direction.",
                            ),
                          jaw_yaw_degrees: z.number(),
                          kind: z.literal("grasp"),
                        })
                        .describe(
                          "The approach direction and the jaw yaw about it, in degrees.",
                        ),
                      z
                        .object({
                          kind: z.literal("quaternion"),
                          quaternion: z.array(z.number()).min(4).max(4),
                        })
                        .describe("A raw quaternion, `[x, y, z, w]`."),
                    ])
                    .describe("A tool orientation as a client gives it."),
                  position: z.array(z.number()).min(3).max(3),
                }),
              ])
              .describe(
                "One corner of a move: joints, or a tool pose solved for joints.",
              ),
          ),
        })
        .strict()
        .describe(
          "Waypoints are corners: the arm comes to rest and settles at each one.",
        ),
      z
        .object({
          gripper_opening: z
            .union([
              z
                .number()
                .describe(
                  "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                ),
              z
                .null()
                .describe(
                  "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
                ),
            ])
            .describe(
              "The gripper's newest target, 0 closed to 1 open, if the stream drives it.",
            )
            .default(null),
          joints: z.array(z.number()),
          kind: z.literal("stream_joints"),
          max_joint_speed: z.union([z.number(), z.null()]).default(null),
          stream_command_id: z.union([z.string(), z.null()]).default(null),
        })
        .strict()
        .describe(
          "Stream trajectory destinations. The first goal, without\n`stream_command_id`, starts a stream when the arm is holding; later goals\ncarry the command id it returned. Each cycle the arm moves toward the\nnewest goal at up to the first goal's speed. The stream ends stopped when\ngoals stop arriving, on stop, or on a trip.",
        ),
      z
        .object({
          kind: z.literal("set_gripper"),
          max_joint_speed: z
            .union([
              z
                .number()
                .describe(
                  "Peak gripper speed in rad/s; uses the arm default and maximum.",
                ),
              z
                .null()
                .describe(
                  "Peak gripper speed in rad/s; uses the arm default and maximum.",
                ),
            ])
            .describe(
              "Peak gripper speed in rad/s; uses the arm default and maximum.",
            )
            .default(null),
          opening: z.number(),
        })
        .strict()
        .describe(
          "Open or close the gripper, 0 closed to 1 open, whether the arm is holding\nor moving. It completes on arrival or on contact.",
        ),
    ])
    .describe("A command for one arm."),
  StationInstance: z
    .object({ id: z.string() })
    .describe(
      "Identifies one station process for retry and event-cursor fencing.",
    ),
  StationState: z
    .object({
      arms: z.array(
        z
          .object({
            active_command: z.union([
              z
                .object({
                  accepted_at: z.number(),
                  achieved: z.union([
                    z
                      .object({
                        joint_error_radians: z.union([z.number(), z.null()]),
                        joint_errors_radians: z
                          .union([
                            z
                              .array(z.number())
                              .describe(
                                "Each joint's absolute distance from the target joints.",
                              ),
                            z
                              .null()
                              .describe(
                                "Each joint's absolute distance from the target joints.",
                              ),
                          ])
                          .describe(
                            "Each joint's absolute distance from the target joints.",
                          ),
                        joints: z.array(z.number()),
                        orientation: z
                          .union([
                            z
                              .object({
                                approach: z.array(z.number()).min(3).max(3),
                                degrees_from_down: z.number(),
                                jaw_yaw_degrees: z.number(),
                                jaw_yaw_reference: z
                                  .enum(["world_x", "world_y"])
                                  .describe(
                                    "The world axis jaw yaw zero is projected from.",
                                  ),
                                target: z.union([
                                  z
                                    .object({
                                      approach_error_degrees: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      orientation_error_degrees: z.number(),
                                    })
                                    .describe(
                                      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                    ),
                                  z.null(),
                                ]),
                              })
                              .describe(
                                "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                              ),
                            z.null(),
                          ])
                          .describe(
                            "The tool orientation in grasp terms, against the target's when there is one.",
                          ),
                        orientation_error_radians: z.union([
                          z.number(),
                          z.null(),
                        ]),
                        position: z
                          .union([
                            z
                              .array(z.number())
                              .min(3)
                              .max(3)
                              .describe(
                                "The tool pose at `joints`; absent when the controller has no tool link.",
                              ),
                            z
                              .null()
                              .describe(
                                "The tool pose at `joints`; absent when the controller has no tool link.",
                              ),
                          ])
                          .describe(
                            "The tool pose at `joints`; absent when the controller has no tool link.",
                          ),
                        position_error_meters: z.union([z.number(), z.null()]),
                        quaternion: z.union([
                          z.array(z.number()).min(4).max(4),
                          z.null(),
                        ]),
                        sampled_at: z.number(),
                      })
                      .describe(
                        "Where the arm was, measured, when a command ended.",
                      ),
                    z.null(),
                  ]),
                  checkpoints: z.array(
                    z
                      .object({
                        label: z.string(),
                        measured_joints: z.array(z.number()),
                        target_position: z.union([
                          z.array(z.number()).min(3).max(3),
                          z.null(),
                        ]),
                        target_quaternion: z.union([
                          z.array(z.number()).min(4).max(4),
                          z.null(),
                        ]),
                      })
                      .describe(
                        "Where the arm stood, as measured, when it settled at the end of a segment.",
                      ),
                  ),
                  command_id: z.string(),
                  completion: z.union([
                    z
                      .object({
                        detail: z.union([z.string(), z.null()]),
                        finished_at: z.number(),
                        gripper: z
                          .union([
                            z
                              .union([
                                z
                                  .object({
                                    kind: z.literal("reached"),
                                    measured_angle_radians: z.number(),
                                  })
                                  .describe(
                                    "The jaw arrived at the angle it was asked for.",
                                  ),
                                z
                                  .object({
                                    kind: z.literal("stopped_short"),
                                    measured_angle_radians: z.number(),
                                    target_angle_radians: z.number(),
                                  })
                                  .describe(
                                    "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                  ),
                              ])
                              .describe(
                                "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                              ),
                            z.null(),
                          ])
                          .describe(
                            "How a gripper command ended; null for every other command.",
                          ),
                        outcome: z
                          .enum([
                            "completed",
                            "stopped",
                            "tripped",
                            "timed_out",
                            "stopped_short",
                            "superseded",
                            "faulted",
                            "interrupted",
                            "failed",
                          ])
                          .describe("How a command ended."),
                      })
                      .describe(
                        "The outcome and timestamp are recorded together when a command ends.",
                      ),
                    z.null(),
                  ]),
                  goal_joints: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                      z
                        .null()
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                    ])
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                  group_id: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                      z
                        .null()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                    ])
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                  joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                      z
                        .null()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                    ])
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                  label: z
                    .string()
                    .describe(
                      "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                    ),
                  path_limited: z.union([
                    z
                      .object({ reason: z.string() })
                      .describe(
                        "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                      ),
                    z.null(),
                  ]),
                  pose_limited: z.union([
                    z
                      .object({
                        advice: z
                          .string()
                          .describe(
                            "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                          ),
                        orientation_shortfall_radians: z
                          .number()
                          .describe(
                            "From the closest pose's orientation to the requested one.",
                          ),
                        position_shortfall_meters: z
                          .number()
                          .describe(
                            "From the closest pose's tool position to the requested one.",
                          ),
                        reason: z
                          .union([
                            z
                              .literal("at_joint_limit")
                              .describe(
                                "The best pose it found has a joint at its limit, and no step improves it.",
                              ),
                            z
                              .literal("stalled")
                              .describe(
                                "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                              ),
                            z
                              .literal("iteration_limit")
                              .describe(
                                "It was still improving when it ran out of iterations.",
                              ),
                            z
                              .literal("out_of_reach")
                              .describe(
                                "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                              ),
                          ])
                          .describe("Why a solve stopped short."),
                        waypoint: z
                          .number()
                          .int()
                          .gte(0)
                          .describe(
                            "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                          ),
                      })
                      .describe(
                        "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                      ),
                    z.null(),
                  ]),
                  target_joints: z.union([z.array(z.number()), z.null()]),
                  target_position: z.union([
                    z.array(z.number()).min(3).max(3),
                    z.null(),
                  ]),
                  target_quaternion: z.union([
                    z.array(z.number()).min(4).max(4),
                    z.null(),
                  ]),
                  target_status: z.union([
                    z
                      .enum([
                        "reached",
                        "stopped_short",
                        "partial",
                        "not_started",
                      ])
                      .describe(
                        "Whether the arm ended where the command was aimed.",
                      ),
                    z.null(),
                  ]),
                  trace_context: z
                    .union([
                      z
                        .object({
                          traceparent: z
                            .string()
                            .describe(
                              "`00-<trace id>-<parent span id>-<flags>`.",
                            ),
                          tracestate: z
                            .union([z.string(), z.null()])
                            .default(null),
                        })
                        .describe(
                          "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The trace context the command arrived with, if any.",
                    ),
                })
                .strict()
                .describe("One command, from acceptance to its end."),
              z.null(),
            ]),
            active_trip: z
              .union([
                z
                  .string()
                  .describe(
                    "The protective check currently preventing new motion, if any.",
                  ),
                z
                  .null()
                  .describe(
                    "The protective check currently preventing new motion, if any.",
                  ),
              ])
              .describe(
                "The protective check currently preventing new motion, if any.",
              )
              .default(null),
            arm: z.string(),
            base_pose: z
              .union([
                z
                  .object({
                    position_meters: z.array(z.number()).min(3).max(3),
                    yaw_radians: z.number().default(0),
                  })
                  .strict()
                  .describe(
                    "A fixed placement of an arm's base: a position, and a turn about the world's\nvertical axis.",
                  ),
                z.null(),
              ])
              .describe(
                "The arm's URDF root in the station world frame. None means the world origin.",
              )
              .default(null),
            commanded_joints: z
              .union([
                z
                  .array(z.number())
                  .describe(
                    "Joint setpoint sent in the latest sample, if the arm was commanded.",
                  ),
                z
                  .null()
                  .describe(
                    "Joint setpoint sent in the latest sample, if the arm was commanded.",
                  ),
              ])
              .describe(
                "Joint setpoint sent in the latest sample, if the arm was commanded.",
              ),
            controller: z
              .enum(["ik", "policy"])
              .describe("Which controller turns commands into motion."),
            fault: z.union([z.string(), z.null()]),
            gripper_opening: z
              .union([
                z
                  .number()
                  .describe("0 closed to 1 open; none without a gripper."),
                z
                  .null()
                  .describe("0 closed to 1 open; none without a gripper."),
              ])
              .describe("0 closed to 1 open; none without a gripper."),
            interlocks: z
              .array(
                z
                  .union([
                    z
                      .object({ kind: z.literal("arm_disconnected") })
                      .describe(
                        "The bus stopped answering. Clears when the arm reconnects and holds.",
                      ),
                    z
                      .object({
                        kind: z.literal("recovery_approval_required"),
                        motors: z.array(z.string()),
                      })
                      .describe(
                        "The arm reconnected unpowered or failed its checks. Clears when an\noperator sends `approve_recovery` after supporting the arm.",
                      ),
                    z
                      .object({
                        camera: z.string(),
                        kind: z.literal("required_camera_unavailable"),
                      })
                      .describe(
                        "A camera in the lease's `required_cameras` dropped. Clears when the\nholder reports a fresh frame, or when the lease ends.",
                      ),
                  ])
                  .describe(
                    "A condition on an arm that refuses motion until it clears. `stop`,\n`disable`, `approve_recovery` and `clear_fault` are still accepted.",
                  ),
              )
              .describe(
                "Conditions refusing motion on this arm until they clear.",
              ),
            last_command: z.union([
              z
                .object({
                  accepted_at: z.number(),
                  achieved: z.union([
                    z
                      .object({
                        joint_error_radians: z.union([z.number(), z.null()]),
                        joint_errors_radians: z
                          .union([
                            z
                              .array(z.number())
                              .describe(
                                "Each joint's absolute distance from the target joints.",
                              ),
                            z
                              .null()
                              .describe(
                                "Each joint's absolute distance from the target joints.",
                              ),
                          ])
                          .describe(
                            "Each joint's absolute distance from the target joints.",
                          ),
                        joints: z.array(z.number()),
                        orientation: z
                          .union([
                            z
                              .object({
                                approach: z.array(z.number()).min(3).max(3),
                                degrees_from_down: z.number(),
                                jaw_yaw_degrees: z.number(),
                                jaw_yaw_reference: z
                                  .enum(["world_x", "world_y"])
                                  .describe(
                                    "The world axis jaw yaw zero is projected from.",
                                  ),
                                target: z.union([
                                  z
                                    .object({
                                      approach_error_degrees: z.number(),
                                      jaw_yaw_degrees: z.number(),
                                      jaw_yaw_reference: z
                                        .enum(["world_x", "world_y"])
                                        .describe(
                                          "The world axis jaw yaw zero is projected from.",
                                        ),
                                      orientation_error_degrees: z.number(),
                                    })
                                    .describe(
                                      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                                    ),
                                  z.null(),
                                ]),
                              })
                              .describe(
                                "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                              ),
                            z.null(),
                          ])
                          .describe(
                            "The tool orientation in grasp terms, against the target's when there is one.",
                          ),
                        orientation_error_radians: z.union([
                          z.number(),
                          z.null(),
                        ]),
                        position: z
                          .union([
                            z
                              .array(z.number())
                              .min(3)
                              .max(3)
                              .describe(
                                "The tool pose at `joints`; absent when the controller has no tool link.",
                              ),
                            z
                              .null()
                              .describe(
                                "The tool pose at `joints`; absent when the controller has no tool link.",
                              ),
                          ])
                          .describe(
                            "The tool pose at `joints`; absent when the controller has no tool link.",
                          ),
                        position_error_meters: z.union([z.number(), z.null()]),
                        quaternion: z.union([
                          z.array(z.number()).min(4).max(4),
                          z.null(),
                        ]),
                        sampled_at: z.number(),
                      })
                      .describe(
                        "Where the arm was, measured, when a command ended.",
                      ),
                    z.null(),
                  ]),
                  checkpoints: z.array(
                    z
                      .object({
                        label: z.string(),
                        measured_joints: z.array(z.number()),
                        target_position: z.union([
                          z.array(z.number()).min(3).max(3),
                          z.null(),
                        ]),
                        target_quaternion: z.union([
                          z.array(z.number()).min(4).max(4),
                          z.null(),
                        ]),
                      })
                      .describe(
                        "Where the arm stood, as measured, when it settled at the end of a segment.",
                      ),
                  ),
                  command_id: z.string(),
                  completion: z.union([
                    z
                      .object({
                        detail: z.union([z.string(), z.null()]),
                        finished_at: z.number(),
                        gripper: z
                          .union([
                            z
                              .union([
                                z
                                  .object({
                                    kind: z.literal("reached"),
                                    measured_angle_radians: z.number(),
                                  })
                                  .describe(
                                    "The jaw arrived at the angle it was asked for.",
                                  ),
                                z
                                  .object({
                                    kind: z.literal("stopped_short"),
                                    measured_angle_radians: z.number(),
                                    target_angle_radians: z.number(),
                                  })
                                  .describe(
                                    "The jaw stopped short of its target and stayed there: it closed on something,\nwas blocked opening, or its target was clipped to the jaw's range.",
                                  ),
                              ])
                              .describe(
                                "How a gripper command ended, in the jaw's own angle. The controller sees where the jaw\nstopped, not what stopped it, so it cannot tell a held object from a blocked jaw.",
                              ),
                            z.null(),
                          ])
                          .describe(
                            "How a gripper command ended; null for every other command.",
                          ),
                        outcome: z
                          .enum([
                            "completed",
                            "stopped",
                            "tripped",
                            "timed_out",
                            "stopped_short",
                            "superseded",
                            "faulted",
                            "interrupted",
                            "failed",
                          ])
                          .describe("How a command ended."),
                      })
                      .describe(
                        "The outcome and timestamp are recorded together when a command ends.",
                      ),
                    z.null(),
                  ]),
                  goal_joints: z
                    .union([
                      z
                        .array(z.number())
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                      z
                        .null()
                        .describe(
                          "The newest goal of a joint stream, after clamping to the joint limits.",
                        ),
                    ])
                    .describe(
                      "The newest goal of a joint stream, after clamping to the joint limits.",
                    ),
                  group_id: z
                    .union([
                      z
                        .string()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                      z
                        .null()
                        .describe(
                          "The grouped motion this command is one arm's part of, if it is.",
                        ),
                    ])
                    .describe(
                      "The grouped motion this command is one arm's part of, if it is.",
                    ),
                  joint_speed: z
                    .union([
                      z
                        .number()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                      z
                        .null()
                        .describe(
                          "Speed actually used, after clamping, for motions.",
                        ),
                    ])
                    .describe(
                      "Speed actually used, after clamping, for motions.",
                    ),
                  label: z
                    .string()
                    .describe(
                      "What the command is: `enable`, `stream_joints`, or the label a move carried.",
                    ),
                  path_limited: z.union([
                    z
                      .object({ reason: z.string() })
                      .describe(
                        "A move cut short where its straight joint path would first fold the arm into\nitself or drive it down into the table: the plan's target is the last clear\nsample before that, not what was asked for.",
                      ),
                    z.null(),
                  ]),
                  pose_limited: z.union([
                    z
                      .object({
                        advice: z
                          .string()
                          .describe(
                            "The shortfall and the closest pose's tool position in millimetres, then what\nwould be reachable exactly instead, in the terms a pose takes.",
                          ),
                        orientation_shortfall_radians: z
                          .number()
                          .describe(
                            "From the closest pose's orientation to the requested one.",
                          ),
                        position_shortfall_meters: z
                          .number()
                          .describe(
                            "From the closest pose's tool position to the requested one.",
                          ),
                        reason: z
                          .union([
                            z
                              .literal("at_joint_limit")
                              .describe(
                                "The best pose it found has a joint at its limit, and no step improves it.",
                              ),
                            z
                              .literal("stalled")
                              .describe(
                                "No step improves the best pose it found, away from any limit: the target\nis out of reach, or the arm is in a singular pose that cannot move toward it.",
                              ),
                            z
                              .literal("iteration_limit")
                              .describe(
                                "It was still improving when it ran out of iterations.",
                              ),
                            z
                              .literal("out_of_reach")
                              .describe(
                                "The target is farther from the arm's first joint than the arm's links\nreach; the best pose stretches toward it.",
                              ),
                          ])
                          .describe("Why a solve stopped short."),
                        waypoint: z
                          .number()
                          .int()
                          .gte(0)
                          .describe(
                            "The waypoint the move stops at, counted from 0; later waypoints are left out.",
                          ),
                      })
                      .describe(
                        "A pose move whose target no joints reach exactly, run to the closest pose the\nsolver found instead: the plan's target is that pose, not what was asked for.",
                      ),
                    z.null(),
                  ]),
                  target_joints: z.union([z.array(z.number()), z.null()]),
                  target_position: z.union([
                    z.array(z.number()).min(3).max(3),
                    z.null(),
                  ]),
                  target_quaternion: z.union([
                    z.array(z.number()).min(4).max(4),
                    z.null(),
                  ]),
                  target_status: z.union([
                    z
                      .enum([
                        "reached",
                        "stopped_short",
                        "partial",
                        "not_started",
                      ])
                      .describe(
                        "Whether the arm ended where the command was aimed.",
                      ),
                    z.null(),
                  ]),
                  trace_context: z
                    .union([
                      z
                        .object({
                          traceparent: z
                            .string()
                            .describe(
                              "`00-<trace id>-<parent span id>-<flags>`.",
                            ),
                          tracestate: z
                            .union([z.string(), z.null()])
                            .default(null),
                        })
                        .describe(
                          "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
                        ),
                      z.null(),
                    ])
                    .describe(
                      "The trace context the command arrived with, if any.",
                    ),
                })
                .strict()
                .describe("One command, from acceptance to its end."),
              z.null(),
            ]),
            measured_joint_efforts: z
              .array(z.number())
              .describe(
                "Measured joint efforts in the hardware profile's effort units, in\n`measured_joints` order.",
              ),
            measured_joint_velocities: z
              .array(z.number())
              .describe(
                "Measured joint velocities in radians per second, in `measured_joints` order.",
              ),
            measured_joints: z.array(z.number()),
            mode: z
              .union([
                z
                  .literal("disabled")
                  .describe("Torque off; only `enable` is accepted."),
                z
                  .literal("holding")
                  .describe("Torque on, holding between commands."),
                z.literal("moving").describe("Following a motion or a stream."),
                z
                  .literal("faulted")
                  .describe(
                    "Powered off after a hard fault; refuses everything until `clear_fault`.",
                  ),
                z
                  .literal("disconnected")
                  .describe(
                    "The bus stopped answering: torque is off as far as the bus allowed, and\nthe port is opened again every second. Refuses everything meanwhile.",
                  ),
                z
                  .literal("reconnected")
                  .describe(
                    "The bus answers again, set up as at startup, with torque off. Nothing\nresumes on its own: as when disabled, only `enable` is accepted.",
                  ),
              ])
              .describe("Where the arm stands."),
            recovery_issue: z.union([
              z
                .object({
                  detail: z.string(),
                  motors_without_torque: z.array(
                    z.number().int().gte(0).lte(255),
                  ),
                })
                .describe(
                  "Why automatic recovery stopped, including motors found without torque.",
                ),
              z.null(),
            ]),
            rest_pose: z
              .union([
                z
                  .array(z.number())
                  .describe(
                    "The pose the arm was enabled at; shutdown returns there first.",
                  ),
                z
                  .null()
                  .describe(
                    "The pose the arm was enabled at; shutdown returns there first.",
                  ),
              ])
              .describe(
                "The pose the arm was enabled at; shutdown returns there first.",
              ),
            revision: z
              .number()
              .int()
              .gte(0)
              .describe(
                "Bumped each time an interlock is raised; commands may carry it as\n`expected_revision`.",
              ),
            sampled_at_unix_seconds: z
              .union([
                z
                  .number()
                  .describe(
                    "When the newest sample was taken, in Unix seconds; none before the first.",
                  ),
                z
                  .null()
                  .describe(
                    "When the newest sample was taken, in Unix seconds; none before the first.",
                  ),
              ])
              .describe(
                "When the newest sample was taken, in Unix seconds; none before the first.",
              ),
            segment: z.union([z.string(), z.null()]),
            status: z
              .string()
              .describe("Why the arm is as it is, in a few words."),
            tool: z
              .union([
                z
                  .object({
                    model_points: z
                      .array(z.array(z.number()).min(3).max(3))
                      .describe(
                        "Each joint's origin, then the tool's: the points a viewer draws the arm through.",
                      ),
                    orientation: z
                      .object({
                        approach: z.array(z.number()).min(3).max(3),
                        degrees_from_down: z.number(),
                        jaw_yaw_degrees: z.number(),
                        jaw_yaw_reference: z
                          .enum(["world_x", "world_y"])
                          .describe(
                            "The world axis jaw yaw zero is projected from.",
                          ),
                        target: z.union([
                          z
                            .object({
                              approach_error_degrees: z.number(),
                              jaw_yaw_degrees: z.number(),
                              jaw_yaw_reference: z
                                .enum(["world_x", "world_y"])
                                .describe(
                                  "The world axis jaw yaw zero is projected from.",
                                ),
                              orientation_error_degrees: z.number(),
                            })
                            .describe(
                              "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
                            ),
                          z.null(),
                        ]),
                      })
                      .describe(
                        "A tool orientation in grasp terms, rounded for people to read: where the jaws\npoint, how far that is from straight down, and the jaw yaw about it. With a\ntarget, also how far the orientation is from it.",
                      ),
                    position: z.array(z.number()).min(3).max(3),
                    quaternion: z.array(z.number()).min(4).max(4),
                  })
                  .describe(
                    "The tool's pose at some joints, in the station's world frame.",
                  ),
                z.null(),
              ])
              .describe(
                "The tool's pose at the measured joints, its orientation in grasp terms,\nand the joint origins a viewer sketches the arm through.",
              ),
          })
          .describe("One arm as clients see it."),
      ),
      description: z
        .union([
          z
            .string()
            .describe(
              "What a model is told about this robot: layout, frames, reach.",
            ),
          z
            .null()
            .describe(
              "What a model is told about this robot: layout, frames, reach.",
            ),
        ])
        .describe(
          "What a model is told about this robot: layout, frames, reach.",
        ),
      jaw_closing_axis: z
        .enum(["x", "y"])
        .describe("The tool-frame axis the jaws close along."),
      joint_names: z.array(z.string()),
      lease: z
        .union([
          z
            .object({
              expires_in_seconds: z
                .number()
                .describe(
                  "The lease lapses this long after its grant or last explicit renewal.",
                ),
              holder: z
                .union([
                  z
                    .literal("agent")
                    .describe(
                      "A model loop, started from the terminal or the agent server.",
                    ),
                  z
                    .literal("manual")
                    .describe(
                      "A person driving the arms from the viewer's manual controls.",
                    ),
                ])
                .describe("The kind of controller holding the lease."),
              label: z.string(),
              on_interruption: z
                .union([
                  z
                    .literal("wait_for_approval")
                    .describe(
                      "Keep the lease and wait for an operator's `approve_recovery`.",
                    ),
                  z.literal("return_lease").describe("Give the lease up."),
                ])
                .describe(
                  "What happens to the lease when an arm is interrupted and needs an\noperator's approval to recover.",
                ),
              required_cameras: z
                .array(z.string())
                .describe("Cameras the holder cannot work without."),
            })
            .describe("The current lease, as everyone may see it."),
          z.null(),
        ])
        .describe("None while no one holds the lease: anyone may command."),
      max_joint_speed: z.number(),
      robot: z
        .union([
          z
            .string()
            .describe(
              "The profile name, absent for a bare controller configuration.",
            ),
          z
            .null()
            .describe(
              "The profile name, absent for a bare controller configuration.",
            ),
        ])
        .describe(
          "The profile name, absent for a bare controller configuration.",
        ),
      surface_guard: z
        .union([
          z
            .object({
              leeway_meters: z.number(),
              surface_height_meters: z.number(),
            })
            .strict(),
          z.null(),
        ])
        .describe("None when the profile turns the table guard off."),
    })
    .describe(
      "The whole station, as `GET /state` and the event stream provide it.",
    ),
  SurfaceGuardConfig: z
    .object({ leeway_meters: z.number(), surface_height_meters: z.number() })
    .strict(),
  TargetComparison: z
    .object({
      approach_error_degrees: z.number(),
      jaw_yaw_degrees: z.number(),
      jaw_yaw_reference: z
        .enum(["world_x", "world_y"])
        .describe("The world axis jaw yaw zero is projected from."),
      orientation_error_degrees: z.number(),
    })
    .describe(
      "How an orientation compares with the one it was sent toward. Each jaw yaw\nnames its own reference: a change of reference near the switch is not a\nphysical error.",
    ),
  TargetStatus: z
    .enum(["reached", "stopped_short", "partial", "not_started"])
    .describe("Whether the arm ended where the command was aimed."),
  ToolOrientation: z
    .object({
      approach: z.array(z.number()).min(3).max(3),
      jaw_yaw_degrees: z.number(),
    })
    .describe(
      "An orientation in the terms a pose takes it: the approach and the jaw yaw\nabout it, at full precision, in degrees from 0 up to but excluding 360, so\nsending them back reproduces the rotation.",
    ),
  TraceContext: z
    .object({
      traceparent: z
        .string()
        .describe("`00-<trace id>-<parent span id>-<flags>`."),
      tracestate: z.union([z.string(), z.null()]).default(null),
    })
    .describe(
      "The caller's W3C trace context for a command, so a recorder can make the\ncommand's span a child of the caller's span.",
    ),
  TraceFile: z
    .object({
      modifiedUnixSeconds: z.number(),
      name: z.string(),
      sizeBytes: z.number().int().gte(0),
    })
    .describe("One trace file, as `GET /traces` lists it."),
  TraceSpan: z
    .object({
      attributes: z.record(z.string(), z.any()).default({}),
      end_time_unix_nano: z.string().regex(/^[0-9]+$/),
      name: z.string(),
      parent_span_id: z.union([z.string(), z.null()]).default(null),
      span_id: z.string(),
      start_time_unix_nano: z.string().regex(/^[0-9]+$/),
      status: z
        .object({
          code: z
            .enum(["STATUS_CODE_UNSET", "STATUS_CODE_OK", "STATUS_CODE_ERROR"])
            .default("STATUS_CODE_UNSET"),
          message: z.union([z.string(), z.null()]).default(null),
        })
        .default({ code: "STATUS_CODE_UNSET", message: null }),
      trace_id: z.string(),
    })
    .describe("One span of a model loop's trace, in OpenTelemetry's terms."),
  UnixNanos: z.string().regex(/^[0-9]+$/),
  ViewerControl: z
    .object({
      mode: z
        .union([
          z
            .literal("llm")
            .describe("A model loop drives; the viewer only watches."),
          z
            .literal("so101_manual")
            .describe("The viewer's pose controls drive a single SO-101."),
        ])
        .describe("What the viewer shows for driving the arm."),
    })
    .describe("`GET /viewer/api/control-mode`."),
  ViewerControlMode: z
    .union([
      z
        .literal("llm")
        .describe("A model loop drives; the viewer only watches."),
      z
        .literal("so101_manual")
        .describe("The viewer's pose controls drive a single SO-101."),
    ])
    .describe("What the viewer shows for driving the arm."),
  Waypoint: z
    .union([
      z.object({ joints: z.array(z.number()), kind: z.literal("joints") }),
      z.object({
        kind: z.literal("pose"),
        orientation: z
          .union([
            z
              .object({
                approach: z
                  .union([
                    z
                      .union([
                        z.literal("down").describe("World -Z."),
                        z.literal("forward").describe("World +X."),
                      ])
                      .describe("A named approach direction."),
                    z.array(z.number()).min(3).max(3),
                  ])
                  .describe(
                    "The direction the jaws point along: a named one, or a nonzero world direction.",
                  ),
                jaw_yaw_degrees: z.number(),
                kind: z.literal("grasp"),
              })
              .describe(
                "The approach direction and the jaw yaw about it, in degrees.",
              ),
            z
              .object({
                kind: z.literal("quaternion"),
                quaternion: z.array(z.number()).min(4).max(4),
              })
              .describe("A raw quaternion, `[x, y, z, w]`."),
          ])
          .describe("A tool orientation as a client gives it."),
        position: z.array(z.number()).min(3).max(3),
      }),
    ])
    .describe(
      "One corner of a move: joints, or a tool pose solved for joints.",
    ),
});
export type ApiSchemas = z.infer<typeof apiSchemas>;
