use serde_json::json;
use protocol::json::canonicalize_json_value;

#[test]
fn canonicalize_simple_object() {
    let v = json!({"b": false, "c": 120, "a": "Hello!"});
    let bytes = canonicalize_json_value(&v).expect("canonicalize");
    let s = String::from_utf8(bytes).expect("utf8");
    assert_eq!(s, "{\"a\":\"Hello!\",\"b\":false,\"c\":120}".to_string());
}

#[test]
fn canonicalize_nested_and_array() {
    let v = json!({"z": [3,2,1], "m": {"y": "yes", "x": [ true, null ]}});
    let bytes = canonicalize_json_value(&v).expect("canonicalize");
    let s = String::from_utf8(bytes).expect("utf8");
    // Ensure predictable ordering inside nested object
    assert!(s.contains("\"m\":{"));
    assert!(s.contains("\"x\":[true,null]"));
}
