//! Polite HTTPS client: identifiable UA, ≤1 req/s per host, ranged GETs
//! (plan NFR-07). Sync by design in M1 (DD-M1-1): one streamed COG window at a
//! time; concurrency arrives with M2 if needed.

use anyhow::{anyhow, Result};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const USER_AGENT: &str = "HoloScope/0.2 (rust-wgpu; dev build; contact: holoscope-dev@localhost)";

pub struct Client {
    agent: ureq::Agent,
    min_interval: Duration,
    last: Mutex<HashMap<String, Instant>>,
}

impl Default for Client {
    fn default() -> Self {
        Self::new(Duration::from_secs(1))
    }
}

impl Client {
    pub fn new(min_interval: Duration) -> Self {
        // ureq 2.12 does not auto-attach a TLS backend; native-tls =
        // schannel on Windows (plan target), openssl on Linux, SecureTransport on macOS
        let tls = native_tls::TlsConnector::new().expect("native TLS backend");
        let agent = ureq::AgentBuilder::new()
            .tls_connector(std::sync::Arc::new(tls))
            .build();
        Self {
            agent,
            min_interval,
            last: Mutex::new(HashMap::new()),
        }
    }

    fn wait_for_slot(&self, url: &str) {
        let host = url
            .trim_start_matches("https://")
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap_or("")
            .to_string();
        let mut map = self.last.lock().unwrap();
        if let Some(t) = map.get(&host) {
            let elapsed = t.elapsed();
            if elapsed < self.min_interval {
                std::thread::sleep(self.min_interval - elapsed);
            }
        }
        map.insert(host, Instant::now());
    }

    /// Ranged GET. `range = Some((start, len))` sends `Range: bytes=…`.
    pub fn get_range(&self, url: &str, range: Option<(u64, u64)>) -> Result<Vec<u8>> {
        self.wait_for_slot(url);
        let mut req = self
            .agent
            .request("GET", url)
            .set("User-Agent", USER_AGENT);
        if let Some((start, len)) = range {
            req = req.set("Range", &format!("bytes={start}-{}", start + len - 1));
        }
        let resp = req.call().map_err(|e| anyhow!("GET {url}: {e}"))?;
        let status = resp.status();
        if !(200..300).contains(&status) {
            return Err(anyhow!("GET {url}: HTTP {status}"));
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)?;
        Ok(bytes)
    }

    /// POST with an `application/x-www-form-urlencoded` body (Overpass).
    pub fn post_body(&self, url: &str, body: String) -> Result<Vec<u8>> {
        self.wait_for_slot(url);
        let resp = self
            .agent
            .post(url)
            .set("User-Agent", USER_AGENT)
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&body)
            .map_err(|e| anyhow!("POST {url}: {e}"))?;
        let status = resp.status();
        if !(200..300).contains(&status) {
            return Err(anyhow!("POST {url}: HTTP {status}"));
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut resp.into_reader(), &mut bytes)?;
        Ok(bytes)
    }

    pub fn get_json(&self, url: &str) -> Result<serde_json::Value> {
        let bytes = self.get_range(url, None)?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_spaces_requests() {
        let c = Client::new(Duration::from_millis(120));
        let url = "https://example.invalid/x";
        let t0 = Instant::now();
        c.wait_for_slot(url);
        c.wait_for_slot(url);
        assert!(t0.elapsed() >= Duration::from_millis(100));
        // different host is not throttled
        let t1 = Instant::now();
        c.wait_for_slot("https://other.invalid/y");
        assert!(t1.elapsed() < Duration::from_millis(100));
    }
}
