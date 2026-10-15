use hs_ingest::{cog, http, open_cog, fetch_tile_cached};
use flate2::read::ZlibDecoder;
use std::io::Read;
fn main() {
    let cache = hs_cache::Cache::open(&std::path::PathBuf::from("/tmp/hs-dump-cache")).unwrap();
    let client = http::Client::default();
    let meta = open_cog(&cache, &client, 46.68, 7.85).unwrap();
    println!("endian_be={} comp={} tiles={}x{}", meta.big_endian, meta.compression, meta.tiles_x(), meta.tiles_y());
    let head = client.get_range(&meta.url, Some((0, 65536))).unwrap();
    // scan IFD for tag 317 (predictor) and 338 extra samples
    let be = &head[0..2] == b"MM";
    let rd16 = |o: usize| -> u16 { let b=&head[o..o+2]; if be { u16::from_be_bytes([b[0],b[1]]) } else { u16::from_le_bytes([b[0],b[1]]) } };
    let rd32 = |o: usize| -> u32 { let b=&head[o..o+4]; if be { u32::from_be_bytes([b[0],b[1],b[2],b[3]]) } else { u32::from_le_bytes([b[0],b[1],b[2],b[3]]) } };
    let ifd = rd32(4) as usize;
    let n = rd16(ifd) as usize;
    for i in 0..n {
        let e = ifd + 2 + i*12;
        let tag = rd16(e);
        if tag == 317 || tag == 338 || tag == 259 || tag == 339 {
            println!("tag {tag} = {}", rd32(e+8));
        }
    }
    println!("head magic: {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x} {:02x}",
        head[0], head[1], head[2], head[3], head[4], head[5], head[6], head[7]);
    // tile containing pixel (2800, 1100)
    let tx = 2800 / meta.tile_w; let ty = 1100 / meta.tile_h;
    let idx = (ty * meta.tiles_x() + tx) as usize;
    let raw = fetch_tile_cached(&cache, &client, &meta, idx).unwrap();
    println!("raw len={} offset={} count={}", raw.len(), meta.tile_offsets[idx], meta.tile_counts[idx]);
    println!("raw first bytes: {:02x} {:02x} {:02x} {:02x}", raw[0], raw[1], raw[2], raw[3]);
    let mut d = ZlibDecoder::new(&raw[..]);
    let mut buf = Vec::new();
    let n = d.read_to_end(&mut buf).unwrap();
    println!("inflated {n} bytes");
    let mut le_min = f32::INFINITY; let mut le_max = f32::NEG_INFINITY;
    let mut be_min = f32::INFINITY; let mut be_max = f32::NEG_INFINITY;
    for i in 0..(buf.len()/4) {
        let b = [buf[i*4], buf[i*4+1], buf[i*4+2], buf[i*4+3]];
        let l = f32::from_le_bytes(b); let g = f32::from_be_bytes(b);
        le_min = le_min.min(l); le_max = le_max.max(l);
        be_min = be_min.min(g); be_max = be_max.max(g);
    }
    println!("LE range: {le_min:.1} .. {le_max:.1}");
    println!("BE range: {be_min:.1} .. {be_max:.1}");
}
