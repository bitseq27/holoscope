//! Geocoding: Photon autocomplete + Nominatim resolve, cache-first
//! (plan §5.1/§11.2, NFR-07 policy compliant).

use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct PhotonHit {
    pub label: String,
    pub sub: String,
    pub lat: f64,
    pub lon: f64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ResolvedPlace {
    pub label: String,
    pub lat: f64,
    pub lon: f64,
    /// (min_lat, max_lat, min_lon, max_lon) suggestion, if the service gave one
    pub bbox: Option<[f64; 4]>,
}

/// Parse a Photon API response (`/api/?q=…&limit=5`).
pub fn parse_photon(v: &serde_json::Value) -> Vec<PhotonHit> {
    let mut out = Vec::new();
    let Some(feats) = v.get("features").and_then(|f| f.as_array()) else {
        return out;
    };
    for f in feats.iter().take(5) {
        let props = f.get("properties").cloned().unwrap_or_default();
        let coords = f
            .pointer("/geometry/coordinates")
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default();
        if coords.len() < 2 {
            continue;
        }
        let lon = coords[0].as_f64().unwrap_or(0.0);
        let lat = coords[1].as_f64().unwrap_or(0.0);
        let get = |k: &str| props.get(k).and_then(|s| s.as_str()).unwrap_or("").to_string();
        let name = get("name");
        if name.is_empty() {
            continue;
        }
        let parts = [get("city"), get("state"), get("country")];
        let sub = parts
            .iter()
            .filter(|s| !s.is_empty() && **s != name)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        out.push(PhotonHit { label: name, sub, lat, lon });
    }
    out
}

/// Parse a Nominatim `jsonv2` search response (first result).
pub fn parse_nominatim(v: &serde_json::Value) -> Option<ResolvedPlace> {
    let first = v.as_array()?.first()?.clone();
    let lat = first.get("lat").and_then(|x| x.as_str()).and_then(|s| s.parse().ok())?;
    let lon = first.get("lon").and_then(|x| x.as_str()).and_then(|s| s.parse().ok())?;
    let label = first
        .get("display_name")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let bbox = first.get("boundingbox").and_then(|b| b.as_array()).and_then(|b| {
        if b.len() != 4 {
            return None;
        }
        let v: Option<Vec<f64>> = b.iter().map(|x| x.as_str().and_then(|s| s.parse().ok())).collect();
        let v = v?;
        Some([v[0], v[1], v[2], v[3]]) // minlat, maxlat, minlon, maxlon
    });
    Some(ResolvedPlace { label, lat, lon, bbox })
}

pub fn photon_url(q: &str) -> String {
    format!(
        "https://photon.komoot.io/api/?q={}&limit=5",
        urlencoded(q)
    )
}

pub fn nominatim_url(q: &str) -> String {
    format!(
        "https://nominatim.openstreetmap.org/search?format=jsonv2&limit=1&q={}",
        urlencoded(q)
    )
}

/// Parse raw `lat, lon` (decimal degrees) typed by the user (FR-01).
pub fn parse_raw_coords(q: &str) -> Option<(f64, f64)> {
    let mut it = q.split(',');
    let lat = it.next()?.trim().parse::<f64>().ok()?;
    let lon = it.next()?.trim().parse::<f64>().ok()?;
    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
        Some((lat, lon))
    } else {
        None
    }
}

pub fn urlencoded(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => out.push(c),
            ' ' => out.push('+'),
            _ => {
                let mut b = [0u8; 4];
                for x in c.encode_utf8(&mut b).as_bytes() {
                    out.push_str(&format!("%{x:02X}"));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHOTON_FIXTURE: &str = r#"{
      "features": [
        {
          "geometry": { "coordinates": [135.7690, 35.0198] },
          "properties": { "name": "Kyoto", "city": "Kyoto", "state": "Kansai", "country": "Japan" }
        },
        {
          "geometry": { "coordinates": [-0.1276, 51.5072] },
          "properties": { "name": "London", "country": "United Kingdom" }
        }
      ]
    }"#;

    const NOMINATIM_FIXTURE: &str = r#"[{
      "place_id": 1, "lat": "35.0116", "lon": "135.7681",
      "display_name": "Kyoto, Kansai, Japan",
      "boundingbox": ["34.8748", "35.3212", "135.4488", "135.9431"]
    }]"#;

    #[test]
    fn photon_parse() {
        let v: serde_json::Value = serde_json::from_str(PHOTON_FIXTURE).unwrap();
        let hits = parse_photon(&v);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].label, "Kyoto");
        assert!((hits[0].lat - 35.0198).abs() < 1e-9);
        assert!(hits[0].sub.contains("Japan"));
    }

    #[test]
    fn nominatim_parse() {
        let v: serde_json::Value = serde_json::from_str(NOMINATIM_FIXTURE).unwrap();
        let p = parse_nominatim(&v).unwrap();
        assert!((p.lat - 35.0116).abs() < 1e-9);
        let bb = p.bbox.unwrap();
        assert!((bb[0] - 34.8748).abs() < 1e-9);
    }

    #[test]
    fn raw_coords() {
        assert_eq!(parse_raw_coords("48.8584, 2.2945"), Some((48.8584, 2.2945)));
        assert_eq!(parse_raw_coords("Kyoto"), None);
        assert_eq!(parse_raw_coords("91.0, 0.0"), None);
    }
}
