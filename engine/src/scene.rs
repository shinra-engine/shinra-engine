use glam::{Mat4, Vec3};
use std::sync::Arc;

use crate::mesh::Mesh;

pub enum Projection {
    Perspective {
        fov_y_radians: f32,
        aspect: f32,
        znear: f32,
        zfar: f32,
    },
    Orthographic {
        half_height: f32,
        aspect: f32,
        znear: f32,
        zfar: f32,
    },
}

pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub projection: Projection,
}

impl Camera {
    pub fn view_proj(&self) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye, self.target, self.up);
        let proj = match self.projection {
            Projection::Perspective {
                fov_y_radians,
                aspect,
                znear,
                zfar,
            } => Mat4::perspective_rh(fov_y_radians, aspect, znear, zfar),
            Projection::Orthographic {
                half_height,
                aspect,
                znear,
                zfar,
            } => {
                let half_width = half_height * aspect;
                Mat4::orthographic_rh(
                    -half_width,
                    half_width,
                    -half_height,
                    half_height,
                    znear,
                    zfar,
                )
            }
        };
        proj * view
    }
}

pub struct MeshHandle(pub Arc<Mesh>);
pub struct Model(pub Mat4);
pub struct BunnyTag;
pub struct TeapotTag;
pub struct PlayerControlled;

pub struct Scene {
    pub camera: Camera,
    pub world: hecs::World,
}

impl Scene {
    pub fn new(camera: Camera) -> Self {
        Self {
            camera,
            world: hecs::World::new(),
        }
    }

    pub fn spawn_mesh(&mut self, mesh: Arc<Mesh>, model: Mat4) -> hecs::Entity {
        self.world.spawn((MeshHandle(mesh), Model(model)))
    }

    pub fn set_model(&mut self, e: hecs::Entity, model: Mat4) {
        if let Ok(mut m) = self.world.get::<&mut Model>(e) {
            *m = Model(model);
        }
    }
}

pub fn orbit_eye(t_seconds: f32, radius: f32, height: f32) -> Vec3 {
    Vec3::new(t_seconds.cos() * radius, height, t_seconds.sin() * radius)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn default_perspective() -> Camera {
        Camera {
            eye: Vec3::new(3.0, 1.5, 0.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            projection: Projection::Perspective {
                fov_y_radians: PI / 3.0,
                aspect: 16.0 / 9.0,
                znear: 0.1,
                zfar: 100.0,
            },
        }
    }

    fn default_ortho() -> Camera {
        Camera {
            eye: Vec3::new(3.0, 1.5, 0.0),
            target: Vec3::ZERO,
            up: Vec3::Y,
            projection: Projection::Orthographic {
                half_height: 2.0,
                aspect: 16.0 / 9.0,
                znear: 0.1,
                zfar: 100.0,
            },
        }
    }

    #[test]
    fn perspective_view_proj_is_finite() {
        let m = default_perspective().view_proj();
        assert!(m.to_cols_array().iter().all(|v| v.is_finite()));
    }

    #[test]
    fn orthographic_view_proj_is_finite() {
        let m = default_ortho().view_proj();
        assert!(m.to_cols_array().iter().all(|v| v.is_finite()));
    }

    #[test]
    fn orbit_eye_at_zero() {
        let v = orbit_eye(0.0, 3.0, 1.5);
        assert!((v.x - 3.0).abs() < 1e-6);
        assert!((v.y - 1.5).abs() < 1e-6);
        assert!(v.z.abs() < 1e-6);
    }

    #[test]
    fn plus_x_point_in_front_of_camera() {
        let cam = default_perspective();
        let vp = cam.view_proj();
        let point = glam::Vec4::new(1.0, 0.0, 0.0, 1.0);
        let clip = vp * point;
        assert!(clip.w > 0.0, "point should be in front of camera");
    }
}
