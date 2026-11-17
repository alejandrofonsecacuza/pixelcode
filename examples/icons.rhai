// A set of 16x16 UI icons built from grids and shapes, on an 80x16 canvas.
// pixel run examples/icons.rhai --size 80x16 -o icons.png --scale 8 --show

let ink = "#1d2433";
let accent = "#3b82f6";

// 1. Save (floppy disk) with grid(): full per-pixel control
grid(0, 0, [
    "..............",
    ".KKKKKKKKKKKK.",
    ".KWWWWWWWWKAK.",
    ".KWWWWWWWWKAK.",
    ".KWWWWWWWWKKK.",
    ".KKKKKKKKKKKK.",
    ".KKKKKKKKKKKK.",
    ".KKAAAAAAAAKK.",
    ".KKAKKKKKKAKK.",
    ".KKAKKKKKKAKK.",
    ".KKAAAAAAAAKK.",
    ".KKKKKKKKKKKK.",
], #{K: ink, W: "#e5e7eb", A: accent});

// 2. Search: circle with an inner stroke plus a handle
circle(23.5, 6.5, 5, #{stroke: ink, width: 2});
line(27, 10, 30, 13, #{color: ink, width: 2, cap: "square"});

// 3. Check mark: thick polyline
polyline([[34, 8], [37, 11], [44, 4]], #{color: "#16a34a", width: 2});

// 4. Gear: polygon generated in a loop; the hole is cut with blend "erase"
let pts = [];
for i in 0..24 {
    let a = (i - 0.5) * TAU / 24;
    let r = if (i / 2) % 2 == 0 { 8.0 } else { 5.6 };
    pts.push([56 + r * cos(a), 8 + r * sin(a)]);
}
polygon(pts, ink);
circle(56, 8, 2.2, #{fill: "#000", blend: "erase"});

// 5. Warning: triangle from an SVG path plus an exclamation mark
path("M 72 1 L 79 14 L 65 14 Z", "#f59e0b");
rect(71, 5, 2, 5, ink);
rect(71, 11, 2, 2, ink);
