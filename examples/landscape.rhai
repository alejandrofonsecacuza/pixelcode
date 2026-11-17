// 256x144 pixel-art landscape: dithered gradient sky, sun, mountains, stars and a sprite.
// pixel run examples/landscape.rhai --size 256x144 -o landscape.png --scale 4 --show

layer("sky");
rect(0, 0, W, H, linear(0, 0, 0, 100, ["#0b0e2a", "#3b1f5c", "#e0605a", "#ffc46b"], #{steps: 7, dither: true}));

seed(42);
for i in 0..60 {
    pixel(rand(0, W - 1), rand(0, 50), rgba(255, 255, 255, rand(90, 255)));
}

circle(190.5, 78.5, 18, radial(190, 78, 18, ["#fff6c8", "#ffb347"], #{steps: 3}));

layer("mountains");
polygon([0, 144, 0, 96, 30, 70, 58, 92, 90, 58, 128, 100, 160, 76, 200, 104, 230, 84, 256, 98, 256, 144], "#2b1d3f");
polygon([0, 144, 0, 116, 40, 100, 80, 118, 120, 104, 170, 122, 210, 108, 256, 120, 256, 144], "#1a1229");

layer("foreground");
rect(0, 128, W, 16, "#120c1c");
path("M 20 128 Q 40 110 60 128 Z", #{fill: "#0d2a1f"});

let cat = [
    "B......B",
    "BB....BB",
    "BBBBBBBB",
    "BYBBBBYB",
    "BBBPPBBB",
    ".BBBBBB.",
    ".BBBBBBB",
    ".B.B.B.B",
];
grid(110, 112, cat, #{B: "#000000", Y: "#ffe14d", P: "#ff8fb1"}, 2);

text(4, 4, "pixelcode", #{fill: "#ffffff", scale: 1});
print(`layers: ${layers()}`);
