use hs_ingest::{cog, http};
fn main() {
    let client = http::Client::default();
    let url = cog::copernicus_url(35, 135);
    println!("url: {url}");
    let head = client.get_range(&url, Some((0, 262_144))).unwrap();
    println!("head bytes: {}", head.len());
    let meta = cog::parse_header(&head, &url).unwrap();
    println!("width={} height={} tile={}x{} comp={} bits={} fmt={}",
        meta.width, meta.height, meta.tile_w, meta.tile_h, meta.compression, meta.bits, meta.sample_format);
    println!("scale={:?} tie=({},{})", meta.pixel_scale, meta.tiepoint_lon, meta.tiepoint_lat);
    println!("n_offsets={} n_counts={}", meta.tile_offsets.len(), meta.tile_counts.len());
    println!("first offsets: {:?}", &meta.tile_offsets[..8.min(meta.tile_offsets.len())]);
    println!("first counts:  {:?}", &meta.tile_counts[..8.min(meta.tile_counts.len())]);
    println!("max count: {}", meta.tile_counts.iter().max().unwrap_or(&0));
}
