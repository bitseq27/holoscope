//! Cloud-Optimized GeoTIFF DEM reader — ranged GETs only (plan §5.1).
//!
//! Supports the Copernicus DEM GLO-30 layout: single image, 512×512 tiles,
//! DEFLATE compression, float32 samples, ModelPixelScale + ModelTiepoint
//! georeferencing. The parser is pure (header bytes in → meta out) so it is
//! unit-testable against synthetic COGs with no network.

use anyhow::{anyhow, bail, Result};
use flate2::read::ZlibDecoder;
use std::io::Read;

pub const TILE: i64 = 512;

#[derive(Clone, Debug)]
pub struct CogMeta {
    pub width: i64,
    pub height: i64,
    pub tile_w: i64,
    pub tile_h: i64,
    pub compression: u16,
    pub predictor: u16,
    pub bits: u16,
    pub sample_format: u16,
    /// degrees per pixel (x, y>0)
    pub pixel_scale: (f64, f64),
    /// lon/lat of pixel (0,0) center-top corner (tiepoint)
    pub tiepoint_lon: f64,
    pub tiepoint_lat: f64,
    pub tile_offsets: Vec<u64>,
    pub tile_counts: Vec<u32>,
    pub big_endian: bool,
    pub url: String,
}

impl CogMeta {
    pub fn tiles_x(&self) -> i64 {
        (self.width + self.tile_w - 1) / self.tile_w
    }
    pub fn tiles_y(&self) -> i64 {
        (self.height + self.tile_h - 1) / self.tile_h
    }
    /// lon/lat of pixel center (col, row)
    pub fn pixel_center(&self, col: f64, row: f64) -> (f64, f64) {
        (
            self.tiepoint_lon + (col + 0.5) * self.pixel_scale.0,
            self.tiepoint_lat - (row + 0.5) * self.pixel_scale.1,
        )
    }
    /// fractional pixel coords of a lon/lat
    pub fn lonlat_to_pixel(&self, lon: f64, lat: f64) -> (f64, f64) {
        (
            (lon - self.tiepoint_lon) / self.pixel_scale.0 - 0.5,
            (self.tiepoint_lat - lat) / self.pixel_scale.1 - 0.5,
        )
    }
}

struct Le<'a>(&'a [u8], bool);
impl Le<'_> {
    fn u16(&self, o: usize) -> u16 {
        let b = &self.0[o..o + 2];
        if self.1 {
            u16::from_be_bytes([b[0], b[1]])
        } else {
            u16::from_le_bytes([b[0], b[1]])
        }
    }
    fn u32(&self, o: usize) -> u32 {
        let b = &self.0[o..o + 4];
        if self.1 {
            u32::from_be_bytes([b[0], b[1], b[2], b[3]])
        } else {
            u32::from_le_bytes([b[0], b[1], b[2], b[3]])
        }
    }
    fn u64(&self, o: usize) -> u64 {
        let b = &self.0[o..o + 8];
        if self.1 {
            u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        } else {
            u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
        }
    }
    fn f64_at(&self, o: usize) -> f64 {
        f64::from_bits(self.u64(o))
    }
}

/// Parse a COG header/IFD from `head` bytes (must cover the IFD and, ideally,
/// the tile offset/count arrays). Pure function — no IO.
pub fn parse_header(head: &[u8], url: &str) -> Result<CogMeta> {
    if head.len() < 8 {
        bail!("header too small");
    }
    let be = match &head[0..2] {
        b"II" => false,
        b"MM" => true,
        _ => bail!("not a TIFF"),
    };
    let r = Le(head, be);
    if r.u16(2) != 42 {
        bail!("not a classic TIFF (bigtiff unsupported)");
    }
    fn tsize(typ: u16) -> usize {
        match typ {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 | 16 | 17 => 8,
            _ => 1,
        }
    }
    let ifd = r.u32(4) as usize;
    if ifd + 2 > head.len() {
        bail!("IFD offset {} beyond fetched head ({} bytes)", ifd, head.len());
    }
    let n = r.u16(ifd) as usize;
    let mut m = CogMeta {
        width: 0,
        height: 0,
        tile_w: TILE,
        tile_h: TILE,
        compression: 1,
        predictor: 1,
        bits: 32,
        sample_format: 3,
        pixel_scale: (0.0, 0.0),
        tiepoint_lon: 0.0,
        tiepoint_lat: 0.0,
        tile_offsets: Vec::new(),
        tile_counts: Vec::new(),
        big_endian: be,
        url: url.to_string(),
    };
    let mut offs_off = 0usize;
    let mut offs_count = 0usize;
    let mut cnts_off = 0usize;
    let mut cnts_count = 0usize;
    let mut scale_off = 0usize;
    let mut scale_count = 0usize;
    let mut tie_off = 0usize;
    let mut tie_count = 0usize;
    for i in 0..n {
        let e = ifd + 2 + i * 12;
        if e + 12 > head.len() {
            bail!("IFD entry beyond head");
        }
        let tag = r.u16(e);
        let typ = r.u16(e + 2);
        let count = r.u32(e + 4) as usize;
        let vo = e + 8; // value-or-offset
        let val32 = r.u32(vo) as usize;
        let inline = count * tsize(typ) <= 4;
        let arr_off = if inline { vo } else { val32 };
        match tag {
            256 => m.width = val32 as i64,
            257 => m.height = val32 as i64,
            258 => m.bits = val32 as u16,
            259 => m.compression = val32 as u16,
            317 => m.predictor = val32 as u16,
            322 => m.tile_w = val32 as i64,
            323 => m.tile_h = val32 as i64,
            339 => m.sample_format = val32 as u16,
            324 => {
                offs_off = arr_off;
                offs_count = count;
            }
            325 => {
                cnts_off = arr_off;
                cnts_count = count;
            }
            33550 => {
                scale_off = arr_off;
                scale_count = count;
            }
            33922 => {
                tie_off = arr_off;
                tie_count = count;
            }
            _ => {}
        }
        let _ = typ;
    }
    if m.width == 0 || m.height == 0 {
        bail!("missing image dimensions");
    }
    // helper to read an external array of u32/u64/double
    let need = |off: usize, len: usize| -> Result<()> {
        if off + len > head.len() {
            bail!("array at {off}+{len} beyond head ({} bytes)", head.len());
        }
        Ok(())
    };
    // tile offsets (u64 for COGs > 4 GB, u32 otherwise — Copernicus uses u64)
    {
        let bytes_per = 8; // assume BIGTIFF-style u64 offsets? classic TIFF: type 16/17
        // re-read type properly:
        let mut typ = 16u16;
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            if r.u16(e) == 324 {
                typ = r.u16(e + 2);
            }
        }
        let bpe = if typ == 16 || typ == 17 { 8 } else { 4 };
        need(offs_off, offs_count * bpe)?;
        for i in 0..offs_count {
            let o = offs_off + i * bpe;
            m.tile_offsets.push(if bpe == 8 { r.u64(o) } else { r.u32(o) as u64 });
        }
        let _ = bytes_per;
    }
    {
        let mut typ = 4u16;
        for i in 0..n {
            let e = ifd + 2 + i * 12;
            if r.u16(e) == 325 {
                typ = r.u16(e + 2);
            }
        }
        let bpe = if typ == 16 || typ == 17 { 8 } else { 4 };
        need(cnts_off, cnts_count * bpe)?;
        for i in 0..cnts_count {
            let o = cnts_off + i * bpe;
            m.tile_counts.push(if bpe == 8 { r.u64(o) as u32 } else { r.u32(o) });
        }
    }
    if scale_count >= 2 {
        need(scale_off, scale_count * 8)?;
        m.pixel_scale = (r.f64_at(scale_off), r.f64_at(scale_off + 8));
    }
    if tie_count >= 6 {
        need(tie_off, tie_count * 8)?;
        m.tiepoint_lon = r.f64_at(tie_off + 24);
        m.tiepoint_lat = r.f64_at(tie_off + 32);
    }
    if m.pixel_scale.0 == 0.0 {
        bail!("missing ModelPixelScale");
    }
    Ok(m)
}

/// Undo TIFF floating-point predictor (tag 317 = 3, TIFF Tech Note 3):
/// per-row byte-wise horizontal accumulation, then de-plane the four byte
/// planes back into interleaved samples (which end up MSB-first).
fn undo_fp_predictor(buf: &mut [u8], cols: usize, rows: usize) {
    let rowbytes = cols * 4;
    for r in 0..rows {
        let o = r * rowbytes;
        let end = (o + rowbytes).min(buf.len());
        for i in (o + 1)..end {
            buf[i] = buf[i].wrapping_add(buf[i - 1]);
        }
        let tmp = buf[o..end].to_vec();
        let n = (end - o) / 4;
        for b in 0..4usize {
            for x in 0..n {
                let src = b * n + x;
                if src < tmp.len() {
                    buf[o + x * 4 + b] = tmp[src];
                }
            }
        }
    }
}

/// Decompress one tile (DEFLATE/zlib or raw) into f32 samples.
pub fn decode_tile(raw: &[u8], meta: &CogMeta) -> Result<Vec<f32>> {
    let mut buf: Vec<u8> = match meta.compression {
        8 | 32946 => {
            let mut d = ZlibDecoder::new(raw);
            let mut out = Vec::new();
            d.read_to_end(&mut out)?;
            out
        }
        1 => raw.to_vec(),
        c => bail!("unsupported compression {c}"),
    };
    if meta.bits != 32 || meta.sample_format != 3 {
        bail!("only float32 samples supported (got bits={} fmt={})", meta.bits, meta.sample_format);
    }
    if meta.predictor == 3 {
        undo_fp_predictor(&mut buf, meta.tile_w as usize, meta.tile_h as usize);
    } else if meta.predictor != 1 {
        bail!("unsupported predictor {}", meta.predictor);
    }
    let n = buf.len() / 4;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let b = [buf[i * 4], buf[i * 4 + 1], buf[i * 4 + 2], buf[i * 4 + 3]];
        // predictor-3 samples come out MSB-first; plain tiles follow file endianness
        let v = if meta.predictor == 3 {
            f32::from_be_bytes(b)
        } else if meta.big_endian {
            f32::from_be_bytes(b)
        } else {
            f32::from_le_bytes(b)
        };
        // sanity: real elevations are within ±10 km; anything else is nodata/corrupt
        out.push(if v.is_finite() && v.abs() < 10_000.0 { v } else { 0.0 });
    }
    Ok(out)
}

/// Copernicus DEM GLO-30 object URL for the 1° tile containing (lat, lon).
pub fn copernicus_url(lat_f: i64, lon_f: i64) -> String {
    let ns = if lat_f >= 0 { "N" } else { "S" };
    let ew = if lon_f >= 0 { "E" } else { "W" };
    let base = format!(
        "Copernicus_DSM_COG_10_{ns}{:02}_00_{ew}{:03}_00_DEM",
        lat_f.abs(),
        lon_f.abs()
    );
    format!("https://copernicus-dem-30m.s3.amazonaws.com/{base}/{base}.tif")
}

/// Geodetic window → regular lat/lon grid of elevations, fetched tile-by-tile
/// through `fetch_tile(meta, tile_index) -> raw bytes` (cache-aware in the app).
pub fn read_window<F>(meta: &CogMeta, lon0: f64, lat0: f64, lon1: f64, lat1: f64, fetch_tile: F) -> Result<super::GeoGrid>
where
    F: Fn(&CogMeta, usize) -> Result<Vec<u8>>,
{
    let (c0, r1) = meta.lonlat_to_pixel(lon0, lat0.min(lat1)); // top-left pixel
    let (c1, r0) = meta.lonlat_to_pixel(lon1, lat0.max(lat1));
    let cx0 = c0.floor().max(0.0) as i64;
    let cx1 = (c1.ceil() - 1.0).min(meta.width as f64 - 1.0) as i64;
    let ry0 = r0.floor().max(0.0) as i64;
    let ry1 = (r1.ceil() - 1.0).min(meta.height as f64 - 1.0) as i64;
    if cx1 < cx0 || ry1 < ry0 {
        bail!("empty window");
    }
    let nx = (cx1 - cx0 + 1) as usize;
    let ny = (ry1 - ry0 + 1) as usize;
    let mut zs = vec![0.0f32; nx * ny];
    let tw = meta.tile_w;
    let th = meta.tile_h;
    for ty in (ry0 / th)..=(ry1 / th) {
        for tx in (cx0 / tw)..=(cx1 / tw) {
            let idx = (ty * meta.tiles_x() + tx) as usize;
            if idx >= meta.tile_offsets.len() {
                return Err(anyhow!("tile idx {idx} out of range"));
            }
            let samples = decode_tile(&fetch_tile(meta, idx)?, meta)?;
            let py0 = ty * th;
            let px0 = tx * tw;
            for ry in ry0.max(py0)..=ry1.min(py0 + th - 1) {
                for cx in cx0.max(px0)..=cx1.min(px0 + tw - 1) {
                    let sx = (cx - px0) as usize;
                    let sy = (ry - py0) as usize;
                    let v = samples[sy * tw as usize + sx];
                    zs[(ry - ry0) as usize * nx + (cx - cx0) as usize] = v;
                }
            }
        }
    }
    let (wlon, wlat) = meta.pixel_center(cx0 as f64, ry0 as f64);
    Ok(super::GeoGrid {
        nx,
        ny,
        lon0: wlon,
        lat0: wlat,
        dlon: meta.pixel_scale.0,
        dlat: -meta.pixel_scale.1,
        zs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal synthetic COG: 3×3 tiles of 2×2 float32, uncompressed, LE.
    fn synth_cog() -> (Vec<u8>, Vec<u64>, Vec<u32>) {
        let tw = 2i64;
        let th = 2i64;
        let w = 6i64;
        let h = 6i64;
        let nt = ((w + tw - 1) / tw) * ((h + th - 1) / th);
        let header_len = 8u32;
        let n_entries = 12u16;
        let ifd_off = 8u32;
        let ifd_len = 2 + n_entries as u32 * 12 + 4;
        let mut data_off = ifd_off + ifd_len;
        // external arrays: tile offsets (u32), counts (u32), scale (3×f64), tie (6×f64)
        let offs_off = data_off;
        data_off += nt as u32 * 4;
        let cnts_off = data_off;
        data_off += nt as u32 * 4;
        let scale_off = data_off;
        data_off += 3 * 8;
        let tie_off = data_off;
        data_off += 6 * 8;
        // tile pixel data
        let mut offsets = Vec::new();
        let mut counts = Vec::new();
        let mut tiles = Vec::new();
        for t in 0..nt as usize {
            let mut px = Vec::new();
            for i in 0..(tw * th) as usize {
                px.push(t as f32 * 100.0 + i as f32);
            }
            let mut raw = Vec::new();
            for v in px {
                raw.extend_from_slice(&v.to_le_bytes());
            }
            offsets.push(data_off as u64);
            counts.push(raw.len() as u32);
            data_off += raw.len() as u32;
            tiles.push(raw);
        }
        let mut b = Vec::new();
        b.extend_from_slice(b"II");
        b.extend_from_slice(&42u16.to_le_bytes());
        b.extend_from_slice(&ifd_off.to_le_bytes());
        // IFD
        b.extend_from_slice(&n_entries.to_le_bytes());
        let mut entry = |tag: u16, typ: u16, count: u32, value_or_off: u32| {
            b.extend_from_slice(&tag.to_le_bytes());
            b.extend_from_slice(&typ.to_le_bytes());
            b.extend_from_slice(&count.to_le_bytes());
            b.extend_from_slice(&value_or_off.to_le_bytes());
        };
        entry(256, 4, 1, w as u32);
        entry(257, 4, 1, h as u32);
        entry(258, 3, 1, 32);
        entry(259, 3, 1, 1); // uncompressed
        entry(277, 3, 1, 1);
        entry(322, 4, 1, tw as u32);
        entry(323, 4, 1, th as u32);
        entry(324, 4, nt as u32, offs_off);
        entry(325, 4, nt as u32, cnts_off);
        entry(339, 3, 1, 3); // float
        entry(33550, 12, 3, scale_off);
        entry(33922, 12, 6, tie_off);
        b.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        // arrays
        for o in &offsets {
            b.extend_from_slice(&(*o as u32).to_le_bytes());
        }
        for c in &counts {
            b.extend_from_slice(&c.to_le_bytes());
        }
        let s = 0.001f64;
        b.extend_from_slice(&s.to_bits().to_le_bytes());
        b.extend_from_slice(&s.to_bits().to_le_bytes());
        b.extend_from_slice(&0.0f64.to_bits().to_le_bytes());
        let tie = [0.0f64, 0.0, 0.0, 135.0, 35.0, 0.0];
        for t in tie {
            b.extend_from_slice(&t.to_bits().to_le_bytes());
        }
        for t in &tiles {
            b.extend_from_slice(t);
        }
        let _ = header_len;
        (b, offsets, counts)
    }

    #[test]
    fn parse_and_read_window() {
        let (blob, _offs, _cnts) = synth_cog();
        let meta = parse_header(&blob, "mem://synth").unwrap();
        assert_eq!(meta.width, 6);
        assert_eq!(meta.tile_w, 2);
        assert_eq!(meta.tile_offsets.len(), 9);
        assert!((meta.pixel_scale.0 - 0.001).abs() < 1e-12);
        assert!((meta.tiepoint_lon - 135.0).abs() < 1e-9);
        let grid = read_window(&meta, 135.0, 35.0, 135.006, 34.994, |m, idx| {
            let o = m.tile_offsets[idx] as usize;
            let c = m.tile_counts[idx] as usize;
            Ok(blob[o..o + c].to_vec())
        })
        .unwrap();
        assert_eq!(grid.nx, 6);
        assert_eq!(grid.ny, 6);
        // tile 0 (top-left) values 0,1,2,3
        assert_eq!(grid.zs[0], 0.0);
        assert_eq!(grid.zs[1], 1.0);
        // tile 4 (center) base 400
        assert_eq!(grid.zs[2 * 6 + 2], 400.0);
    }
}
