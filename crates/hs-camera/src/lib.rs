//! hs-camera — fly / orbit controllers + view-projection (plan §9, M0 subset).

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    Fly,
    Orbit,
    Walk,
}

#[derive(Clone, Debug)]
pub struct Camera {
    pub mode: Mode,
    pub pos: Vec3,
    pub yaw: f32,   // rad, 0 = +X
    pub pitch: f32, // rad
    pub fov_deg: f32,
    pub speed: f32,          // m/s for fly mode
    pub orbit_dist: f32,     // orbit: distance to target
    pub orbit_target: Vec3,
    pub damping: f32,
    pub sensitivity: f32,
    pub head_height: f32,    // walk mode eye height (plan §9.3: 1.2–2.2 m)
    pub ground_clamp: bool,  // fly mode soft terrain clamp
    vel: Vec3,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            mode: Mode::Fly,
            pos: Vec3::new(-2600.0, -3400.0, 1900.0),
            yaw: 0.82,
            pitch: -0.42,
            fov_deg: 42.0,
            speed: 220.0,
            orbit_dist: 4200.0,
            orbit_target: Vec3::new(0.0, 0.0, 120.0),
            damping: 0.35,
            sensitivity: 1.0,
            head_height: 1.7,
            ground_clamp: true,
            vel: Vec3::ZERO,
        }
    }
}

impl Camera {
    pub fn look_dir(&self) -> Vec3 {
        Vec3::new(
            self.yaw.cos() * self.pitch.cos(),
            self.yaw.sin() * self.pitch.cos(),
            self.pitch.sin(),
        )
    }

    /// Orbit mode derives position from target + spherical coords.
    pub fn sync_orbit(&mut self) {
        if self.mode == Mode::Orbit {
            let d = self.look_dir();
            self.pos = self.orbit_target - d * self.orbit_dist;
        }
    }

    /// Per-frame fly update. `keys` = [fwd, back, left, right, up, down], boost multiplier.
    pub fn update_fly(&mut self, dt: f32, keys: [bool; 6], boost: f32, mouse_delta: (f32, f32), sensitivity: f32) {
        if self.mode != Mode::Fly {
            return;
        }
        self.yaw -= mouse_delta.0 * 0.0026 * sensitivity;
        self.pitch = (self.pitch - mouse_delta.1 * 0.0026 * sensitivity).clamp(-1.45, 1.45);
        let fwd = self.look_dir();
        let right = fwd.cross(Vec3::Z).normalize_or_zero();
        let up = Vec3::Z;
        let mut wish = Vec3::ZERO;
        if keys[0] {
            wish += fwd;
        }
        if keys[1] {
            wish -= fwd;
        }
        if keys[2] {
            wish -= right;
        }
        if keys[3] {
            wish += right;
        }
        if keys[4] {
            wish += up;
        }
        if keys[5] {
            wish -= up;
        }
        if wish.length_squared() > 0.0 {
            wish = wish.normalize() * self.speed * boost;
        }
        let k = 1.0 - self.damping.clamp(0.0, 0.95).powf(dt * 60.0);
        self.vel += (wish - self.vel) * k;
        self.pos += self.vel * dt;
    }

    /// Walk / head-height mode (plan §9.1): WASD on the ground plane,
    /// mouse-look, terrain-follow with step smoothing.
    pub fn update_walk(
        &mut self,
        dt: f32,
        keys: [bool; 4],
        mouse_delta: (f32, f32),
        boost: f32,
        ground: &dyn Fn(f32, f32) -> f32,
    ) {
        self.yaw -= mouse_delta.0 * 0.0026 * self.sensitivity;
        self.pitch = (self.pitch - mouse_delta.1 * 0.0026 * self.sensitivity).clamp(-0.55, 0.45);
        let fwd = Vec3::new(self.yaw.cos(), self.yaw.sin(), 0.0);
        let right = Vec3::new(-fwd.y, fwd.x, 0.0);
        let mut wish = Vec3::ZERO;
        if keys[0] {
            wish += fwd;
        }
        if keys[1] {
            wish -= fwd;
        }
        if keys[2] {
            wish -= right;
        }
        if keys[3] {
            wish += right;
        }
        if wish.length_squared() > 0.0 {
            wish = wish.normalize() * 1.6 * boost; // m/s walk / run
        }
        let k = 1.0 - self.damping.clamp(0.0, 0.95).powf(dt * 60.0);
        self.vel += (wish - self.vel) * k;
        self.pos.x += self.vel.x * dt;
        self.pos.y += self.vel.y * dt;
        let target_z = ground(self.pos.x, self.pos.y) + self.head_height;
        self.pos.z += (target_z - self.pos.z) * (dt * 8.0).min(1.0);
    }

    /// Soft terrain clamp for fly mode (never sub-terrain).
    pub fn clamp_ground(&mut self, ground: &dyn Fn(f32, f32) -> f32) {
        let gz = ground(self.pos.x, self.pos.y) + 1.0;
        if self.pos.z < gz {
            self.pos.z = gz;
        }
    }

    pub fn orbit_drag(&mut self, dx: f32, dy: f32, sensitivity: f32) {
        self.yaw -= dx * 0.005 * sensitivity;
        self.pitch = (self.pitch - dy * 0.005 * sensitivity).clamp(-1.45, 0.05);
        self.sync_orbit();
    }

    pub fn dolly(&mut self, factor: f32) {
        if self.mode == Mode::Orbit {
            self.orbit_dist = (self.orbit_dist * factor).clamp(20.0, 60_000.0);
            self.sync_orbit();
        } else {
            self.speed = (self.speed * factor).clamp(0.5, 4000.0);
        }
    }

    /// Right-handed view matrix (camera looks down -Z of its view space).
    pub fn view(&self) -> Mat4 {
        let dir = self.look_dir();
        Mat4::look_at_rh(self.pos, self.pos + dir, Vec3::Z)
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(self.fov_deg.to_radians(), aspect, 0.5)
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walk_keeps_head_height() {
        let mut cam = Camera::default();
        cam.mode = Mode::Walk;
        cam.pos = Vec3::new(10.0, 10.0, 80.0);
        let ground = |_: f32, _: f32| 33.0;
        for _ in 0..240 {
            cam.update_walk(1.0 / 60.0, [true, false, false, false], (0.3, 0.0), 1.0, &ground);
        }
        assert!((cam.pos.z - (33.0 + 1.7)).abs() < 0.05, "z={}", cam.pos.z);
        assert!(cam.pos.x > 10.0);
    }

    /// Plan NFR precision (risk #6): at head height, the f32 view-space path
    /// must agree with an f64 reference to < 0.5 mm for region-local coords.
    #[test]
    fn head_height_precision() {
        use glam::{DMat4, DVec3};
        let pos = Vec3::new(1234.5678, -678.9012, 34.7);
        let vert = Vec3::new(1235.0, -678.0, 34.7);
        let cam = Camera {
            pos,
            yaw: 0.6,
            pitch: -0.15,
            fov_deg: 65.0,
            ..Default::default()
        };
        let v32 = cam.view() * vert.extend(1.0);
        let d = cam.look_dir().as_dvec3();
        let view64 = DMat4::look_at_rh(pos.as_dvec3(), pos.as_dvec3() + d, DVec3::Z);
        let v64 = view64 * vert.as_dvec3().extend(1.0);
        let err = (v32.x as f64 - v64.x)
            .abs()
            .max((v32.y as f64 - v64.y).abs())
            .max((v32.z as f64 - v64.z).abs());
        assert!(err < 5e-4, "view-space swim too large: {err} m");
    }
}
