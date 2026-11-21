import type { StationState } from "@hebbian/robot-use-client";
import {
  colorRobot,
  disposeRobotMaterials,
  SceneErrorBoundary,
  setRobotJoints,
  useRobotModel,
} from "@hebbian/so101-model/robot-model";
import { Canvas, useThree } from "@react-three/fiber";
import { useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { TransformControls } from "three/addons/controls/TransformControls.js";
import type { URDFRobot } from "urdf-loader";
import { Button } from "@/components/ui/button.js";
import { intersectsBaseSurface } from "../../base-surface.js";

const SO101_JOINTS = [
  "shoulder_pan",
  "shoulder_lift",
  "elbow_flex",
  "wrist_flex",
  "wrist_roll",
];
type Position = [number, number, number];
const DEFAULT_CAMERA_POSITION: Position = [0.65, -0.65, 0.55];
const DEFAULT_CAMERA_TARGET: Position = [0.16, 0, 0.14];
type SceneArm = {
  arm: string;
  basePose: StationState["arms"][number]["base_pose"];
  measuredJoints: (number | null)[];
  commandedJoints?: (number | null)[] | null;
  destinationJoints?: number[] | null;
  destinationLimited?: boolean;
  gripperOpening: number | null;
  gripperAngleRadians?: number;
};
export type SceneProjection = {
  arm: string;
  joints: number[];
  opening: number;
  withinTolerance: boolean;
  stale: boolean;
};
export type RobotSceneProps = {
  showDetails?: boolean;
  cameraPosition?: Position;
  cameraTarget?: Position;
  jointNames: string[];
  arms: SceneArm[];
  showTarget: boolean;
  showCommanded: boolean;
  target: Position | null;
  projection: SceneProjection | null;
  surfaceGuard: StationState["surface_guard"];
  onTargetChange?: (position: Position) => void;
  onTargetDrag?: (dragging: boolean) => void;
  onProjectionValidityChange?: (valid: boolean) => void;
};

function ArmModels({
  template,
  arm,
  jointNames,
  showTarget,
  showCommanded,
  projection,
  surfaceGuard,
  onProjectionValidityChange,
  dark,
}: {
  template: URDFRobot;
  dark: boolean;
  arm: SceneArm;
  jointNames: string[];
  showTarget: boolean;
  showCommanded: boolean;
  projection: SceneProjection | null;
  surfaceGuard: StationState["surface_guard"];
  onProjectionValidityChange?: (valid: boolean) => void;
}) {
  const { invalidate } = useThree();
  const lastValidPreview = useRef<{ joints: number[]; opening: number } | null>(
    null,
  );
  const [baseX = 0, baseY = 0, baseZ = 0] = arm.basePose?.position_meters ?? [];
  const baseYaw = arm.basePose?.yaw_radians ?? 0;
  const models = useMemo(() => {
    const measured = template.clone() as URDFRobot;
    const commanded = template.clone() as URDFRobot;
    const destination = template.clone() as URDFRobot;
    const preview = template.clone() as URDFRobot;
    for (const model of [measured, commanded, destination, preview]) {
      model.position.set(baseX, baseY, baseZ);
      model.rotation.z = baseYaw;
      // WHY: URDFLoader clamps to the URDF's nominal limits and cloning resets
      // ignoreLimits; draw the joints the station reports within the arm's calibrated range.
      for (const joint of Object.values(model.joints))
        joint.ignoreLimits = true;
    }
    commanded.visible = false;
    destination.visible = false;
    preview.visible = false;
    colorRobot(measured, dark ? 0xe5e5e5 : 0x999999, 1);
    colorRobot(commanded, 0x775cc4, 0.28);
    colorRobot(destination, 0x16865a, 0.32);
    colorRobot(preview, 0x087f8c, 0.4);
    return { measured, commanded, destination, preview };
  }, [template, baseX, baseY, baseZ, baseYaw, dark]);

  useEffect(
    () => () => {
      for (const model of Object.values(models)) disposeRobotMaterials(model);
    },
    [models],
  );
  useEffect(() => {
    const measuredAvailable =
      arm.measuredJoints.length === jointNames.length &&
      arm.measuredJoints.every((joint) => joint !== null);
    models.measured.visible = measuredAvailable;
    if (measuredAvailable)
      setRobotJoints(
        models.measured,
        jointNames,
        arm.measuredJoints,
        arm.gripperOpening,
        arm.gripperAngleRadians,
      );
    const commanded = arm.commandedJoints;
    models.commanded.visible = Boolean(
      showCommanded &&
        commanded &&
        commanded.length === jointNames.length &&
        commanded.every((joint) => joint !== null),
    );
    if (models.commanded.visible && commanded)
      setRobotJoints(
        models.commanded,
        jointNames,
        commanded,
        arm.gripperOpening,
      );
    const destination = arm.destinationJoints;
    models.destination.visible = Boolean(
      showTarget &&
        destination &&
        destination.length === jointNames.length &&
        destination.every(Number.isFinite),
    );
    if (models.destination.visible && destination)
      setRobotJoints(
        models.destination,
        jointNames,
        destination,
        arm.gripperOpening,
      );
    invalidate();
  }, [arm, jointNames, models, showTarget, showCommanded, invalidate]);

  const jointNamesKey = JSON.stringify(jointNames);
  const surfaceHeight = surfaceGuard?.surface_height_meters;
  const surfaceLeeway = surfaceGuard?.leeway_meters;
  useEffect(() => {
    const names = JSON.parse(jointNamesKey) as string[];
    models.preview.visible =
      projection?.arm === arm.arm &&
      projection.joints.length === names.length &&
      projection.joints.every(Number.isFinite);
    if (!models.preview.visible || !projection) {
      lastValidPreview.current = null;
      invalidate();
      return;
    }
    setRobotJoints(
      models.preview,
      names,
      projection.joints,
      projection.opening,
    );
    const surfaceZ = baseZ + (surfaceHeight ?? 0) - (surfaceLeeway ?? 0);
    const hasSurfaceGuard =
      surfaceHeight !== undefined && surfaceLeeway !== undefined;
    const valid =
      !hasSurfaceGuard || !intersectsBaseSurface(models.preview, surfaceZ);
    if (valid) {
      lastValidPreview.current = {
        joints: [...projection.joints],
        opening: projection.opening,
      };
    } else if (lastValidPreview.current) {
      setRobotJoints(
        models.preview,
        names,
        lastValidPreview.current.joints,
        lastValidPreview.current.opening,
      );
      if (hasSurfaceGuard && intersectsBaseSurface(models.preview, surfaceZ)) {
        lastValidPreview.current = null;
        models.preview.visible = false;
      }
    } else {
      models.preview.visible = false;
    }
    onProjectionValidityChange?.(valid);
    models.preview.traverse((object) => {
      if (
        object instanceof THREE.Mesh &&
        object.material instanceof THREE.MeshStandardMaterial
      )
        object.material.color.setHex(
          projection.stale
            ? 0x808080
            : projection.withinTolerance
              ? 0x087f8c
              : 0xc18b16,
        );
    });
    invalidate();
  }, [
    projection,
    arm.arm,
    jointNamesKey,
    models,
    baseZ,
    surfaceHeight,
    surfaceLeeway,
    onProjectionValidityChange,
    invalidate,
  ]);

  return (
    <>
      <primitive object={models.measured} />
      <primitive object={models.commanded} />
      <primitive object={models.destination} />
      <primitive object={models.preview} />
    </>
  );
}

function SceneControls({
  target,
  onTargetChange,
  onTargetDrag,
  resetCount,
  cameraPosition = DEFAULT_CAMERA_POSITION,
  cameraTarget = DEFAULT_CAMERA_TARGET,
}: Pick<
  RobotSceneProps,
  | "target"
  | "onTargetChange"
  | "onTargetDrag"
  | "cameraPosition"
  | "cameraTarget"
> & {
  resetCount: number;
}) {
  const { camera, gl, scene, invalidate } = useThree();
  const controls = useRef<OrbitControls | null>(null);
  const targetControls = useRef<TransformControls | null>(null);
  const targetMesh = useRef<THREE.Mesh | null>(null);
  const targetCallback = useRef(onTargetChange);
  const dragCallback = useRef(onTargetDrag);
  targetCallback.current = onTargetChange;
  dragCallback.current = onTargetDrag;
  useEffect(() => {
    camera.up.set(0, 0, 1);
    const orbit = new OrbitControls(camera, gl.domElement);
    orbit.minDistance = 0.25;
    orbit.maxDistance = 6;
    controls.current = orbit;
    const mesh = new THREE.Mesh(
      new THREE.SphereGeometry(0.012, 16, 12),
      new THREE.MeshBasicMaterial({ color: 0xc14d31 }),
    );
    mesh.visible = false;
    targetMesh.current = mesh;
    scene.add(mesh);
    const transform = new TransformControls(camera, gl.domElement);
    transform.setMode("translate");
    transform.setSize(0.7);
    const helper = transform.getHelper();
    helper.visible = false;
    scene.add(helper);
    targetControls.current = transform;
    const changed = () => invalidate();
    const objectChanged = () =>
      targetCallback.current?.(mesh.position.toArray() as Position);
    const dragChanged = (event: { value: unknown }) => {
      const dragging = event.value === true;
      orbit.enabled = !dragging;
      dragCallback.current?.(dragging);
    };
    orbit.addEventListener("change", changed);
    transform.addEventListener("change", changed);
    transform.addEventListener("objectChange", objectChanged);
    transform.addEventListener("dragging-changed", dragChanged);
    return () => {
      controls.current = null;
      targetControls.current = null;
      targetMesh.current = null;
      orbit.dispose();
      transform.dispose();
      scene.remove(mesh, helper);
      mesh.geometry.dispose();
      mesh.material.dispose();
    };
  }, [camera, gl, scene, invalidate]);
  // biome-ignore lint/correctness/useExhaustiveDependencies: The reset button advances resetCount to repeat the effect.
  useEffect(() => {
    camera.position.set(...cameraPosition);
    controls.current?.target.set(...cameraTarget);
    controls.current?.update();
    invalidate();
  }, [camera, cameraPosition, cameraTarget, invalidate, resetCount]);
  useEffect(() => {
    const mesh = targetMesh.current;
    const transform = targetControls.current;
    if (!mesh || !transform) return;
    if (target) {
      mesh.position.set(...target);
      mesh.visible = true;
      transform.getHelper().visible = true;
      transform.attach(mesh);
    } else {
      transform.detach();
      mesh.visible = false;
      transform.getHelper().visible = false;
    }
    invalidate();
  }, [target, invalidate]);
  return null;
}

function RobotSceneCanvas(
  props: RobotSceneProps & {
    modelArm: string;
    dark: boolean;
    resetCount: number;
  },
) {
  const { dark, resetCount, modelArm } = props;
  const loaded = useRobotModel(
    `/model/${encodeURIComponent(modelArm)}/robot.urdf`,
  );
  if (loaded === null) return <p>Loading SO-101 model…</p>;
  if (loaded.error !== null) return <p>{loaded.error}</p>;
  const robotModel = loaded.robot;
  return (
    <SceneErrorBoundary>
      <Canvas
        frameloop="demand"
        camera={{ fov: 40, near: 0.01, far: 30 }}
        aria-label="Interactive robot model: drag to orbit, scroll to zoom"
      >
        <color attach="background" args={[dark ? "#252525" : "#eeeeee"]} />
        <hemisphereLight args={[0xffffff, 0x888888, 1.5]} />
        <directionalLight position={[2, -3, 4]} intensity={1.5} />
        <gridHelper
          args={[2, 20, dark ? 0x666666 : 0xaaaaaa, dark ? 0x404040 : 0xd8d8d8]}
          rotation={[Math.PI / 2, 0, 0]}
        />
        <axesHelper args={[0.2]} />
        {props.arms.map((arm) => (
          <ArmModels
            key={arm.arm}
            template={robotModel}
            dark={dark}
            arm={arm}
            jointNames={props.jointNames}
            showTarget={props.showTarget}
            showCommanded={props.showCommanded}
            projection={props.projection}
            surfaceGuard={props.surfaceGuard}
            onProjectionValidityChange={props.onProjectionValidityChange}
          />
        ))}
        <SceneControls
          target={props.target}
          onTargetChange={props.onTargetChange}
          onTargetDrag={props.onTargetDrag}
          resetCount={resetCount}
          cameraPosition={props.cameraPosition}
          cameraTarget={props.cameraTarget}
        />
      </Canvas>
    </SceneErrorBoundary>
  );
}

export function RobotScene(props: RobotSceneProps) {
  const [resetCount, setResetCount] = useState(0);
  const [dark, setDark] = useState(
    () => document.documentElement.getAttribute("data-theme") === "dark",
  );
  useEffect(() => {
    const observer = new MutationObserver(() =>
      setDark(document.documentElement.getAttribute("data-theme") === "dark"),
    );
    observer.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme"],
    });
    return () => observer.disconnect();
  }, []);
  const modelArm = props.arms[0]?.arm;
  const supportedJoints =
    props.jointNames.length === SO101_JOINTS.length &&
    SO101_JOINTS.every((joint, index) => joint === props.jointNames[index]);
  return (
    <section className="robot-scene" aria-label="Robot visualization">
      <header>
        <Button
          variant="outline"
          onClick={() => setResetCount((count) => count + 1)}
        >
          Reset view
        </Button>
      </header>
      <div className="robot-scene-canvas">
        {supportedJoints && modelArm !== undefined && (
          <RobotSceneCanvas
            {...props}
            modelArm={modelArm}
            dark={dark}
            resetCount={resetCount}
          />
        )}
        {!props.jointNames.length && <p>Waiting for arm data…</p>}
        {props.jointNames.length > 0 && !supportedJoints && (
          <p>3D assets are available for the SO-101 only.</p>
        )}
      </div>
      {props.showDetails !== false && (
        <>
          <p className="robot-scene-note">
            Gray: measured. Purple: commanded. Green: destination. Teal: preview
            (amber if approximate, gray if stale).
          </p>
          {props.showTarget &&
            props.arms.some((arm) => arm.destinationJoints) && (
              <p className="robot-scene-note">
                Destination:{" "}
                {props.arms
                  .filter((arm) => arm.destinationJoints)
                  .map(
                    (arm) =>
                      `${arm.arm}${arm.destinationLimited ? " (limited stop)" : ""}`,
                  )
                  .join(", ")}
                . Gripper shown at measured opening.
              </p>
            )}
          <p className="robot-scene-note">
            Simplified geometry; the table, camera mounts and objects are not
            modeled. Gripper opening is approximate.
          </p>
          <details>
            <summary>Joint values</summary>
            {props.arms.map((arm) => (
              <div key={arm.arm}>
                <strong>{arm.arm}</strong>
                {arm.measuredJoints.map((joint, index) => (
                  <span key={props.jointNames[index] ?? index}>
                    {" "}
                    {props.jointNames[index] ?? `J${index + 1}`}:{" "}
                    {joint === null
                      ? "—"
                      : `${THREE.MathUtils.radToDeg(joint).toFixed(1)}°`}
                  </span>
                ))}
              </div>
            ))}
          </details>
        </>
      )}
    </section>
  );
}
