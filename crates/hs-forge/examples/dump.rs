use hs_forge::{forge_region, ForgeParams};
fn main() {
    let hf = hs_geo::demo_heightfield(4000.0, 8.0);
    let r = forge_region(&hf, &ForgeParams { target_points: 400_000, ..Default::default() });
    for n in &r.nodes {
        let mut minp = [i16::MAX; 3];
        let mut maxp = [i16::MIN; 3];
        let mut sat = 0usize;
        for p in &n.points {
            for k in 0..3 {
                minp[k] = minp[k].min(p.pos[k]);
                maxp[k] = maxp[k].max(p.pos[k]);
                if p.pos[k].abs() >= 32767 {
                    sat += 1;
                }
            }
        }
        println!(
            "{:10} n={:7} qscale={:.4} min={:?} max={:?} saturated={}",
            n.name, n.points.len(), n.quant_scale, minp, maxp, sat
        );
    }
}
