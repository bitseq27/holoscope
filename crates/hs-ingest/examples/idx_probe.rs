use hs_ingest::{http, ot};
fn main() {
    let client = http::Client::default();
    let url = ot::tile_index_url("CHANNEL_IS");
    let z = client.get_range(&url, None).unwrap();
    println!("zip bytes {}", z.len());
    match ot::parse_tile_index(&z) {
        Ok(t) => {
            println!("tiles {}", t.len());
            for (b, u) in t.iter().take(4) {
                println!("{:.3},{:.3},{:.3},{:.3}  {}", b[0], b[1], b[2], b[3], &u[..u.len().min(80)]);
            }
        }
        Err(e) => println!("ERR {e:#}"),
    }
}
