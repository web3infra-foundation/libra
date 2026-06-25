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
        children: vec!["child1".to_string(), "child2".to_string()],
        timestamp: Some(123),
        boundary: false,
    };

    assert_eq!(
        format_rev_list_entry(&entry, true, false, true, false, false, false),
        "123 abc123 def456 789abc"
    );
    assert_eq!(
        format_rev_list_entry(&entry, true, false, false, false, false, false),
        "abc123 def456 789abc"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, true, true, false, false, false),
        "123 abc123 child1 child2"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, false, true, false, false, false),
        "123 abc123"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, false, false, true, false, false),
        "<abc123"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, false, false, true, true, false),
        "+abc123"
    );
    assert_eq!(
        format_rev_list_entry(&entry, false, false, false, false, false, true),
        "+abc123"
    );

    let right = RevListEntry {
        commit: "fed321".to_string(),
        side: Some(RevListSide::Right),
        cherry_equivalent: Some(false),
        parents: Vec::new(),
        children: Vec::new(),
        timestamp: None,
        boundary: false,
    };
    assert_eq!(
        format_rev_list_entry(&right, false, false, false, true, false, true),
        ">fed321"
    );

    // Boundary commits are marked `-` and never carry side/cherry markers, but still
    // surface `--timestamp`/`--parents`/`--children` metadata through the same path.
    let boundary = RevListEntry {
        commit: "b0undary".to_string(),
        side: Some(RevListSide::Left),
        cherry_equivalent: Some(true),
        parents: vec!["par111".to_string()],
        children: vec!["chi222".to_string()],
        timestamp: Some(456),
        boundary: true,
    };
    assert_eq!(
        format_rev_list_entry(&boundary, true, false, true, true, true, false),
        "456 -b0undary par111",
        "boundary entry: leading `-`, no side/cherry marker, timestamp + parents preserved"
    );
    assert_eq!(
        format_rev_list_entry(&boundary, false, true, false, false, false, false),
        "-b0undary chi222",
        "boundary entry with --children"
    );
}
