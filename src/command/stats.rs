use crate::cli::StatsArgs;
use crate::utils::output::OutputConfig;
use crate::utils::error::CliError;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub async fn execute_safe(args: StatsArgs, _output: &OutputConfig) -> Result<(), CliError> {
    let mut stats = HashMap::new();
    visit_dirs(Path::new("."), &mut stats).map_err(|e| CliError::fatal(e.to_string()))?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&stats).unwrap_or_default());
    } else {
        println!("{:<15} | {}", "扩展名", "文件数");
        println!("----------------------------");
        for (ext, count) in &stats {
            println!("{:<15} | {}", ext, count);
        }
    }
    Ok(())
}

fn visit_dirs(dir: &Path, stats: &mut HashMap<String, usize>) -> std::io::Result<()> {
    if !dir.is_dir() { return Ok(()); }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if ["target", ".libra", ".git"].contains(&name) { continue; }
        }
        if path.is_dir() { visit_dirs(&path, stats)?; }
        else if path.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("no_extension");
            *stats.entry(ext.to_string()).or_insert(0) += 1;
        }
    }
    Ok(())
}