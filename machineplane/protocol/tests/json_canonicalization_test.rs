use serde_json::json;
use protocol::json::canonicalize_json_value;

#[test]
fn canonicalize_basic_object() {
    let v1 = json!({"b":1, "a":2});
    let v2 = json!({"a":2, "b":1});
    let c1 = canonicalize_json_value(&v1).expect("canonicalize v1");
    let c2 = canonicalize_json_value(&v2).expect("canonicalize v2");
    assert_eq!(c1, c2);
}

#[test]
fn canonicalize_nested_structure() {
    let v1 = json!({"z": [ {"b":2, "a":1}, {"c":3} ], "a": {"y": 5}});
    let v2 = json!({"a": {"y":5}, "z": [ {"a":1, "b":2}, {"c":3} ]});
    let c1 = canonicalize_json_value(&v1).expect("canonicalize v1");
    let c2 = canonicalize_json_value(&v2).expect("canonicalize v2");
    assert_eq!(c1, c2);
}
