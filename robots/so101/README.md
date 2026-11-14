# SO-101 model

From [TheRobotStudio SO-ARM100](https://github.com/TheRobotStudio/SO-ARM100),
commit `a758567c3978dfeefe282ede0500085a48fe8f78`, under Apache-2.0;
see [LICENSE.txt](LICENSE.txt).
`robot.urdf` derives from `Simulation/SO101/so101_new_calib.urdf` with mesh paths
flattened. The meshes are quadric-decimated copies of the original STLs.

The executor uses this geometry for kinematics and model-based collision checks.
These checks sample configured geometry; they do not establish calibration or
clearance from workspace objects or another arm. Verify the physical assembly
and its configured limits separately.
