//! 地圖式鏡頭：目標點＋距離＋方位＋俯角；拖曳平移、旋轉傾斜、滾輪往游標縮放、鍵盤、飛行動畫。
use crate::geo::HeightGrid;
use glam::{Mat4, Vec2, Vec3, Vec4};

pub struct Anim {
    from: [f32; 5],
    to: [f32; 5],
    t: f32,
    dur: f32,
    arc: f32,
}

pub struct Camera {
    pub target: Vec3,
    pub dist: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub min_pitch: f32,
    pub max_pitch: f32,
    pub fov: f32,
    pub anim: Option<Anim>,
    pub pos: Vec3,
    pub view: Mat4,
    pub proj: Mat4,
    pub vp: Mat4,
    pub near: f32,
    pub vw: f32,
    pub vh: f32,
}

impl Camera {
    pub fn new() -> Camera {
        Camera {
            target: Vec3::ZERO, dist: 26000.0, yaw: 0.3, pitch: 0.75, min_pitch: 0.12, max_pitch: std::f32::consts::FRAC_PI_2 - 0.001, fov: 45f32.to_radians(),
            anim: None, pos: Vec3::ZERO, view: Mat4::IDENTITY, proj: Mat4::IDENTITY, vp: Mat4::IDENTITY, near: 1.0, vw: 1.0, vh: 1.0,
        }
    }
    pub fn update(&mut self, hg: &HeightGrid, dt: f32, w: f32, h: f32) -> bool {
        let mut moving = false;
        if let Some(a) = &mut self.anim {
            a.t += dt;
            let k = if a.dur > 0.0 { (a.t / a.dur).min(1.0) } else { 1.0 };
            let e = if k < 0.5 { 4.0 * k * k * k } else { 1.0 - (-2.0 * k + 2.0f32).powi(3) / 2.0 };
            let l = |i: usize| a.from[i] + (a.to[i] - a.from[i]) * e;
            self.target.x = l(0);
            self.target.z = l(1);
            self.dist = l(2) + (std::f32::consts::PI * e).sin() * a.arc;
            self.yaw = l(3);
            self.pitch = l(4);
            moving = true;
            if k >= 1.0 { self.anim = None; }
        }
        let th = hg.at(self.target.x as f64, -self.target.z as f64).max(0.0);
        self.target.y += (th - self.target.y) * 0.25;
        let (cp, sp) = (self.pitch.cos(), self.pitch.sin());
        let mut pos = Vec3::new(self.target.x + self.yaw.sin() * cp * self.dist, self.target.y + sp * self.dist, self.target.z + self.yaw.cos() * cp * self.dist);
        let gh = hg.at(pos.x as f64, -pos.z as f64).max(0.0) + 8.0;
        if pos.y < gh { pos.y = gh; }
        self.pos = pos;
        self.near = (self.dist * 0.004).max(1.0);
        let far = (self.dist * 6.0).max(400000.0);
        self.vw = w;
        self.vh = h;
        self.view = Mat4::look_at_rh(pos, self.target, Vec3::Y);
        self.proj = Mat4::perspective_rh(self.fov, w / h.max(1.0), self.near, far);
        self.vp = self.proj * self.view;
        moving
    }
    /// 世界座標 → 螢幕像素（邏輯點）
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let c = self.vp * Vec4::new(p.x, p.y, p.z, 1.0);
        if c.w <= 0.0 { return None; }
        let n = c.truncate() / c.w;
        if n.z > 1.0 { return None; }
        Some(Vec2::new((n.x + 1.0) / 2.0 * self.vw, (1.0 - n.y) / 2.0 * self.vh))
    }
    pub fn ray(&self, sx: f32, sy: f32) -> (Vec3, Vec3) {
        let inv = self.vp.inverse();
        let nx = sx / self.vw * 2.0 - 1.0;
        let ny = -(sy / self.vh * 2.0 - 1.0);
        let a = inv * Vec4::new(nx, ny, 0.0, 1.0);
        let b = inv * Vec4::new(nx, ny, 1.0, 1.0);
        let a = a.truncate() / a.w;
        let b = b.truncate() / b.w;
        (a, (b - a).normalize())
    }
    /// 螢幕 → 地面（高度場步進）
    pub fn ground_at(&self, hg: &HeightGrid, sx: f32, sy: f32) -> Option<Vec3> {
        let (o, d) = self.ray(sx, sy);
        if d.y >= -1e-4 { return None; }
        let mut t = 0.0f32;
        let mut step = (self.dist * 0.01).max(5.0);
        let max_t = self.dist * 6.0 + 50000.0;
        let mut prev: Option<f32> = None;
        let hgt = |t: f32| hg.at((o.x + d.x * t) as f64, -(o.z + d.z * t) as f64).max(0.0);
        while t < max_t {
            let y = o.y + d.y * t;
            if y <= hgt(t) {
                if let Some(p) = prev {
                    let (mut a, mut b) = (p, t);
                    for _ in 0..12 {
                        let m = (a + b) / 2.0;
                        if o.y + d.y * m <= hgt(m) { b = m } else { a = m }
                    }
                    t = b;
                }
                return Some(o + d * t);
            }
            prev = Some(t);
            t += step;
            step *= 1.02;
        }
        let tp = -o.y / d.y;
        Some(o + d * tp)
    }
    pub fn fly_to(&mut self, x: f32, y: f32, dist: Option<f32>, bearing_deg: Option<f32>, pitch_deg: Option<f32>, secs: f32) {
        let to_yaw = bearing_deg.map(|b| -b.to_radians()).unwrap_or(self.yaw);
        let mut dy = to_yaw - self.yaw;
        dy = dy.sin().atan2(dy.cos());
        let to = [x, -y, dist.unwrap_or(self.dist), self.yaw + dy, pitch_deg.map(|p| p.to_radians().clamp(self.min_pitch, self.max_pitch)).unwrap_or(self.pitch)];
        let from = [self.target.x, self.target.z, self.dist, self.yaw, self.pitch];
        let far = ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2)).sqrt();
        self.anim = Some(Anim { from, to, t: 0.0, dur: secs, arc: (far * 0.6 - from[2].max(to[2]) * 0.5).max(0.0) });
    }
    pub fn meters_per_pixel(&self) -> f32 {
        2.0 * self.dist * (self.fov / 2.0).tan() / self.vh.max(1.0)
    }
    pub fn bearing_deg(&self) -> f32 {
        -self.yaw.to_degrees()
    }
}
