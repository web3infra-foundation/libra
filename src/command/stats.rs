use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// 忽略的目录名
const IGNORE_DIRS: [&str; 2] = [".libra", "target"];

/// 判断路径是否应该被忽略
fn should_ignore(path: &Path) -> bool {
    path.components()
        .any(|comp| IGNORE_DIRS.contains(&comp.as_os_str().to_str().unwrap_or("")))
}

/// 获取文件扩展名，若无扩展名则返回 "no_extension"
fn get_extension_name(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "no_extension".to_string())
}

/// 执行 stats 命令
pub fn execute(output_json: bool) -> Result<()> {
    let mut counts: HashMap<String, usize> = HashMap::new();

    // 遍历当前目录
    for entry in fs::read_dir(".")? {
        let entry = entry?;
        let path = entry.path();

        // 跳过目录，只统计文件
        if !path.is_file() {
            continue;
        }

        // 跳过忽略的目录中的文件
        if should_ignore(&path) {
            continue;
        }

        let ext = get_extension_name(&path);
        *counts.entry(ext).or_insert(0) += 1;
    }

    // 输出结果
    if output_json {
        println!("{}", serde_json::to_string_pretty(&counts)?);
    } else {
        println!("File type statistics:");
        let mut entries: Vec<_> = counts.iter().collect();
        entries.sort_by_key(|(_, count)| std::cmp::Reverse(*count));

        for (ext, count) in entries {
            println!("  {}: {}", ext, count);
        }

        let total: usize = counts.values().sum();
        println!("\nTotal files: {}", total);
    }

    Ok(())
}
