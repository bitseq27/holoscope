use hs_ingest::ot;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let deg = [-116.612, 33.122, -116.606, 33.128];
    let clip = ot::utm_box(deg, 11, false);
    println!("clip (utm11N): {:.1},{:.1},{:.1},{:.1}", clip[0], clip[1], clip[2], clip[3]);
    match ot::decode_laz(&bytes, clip, 25_000_000) {
        Ok(pts) => {
            let b = ot::points_bbox(&pts);
            println!("pts={} bbox={:.1},{:.1},{:.1},{:.1}", pts.len(), b[0], b[1], b[2], b[3]);
            println!("z range {:.1}..{:.1}", pts.iter().map(|p| p.z).fold(f64::INFINITY, f64::min), pts.iter().map(|p| p.z).fold(f64::NEG_INFINITY, f64::max));
            println!("first {:?}", &pts[..3.min(pts.len())]);
        }
        Err(e) => println!("ERR {e:#}"),
    }
}
