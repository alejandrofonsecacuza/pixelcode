use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use pixel_core::raster::{self, Cap, FillRule};
use pixel_core::span::normalize;
use pixel_core::*;

fn bench(c: &mut Criterion) {
    let red = Paint::solid(Color::rgba(220, 40, 40, 255));
    let glass = Paint::solid(Color::rgba(40, 90, 220, 140));

    c.bench_function("fill 4096x4096 opaque", |b| {
        let mut r = Raster::new(4096, 4096);
        let mut s = Vec::new();
        raster::rect(0.0, 0.0, 4096.0, 4096.0, r.bounds(), &mut s);
        b.iter(|| r.paint_spans(black_box(&s), &red, BlendMode::Over));
    });

    c.bench_function("blend 4096x4096 alpha", |b| {
        let mut r = Raster::new(4096, 4096);
        r.fill_all(Color::WHITE.to_px());
        let mut s = Vec::new();
        raster::rect(0.0, 0.0, 4096.0, 4096.0, r.bounds(), &mut s);
        b.iter(|| r.paint_spans(black_box(&s), &glass, BlendMode::Over));
    });

    c.bench_function("10k circles r=12 on 4096", |b| {
        let mut r = Raster::new(4096, 4096);
        let bounds = r.bounds();
        let mut s = Vec::new();
        b.iter(|| {
            for i in 0..10_000 {
                let x = (i * 7919 % 4096) as f64;
                let y = (i * 104_729 % 4096) as f64;
                s.clear();
                s.extend(
                    raster::ellipse_rows(x, y, 12.0, 12.0, 0, 4096)
                        .into_iter()
                        .filter(|s| s.x0 < s.x1),
                );
                normalize(&mut s, bounds);
                r.paint_spans(&s, &red, BlendMode::Over);
            }
        });
    });

    c.bench_function("star polygon 2000 verts on 4096", |b| {
        let pts: Vec<(f64, f64)> = (0..2000)
            .map(|i| {
                let a = i as f64 / 2000.0 * std::f64::consts::TAU;
                let rr = if i % 2 == 0 { 2000.0 } else { 900.0 };
                (2048.0 + rr * a.cos(), 2048.0 + rr * a.sin())
            })
            .collect();
        let mut r = Raster::new(4096, 4096);
        let bounds = r.bounds();
        b.iter(|| {
            let mut s = Vec::new();
            raster::polygon(
                std::slice::from_ref(&pts),
                FillRule::NonZero,
                bounds,
                &mut s,
            );
            normalize(&mut s, bounds);
            r.paint_spans(&s, &red, BlendMode::Over);
        });
    });

    c.bench_function("thick polyline 1000 segs w=5", |b| {
        let pts: Vec<(f64, f64)> = (0..1000)
            .map(|i| (i as f64 * 4.0, 2048.0 + (i as f64 * 0.1).sin() * 800.0))
            .collect();
        let r = Raster::new(4096, 4096);
        b.iter(|| {
            let mut s = Vec::new();
            raster::stroke_polyline(black_box(&pts), false, 5.0, Cap::Round, r.bounds(), &mut s);
            s
        });
    });

    c.bench_function("dithered radial gradient 2048x2048", |b| {
        let g = Gradient::even(&[Color::rgb(10, 10, 40), Color::rgb(250, 200, 80)], 6, true);
        let p = Paint::Gradient(g, GradientShape::radial(1024.0, 1024.0, 1024.0));
        let mut r = Raster::new(2048, 2048);
        let mut s = Vec::new();
        raster::rect(0.0, 0.0, 2048.0, 2048.0, r.bounds(), &mut s);
        b.iter(|| r.paint_spans(&s, &p, BlendMode::Replace));
    });

    c.bench_function("flood fill 2048x2048", |b| {
        let r = Raster::new(2048, 2048);
        b.iter(|| {
            let mut s = Vec::new();
            raster::flood(&r, 1000, 1000, 0, r.bounds(), &mut s);
            s
        });
    });

    c.bench_function("composite 4096x4096 x4 layers", |b| {
        let mut d = Document::new(4096, 4096);
        d.layers[0].raster.fill_all(Color::WHITE.to_px());
        for k in 0..3 {
            let i = d.ensure_layer(&format!("l{k}"));
            let mut s = Vec::new();
            raster::rect(k as f64 * 500.0, 0.0, 2500.0, 4096.0, d.bounds(), &mut s);
            d.layers[i].raster.paint_spans(&s, &glass, BlendMode::Over);
        }
        b.iter(|| d.composite_all());
    });

    c.bench_function("snapshot + diff 4096x4096", |b| {
        let mut d = Document::new(4096, 4096);
        d.layers[0].raster.fill_all(Color::WHITE.to_px());
        b.iter(|| {
            let snap = d.clone();
            d.layers[0].raster.set(1234, 2345, 0xff00_00ff);
            d.diff(&snap)
        });
    });
}

criterion_group!(benches, bench);
criterion_main!(benches);
