//! Exports a canvas as a self-contained HTML page: image, layers and history embedded, with an
//! interactive viewer (crisp zoom, pixel inspector, palette, timeline). It works offline and can
//! be published as-is as a claude.ai Artifact.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use pixel_core::{Author, Document, Image, Step};
use serde_json::{Value, json};

const TEMPLATE: &str = include_str!("share.html");
/// Largest side of the final image and of the layers inside the page.
const MAX_DISPLAY: u32 = 4096;
/// Largest side of each history step.
const MAX_FRAME: u32 = 1024;
const MAX_FRAMES: usize = 120;
/// Budget for embedded images (base64): leaves headroom under an Artifact's 16 MB limit.
const BUDGET: usize = 12 * 1024 * 1024;

pub struct Export {
    pub html: String,
    /// History steps included (counting the final state).
    pub frames: usize,
    /// History steps available (counting the final state).
    pub steps: usize,
    /// Downscale factor of the final image (1 = actual size).
    pub scale: u32,
}

fn png64(img: &Image) -> Result<String, String> {
    Ok(STANDARD.encode(img.encode_png(false)?))
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// UTC date in ISO 8601 without dependencies (Howard Hinnant's civil-from-days algorithm).
fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Wrapper for opening the page as a standalone file. A claude.ai Artifact adds its own on
/// publish (with this same reset), so it is left out when publishing.
const STANDALONE_HEAD: &str = concat!(
    "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n",
    "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1, viewport-fit=cover\">\n",
    "<style>:root{padding-top:env(safe-area-inset-top,0px);padding-bottom:env(safe-area-inset-bottom,0px)}",
    "body{margin:0}img{max-width:100%}[hidden]{display:none!important}</style>\n</head>\n<body>\n"
);
const STANDALONE_TAIL: &str = "\n</body>\n</html>\n";

/// `past` holds the previous states, oldest first. With `artifact`, the page is produced without
/// `<!doctype>`/`<html>`/`<body>`, as Claude Code's Artifact tool expects.
pub fn build(
    name: &str,
    title: Option<&str>,
    doc: &Document,
    past: &[(Document, Step)],
    artifact: bool,
) -> Result<Export, String> {
    let k = doc.width.max(doc.height).div_ceil(MAX_DISPLAY).max(1);
    let final_png = png64(&doc.composite_downscaled(doc.bounds(), k, None))?;
    let mut used = final_png.len();

    let multi = doc.layers.len() > 1;
    let mut layers: Vec<Value> = Vec::new();
    for (i, l) in doc.layers.iter().enumerate() {
        let png = if multi {
            png64(&doc.composite_downscaled(doc.bounds(), k, Some(i)))?
        } else {
            String::new()
        };
        used += png.len();
        layers.push(json!({
            "name": l.name,
            "visible": l.visible,
            "empty": l.raster.bbox().is_none(),
            "opacity": l.opacity as f64 / 255.0,
            "blend": l.blend.name(),
            "png": png,
        }));
    }

    // History: from the newest backwards while it fits in the budget.
    // State k is the one left by step k-1 (state 0 is the start).
    let label = |k: usize| -> String {
        match k.checked_sub(1).and_then(|j| past.get(j)) {
            None => "start".to_string(),
            Some((_, st)) => {
                let who = if st.author == Author::User {
                    "you"
                } else {
                    "agent"
                };
                format!("{who}: {}", st.label)
            }
        }
    };
    let start = past.len().saturating_sub(MAX_FRAMES - 1);
    let mut frames_rev: Vec<String> = Vec::new();
    let mut labels_rev: Vec<String> = Vec::new();
    for (k, (d, _)) in past.iter().enumerate().skip(start).rev() {
        let kf = d.width.max(d.height).div_ceil(MAX_FRAME).max(1);
        let png = png64(&d.composite_downscaled(d.bounds(), kf, None))?;
        if used + png.len() > BUDGET {
            break;
        }
        used += png.len();
        frames_rev.push(png);
        labels_rev.push(label(k));
    }
    let mut frames: Vec<String> = frames_rev.into_iter().rev().collect();
    let mut labels: Vec<String> = labels_rev.into_iter().rev().collect();
    frames.push(final_png);
    labels.push(label(past.len()));

    let data = json!({
        "name": name,
        "width": doc.width,
        "height": doc.height,
        "scale": k,
        "created": now_iso(),
        "steps": past.len() + 1,
        "layers": layers,
        "frames": frames,
        "labels": labels,
    });
    let title = escape_html(title.filter(|t| !t.trim().is_empty()).unwrap_or(name));
    let json = data.to_string().replace("</", "<\\/");
    let body = TEMPLATE
        .replacen("__PIXEL_DATA__", &json, 1)
        .replace("__PIXEL_TITLE__", &title);
    let html = if artifact {
        body
    } else {
        format!("{STANDALONE_HEAD}{body}{STANDALONE_TAIL}")
    };
    Ok(Export {
        html,
        frames: frames_len(&data),
        steps: past.len() + 1,
        scale: k,
    })
}

fn frames_len(data: &Value) -> usize {
    data["frames"].as_array().map_or(0, Vec::len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pixel_core::Color;

    #[test]
    fn page_embeds_frames_and_escapes() {
        let mut d = Document::new(8, 8);
        let mut past = Vec::new();
        for i in 0..3 {
            past.push((d.clone(), Step::user(format!("edit {i}"))));
            d.layers[0].raster.set(i, i, Color::rgb(255, 0, 0).to_px());
        }
        let e = build("cat", Some("<Cat> & co"), &d, &past, true).unwrap();
        assert_eq!((e.frames, e.steps, e.scale), (4, 4, 1));
        assert!(e.html.starts_with("<title>&lt;Cat&gt; &amp; co</title>"));
        let file = build("cat", None, &d, &[], false).unwrap();
        assert!(file.html.starts_with("<!doctype html>"));
        assert!(file.html.contains("<title>cat</title>"));
        assert!(!e.html.contains("__PIXEL_"));
        assert!(e.html.contains("\"frames\":[\"iVBOR"));
        assert!(e.html.contains("\"labels\":[\"start\",\"you: edit 0\""));
        assert!(now_iso().ends_with('Z'));
    }
}
