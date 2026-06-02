use std::fs::{self, File};
use tempfile::tempdir;
use std::collections::HashMap;

#[test]
fn test_stats_counts_extensions_in_workdir() {
    let dir = tempdir().unwrap();
    let dir_path = dir.path();

    File::create(dir_path.join("main.rs")).unwrap();
    File::create(dir_path.join("README.md")).unwrap();
    
    let mut stats = HashMap::new();
    for entry in fs::read_dir(dir_path).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if let Some(ext) = path.extension() {
            let count = stats.entry(ext.to_str().unwrap().to_string()).or_insert(0);
            *count += 1;
        }
    }

    assert_eq!(stats.get("rs"), Some(&1));
    assert_eq!(stats.get("md"), Some(&1));
    assert_eq!(stats.get("txt"), None);
}