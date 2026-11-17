// Load every clip of animation files and report why clips are rejected.
// usage: validate_animations file.animation.json [more files...]
fn main() {
    let mut loaded = 0usize;
    let mut reasons = std::collections::BTreeMap::<String, (usize, String)>::new();
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read animation file");
        let text = String::from_utf8_lossy(&bytes);
        let doc: serde_json::Value =
            serde_json::from_str(text.trim_start_matches('\u{feff}')).expect("parse animation file");
        let Some(clips) = doc["animations"].as_object() else {
            continue;
        };
        for name in clips.keys() {
            match openysm_runtime::Clip::load(text.trim_start_matches('\u{feff}').as_bytes(), name) {
                Ok(_) => loaded += 1,
                Err(error) => {
                    // Group "unsupported Molang at byte 12" style errors by kind.
                    let kind: String = error
                        .split(|c: char| c.is_ascii_digit())
                        .next()
                        .unwrap_or(&error)
                        .trim()
                        .into();
                    let entry = reasons.entry(kind).or_insert((0, format!("{path} :: {name}")));
                    entry.0 += 1;
                }
            }
        }
    }
    let failed: usize = reasons.values().map(|r| r.0).sum();
    println!("{loaded} clips load, {failed} rejected");
    for (reason, (count, example)) in &reasons {
        println!("{count:5}  {reason}  (e.g. {example})");
    }
}
