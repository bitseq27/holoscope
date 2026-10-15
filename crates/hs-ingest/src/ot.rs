//! OpenTopography auto-LiDAR (free, keyless): catalog → tile index → LAZ tiles.
//!
//! Chain (verified live 2026-10-12, BUILD_LOG session 14):
//!   1. GET /API/otCatalog?minx&miny&maxx&maxy&productFormat=pointcloud&detail=true
//!   2. pick first dataset whose geojson footprint intersects the request box
//!   3. GET https://opentopography.s3.sdsc.edu/pc-bulk/{alt}/{alt}_TileIndex.zip
//!      (ZIP with .shp tile bounds + .dbf tile URLs)
//!   4. download intersecting LAZ tiles (cached), decompress with `laz`,
//!      clip to the request box, feed the normal LiDAR node path.
//! The old `apibeam` clipping endpoint is dead (404, probed) — tiles are the
//! documented path now (OT_Tile_Index_Search notebook).

use anyhow::{anyhow, Result};
use serde_json::Value;

use crate::lidar::LasPoint;

const CATALOG: &str = "https://portal.opentopography.org/API/otCatalog";
const BULK: &str = "https://opentopography.s3.sdsc.edu/pc-bulk";

pub struct OtDataset {
    pub alt_name: String,
    pub ot_id: String,
    pub bbox: [f64; 4], // minx miny maxx maxy (degrees)
    pub epsg: Option<u32>,
}

/// WGS84 → UTM (Snyder transverse Mercator, metre-grade — fine for tile pick).
pub fn wgs84_to_utm(lat: f64, lon: f64, zone: u32, south: bool) -> (f64, f64) {
    let a = 6378137.0f64;
    let f = 1.0 / 298.257223563;
    let e2 = f * (2.0 - f);
    let ep2 = e2 / (1.0 - e2);
    let k0 = 0.9996;
    let lat_r = lat.to_radians();
    let lon_r = lon.to_radians();
    let lon0 = ((zone as f64 - 1.0) * 6.0 - 180.0 + 3.0).to_radians();
    let n = a / (1.0 - e2 * lat_r.sin().powi(2)).sqrt();
    let t = lat_r.tan().powi(2);
    let c = ep2 * lat_r.cos().powi(2);
    let aa = (lon_r - lon0) * lat_r.cos();
    let m = a
        * ((1.0 - e2 / 4.0 - 3.0 * e2 * e2 / 64.0) * lat_r
            - (3.0 * e2 / 8.0 + 3.0 * e2 * e2 / 32.0) * (2.0 * lat_r).sin()
            + (15.0 * e2 * e2 / 256.0) * (4.0 * lat_r).sin());
    let x = k0 * n * (aa + (1.0 - t + c) * aa.powi(3) / 6.0) + 500_000.0;
    let mut y = k0
        * (m
            + n * lat_r.tan()
                * (aa * aa / 2.0 + (5.0 - t + 9.0 * c) * aa.powi(4) / 24.0));
    if south {
        y += 10_000_000.0;
    }
    (x, y)
}

/// Degrees bbox → UTM metres bbox for a zone/hemisphere.
pub fn utm_box(bbox: [f64; 4], zone: u32, south: bool) -> [f64; 4] {
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for lon in [bbox[0], bbox[2], (bbox[0] + bbox[2]) * 0.5] {
        for lat in [bbox[1], bbox[3], (bbox[1] + bbox[3]) * 0.5] {
            let (x, y) = wgs84_to_utm(lat, lon, zone, south);
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    [minx, miny, maxx, maxy]
}

/// Request bbox (degrees) → dataset CRS metres, when that CRS is UTM.
pub fn bbox_to_dataset_crs(bbox: [f64; 4], epsg: u32) -> Option<[f64; 4]> {
    let (north, zone) = if (32601..=32660).contains(&epsg) {
        (true, epsg - 32600)
    } else if (32701..=32760).contains(&epsg) {
        (false, epsg - 32700)
    } else {
        return None; // state-plane & friends: auto-lidar skips (documented)
    };
    let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for lon in [bbox[0], bbox[2], (bbox[0] + bbox[2]) * 0.5] {
        for lat in [bbox[1], bbox[3], (bbox[1] + bbox[3]) * 0.5] {
            let (x, y) = wgs84_to_utm(lat, lon, zone, !north);
            minx = minx.min(x);
            maxx = maxx.max(x);
            miny = miny.min(y);
            maxy = maxy.max(y);
        }
    }
    Some([minx, miny, maxx, maxy])
}

/// Query the catalog for point-cloud datasets intersecting a bbox.
pub fn catalog_pointclouds(client: &crate::http::Client, bbox: [f64; 4]) -> Result<Vec<OtDataset>> {
    let url = format!(
        "{CATALOG}?minx={}&miny={}&maxx={}&maxy={}&productFormat=pointcloud&detail=true",
        bbox[0], bbox[1], bbox[2], bbox[3]
    );
    let v = client.get_json(&url)?;
    let mut out = Vec::new();
    for d in v.get("Datasets").and_then(|d| d.as_array()).cloned().unwrap_or_default() {
        let x = d.get("Dataset").unwrap_or(&d);
        let alt = x.get("alternateName").and_then(|s| s.as_str()).unwrap_or("").to_string();
        let ot_id = x
            .pointer("/identifier/value")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        if alt.is_empty() {
            continue;
        }
        // footprint bbox from the geojson (Polygon or MultiPolygon)
        let geom = x.pointer("/spatialCoverage/geo/geojson/features/0/geometry");
        let coords = geom.and_then(|g| g.get("coordinates")).and_then(|c| c.as_array());
        let is_multi = geom
            .and_then(|g| g.get("type").and_then(|t| t.as_str()))
            .map(|t| t == "MultiPolygon")
            .unwrap_or(false);
        let (mut minx, mut miny, mut maxx, mut maxy) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        let mut eat_ring = |ring: &Value, s: &mut [f64; 4]| {
            for p in ring.as_array().into_iter().flatten() {
                if let (Some(px), Some(py)) =
                    (p.get(0).and_then(|n| n.as_f64()), p.get(1).and_then(|n| n.as_f64()))
                {
                    s[0] = s[0].min(px);
                    s[1] = s[1].min(py);
                    s[2] = s[2].max(px);
                    s[3] = s[3].max(py);
                }
            }
        };
        let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        if let Some(coords) = coords {
            if is_multi {
                for poly in coords {
                    for ring in poly.as_array().into_iter().flatten() {
                        eat_ring(ring, &mut bounds);
                    }
                }
            } else {
                for ring in coords {
                    eat_ring(ring, &mut bounds);
                }
            }
        }
        minx = bounds[0];
        miny = bounds[1];
        maxx = bounds[2];
        maxy = bounds[3];
        let epsg = x
            .pointer("/spatialCoverage/additionalProperty")
            .and_then(|v| v.as_array())
            .and_then(|arr| {
                arr.iter().find(|p| {
                    p.get("name").and_then(|n| n.as_str()) == Some("EPSG (Horizontal)")
                })
            })
            .and_then(|p| p.get("value"))
            .and_then(|v| v.as_str().map(str::to_string).or_else(|| v.as_u64().map(|n| n.to_string())))
            .and_then(|t| t.trim().parse::<u32>().ok());
        if minx > maxx {
            continue;
        }
        if maxx < bbox[0] || minx > bbox[2] || maxy < bbox[1] || miny > bbox[3] {
            continue;
        }
        out.push(OtDataset {
            alt_name: alt,
            ot_id,
            bbox: [minx, miny, maxx, maxy],
            epsg,
        });
    }
    Ok(out)
}

pub fn tile_index_url(alt: &str) -> String {
    format!("{BULK}/{alt}/{alt}_TileIndex.zip")
}

/// Parse the TileIndex zip → per-tile (bbox, url).
pub fn parse_tile_index(zip_bytes: &[u8]) -> Result<Vec<([f64; 4], String)>> {
    let cursor = std::io::Cursor::new(zip_bytes);
    let mut zip = zip::ZipArchive::new(cursor).map_err(|e| anyhow!("tile index zip: {e}"))?;
    let mut shp: Option<Vec<u8>> = None;
    let mut dbf: Option<Vec<u8>> = None;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| anyhow!("zip entry: {e}"))?;
        let name = f.name().to_lowercase();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut f, &mut buf)?;
        if name.ends_with(".shp") {
            shp = Some(buf);
        } else if name.ends_with(".dbf") {
            dbf = Some(buf);
        }
    }
    let (shp, dbf) = match (shp, dbf) {
        (Some(a), Some(b)) => (a, b),
        _ => return Err(anyhow!("tile index zip lacks .shp/.dbf")),
    };
    let boxes = shp_record_boxes(&shp)?;
    let urls = dbf_column_strings(&dbf, "URL")?;
    let n = boxes.len().min(urls.len());
    Ok((0..n).map(|i| (boxes[i], urls[i].clone())).collect())
}

/// Shapefile (polygon type 5) per-record bounding boxes, in record order.
fn shp_record_boxes(shp: &[u8]) -> Result<Vec<[f64; 4]>> {
    if shp.len() < 100 {
        return Err(anyhow!("shp too small"));
    }
    let mut out = Vec::new();
    let mut off = 100usize;
    while off + 8 <= shp.len() {
        let content_len = (u32::from_be_bytes([shp[off + 4], shp[off + 5], shp[off + 6], shp[off + 7]]) as usize) * 2;
        let rec = &shp[off + 8..];
        if rec.len() < 4 + 32 {
            break;
        }
        let shape_type = i32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
        if shape_type == 5 || shape_type == 15 || shape_type == 25 {
            let g = |k: usize| f64::from_le_bytes(rec[4 + k * 8..12 + k * 8].try_into().unwrap());
            out.push([g(0), g(1), g(2), g(3)]);
        }
        off += 8 + content_len;
    }
    Ok(out)
}

/// Minimal dBASE III reader: one character column by name.
fn dbf_column_strings(dbf: &[u8], want: &str) -> Result<Vec<String>> {
    if dbf.len() < 32 {
        return Err(anyhow!("dbf too small"));
    }
    let n_records = u32::from_le_bytes([dbf[4], dbf[5], dbf[6], dbf[7]]) as usize;
    let header_size = u16::from_le_bytes([dbf[8], dbf[9]]) as usize;
    let record_size = u16::from_le_bytes([dbf[10], dbf[11]]) as usize;
    struct Field {
        name: String,
        len: usize,
        off: usize,
    }
    let mut fields = Vec::new();
    let mut p = 32usize;
    let mut off_in_rec = 1usize; // deletion flag
    while p + 32 <= header_size && dbf[p] != 0x0D {
        let name = String::from_utf8_lossy(&dbf[p..p + 11]).trim_end_matches('\0').to_string();
        let len = dbf[p + 16] as usize;
        fields.push(Field { name, len, off: off_in_rec });
        off_in_rec += len;
        p += 32;
    }
    let f = fields
        .iter()
        .find(|f| f.name.eq_ignore_ascii_case(want))
        .ok_or_else(|| anyhow!("dbf has no {want} column"))?;
    let mut out = Vec::with_capacity(n_records);
    let mut base = header_size;
    for _ in 0..n_records {
        if base + record_size > dbf.len() {
            break;
        }
        let s = String::from_utf8_lossy(&dbf[base + f.off..base + f.off + f.len])
            .trim()
            .to_string();
        out.push(s);
        base += record_size;
    }
    Ok(out)
}

/// Decompress a LAZ tile and decode points inside `clip` (minx,miny,maxx,maxy).
pub fn decode_laz(bytes: &[u8], clip: [f64; 4], cap: u64) -> Result<Vec<LasPoint>> {
    use std::io::{Cursor, Read, Seek, SeekFrom};
    if bytes.len() < 227 || &bytes[0..4] != b"LASF" {
        return Err(anyhow!("not a LAS/LAZ file"));
    }
    let header_size = u16::from_le_bytes([bytes[94], bytes[95]]) as usize;
    let point_offset = u32::from_le_bytes([bytes[96], bytes[97], bytes[98], bytes[99]]) as usize;
    let n_vlr = u32::from_le_bytes([bytes[100], bytes[101], bytes[102], bytes[103]]) as usize;
    let format = bytes[104] & 0x0F;
    let record_len = u16::from_le_bytes([bytes[105], bytes[106]]) as usize;
    let mut count = u32::from_le_bytes([bytes[107], bytes[108], bytes[109], bytes[110]]) as u64;
    let ver_major = bytes[24];
    let ver_minor = bytes[25];
    if (ver_major, ver_minor) >= (1, 4) && bytes.len() > 255 {
        let c64 = u64::from_le_bytes(bytes[247..255].try_into().unwrap());
        if c64 > 0 {
            count = c64;
        }
    }
    let scale = [
        f64::from_le_bytes(bytes[131..139].try_into().unwrap()),
        f64::from_le_bytes(bytes[139..147].try_into().unwrap()),
        f64::from_le_bytes(bytes[147..155].try_into().unwrap()),
    ];
    let offset = [
        f64::from_le_bytes(bytes[155..163].try_into().unwrap()),
        f64::from_le_bytes(bytes[163..171].try_into().unwrap()),
        f64::from_le_bytes(bytes[171..179].try_into().unwrap()),
    ];
    // locate the laszip VLR
    let mut cur = Cursor::new(bytes);
    // VLR record = reserved u16 + user_id[16] + record_id u16 + length u16
    //              + description[32] = 54-byte header, then payload
    let mut pos = header_size;
    let mut vlr = None;
    for _ in 0..n_vlr {
        if pos + 54 > bytes.len() {
            break;
        }
        let uid = &bytes[pos + 2..pos + 18];
        let rec_id = u16::from_le_bytes([bytes[pos + 18], bytes[pos + 19]]);
        let rec_len = u16::from_le_bytes([bytes[pos + 20], bytes[pos + 21]]) as usize;
        let name = String::from_utf8_lossy(uid).trim_end_matches('\0').to_string();
        if name.contains("laszip") || rec_id == 22204 {
            cur.seek(SeekFrom::Start((pos + 54) as u64))?;
            vlr = Some(laz::LazVlr::read_from(&mut cur)?);
            break;
        }
        pos += 54 + rec_len;
    }
    if count > cap {
        return Err(anyhow!("tile has {count} points (> cap {cap}); skipping"));
    }
    let vlr = vlr.ok_or_else(|| anyhow!("no laszip VLR (plain LAS? use parse_las)"))?;
    let items_size = vlr.items_size() as usize;
    cur.seek(SeekFrom::Start(point_offset as u64))?;
    let mut dec = laz::LasZipDecompressor::new(&mut cur, vlr)?;
    let class_off: usize = match format {
        0..=3 => 15,
        6..=8 => 16,
        f => return Err(anyhow!("unsupported format {f}")),
    };
    let mut rec = vec![0u8; record_len.max(items_size)];
    let mut out = Vec::new();
    for _ in 0..count {
        dec.decompress_one(&mut rec)?;
        let xi = i32::from_le_bytes([rec[0], rec[1], rec[2], rec[3]]);
        let yi = i32::from_le_bytes([rec[4], rec[5], rec[6], rec[7]]);
        let zi = i32::from_le_bytes([rec[8], rec[9], rec[10], rec[11]]);
        let x = xi as f64 * scale[0] + offset[0];
        let y = yi as f64 * scale[1] + offset[1];
        let z = zi as f64 * scale[2] + offset[2];
        if x < clip[0] || x > clip[2] || y < clip[1] || y > clip[3] {
            continue;
        }
        out.push(LasPoint {
            x,
            y,
            z,
            intensity: u16::from_le_bytes([rec[12], rec[13]]),
            classification: rec[class_off] & 0x1F,
        });
    }
    Ok(out)
}

/// One-stop auto-LiDAR: catalog → index → tiles → points (clipped).
pub fn fetch_lidar(
    client: &crate::http::Client,
    cache: &hs_cache::Cache,
    bbox: [f64; 4],
    max_tiles: usize,
    progress: &dyn Fn(&str, f32),
) -> Result<(Vec<LasPoint>, String)> {
    progress("lidar catalog", 0.0);
    let datasets = catalog_pointclouds(client, bbox)?;
    let mut chosen = None;
    let mut crs_box = None;
    for ds in &datasets {
        if chosen.is_none() {
            chosen = Some(ds); // remember first intersecting dataset
        }
        if let Some(cb) = ds.epsg.and_then(|e| bbox_to_dataset_crs(bbox, e)) {
            chosen = Some(ds);
            crs_box = Some(cb);
            break;
        }
    }
    let ds = chosen.ok_or_else(|| anyhow!("no open LiDAR coverage for this extent"))?;
    progress(&format!("lidar index {}", ds.alt_name), 0.1);
    let idx_url = tile_index_url(&ds.alt_name);
    let zip_bytes = cache.blob_fetch("ot-tileindex", &idx_url, || client.get_range(&idx_url, None))?;
    let tiles = parse_tile_index(&zip_bytes)?;
    // CRS: EPSG property first; older datasets only reveal UTM via the
    // MGRS-style tile names (e.g. 11SGC1357.laz → zone 11, band S = north)
    let crs_box = crs_box.or_else(|| {
        let (_, url) = tiles.first()?;
        let base = url.rsplit('/').next()?;
        let zone = base.get(0..2)?.parse::<u32>().ok()?;
        let band = base.as_bytes().get(2)?;
        if !(b'A'..=b'X').contains(band) || !(1..=60).contains(&zone) {
            return None;
        }
        let south = *band < b'N';
        Some(utm_box(bbox, zone, south))
    });
    let crs_box = crs_box.ok_or_else(|| {
        anyhow!("dataset CRS not UTM-derivable (state-plane etc.) — auto-LiDAR skips; use --las")
    })?;
    let hit: Vec<&([f64; 4], String)> = tiles
        .iter()
        .filter(|(b, _)| {
            b[2] >= crs_box[0] && b[0] <= crs_box[2] && b[3] >= crs_box[1] && b[1] <= crs_box[3]
        })
        .take(max_tiles)
        .collect();
    if hit.is_empty() {
        return Err(anyhow!("coverage exists but no tiles intersect the extent"));
    }
    let mut pts = Vec::new();
    for (i, (_, url)) in hit.iter().enumerate() {
        progress(&format!("lidar tile {}/{}", i + 1, hit.len()), 0.2 + 0.7 * i as f32 / hit.len() as f32);
        let u = url.clone();
        let bytes = cache.blob_fetch("ot-laz", &u, || client.get_range(&u, None))?;
        let compressed = bytes.len() > 105 && bytes[104] & 0x80 != 0;
        let mut p = if !compressed {
            // plain LAS inside a .laz-named URL happens; clip in CRS metres
            let las = crate::lidar::parse_las(&bytes)?;
            las.points
                .into_iter()
                .filter(|p| {
                    p.x >= crs_box[0] && p.x <= crs_box[2] && p.y >= crs_box[1] && p.y <= crs_box[3]
                })
                .collect()
        } else {
            match decode_laz(&bytes, crs_box, 25_000_000) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("[ot] tile skipped: {e}");
                    continue;
                }
            }
        };
        pts.append(&mut p);
    }
    progress("lidar done", 0.95);
    if pts.is_empty() {
        return Err(anyhow!("tiles contained no points in the extent"));
    }
    Ok((pts, ds.alt_name.clone()))
}

/// bbox of a parsed point set
pub fn points_bbox(pts: &[LasPoint]) -> [f64; 4] {
    let (mut a, mut b, mut c, mut d) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for p in pts {
        a = a.min(p.x);
        b = b.min(p.y);
        c = c.max(p.x);
        d = d.max(p.y);
    }
    [a, b, c, d]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// decode_laz must reject plain LAS with a clear message (routing lives
    /// in fetch_lidar via the compression bit).
    #[test]
    fn decode_laz_rejects_plain_las() {
        let pts: Vec<LasPoint> = (0..10)
            .map(|i| LasPoint {
                x: i as f64,
                y: 0.0,
                z: 1.0,
                intensity: 1,
                classification: 2,
            })
            .collect();
        let las = crate::lidar::write_las(&pts);
        let e = decode_laz(&las, [-1.0, -1.0, 100.0, 100.0], 1000).unwrap_err();
        assert!(e.to_string().contains("laszip"), "{e}");
    }

    /// UTM forward transform sanity: zone centre meridian → ~500 km easting.
    #[test]
    fn utm_forward_sane() {
        let (x, y) = wgs84_to_utm(0.0, -177.0, 1, false); // zone 1 central -177
        assert!((x - 500_000.0).abs() < 1.0, "easting {x}");
        assert!(y.abs() < 1.0, "equator northing {y}");
        let (x2, _) = wgs84_to_utm(33.125, -116.609, 11, false);
        assert!(x2 > 400_000.0 && x2 < 700_000.0, "zone 11 easting {x2}");
    }
}
