use std::path::Path;
use std::sync::Arc;

use hebbian_robot_geometry::robot::Robot;
use hebbian_robot_geometry::self_collision::SelfCollisionModel;
use nalgebra::Point3;

#[test]
fn the_wrist_camera_is_included_when_it_is_the_lowest_moving_part() -> anyhow::Result<()> {
    let model = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../robots/so101/robot-soft-gripper-camera.urdf");
    let joint_names = [
        "shoulder_pan",
        "shoulder_lift",
        "elbow_flex",
        "wrist_flex",
        "wrist_roll",
        "gripper",
    ]
    .map(str::to_owned);
    let robot = Arc::new(Robot::load(&model, &joint_names)?);
    // WHY: This configuration puts the wrist camera below the fingers and bracket.
    let joints = [0.0, -0.5, 1.5, -0.5, 1.5, 0.0];
    let camera = robot.link_pose("wrist_camera_board_link", &joints)?;
    // CONSTRAINT: Board width and front/rear reserves, independently specified by the
    // supplier module dimensions and OEM rear-connector drawing.
    let mut lowest_camera_corner = f64::INFINITY;
    for horizontal in [-0.019, 0.019] {
        for vertical in [-0.019, 0.019] {
            for depth in [-0.0062, 0.01605] {
                let corner = camera * Point3::new(horizontal, vertical, depth);
                lowest_camera_corner = lowest_camera_corner.min(corner.z);
            }
        }
    }
    assert!((robot.lowest_point(&joints)? - lowest_camera_corner).abs() < 1e-8);
    assert!(
        SelfCollisionModel::new(robot, 0.001)?
            .collision(&joints)?
            .is_none()
    );
    Ok(())
}
