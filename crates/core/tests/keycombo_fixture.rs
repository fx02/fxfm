use fxfm_core::keymap::KeyCombo;

#[test]
fn fixture_combos_roundtrip() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/keycombos.json"
    ))
    .unwrap();
    let cases: Vec<serde_json::Value> = serde_json::from_str(&text).unwrap();
    assert!(cases.len() >= 20);
    for case in cases {
        let combo = case["combo"].as_str().unwrap();
        let parsed: KeyCombo = combo.parse().unwrap_or_else(|e| panic!("{combo}: {e}"));
        assert_eq!(parsed.to_string(), combo);
    }
}
