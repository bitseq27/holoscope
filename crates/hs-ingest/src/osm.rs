//! OSM vector fetch via the Overpass API (plan §5.1/§6.2, M2 slice).
//!
//! One bounded bbox query per load, cached forever in hs-cache. `out geom`
//! gives way geometries inline, so no node-join pass is needed. Extents above
//! `MAX_OVERPASS_KM2` skip vectors (Geofabrik extract path is M2.5/backlog).

use anyhow::Result;
use serde::Deserialize;
use std::collections::BTreeMap;

pub const MAX_OVERPASS_KM2: f64 = 100.0;
const OVERPASS_URL: &str = "https://overpass-api.de/api/interpreter";

#[derive(Clone, Debug)]
pub struct OsmWay {
    pub id: i64,
    pub tags: BTreeMap<String, String>,
    /// (lat, lon) per vertex
    pub geom: Vec<(f64, f64)>,
}

impl OsmWay {
    pub fn tag(&self, k: &str) -> Option<&str> {
        self.tags.get(k).map(|s| s.as_str())
    }
    pub fn closed(&self) -> bool {
        self.geom.len() >= 4 && self.geom[0] == self.geom[self.geom.len() - 1]
    }
}

#[derive(Clone, Debug)]
pub struct OsmNode {
    pub lat: f64,
    pub lon: f64,
    pub tags: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub struct OsmData {
    pub ways: Vec<OsmWay>,
    pub nodes: Vec<OsmNode>,
}

/// Overpass QL, stage 1: transport + water + place labels (light, high value).
pub fn overpass_query(bbox: [f64; 4]) -> String {
    overpass_query_stage(bbox, 1)
}

/// Staged so an overloaded server degrades by class, not totally
/// (stage 2 = buildings/veg, the heavy half).
pub fn overpass_query_stage(bbox: [f64; 4], stage: u32) -> String {
    let [s, w, n, e] = bbox;
    let bb = format!("{s:.5},{w:.5},{n:.5},{e:.5}");
    if stage == 1 {
        format!(
            "[out:json][timeout:180];(\
             way[highway]({bb});\
             way[railway~\"^(rail|tram)$\"]({bb});\
             way[natural=water]({bb});\
             way[waterway]({bb});\
             node[place~\"^(city|town|village|suburb|quarter)$\"]({bb});\
             );out geom;"
        )
    } else {
        format!(
            "[out:json][timeout:180];(\
             way[building]({bb});\
             way[leisure~\"^(park|garden|nature_reserve)$\"]({bb});\
             way[natural~\"^(wood|scrub)$\"]({bb});\
             );out geom;"
        )
    }
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    elements: Vec<Elem>,
}
#[derive(Deserialize)]
struct Elem {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    id: i64,
    #[serde(default)]
    lat: f64,
    #[serde(default)]
    lon: f64,
    #[serde(default)]
    tags: BTreeMap<String, String>,
    #[serde(default)]
    geometry: Vec<Pt>,
}
#[derive(Deserialize)]
struct Pt {
    lat: f64,
    lon: f64,
}

pub fn parse_overpass(bytes: &[u8]) -> Result<OsmData> {
    let resp: Resp = serde_json::from_slice(bytes)?;
    let nodes = resp
        .elements
        .iter()
        .filter(|e| e.r#type == "node" && e.tags.get("name").is_some())
        .map(|e| OsmNode {
            lat: e.lat,
            lon: e.lon,
            tags: e.tags.clone(),
        })
        .collect();
    let ways = resp
        .elements
        .into_iter()
        .filter(|e| e.r#type == "way" && e.geometry.len() >= 2)
        .map(|e| OsmWay {
            id: e.id,
            tags: e.tags,
            geom: e.geometry.into_iter().map(|p| (p.lat, p.lon)).collect(),
        })
        .collect();
    Ok(OsmData { ways, nodes })
}

/// Cache-first Overpass fetch for a bbox [south, west, north, east].
/// Mirrors in failover order (same 1 req/s limiter applies per mirror hop).
const MIRRORS: &[&str] = &[
    OVERPASS_URL,
    "https://overpass.kumi.systems/api/interpreter",
    "https://overpass.private.coffee/api/interpreter",
    "https://overpass.openstreetmap.ru/api/interpreter",
    "https://maps.mail.ru/osm/tools/overpass/api/interpreter",
    "https://www.overpass-api.de/api/interpreter",
    "https://overpass.osm.ch/api/interpreter",
];

/// Whole vector fetch (all chunks, both stages, retries) gets this budget;
/// on expiry whatever landed is used (cache keeps it) instead of hanging.
const FETCH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(240);

/// Cache-first Overpass fetch for a bbox [south, west, north, east].
/// Extents wider than ~5 km are chunked into ≤5 km cells so no single query
/// exceeds Overpass limits (BUG-M6.5-4: Tokyo 10 km used to time out and the
/// whole vector layer degraded).
pub fn fetch_osm(
    cache: &hs_cache::Cache,
    client: &crate::http::Client,
    bbox: [f64; 4],
) -> Result<OsmData> {
    let span_lat = (bbox[2] - bbox[0]).max(1e-6);
    let span_lon = (bbox[3] - bbox[1]).max(1e-6);
    // ~3 km cells: small queries survive Overpass load far better than one
    // big one (BUG-M6.5-4); partial success is accepted (degrade, not fail)
    let n = (span_lat / 0.03).ceil() as usize;
    let n = n.clamp(1, 4);
    let m = (span_lon / 0.03).ceil() as usize;
    let m = m.clamp(1, 4);
    let mut out = OsmData::default();
    let mut failed = 0usize;
    let mut total = 0usize;
    let t0 = std::time::Instant::now();
    'chunks: for i in 0..n {
        for j in 0..m {
            if t0.elapsed() > FETCH_DEADLINE {
                eprintln!("[osm] deadline reached; using partial vectors (re-LOAD later to complete)");
                break 'chunks;
            }
            let s0 = bbox[0] + span_lat * i as f64 / n as f64;
            let n0 = bbox[0] + span_lat * (i + 1) as f64 / n as f64;
            let w0 = bbox[1] + span_lon * j as f64 / m as f64;
            let e0 = bbox[1] + span_lon * (j + 1) as f64 / m as f64;
            total += 1;
            match fetch_osm_chunk(cache, client, [s0, w0, n0, e0], t0) {
                Ok(part) => {
                    out.ways.extend(part.ways);
                    out.nodes.extend(part.nodes);
                }
                Err(e) => {
                    failed += 1;
                    eprintln!("[osm] chunk {i},{j} failed: {e}");
                }
            }
        }
    }
    if out.ways.is_empty() && failed > 0 {
        return Err(anyhow::anyhow!("all {total} Overpass chunks failed"));
    }
    if failed > 0 {
        eprintln!("[osm] {failed}/{total} chunks failed — partial vectors loaded");
    }
    Ok(out)
}

/// Fetch one chunk; on failure split into quadrants and retry (min ~1.5 km),
/// because small queries survive overloaded Overpass instances far better.
fn fetch_osm_chunk(
    cache: &hs_cache::Cache,
    client: &crate::http::Client,
    bbox: [f64; 4],
    t0: std::time::Instant,
) -> Result<OsmData> {
    fetch_osm_chunk_depth(cache, client, bbox, 0, t0)
}

fn fetch_osm_chunk_depth(
    cache: &hs_cache::Cache,
    client: &crate::http::Client,
    bbox: [f64; 4],
    depth: u32,
    t0: std::time::Instant,
) -> Result<OsmData> {
    if t0.elapsed() > FETCH_DEADLINE {
        return Err(anyhow::anyhow!("deadline"));
    }
    match fetch_osm_chunk_staged(cache, client, bbox, t0) {
        Ok(d) => Ok(d),
        Err(e) if depth < 2 && (bbox[2] - bbox[0]) > 0.015 => {
            eprintln!("[osm] chunk failed ({e}); splitting 4-way");
            let mlat = (bbox[0] + bbox[2]) * 0.5;
            let mlon = (bbox[1] + bbox[3]) * 0.5;
            let mut out = OsmData::default();
            let mut fails = 0;
            for box4 in [
                [bbox[0], bbox[1], mlat, mlon],
                [bbox[0], mlon, mlat, bbox[3]],
                [mlat, bbox[1], bbox[2], mlon],
                [mlat, mlon, bbox[2], bbox[3]],
            ] {
                match fetch_osm_chunk_depth(cache, client, box4, depth + 1, t0) {
                    Ok(p) => {
                        out.ways.extend(p.ways);
                        out.nodes.extend(p.nodes);
                    }
                    Err(_) => fails += 1,
                }
            }
            if out.ways.is_empty() && fails == 4 {
                Err(e)
            } else {
                Ok(out)
            }
        }
        Err(e) => Err(e),
    }
}

/// Both stages for one box; stage 2 (buildings/veg) may degrade.
fn fetch_osm_chunk_staged(
    cache: &hs_cache::Cache,
    client: &crate::http::Client,
    bbox: [f64; 4],
    t0: std::time::Instant,
) -> Result<OsmData> {
    let mut out = fetch_stage(cache, client, bbox, 1, t0)?;
    match fetch_stage(cache, client, bbox, 2, t0) {
        Ok(p) => {
            out.ways.extend(p.ways);
            out.nodes.extend(p.nodes);
        }
        Err(e) => eprintln!("[osm] buildings/veg stage degraded for chunk: {e}"),
    }
    Ok(out)
}

fn fetch_stage(
    cache: &hs_cache::Cache,
    client: &crate::http::Client,
    bbox: [f64; 4],
    stage: u32,
    t0: std::time::Instant,
) -> Result<OsmData> {
    // v4: staged+chunked keys
    let key = format!(
        "overpass#v4#{stage}#{:.5},{:.5},{:.5},{:.5}",
        bbox[0], bbox[1], bbox[2], bbox[3]
    );
    let query = overpass_query_stage(bbox, stage);
    let bytes = cache.blob_fetch("osm-overpass", &key, || {
        let body = format!("data={}", crate::geocode::urlencoded(&query));
        let mut last = Err(anyhow::anyhow!("no mirrors configured"));
        for attempt in 0..3u32 {
            if t0.elapsed() > FETCH_DEADLINE {
                break;
            }
            for (mi, url) in MIRRORS.iter().enumerate() {
                match client.post_body(url, body.clone()) {
                    Ok(b) => {
                        if mi > 0 || attempt > 0 {
                            eprintln!("[osm] mirror {url} succeeded (attempt {})", attempt + 1);
                        }
                        return Ok(b);
                    }
                    Err(e) => {
                        // TLS blips (schannel SEC_E_ILLEGAL_MESSAGE etc.),
                        // 5xx overload and timeouts all land here: retryable
                        eprintln!("[osm] mirror {url} failed: {e}");
                        last = Err(e);
                    }
                }
            }
            if attempt < 2 {
                std::thread::sleep(std::time::Duration::from_secs(15 * (attempt as u64 + 1)));
            }
        }
        last
    })?;
    parse_overpass(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"{
      "version": 0.6,
      "elements": [
        {
          "type": "way", "id": 1,
          "tags": { "highway": "residential", "name": "Test St" },
          "geometry": [ { "lat": 35.00, "lon": 135.70 }, { "lat": 35.01, "lon": 135.71 } ]
        },
        {
          "type": "way", "id": 2,
          "tags": { "building": "residential", "height": "9" },
          "geometry": [
            { "lat": 0.0, "lon": 0.0 }, { "lat": 0.0, "lon": 0.001 },
            { "lat": 0.001, "lon": 0.001 }, { "lat": 0.0, "lon": 0.0 }
          ]
        },
        { "type": "node", "id": 3, "lat": 1.0, "lon": 1.0, "tags": {} }
      ]
    }"#;

    #[test]
    fn parse_ways_and_tags() {
        let d = parse_overpass(FIXTURE.as_bytes()).unwrap();
        assert_eq!(d.ways.len(), 2);
        assert_eq!(d.ways[0].tag("highway"), Some("residential"));
        assert!(!d.ways[0].closed());
        assert!(d.ways[1].closed());
        assert_eq!(d.ways[1].tag("height"), Some("9"));
    }

    #[test]
    fn query_contains_layer_set() {
        let q1 = overpass_query_stage([34.9, 135.7, 35.1, 135.9], 1);
        assert!(q1.contains("way[highway]"));
        assert!(q1.contains("node[place"));
        assert!(!q1.contains("way[building]")); // stage 1 is the light half
        let q2 = overpass_query_stage([34.9, 135.7, 35.1, 135.9], 2);
        assert!(q2.contains("way[building]"));
        assert!(q2.contains("way[leisure"));
        assert!(q1.contains("out geom") && q2.contains("out geom"));
    }
}
