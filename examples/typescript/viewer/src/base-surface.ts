import * as THREE from "three";

const SURFACE_TOLERANCE_METERS = 0.0001;

export function intersectsBaseSurface(
  robot: THREE.Object3D,
  surfaceZ: number,
): boolean {
  if (!Number.isFinite(surfaceZ)) return true;
  robot.updateWorldMatrix(true, true);
  let intersects = false;
  let movingMeshCount = 0;
  const worldBounds = new THREE.Box3();
  robot.traverse((object) => {
    if (intersects || !(object instanceof THREE.Mesh)) return;
    let link: THREE.Object3D | null = object;
    while (link && !("isURDFLink" in link)) link = link.parent;
    if (!link || link.name === "base_link") return;
    movingMeshCount += 1;
    const position = object.geometry.getAttribute("position");
    if (!position || position.count === 0) {
      intersects = true;
      return;
    }
    if (!object.geometry.boundingBox) object.geometry.computeBoundingBox();
    if (!object.geometry.boundingBox) {
      intersects = true;
      return;
    }
    worldBounds
      .copy(object.geometry.boundingBox)
      .applyMatrix4(object.matrixWorld);
    if (worldBounds.min.z >= surfaceZ - SURFACE_TOLERANCE_METERS) return;
    const matrix = object.matrixWorld.elements;
    for (let index = 0; index < position.count; index += 1) {
      const z =
        (matrix[2] ?? 0) * position.getX(index) +
        (matrix[6] ?? 0) * position.getY(index) +
        (matrix[10] ?? 0) * position.getZ(index) +
        (matrix[14] ?? 0);
      if (!Number.isFinite(z) || z < surfaceZ - SURFACE_TOLERANCE_METERS) {
        intersects = true;
        return;
      }
    }
  });
  return intersects || movingMeshCount === 0;
}
