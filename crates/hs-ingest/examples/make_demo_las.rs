//! Synthetic urban LiDAR scan (LAS 1.2 fmt 1) for validating the import path:
//! ground, 40 buildings (flat + gable roofs), 60 trees, road corridor.
use hs_ingest::lidar::{write_las, LasPoint};

fn main() {
    let mut pts: Vec<LasPoint> = Vec::new();
    let mut push = |x: f64, y: f64, z: f64, c: u8, i: u16| {
        pts.push(LasPoint { x, y, z, intensity: i, classification: c });
    };
    // ground 1 m grid over 600×600 m
    let mut y = -300.0;
    while y <= 300.0 {
        let mut x = -300.0;
        while x <= 300.0 {
            push(x + 0.2, y + 0.2, 0.05, 2, 120);
            x += 1.0;
        }
        y += 1.0;
    }
    // buildings on a 5×4 block grid
    for by in 0..4 {
        for bx in 0..5 {
            let cx = -220.0 + bx as f64 * 110.0;
            let cy = -180.0 + by as f64 * 120.0;
            let w = 30.0 + ((bx * 7 + by * 3) % 4) as f64 * 6.0;
            let d = 24.0 + ((bx * 5 + by) % 3) as f64 * 8.0;
            let h = 9.0 + ((bx + by * 2) % 5) as f64 * 6.0;
            let gable = (bx + by) % 2 == 0;
            // walls 0.7 m
            let mut z = 0.0;
            while z < h {
                let mut t = 0.0;
                let per = 2.0 * (w + d);
                while t < per {
                    let (px, py) = if t < w {
                        (cx - w / 2.0 + t, cy - d / 2.0)
                    } else if t < w + d {
                        (cx + w / 2.0, cy - d / 2.0 + (t - w))
                    } else if t < 2.0 * w + d {
                        (cx + w / 2.0 - (t - w - d), cy + d / 2.0)
                    } else {
                        (cx - w / 2.0, cy + d / 2.0 - (t - 2.0 * w - d))
                    };
                    push(px, py, z + 1.0, 6, 180);
                    t += 0.7;
                }
                z += 3.0;
            }
            // roof 0.8 m grid
            let mut ry = -d / 2.0;
            while ry <= d / 2.0 {
                let mut rx = -w / 2.0;
                while rx <= w / 2.0 {
                    let rz = if gable {
                        h + (1.0 - (ry.abs() / (d / 2.0))) * 4.0
                    } else {
                        h
                    };
                    push(cx + rx, cy + ry, rz + 0.05, 6, 200);
                    rx += 0.8;
                }
                ry += 0.8;
            }
        }
    }
    // trees: ellipsoid crowns + trunks
    for t in 0..60 {
        let tx = -280.0 + ((t * 37) % 560) as f64;
        let ty = -280.0 + ((t * 53) % 560) as f64;
        let th = 8.0 + (t % 5) as f64 * 2.0;
        for k in 0..4 {
            push(tx, ty, k as f64 * 2.0, 5, 90); // trunk
        }
        let mut i = 0;
        while i < 900 {
            let a = (i as f64 * 0.61803) * std::f64::consts::TAU;
            let r = ((i as f64 * 0.371) % 1.0).sqrt() * 3.2;
            let zz = th + (((i * 7) % 100) as f64 / 100.0 - 0.5) * 3.0;
            push(tx + a.cos() * r, ty + a.sin() * r, zz, 5, 110);
            i += 1;
        }
    }
    let blob = write_las(&pts);
    let path = std::env::args().nth(1).unwrap_or_else(|| "renders/demo_scan.las".into());
    std::fs::write(&path, &blob).expect("write");
    println!("{} points -> {}", pts.len(), path);
}
