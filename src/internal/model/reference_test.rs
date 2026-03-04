use serde_json;

use super::reference::ConfigKind;

#[test]
fn test_config_kind_serialization() {
    let kind = ConfigKind::Branch;
    let serialized = serde_json::to_string(&kind).unwrap();
    // It should be serialized as "Branch" string, not integer
    assert_eq!(serialized, "\"Branch\"");

    let deserialized: ConfigKind = serde_json::from_str(&serialized).unwrap();
    assert_eq!(deserialized, ConfigKind::Branch);

    let kind = ConfigKind::Head;
    let serialized = serde_json::to_string(&kind).unwrap();
    assert_eq!(serialized, "\"Head\"");
}
