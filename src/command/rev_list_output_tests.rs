use super::{
    rev_list_output::{RevListEntry, format_rev_list_entry},
    rev_list_spec::RevListSide,
};

#[test]
fn test_format_rev_list_entry_matches_git_field_order() {
    let entry = RevListEntry {
        commit: "abc123".to_string(),
        side: Some(RevListSide::Left),
        cherry_equivalent: Some(false),
        parents: vec!["def456".to_string(), "789abc".to_string()],
        timestamp: Some(123),
    };

    assert_eq!(
        format_rev_list_entry(&entry, true, true, false, false),
        "123 abc123 def456 789abc"
    );
    assert_eq!(
        format_rev_list_entry(&entry, true, false, false, false),
        "abc123 def456 789abc"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, true, false, false),
        "123 abc123"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, false, true, false),
        "<abc123"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, false, true, true),
        "+abc123"
    );
}
