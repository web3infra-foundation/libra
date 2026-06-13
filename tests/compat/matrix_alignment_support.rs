use std::{collections::BTreeSet, fs, path::PathBuf};

pub(crate) fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub(crate) fn read_repo_file(path: &str) -> String {
    let full_path = repo_root().join(path);
    fs::read_to_string(&full_path).unwrap_or_else(|error| {
        panic!("read {}: {error}", full_path.display());
    })
}

fn pascal_to_kebab(name: &str) -> String {
    let mut out = String::new();
    let mut prev_is_lower_or_digit = false;
    for ch in name.chars() {
        if ch.is_ascii_uppercase() {
            if !out.is_empty() && prev_is_lower_or_digit {
                out.push('-');
            }
            out.push(ch.to_ascii_lowercase());
            prev_is_lower_or_digit = false;
        } else {
            out.push(ch);
            prev_is_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        }
    }
    out
}

pub(crate) fn cli_commands() -> BTreeSet<String> {
    let cli_rs = read_repo_file("src/cli.rs");
    let mut in_commands = false;
    let mut commands = BTreeSet::new();
    for line in cli_rs.lines() {
        if line.trim() == "enum Commands {" {
            in_commands = true;
            continue;
        }
        if in_commands && line == "}" {
            break;
        }
        if !in_commands {
            continue;
        }
        let trimmed = line.trim_start();
        let Some(first) = trimmed.chars().next() else {
            continue;
        };
        if !first.is_ascii_uppercase() {
            continue;
        }
        let ident_end = trimmed
            .find(|ch: char| !ch.is_ascii_alphanumeric())
            .unwrap_or(trimmed.len());
        if trimmed[ident_end..].starts_with('(') {
            commands.insert(pascal_to_kebab(&trimmed[..ident_end]));
        }
    }
    commands
}

pub(crate) fn compatibility_commands() -> BTreeSet<String> {
    let compat = read_repo_file("COMPATIBILITY.md");
    let mut in_matrix = false;
    let mut commands = BTreeSet::new();
    for line in compat.lines() {
        if line == "## Top-level commands (from `src/cli.rs`)" {
            in_matrix = true;
            continue;
        }
        if line == "## Git commands intentionally absent from `src/cli.rs`" {
            in_matrix = false;
        }
        if !in_matrix || !line.starts_with('|') {
            continue;
        }
        let cols = line.split('|').collect::<Vec<_>>();
        if cols.len() < 3 {
            continue;
        }
        let command = cols[1].trim();
        if command.is_empty() || command == "Command" || command.starts_with('-') {
            continue;
        }
        commands.insert(command.to_string());
    }
    commands
}

pub(crate) fn code_router_routes(web_mod: &str) -> BTreeSet<String> {
    let router_region = web_mod
        .split("fn code_router()")
        .nth(1)
        .expect("code_router function exists")
        .split("async fn static_handler")
        .next()
        .expect("static_handler follows code routers");

    router_region
        .lines()
        .filter_map(|line| {
            let start = line.find(".route(\"")? + ".route(\"".len();
            let rest = &line[start..];
            let end = rest.find('"')?;
            Some(format!("/api/code{}", &rest[..end]))
        })
        .collect()
}

pub(crate) fn assert_contains(body: &str, needle: &str, context: &str) {
    assert!(
        body.contains(needle),
        "{context} must contain required text: {needle}"
    );
}

fn flag_values(body: &str, flag: &str) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    let mut rest = body;
    while let Some(index) = rest.find(flag) {
        rest = &rest[index + flag.len()..];
        let value = rest
            .trim_start()
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches('`');
        let first = value.chars().next();
        if first.is_some_and(|ch| ch.is_ascii_lowercase()) {
            values.insert(value.to_string());
        }
    }
    values
}

pub(crate) fn plan_test_targets() -> BTreeSet<String> {
    flag_values(
        &read_repo_file("docs/development/integration-test-plan.md"),
        "--test",
    )
}

pub(crate) fn plan_features() -> BTreeSet<String> {
    flag_values(
        &read_repo_file("docs/development/integration-test-plan.md"),
        "--features",
    )
    .into_iter()
    .flat_map(|value| {
        value
            .split(',')
            .map(str::to_string)
            .collect::<Vec<String>>()
    })
    .collect()
}

pub(crate) fn declared_cargo_targets() -> BTreeSet<String> {
    let cargo = read_repo_file("Cargo.toml");
    let mut in_test = false;
    let mut targets = BTreeSet::new();
    for line in cargo.lines() {
        let trimmed = line.trim();
        if trimmed == "[[test]]" {
            in_test = true;
            continue;
        }
        if in_test && trimmed.starts_with("name = ") {
            if let Some(name) = trimmed.split('"').nth(1) {
                targets.insert(name.to_string());
            }
            in_test = false;
        }
    }
    targets
}

pub(crate) fn declared_features() -> BTreeSet<String> {
    let cargo = read_repo_file("Cargo.toml");
    let mut in_features = false;
    let mut features = BTreeSet::new();
    for line in cargo.lines() {
        let trimmed = line.trim();
        if trimmed == "[features]" {
            in_features = true;
            continue;
        }
        if in_features && trimmed.starts_with('[') {
            break;
        }
        if in_features
            && trimmed
                .chars()
                .next()
                .is_some_and(|ch| ch.is_ascii_alphabetic())
        {
            if let Some(name) = trimmed.split('=').next() {
                features.insert(name.trim().to_string());
            }
        }
    }
    features
}

pub(crate) fn quarantine_tests() -> BTreeSet<String> {
    let path = repo_root().join("tests/flaky_quarantine.toml");
    let Ok(body) = fs::read_to_string(path) else {
        return BTreeSet::new();
    };
    body.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if !trimmed.starts_with("test") {
                return None;
            }
            trimmed.split('"').nth(1).map(str::to_string)
        })
        .collect()
}
