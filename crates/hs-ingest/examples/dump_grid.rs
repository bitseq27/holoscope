use hs_ingest::{cog, http, open_cog, fetch_tile_cached, plan_region};
fn main() {
    let cache = hs_cache::Cache::open(&std::path::PathBuf::from("/tmp/hs-dump-cache")).unwrap();
    let client = http::Client::default();
    let plan = plan_region(35.0033, 35.0393, 135.7501, 135.7861);
    for (la, lo) in &plan.tiles {
        let meta = open_cog(&cache, &client, *la as f64 + 0.5, *lo as f64 + 0.5).unwrap();
        println!("tile {} {} -> {}x{} tiles {}x{}", la, lo, meta.width, meta.height, meta.tiles_x(), meta.tiles_y());
        let g = cog::read_window(&meta, plan.lon0, plan.lat0, plan.lon1, plan.lat1, |m, idx| {
            fetch_tile_cached(&cache, &client, m, idx)
        }).unwrap();
        println!("grid {}x{} lon0={:.3} lat0={:.3}", g.nx, g.ny, g.lon0, g.lat0);
        println!("   dlon={} dlat={} lon_end={} lat_end={}", g.dlon, g.dlat,
            g.lon0 + g.dlon * (g.nx as f64 - 1.0), g.lat0 + g.dlat * (g.ny as f64 - 1.0));
    }
}
