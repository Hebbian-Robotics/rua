import argparse
import hashlib
import json
from pathlib import Path

import numpy as np
import onnx
import onnxruntime
import torch

NORMALIZER_EPSILON = 0.01


class PolicyGraph(torch.nn.Module):
    goal_norm_mean: torch.Tensor
    goal_norm_scale: torch.Tensor
    proprioceptive_norm_mean: torch.Tensor
    proprioceptive_norm_scale: torch.Tensor

    def __init__(self, weights: dict[str, torch.Tensor]) -> None:
        super().__init__()
        for name in ("proprioceptive_norm", "goal_norm"):
            self.register_buffer(f"{name}_mean", weights[f"{name}.mean"].float())
            self.register_buffer(
                f"{name}_scale",
                torch.sqrt(weights[f"{name}.var"].float() + NORMALIZER_EPSILON**2),
            )
        self.encoder = self._stack(weights, "encoder")
        self.decoder = self._stack(weights, "decoder")

    @staticmethod
    def _stack(weights: dict[str, torch.Tensor], prefix: str) -> torch.nn.Sequential:
        indices = sorted(
            int(key.split(".")[1])
            for key in weights
            if key.startswith(prefix + ".") and key.endswith(".weight")
        )
        if indices != list(range(0, len(indices) * 2, 2)):
            raise ValueError(f"Unsupported policy {prefix} architecture")
        modules: list[torch.nn.Module] = []
        for position, index in enumerate(indices):
            weight = weights[f"{prefix}.{index}.weight"].float()
            linear = torch.nn.Linear(weight.shape[1], weight.shape[0])
            linear.weight.data = weight
            linear.bias.data = weights[f"{prefix}.{index}.bias"].float()
            modules.append(linear)
            if position + 1 < len(indices):
                modules.append(torch.nn.ELU())
        return torch.nn.Sequential(*modules)

    # CONSTRAINT: PyTorch declares Module.forward as a writable callable; its contract requires
    # subclasses to override it with the model's actual input signature.
    def forward(self, goal: torch.Tensor, history: torch.Tensor) -> torch.Tensor:
        normalized_goal = (goal - self.goal_norm_mean) / self.goal_norm_scale
        normalized_history = (
            history - self.proprioceptive_norm_mean
        ) / self.proprioceptive_norm_scale
        latent = torch.tanh(
            self.encoder(torch.cat((normalized_goal, normalized_history), dim=-1))
        )
        return self.decoder(torch.cat((latent, normalized_history), dim=-1))


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


JOINT_NAMES = (
    "shoulder_pan",
    "shoulder_lift",
    "elbow_flex",
    "wrist_flex",
    "wrist_roll",
)


def default_joint_positions(training_model: Path) -> list[float]:
    # CONSTRAINT: Relative observations subtract the training model's default pose.
    import mujoco

    model = mujoco.MjModel.from_xml_path(str(training_model))
    return [
        float(model.qpos0[int(model.joint(name).qposadr[0])])  # ty: ignore[missing-attribute]  # Missing from the mujoco stubs.
        for name in JOINT_NAMES
    ]


def export(
    checkpoint_path: Path,
    source_manifest_path: Path,
    training_model: Path,
    output: Path,
) -> None:
    checkpoint = torch.load(checkpoint_path, map_location="cpu", weights_only=True)
    if checkpoint.get("method") != "supermimic_so101_reference_pose_v2":
        raise ValueError("Unsupported SO-101 policy observation contract")
    if sha256(source_manifest_path) != checkpoint.get("source_manifest_sha256"):
        raise ValueError("Policy source manifest does not match checkpoint")
    graph = PolicyGraph(checkpoint["reactive_policy"]).eval()
    goal = torch.zeros(1, 21)
    history = torch.zeros(1, 150)
    torch.onnx.export(
        graph,
        (goal, history),
        str(output),
        input_names=["goal", "history"],
        output_names=["action"],
        opset_version=17,
        dynamo=False,
    )
    model = onnx.load(str(output))
    relative = bool(checkpoint.get("relative_joint_observation", False))
    metadata = {
        "observation_joint_offset": json.dumps(
            default_joint_positions(training_model) if relative else [0.0] * 5
        ),
        "method": checkpoint["method"],
        "action_mode": checkpoint["action_mode"],
        "action_step": repr(float(checkpoint["action_step"])),
        "checkpoint_sha256": sha256(checkpoint_path),
        "source_manifest_sha256": sha256(source_manifest_path),
    }
    for key, value in metadata.items():
        entry = model.metadata_props.add()
        entry.key = key
        entry.value = value
    onnx.save(model, str(output))

    session = onnxruntime.InferenceSession(
        str(output), providers=["CPUExecutionProvider"]
    )
    generator = np.random.default_rng(0)
    sample_goal = generator.normal(size=(8, 21)).astype(np.float32)
    sample_history = generator.normal(size=(8, 150)).astype(np.float32)
    exported = np.concatenate(
        [
            session.run(None, {"goal": goal_row[None], "history": history_row[None]})[0]
            for goal_row, history_row in zip(sample_goal, sample_history, strict=True)
        ]
    )
    with torch.inference_mode():
        reference = graph(
            torch.from_numpy(sample_goal), torch.from_numpy(sample_history)
        )
    difference = float(np.abs(exported - reference.numpy()).max())
    print(f"max difference against torch: {difference:.2e}")
    if difference > 1e-4:
        raise RuntimeError("ONNX export does not match the checkpoint network")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Export a SO-101 SuperMimic checkpoint to ONNX for the Rust executor.\n"
            "\n"
            "The graph takes the raw goal (21) and the raw 10-frame history (150), normalises\n"
            "them, runs encoder and decoder, and returns the unclamped action (5). The clamp\n"
            "to +-1 and the checkpoint-declared action step happen in the executor. The metadata carries what\n"
            "the executor refuses to run without: the contract name, the step, the joint\n"
            "offset, and the checkpoint and manifest hashes.\n"
            "\n"
            "    uv run --group policy python scripts/export_so101_policy_onnx.py         --checkpoint so101-servo-fit-30000-updates.pt         --source-manifest source-manifest.json         --training-model robot-soft-gripper-camera.xml         --output policy.onnx\n"
        )
    )
    parser.add_argument("--checkpoint", type=Path, required=True)
    parser.add_argument("--source-manifest", type=Path, required=True)
    parser.add_argument("--training-model", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    export(
        arguments.checkpoint,
        arguments.source_manifest,
        arguments.training_model,
        arguments.output,
    )


if __name__ == "__main__":
    main()
