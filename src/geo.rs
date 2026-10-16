//! 座標與高程：局部麥卡托公尺（x 東、y 北），原點 (119.76E, 25.53N)。
//! 3D 世界：X = x、Y = 高、Z = -y（與網頁版相同）。

pub const R: f64 = 6378137.0;

#[derive(Clone, Copy, Debug)]
pub struct Geo {
    pub k: f64,
    pub mx0: f64,
    pub my0: f64,
}

impl Geo {
    pub fn new(lon0: f64, lat0: f64) -> Self {
        let d = std::f64::consts::PI / 180.0;
        Geo { k: (lat0 * d).cos(), mx0: R * lon0 * d, my0: R * ((std::f64::consts::PI / 4.0 + lat0 * d / 2.0).tan()).ln() }
    }
    pub fn to_local(&self, lon: f64, lat: f64) -> (f64, f64) {
        let d = std::f64::consts::PI / 180.0;
        ((R * lon * d - self.mx0) * self.k, (R * ((std::f64::consts::PI / 4.0 + lat * d / 2.0).tan()).ln() - self.my0) * self.k)
    }
    pub fn to_ll(&self, x: f64, y: f64) -> (f64, f64) {
        let d = std::f64::consts::PI / 180.0;
        ((x / self.k + self.mx0) / R / d, (2.0 * ((y / self.k + self.my0) / R).exp().atan() - std::f64::consts::PI / 2.0) / d)
    }
}

/// 高程網格（公尺，float），與 terrain.png 同網格。
pub struct HeightGrid {
    pub w: usize,
    pub h: usize,
    pub xmin: f64,
    pub ymax: f64,
    pub pxm: f64,
    pub data: Vec<f32>,
}

impl HeightGrid {
    pub fn at(&self, x: f64, y: f64) -> f32 {
        let fx = (x - self.xmin) / self.pxm - 0.5;
        let fy = (self.ymax - y) / self.pxm - 0.5;
        if fx < 0.0 || fy < 0.0 || fx >= (self.w - 1) as f64 || fy >= (self.h - 1) as f64 {
            return -20.0;
        }
        let (ix, iy) = (fx as usize, fy as usize);
        let (ax, ay) = ((fx - ix as f64) as f32, (fy - iy as f64) as f32);
        let w = self.w;
        let a = self.data[iy * w + ix];
        let b = self.data[iy * w + ix + 1];
        let c = self.data[(iy + 1) * w + ix];
        let d = self.data[(iy + 1) * w + ix + 1];
        (a * (1.0 - ax) + b * ax) * (1.0 - ay) + (c * (1.0 - ax) + d * ax) * ay
    }
}
