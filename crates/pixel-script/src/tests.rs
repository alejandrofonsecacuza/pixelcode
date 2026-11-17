use super::*;

fn exec(doc: &Document, s: &str) -> Document {
    match run(doc, s, &RunOptions::default()) {
        Ok((d, _)) => d,
        Err(e) => panic!("script failed: {}", e.message),
    }
}

fn px(d: &Document, x: i32, y: i32) -> Color {
    Color::from_px(d.composite(IRect::new(x, y, 1, 1)).px[0])
}

#[test]
fn shapes_and_styles() {
    let d = Document::new(64, 64);
    let d = exec(
        &d,
        r##"
        rect(0, 0, W, H, "#000");
        circle(32, 32, 10, #{fill: "red", stroke: "#fff", width: 2});
        line(0, 63, 63, 63, "lime");
        pixel(5, 5, rgb(0, 0, 255));
        "##,
    );
    assert_eq!(px(&d, 5, 5), Color::rgb(0, 0, 255));
    assert_eq!(px(&d, 32, 32), Color::rgb(255, 0, 0));
    assert_eq!(px(&d, 32, 22), Color::WHITE);
    assert_eq!(px(&d, 10, 63), Color::rgb(0, 255, 0));
    assert_eq!(px(&d, 1, 1), Color::BLACK);
}

#[test]
fn errors_are_atomic_and_located() {
    let d = Document::new(16, 16);
    let err = run(
        &d,
        "rect(0,0,4,4,\"red\");\nrect(0,0,4,4, \"nocolor\");",
        &RunOptions::default(),
    )
    .unwrap_err();
    assert!(err.message.contains("invalid color"), "{}", err.message);
    assert!(err.message.contains("line 2"), "{}", err.message);
    assert_eq!(d.layers[0].raster.allocated_tiles(), 0);
}

#[test]
fn grid_layers_and_copy() {
    let d = Document::new(32, 32);
    let d = exec(
        &d,
        r##"
        layer("sprite");
        grid(0, 0, ["A.", ".B"], #{A: "#f00", B: "#00f"}, 2);
        copy(0, 0, 4, 4, 10, 10, #{flip_x: true});
        layer("bg");
        move_layer("bg", 0);
        clear("#222");
        print(layers());
        "##,
    );
    assert_eq!(d.layers[0].name, "bg");
    assert_eq!(px(&d, 1, 1), Color::rgb(255, 0, 0));
    assert_eq!(px(&d, 3, 3), Color::rgb(0, 0, 255));
    assert_eq!(px(&d, 0, 3), Color::rgb(0x22, 0x22, 0x22));
    // flip_x: red moves to the right
    assert_eq!(px(&d, 13, 10), Color::rgb(255, 0, 0));
    assert_eq!(px(&d, 10, 13), Color::rgb(0, 0, 255));
}

#[test]
fn gradients_paths_text_flood() {
    let d = Document::new(64, 64);
    let (d, rep) = run(
        &d,
        r##"
        rect(0, 0, 64, 8, linear(0, 0, 64, 0, ["#000", "#fff"], #{steps: 4}));
        path("M 10 20 L 30 20 L 20 40 Z", #{fill: "#0f0"});
        rect(40, 20, 10, 10, #{stroke: "#fff"});
        flood(45, 25, "#f0f");
        text(0, 50, "Hi", "#ff0");
        print(text_width("Hi"));
        let c = get(45, 25);
        print(c);
        "##,
        &RunOptions::default(),
    )
    .unwrap();
    assert_eq!(rep.logs, vec!["16", "#ff00ff"]);
    assert_eq!(px(&d, 0, 0), Color::BLACK);
    assert_eq!(px(&d, 63, 0), Color::WHITE);
    assert_eq!(px(&d, 20, 25), Color::rgb(0, 255, 0));
    assert_eq!(px(&d, 40, 20), Color::WHITE);
}

#[test]
fn deterministic_rand_and_budget() {
    let d = Document::new(8, 8);
    let s = "seed(7); let v = []; for i in 0..5 { v.push(rand(0, 100)); } print(v);";
    let a = run(&d, s, &RunOptions::default()).unwrap().1.logs;
    let b = run(&d, s, &RunOptions::default()).unwrap().1.logs;
    assert_eq!(a, b);
    let e = run(
        &d,
        "loop {}",
        &RunOptions {
            max_operations: 10_000,
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(e.message.contains("operation budget"), "{}", e.message);
}

#[test]
fn ints_work_in_math() {
    let d = Document::new(8, 8);
    let (_, r) = run(
        &d,
        "print(sin(0)); print(sqrt(16)); print(clamp(5, 0, 3)); print(PI > 3.0);",
        &RunOptions::default(),
    )
    .unwrap();
    assert_eq!(r.logs, vec!["0.0", "4.0", "3", "true"]);
}
