use hs_ingest::{http, ot};
fn main() {
    let client = http::Client::default();
    let cache = hs_cache::Cache::open(&std::path::PathBuf::from("/tmp/hs-ot-cache")).unwrap();
    let bbox = [-116.612, 33.122, -116.606, 33.128];
    let t0 = std::time::Instant::now();
    match ot::fetch_lidar(&client, &cache, bbox, 1, &|s, p| println!("[ot] {s} {:.0}%", p * 100.0)) {
        Ok((pts, name)) => {
            let b = ot::points_bbox(&pts);
            println!(
                "[ot] OK dataset={name} pts={} bbox={:.4},{:.4},{:.4},{:.4} in {:.1}s",
                pts.len(), b[0], b[1], b[2], b[3], t0.elapsed().as_secs_f64()
            );
            println!("[ot] sample: {:?}", &pts[..3.min(pts.len())]);
        }
        Err(e) => println!("[ot] ERR: {e:#}"),
    }
}
