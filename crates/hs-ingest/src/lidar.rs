//! LAS 1.2–1.4 point-cloud reader (plan FR-12 / §5.1 measured-3D path).
//!
//! Supports point record formats 0–3 (LAS 1.2/1.3) and 6–8 (LAS 1.4), which
//! covers USGS 3DEP, EA LiDAR, AHN and OpenTopography deliveries. Coordinates
//! are decoded with the header scale/offset; classification and intensity are
//! kept for layer colouring. CRS handling is pragmatic (see `place_enu`):
//! geographic coords are used as-is, projected/local coords are centred on
//! their centroid (or a user-supplied lat/lon) — full reprojection would need
//! a proj dependency, which the plan rejects for v1.

use anyhow::{bail, Result};

#[derive(Clone, Debug, Default)]
pub struct LasPoint {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub intensity: u16,
    pub classification: u8,
}

#[derive(Clone, Debug, Default)]
pub struct LasData {
    pub points: Vec<LasPoint>,
    pub version: (u8, u8),
    pub format: u8,
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl LasData {
    pub fn centroid(&self) -> [f64; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }
    /// Heuristic: geographic (lon/lat) coordinates fit in [-180,180]×[-90,90].
    pub fn looks_geographic(&self) -> bool {
        self.max[0].abs() <= 180.0 && self.max[1].abs() <= 90.0
    }
}

fn rd16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn rd32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn rd64(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes([
        b[o], b[o + 1], b[o + 2], b[o + 3], b[o + 4], b[o + 5], b[o + 6], b[o + 7],
    ])
}
fn rdf64(b: &[u8], o: usize) -> f64 {
    f64::from_bits(rd64(b, o))
}

pub fn parse_las(bytes: &[u8]) -> Result<LasData> {
    if bytes.len() < 227 || &bytes[0..4] != b"LASF" {
        bail!("not a LAS file");
    }
    let version = (bytes[24], bytes[25]);
    let point_offset = rd32(bytes, 96) as usize;
    let format_raw = bytes[104];
    let format = format_raw & 0x0F; // bit 7 = compression (LAZ) — unsupported
    if format_raw & 0x80 != 0 {
        bail!("LAZ (compressed LAS) unsupported in v1 — decompress first (plan FR-12 lists LAZ for M6+)");
    }
    let record_len = rd16(bytes, 105) as usize;
    let mut count = rd32(bytes, 107) as u64;
    if version.1 >= 4 {
        let c64 = rd64(bytes, 247);
        if c64 > 0 {
            count = c64;
        }
    }
    let scale = [rdf64(bytes, 131), rdf64(bytes, 139), rdf64(bytes, 147)];
    let offset = [rdf64(bytes, 155), rdf64(bytes, 163), rdf64(bytes, 171)];
    let max = [rdf64(bytes, 179), rdf64(bytes, 195), rdf64(bytes, 211)];
    let min = [rdf64(bytes, 187), rdf64(bytes, 203), rdf64(bytes, 219)];
    if record_len == 0 || point_offset + record_len > bytes.len() {
        bail!("bad LAS geometry (offset {point_offset}, record {record_len}, len {})", bytes.len());
    }
    let class_off: usize = match format {
        0..=3 => 15,
        6..=8 => 16,
        f => bail!("unsupported point format {f}"),
    };
    let n_avail = (bytes.len() - point_offset) / record_len;
    let n = (count as usize).min(n_avail);
    let mut out = LasData {
        points: Vec::with_capacity(n),
        version,
        format,
        min,
        max,
    };
    for i in 0..n {
        let r = &bytes[point_offset + i * record_len..];
        let xi = i32::from_le_bytes([r[0], r[1], r[2], r[3]]);
        let yi = i32::from_le_bytes([r[4], r[5], r[6], r[7]]);
        let zi = i32::from_le_bytes([r[8], r[9], r[10], r[11]]);
        out.points.push(LasPoint {
            x: xi as f64 * scale[0] + offset[0],
            y: yi as f64 * scale[1] + offset[1],
            z: zi as f64 * scale[2] + offset[2],
            intensity: rd16(r, 12),
            classification: r[class_off] & 0x1F,
        });
    }
    Ok(out)
}

/// Minimal LAS 1.2 / format 1 writer (tests + the demo-scan example).
pub fn write_las(points: &[LasPoint]) -> Vec<u8> {
    let mut h = vec![0u8; 227];
    h[0..4].copy_from_slice(b"LASF");
    h[24] = 1;
    h[25] = 2;
    let rec_len = 28u16; // format 1
    let off = 227u32;
    h[96..100].copy_from_slice(&off.to_le_bytes());
    h[104] = 1;
    h[105..107].copy_from_slice(&rec_len.to_le_bytes());
    h[107..111].copy_from_slice(&(points.len() as u32).to_le_bytes());
    let scale = [0.001f64, 0.001, 0.001];
    let offset = [0.0f64, 0.0, 0.0];
    let (mut min, mut max) = ([f64::MAX; 3], [f64::MIN; 3]);
    for p in points {
        for k in 0..3 {
            let v = [p.x, p.y, p.z][k];
            min[k] = min[k].min(v);
            max[k] = max[k].max(v);
        }
    }
    if points.is_empty() {
        min = [0.0; 3];
        max = [0.0; 3];
    }
    for k in 0..3 {
        h[131 + k * 8..139 + k * 8].copy_from_slice(&scale[k].to_bits().to_le_bytes());
        h[155 + k * 8..163 + k * 8].copy_from_slice(&offset[k].to_bits().to_le_bytes());
        h[179 + k * 16..187 + k * 16].copy_from_slice(&max[k].to_bits().to_le_bytes());
        h[187 + k * 16..195 + k * 16].copy_from_slice(&min[k].to_bits().to_le_bytes());
    }
    let mut out = h;
    for p in points {
        let xi = (p.x / scale[0]).round() as i32;
        let yi = (p.y / scale[1]).round() as i32;
        let zi = (p.z / scale[2]).round() as i32;
        out.extend_from_slice(&xi.to_le_bytes());
        out.extend_from_slice(&yi.to_le_bytes());
        out.extend_from_slice(&zi.to_le_bytes());
        out.extend_from_slice(&p.intensity.to_le_bytes());
        out.push(0); // return info
        out.push(p.classification);
        out.push(0); // scan angle
        out.push(0); // user data
        out.extend_from_slice(&1u16.to_le_bytes()); // source id
        out.extend_from_slice(&0.0f64.to_bits().to_le_bytes()); // gps time
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn las_roundtrip() {
        let pts: Vec<LasPoint> = (0..500)
            .map(|i| LasPoint {
                x: i as f64 * 0.37,
                y: (i % 17) as f64 * 1.5,
                z: 10.0 + (i % 5) as f64 * 0.5,
                intensity: 100 + (i % 50) as u16,
                classification: [2, 6, 5][i % 3],
            })
            .collect();
        let blob = write_las(&pts);
        let back = parse_las(&blob).unwrap();
        assert_eq!(back.points.len(), 500);
        assert_eq!(back.version, (1, 2));
        assert_eq!(back.format, 1);
        for (a, b) in pts.iter().zip(back.points.iter()) {
            assert!((a.x - b.x).abs() < 1e-3);
            assert!((a.z - b.z).abs() < 1e-3);
            assert_eq!(a.classification, b.classification);
        }
        assert!(back.looks_geographic() == false);
    }
}
