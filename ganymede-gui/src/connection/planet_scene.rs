//! Deterministic scene shared by the build-time opening frame and live animation.
use std::f64::consts::{PI, TAU};

pub(super) const SIDE: usize = 480;
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46, 6, 38, 60,
    28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27, 49, 17, 57, 25, 15,
    47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

fn threshold(index: usize) -> f64 {
    (BAYER[((index / SIDE) & 7) * 8 + ((index % SIDE) & 7)] as f64 + 0.5) / 64.
}
struct SpherePixel {
    index: usize,
    u: f64,
    row: usize,
    lit: f64,
    rim: f64,
}
struct Star {
    index: usize,
    brightness: f64,
    phase: f64,
}
struct Dust {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    brightness: f64,
}
pub(super) struct Scene {
    base: Vec<u8>,
    sphere: Vec<SpherePixel>,
    stars: Vec<Star>,
    dust: Vec<Dust>,
    texture: Vec<f64>,
}
impl Scene {
    #[cfg(test)]
    pub(super) fn terrain(&self) -> &[u8] {
        &self.base
    }

    pub(super) fn new() -> Self {
        let mut base = vec![0; SIDE * SIDE];
        let mut sphere = Vec::new();
        let mut stars = Vec::new();
        let light = norm(0.62, 0.3, 0.72);
        for j in 0..SIDE {
            for i in 0..SIDE {
                let index = j * SIDE + i;
                let x = (i as f64 + 0.5 - SIDE as f64 / 2.) / SIDE as f64;
                let y = (j as f64 + 0.5) / SIDE as f64;
                let d = y - (0.62 + 0.1 * x * x);
                if d > 0. {
                    let lum = if d <= 1. / SIDE as f64 {
                        ground(x, d).max(0.85)
                    } else {
                        ground(x, d)
                    };
                    base[index] = u8::from(lum > threshold(index));
                    continue;
                }
                let glow = 0.045 * (d / 0.02).exp();
                base[index] = u8::from(glow > threshold(index));
                let nx = x / 0.33;
                let ny = -(y - 0.47) / 0.33;
                let r2 = nx * nx + ny * ny;
                if r2 >= 1. {
                    let h = hash(index as i32, 91, 7);
                    if glow <= 0.03 && h < 0.003 {
                        stars.push(Star {
                            index,
                            brightness: 0.35 + h / 0.003 * 0.65,
                            phase: hash(index as i32, 3, 5) * TAU,
                        });
                    }
                    continue;
                }
                let nz = (1. - r2).sqrt();
                let tx = nx * 0.18_f64.cos() - ny * 0.18_f64.sin();
                let ty = nx * 0.18_f64.sin() + ny * 0.18_f64.cos();
                let u = (tx.atan2(nz) + 0.6) / TAU + 0.5;
                let v = 0.5 - ty.clamp(-1., 1.).asin() / PI;
                let lit = (nx * light[0] + ny * light[1] + nz * light[2])
                    .max(0.)
                    .powf(0.85);
                sphere.push(SpherePixel {
                    index,
                    u,
                    row: (v * 256.).floor().min(255.) as usize,
                    lit: 0.035 + lit,
                    rim: 0.16 * smooth(0.82, 1., r2) * (1. - (lit * 5.).min(1.)),
                });
            }
        }
        let mut texture = Vec::with_capacity(512 * 256);
        for j in 0..256 {
            let lat = (0.5 - (j as f64 + 0.5) / 256.) * PI;
            for i in 0..512 {
                let lon = ((i as f64 + 0.5) / 512.) * TAU - PI;
                texture.push(banded(
                    lat.cos() * lon.sin(),
                    lat.sin(),
                    lat.cos() * lon.cos(),
                    lat,
                    lon,
                ));
            }
        }
        let mut seed = 42;
        let dust = (0..10)
            .map(|_| {
                let depth = random(&mut seed);
                let speed = 0.03 + depth * depth * 0.35;
                Dust {
                    x: (random(&mut seed) - 0.6) * 1.2,
                    y: random(&mut seed),
                    vx: speed * (0.22 + random(&mut seed) * 0.18),
                    vy: speed,
                    brightness: 0.12 + depth * 0.45,
                }
            })
            .collect();
        Self {
            base,
            sphere,
            stars,
            dust,
            texture,
        }
    }
    pub(super) fn mask(&self, time: f64) -> Vec<u8> {
        let mut mask = self.base.clone();
        for star in &self.stars {
            let b = star.brightness
                * (0.65 + 0.35 * (time * (1.3 + star.phase * 0.3) + star.phase).sin());
            mask[star.index] |= u8::from(b > threshold(star.index));
        }
        for p in &self.sphere {
            let u = (p.u + time * 0.05 / TAU).rem_euclid(1.);
            let a = self.texture[p.row * 512 + (u * 512.) as usize];
            mask[p.index] = u8::from(a * p.lit + p.rim > threshold(p.index));
        }
        for p in &self.dust {
            let travel = (p.y + 0.1 + time * p.vy).rem_euclid(1.3);
            let y = travel - 0.1;
            let x = p.x + (travel - p.y - 0.1) * p.vx / p.vy;
            let ex = p.vx * 0.12;
            let ey = p.vy * 0.12;
            let steps = ((ex * SIDE as f64).hypot(ey * SIDE as f64).ceil() as usize).max(1);
            for s in 0..=steps {
                let f = s as f64 / steps as f64;
                let i = ((x - ex * f) * SIDE as f64 + SIDE as f64 / 2.).floor() as i32;
                let j = ((y - ey * f) * SIDE as f64).floor() as i32;
                if (0..SIDE as i32).contains(&i) && (0..SIDE as i32).contains(&j) {
                    let index = j as usize * SIDE + i as usize;
                    mask[index] |= u8::from(p.brightness * (1. - 0.75 * f) > threshold(index));
                }
            }
        }
        mask
    }
}

fn smooth(a: f64, b: f64, x: f64) -> f64 {
    let t = ((x - a) / (b - a)).clamp(0., 1.);
    t * t * (3. - 2. * t)
}
fn norm(x: f64, y: f64, z: f64) -> [f64; 3] {
    let l = (x * x + y * y + z * z).sqrt();
    [x / l, y / l, z / l]
}
fn hash(x: i32, y: i32, z: i32) -> f64 {
    let mut h = (x as u32).wrapping_mul(0x27d4eb2d)
        ^ (y as u32).wrapping_mul(0x165667b1)
        ^ (z as u32).wrapping_mul(0x1b873593);
    h = (h ^ (h >> 15)).wrapping_mul(0x85ebca6b);
    h = (h ^ (h >> 13)).wrapping_mul(0xc2b2ae35);
    (h ^ (h >> 16)) as f64 / 4294967296.
}
fn noise(x: f64, y: f64, z: f64) -> f64 {
    let xi = x.floor() as i32;
    let yi = y.floor() as i32;
    let zi = z.floor() as i32;
    let u = smooth(0., 1., x - x.floor());
    let v = smooth(0., 1., y - y.floor());
    let w = smooth(0., 1., z - z.floor());
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let plane = |z| {
        lerp(
            lerp(hash(xi, yi, z), hash(xi + 1, yi, z), u),
            lerp(hash(xi, yi + 1, z), hash(xi + 1, yi + 1, z), u),
            v,
        )
    };
    lerp(plane(zi), plane(zi + 1), w)
}
fn fbm(x: f64, y: f64, z: f64, oct: usize) -> f64 {
    let (mut s, mut a, mut n, mut f) = (0., 0.5, 0., 1.);
    for i in 0..oct {
        s += a * noise(x * f + i as f64 * 17.13, y * f, z * f);
        n += a;
        a *= 0.5;
        f *= 2.02;
    }
    s / n
}
fn banded(x: f64, y: f64, z: f64, lat: f64, lon: f64) -> f64 {
    let t = fbm(x * 2.2 + 5., y * 2.2, z * 2.2, 5);
    let w = fbm(x * 9., y * 9. + 4., z * 9., 3);
    let mut a = 0.55 + 0.2 * (lat * 13. + (t - 0.5) * 5.).sin() + (w - 0.5) * 0.3;
    let dl = lon - 0.7;
    let dlon = dl.sin().atan2(dl.cos()) * lat.cos() / 0.15;
    let e = (((lat + 0.36) / 0.07).powi(2) + dlon * dlon).sqrt();
    a += (0.82 - 0.2 * e - a) * smooth(1.1, 0.75, e) - 0.12 * (-(e - 1.1).powi(2) * 30.).exp();
    (a * (1. - 0.3 * smooth(0.75, 1., y.abs()))).clamp(0., 1.)
}
fn ground(x: f64, d: f64) -> f64 {
    let z = 0.55 / (d + 0.012);
    let wx = x * z;
    let oct = if z < 5. {
        5
    } else if z < 12. {
        4
    } else {
        3
    };
    let height = |x: f64, y: f64| fbm(x * 0.8, 7.3, y * 0.8, oct) * 1.6;
    let h = height(wx, z);
    let hx = (height(wx + 0.04, z) - h) / 0.04;
    let hz = (height(wx, z + 0.04) - h) / 0.04;
    let light = norm(0.7, 0.55, 0.45);
    let sh =
        ((-hx * light[0] + light[1] - hz * light[2]) / (hx * hx + 1. + hz * hz).sqrt()).max(0.);
    let alb = 0.4 + (fbm(wx * 2.5, 1.7, z * 2.5, 2) - 0.5) * 0.5;
    let lum = alb * (0.03 + 1.25 * sh.powf(1.6)) * (0.85 + 0.35 * smooth(0.05, 0.35, d));
    let far = smooth(5., 28., z);
    lum * (1. - far) + 0.26 * far
}
fn random(seed: &mut u32) -> f64 {
    *seed = seed.wrapping_add(0x6d2b79f5);
    let mut t = (*seed ^ (*seed >> 15)).wrapping_mul(1 | *seed);
    t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
    (t ^ (t >> 14)) as f64 / 4294967296.
}
