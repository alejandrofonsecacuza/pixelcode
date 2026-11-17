//! End-to-end test: starts `pixel mcp` and talks to it over stdio like Claude Code would.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

struct Client {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
    id: i64,
}

impl Client {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_pixel"))
            .arg("mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Self { child, out, id: 0 }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params});
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["id"], self.id);
        v
    }

    fn notify(&mut self, method: &str) {
        let stdin = self.child.stdin.as_mut().unwrap();
        writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "method": method})).unwrap();
    }

    fn call(&mut self, tool: &str, args: Value) -> (bool, String, usize) {
        let r = self.send("tools/call", json!({"name": tool, "arguments": args}));
        let res = &r["result"];
        let content = res["content"].as_array().unwrap();
        let text = content
            .iter()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let images = content.iter().filter(|c| c["type"] == "image").count();
        (res["isError"].as_bool().unwrap(), text, images)
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn full_session() {
    let mut c = Client::start();
    let init = c.send("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}));
    assert_eq!(init["result"]["serverInfo"]["name"], "pixelcode");
    c.notify("notifications/initialized");

    let tools = c.send("tools/list", json!({}));
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["canvas", "draw", "view", "history"]);

    let (err, text, _) = c.call("draw", json!({"script": "rect(0,0,1,1,\"red\")"}));
    assert!(err && text.contains("create"), "{text}");

    let (err, text, _) = c.call(
        "canvas",
        json!({"action": "create", "width": 64, "height": 32, "background": "#102030"}),
    );
    assert!(!err, "{text}");

    let (err, text, imgs) = c.call(
        "draw",
        json!({"script": "circle(16, 16, 8, \"#ff0\"); print(\"done\");", "view": "changed"}),
    );
    assert!(!err, "{text}");
    assert!(text.contains("changed") && text.contains("done"), "{text}");
    assert_eq!(imgs, 1);

    let (err, text, _) = c.call(
        "view",
        json!({"mode": "pixels", "x": 12, "y": 8, "w": 8, "h": 2}),
    );
    assert!(!err, "{text}");
    assert!(text.contains("#ffff00"), "{text}");

    let (err, text, _) = c.call("draw", json!({"script": "rect(0,0,4,4, \"nope\")"}));
    assert!(err && text.contains("canvas unchanged"), "{text}");

    let (_, text, _) = c.call("history", json!({"action": "undo"}));
    assert!(text.contains("undo x1"), "{text}");
    let (_, text, _) = c.call("view", json!({"mode": "stats"}));
    assert!(text.contains("1 distinct colors"), "{text}");

    let dir = std::env::temp_dir().join(format!("pixelcode-test-{}", std::process::id()));
    let path = dir.join("out.png");
    let (err, text, _) = c.call(
        "canvas",
        json!({"action": "save", "path": path.to_str().unwrap(), "scale": 2}),
    );
    assert!(!err, "{text}");
    let png = std::fs::read(&path).unwrap();
    assert_eq!(&png[1..4], b"PNG");
    let _ = std::fs::remove_dir_all(dir);

    let unknown = c.send("does/not/exist", json!({}));
    assert_eq!(unknown["error"]["code"], -32601);
}

#[test]
fn person_and_agent_share_the_canvas() {
    let mut c = Client::start();
    c.send(
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "0"}}),
    );
    c.call(
        "canvas",
        json!({"action": "create", "width": 32, "height": 32}),
    );
    let (_, text, _) = c.call("draw", json!({"script": "rect(0, 0, 32, 32, \"#123\");"}));
    assert!(!text.contains("[pixel]"), "{text}");

    // The person changes the canvas (this is how the pixel-studio plugin calls it).
    let (err, text, _) = c.call(
        "draw",
        json!({"script": "polyline([[2, 2], [9, 2]], \"#f00\");", "_author": "user", "_label": "edit 8 px"}),
    );
    assert!(!err && !text.contains("[pixel]"), "{text}");
    let (_, pending, _) = c.call("canvas", json!({"action": "pending", "_author": "user"}));
    assert!(pending.contains("edit"), "{pending}");

    // The agent's next call starts with the notice, and only that one.
    let (_, text, _) = c.call("view", json!({"mode": "stats"}));
    assert!(
        text.contains("[pixel] The person edited canvas \"main\""),
        "{text}"
    );
    assert!(
        text.contains("8 px changed within x=2 y=2 w=8 h=1"),
        "{text}"
    );
    let (_, text, _) = c.call("view", json!({"mode": "stats"}));
    assert!(!text.contains("[pixel]"), "{text}");

    // An undo by the person is reported too.
    c.call("history", json!({"action": "undo", "_author": "user"}));
    let (_, text, _) = c.call("view", json!({"mode": "stats"}));
    assert!(text.contains("(undo)"), "{text}");

    let (_, text, _) = c.call("history", json!({"action": "list"}));
    assert!(text.contains("agent  draw: rect(0, 0, 32, 32"), "{text}");
}
