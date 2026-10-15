use hs_geo::{geodetic_to_ecef, Enu};
fn main() {
    let enu = Enu::new(35.0116, 135.7681, 0.0);
    for (lat, lon) in [(35.0116, 135.7681), (35.02, 135.77), (34.99, 135.75), (35.04, 135.80)] {
        let e = enu.ecef_to_enu(geodetic_to_ecef(lat, lon, 0.0));
        println!("({lat}, {lon}) -> enu ({:.1}, {:.1}, {:.1})", e.x, e.y, e.z);
    }
}
