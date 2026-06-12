/// Libra Compatibility Status View Generator
///
/// This tool reads the compatibility-matrix.yaml and generates a human-readable
/// status report showing progress across phases and priority levels.
///
/// Usage: cargo run --manifest-path tools/compat-status-view/Cargo.toml -- <matrix-path>

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let args: Vec<String> = env::args().collect();
    let matrix_path = if args.len() > 1 {
        args[1].clone()
    } else {
        "docs/development/compatibility-matrix.yaml".to_string()
    };

    if !Path::new(&matrix_path).exists() {
        eprintln!("Error: File not found: {}", matrix_path);
        std::process::exit(1);
    }

    let content = fs::read_to_string(&matrix_path)
        .expect("Failed to read matrix file");

    // Simple YAML parsing for our specific structure
    generate_status_view(&content);
}

#[derive(Debug, Default, Clone)]
struct MatrixEntry {
    command: String,
    flag: String,
    action: String,
    priority: String,
    phase: String,
    status: String,
    declined_ref: String,
    risk: String,
}

fn generate_status_view(content: &str) {
    let mut entries = Vec::new();
    let mut in_entries = false;

    for line in content.lines() {
        if line.contains("entries:") {
            in_entries = true;
            continue;
        }

        if !in_entries {
            continue;
        }

        // Skip non-entry lines
        if !line.trim().starts_with("- command:") && !line.trim().starts_with("command:") {
            continue;
        }

        let mut entry = MatrixEntry::default();

        // Simple line-by-line parsing
        let lines_iter = content[content.find(line).unwrap()..].lines();
        for entry_line in lines_iter.take(30) {
            if entry_line.trim().is_empty() {
                continue;
            }
            if entry_line.trim().starts_with("- command:") || (entry_line.starts_with("  - command:")) {
                if !entry.command.is_empty() {
                    entries.push(entry.clone());
                }
                entry = MatrixEntry::default();
            }

            parse_field(entry_line, &mut entry);

            // Stop at next entry marker
            if entry_line.trim().starts_with("- command:") && !entry.command.is_empty() {
                break;
            }
        }

        if !entry.command.is_empty() {
            entries.push(entry);
        }
    }

    // Group by phase and priority
    let mut by_phase: BTreeMap<String, Vec<MatrixEntry>> = BTreeMap::new();
    for entry in &entries {
        by_phase
            .entry(entry.phase.clone())
            .or_default()
            .push(entry.clone());
    }

    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║           Libra Compatibility Status View - Phase 0             ║");
    println!("║                 Generated: {:<42}║", chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string());
    println!("╚════════════════════════════════════════════════════════════════╝\n");

    // Summary statistics
    let total = entries.len();
    let done = entries.iter().filter(|e| e.status == "done").count();
    let planned = entries.iter().filter(|e| e.status == "planned").count();
    let in_progress = entries.iter().filter(|e| e.status == "in-progress").count();
    let blocked = entries.iter().filter(|e| e.status == "blocked").count();

    println!("SUMMARY");
    println!("{}─ Total entries: {}", "─".repeat(18), total);
    println!("{}─ Done: {} ({:.1}%)", "─".repeat(22), done, (done as f64 / total as f64) * 100.0);
    println!("{}─ Planned: {}", "─".repeat(20), planned);
    println!("{}─ In Progress: {}", "─".repeat(16), in_progress);
    println!("{}─ Blocked: {}", "─".repeat(20), blocked);
    println!();

    // By priority
    println!("BY PRIORITY");
    let mut by_priority: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in &entries {
        *by_priority.entry(&entry.priority).or_default() += 1;
    }
    for (prio, count) in by_priority {
        println!("{}─ {}: {}", "─".repeat(18), prio, count);
    }
    println!();

    // By phase
    println!("BY PHASE");
    for (phase, phase_entries) in &by_phase {
        let done_in_phase = phase_entries.iter().filter(|e| e.status == "done").count();
        let total_in_phase = phase_entries.len();
        println!(
            "{}─ Phase {}: {} / {} done ({:.1}%)",
            "─".repeat(12),
            phase,
            done_in_phase,
            total_in_phase,
            (done_in_phase as f64 / total_in_phase as f64) * 100.0
        );
    }
    println!();

    // Risk summary
    println!("RISK DISTRIBUTION");
    let mut by_risk: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in &entries {
        *by_risk.entry(&entry.risk).or_default() += 1;
    }
    for (risk, count) in by_risk {
        println!("{}─ {}: {}", "─".repeat(18), risk, count);
    }
    println!();

    // Entries with missing test_evidence
    println!("ENTRIES WITH POTENTIAL GAPS");
    let missing_evidence = entries.iter().filter(|e| e.status == "done" && e.declined_ref.is_empty()).count();
    let high_risk_without_controls = entries.iter().filter(|e| e.risk == "high").count();
    println!("{}─ Done entries without declined_ref: {}", "─".repeat(7), missing_evidence);
    println!("{}─ High-risk entries: {}", "─".repeat(20), high_risk_without_controls);
    println!();

    // Declined references summary
    println!("DECLINED REFERENCES");
    let mut declined_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for entry in &entries {
        if !entry.declined_ref.is_empty() {
            *declined_counts.entry(&entry.declined_ref).or_default() += 1;
        }
    }
    for (declined_ref, count) in declined_counts {
        println!("{}─ {} : {} entries", "─".repeat(18), declined_ref, count);
    }
    if declined_counts.is_empty() {
        println!("{}─ (none)", "─".repeat(24));
    }
    println!();

    println!("DETAILED PHASE BREAKDOWN");
    for (phase, phase_entries) in &by_phase {
        println!("\n  Phase {}:", phase);
        let mut by_status: BTreeMap<&str, Vec<&MatrixEntry>> = BTreeMap::new();
        for entry in phase_entries {
            by_status.entry(&entry.status).or_default().push(entry);
        }
        for (status, status_entries) in by_status {
            println!("    {}: {} entries", status, status_entries.len());
        }
    }
    println!();

    println!("For detailed matrix view, see: docs/development/compatibility-matrix.yaml");
    println!("For decline registry, see: docs/improvement/compatibility/declined.md");
}

fn parse_field(line: &str, entry: &mut MatrixEntry) {
    let line = line.trim();

    if let Some(val) = extract_field("command:", line) {
        entry.command = val;
    } else if let Some(val) = extract_field("flag:", line) {
        entry.flag = val;
    } else if let Some(val) = extract_field("action:", line) {
        entry.action = val;
    } else if let Some(val) = extract_field("priority:", line) {
        entry.priority = val;
    } else if let Some(val) = extract_field("phase:", line) {
        entry.phase = val;
    } else if let Some(val) = extract_field("status:", line) {
        entry.status = val;
    } else if let Some(val) = extract_field("declined_ref:", line) {
        entry.declined_ref = val;
    } else if let Some(val) = extract_field("risk:", line) {
        entry.risk = val;
    }
}

fn extract_field(key: &str, line: &str) -> Option<String> {
    if line.starts_with(key) {
        let val = line[key.len()..].trim();
        // Remove quotes if present
        let val = if val.starts_with('"') && val.ends_with('"') {
            &val[1..val.len() - 1]
        } else {
            val
        };
        return Some(val.to_string());
    }
    None
}
