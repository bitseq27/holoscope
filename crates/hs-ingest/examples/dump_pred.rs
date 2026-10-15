use hs_ingest::{cog, http, open_cog, fetch_tile_cached};
use flate2::read::ZlibDecoder;
use std::io::Read;

fn stats(v: &[f32]) -> (f32, f32, f32) {
    let mut mn = f32::INFINITY; let mut mx = f32::NEG_INFINITY; let mut sum = 0.0;
    for x in v { mn = mn.min(*x); mx = mx.max(*x); sum += *x; }
    (mn, mx, sum / v.len() as f32)
}

fn main() {
    let cache = hs_cache::Cache::open(&std::path::PathBuf::from("/tmp/hs-dump-cache")).unwrap();
    let client = http::Client::default();
    let meta = open_cog(&cache, &client, 46.68, 7.85).unwrap();
    let tx = 2800 / meta.tile_w; let ty = 1100 / meta.tile_h;
    let idx = (ty * meta.tiles_x() + tx) as usize;
    let raw = fetch_tile_cached(&cache, &client, &meta, idx).unwrap();
    let mut d = ZlibDecoder::new(&raw[..]);
    let mut buf = Vec::new();
    d.read_to_end(&mut buf).unwrap();
    let cols = meta.tile_w as usize;
    let bpp = 4usize;
    let rowbytes = cols * bpp;
    let rows = meta.tile_h as usize;

    // candidate A: acc stride=bpp over row, then de-plane
    let mut a = buf.clone();
    for r in 0..rows {
        let o = r * rowbytes;
        for i in bpp..rowbytes { a[o+i] = a[o+i].wrapping_add(a[o+i-bpp]); }
        let tmp = a[o..o+rowbytes].to_vec();
        for b in 0..bpp { for x in 0..cols { a[o + x*bpp + b] = tmp[b*cols + x]; } }
    }
    // candidate B: de-plane first, then acc stride=1 within each plane
    let mut b2 = buf.clone();
    for r in 0..rows {
        let o = r * rowbytes;
        let tmp = b2[o..o+rowbytes].to_vec();
        for b in 0..bpp { for x in 0..cols { b2[o + x*bpp + b] = tmp[b*cols + x]; } }
        for p in 0..bpp {
            let po = o + p*cols;
            for i in 1..cols { b2[po+i] = b2[po+i].wrapping_add(b2[po+i-1]); }
        }
    }
    // candidate C: acc stride=1 over whole row, then de-plane
    let mut c = buf.clone();
    for r in 0..rows {
        let o = r * rowbytes;
        for i in 1..rowbytes { c[o+i] = c[o+i].wrapping_add(a[o+i-1] - a[o+i-1] + c[o+i-1]); }
        let tmp = c[o..o+rowbytes].to_vec();
        for b in 0..bpp { for x in 0..cols { c[o + x*bpp + b] = tmp[b*cols + x]; } }
    }
    for (name, arr) in [("A", &a), ("B", &b2), ("C", &c)] {
        let le: Vec<f32> = (0..arr.len()/4).map(|i| f32::from_le_bytes([arr[i*4],arr[i*4+1],arr[i*4+2],arr[i*4+3]])).collect();
        let be: Vec<f32> = (0..arr.len()/4).map(|i| f32::from_be_bytes([arr[i*4],arr[i*4+1],arr[i*4+2],arr[i*4+3]])).collect();
        let (l1,l2,l3) = stats(&le); let (g1,g2,g3) = stats(&be);
        println!("{name}: LE {l1:9.1} .. {l2:9.1} mean {l3:9.1} | BE {g1:9.1} .. {g2:9.1} mean {g3:9.1}");
    }
}
