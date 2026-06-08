use std::{collections::HashMap, fs, path::Path};

const MAILMAP_FILE: &str = ".mailmap";
const MAX_MAILMAP_LINE_BYTES: usize = 256;

#[derive(Debug, Default)]
pub(super) struct Mailmap {
    by_email: HashMap<String, Identity>,
    by_name_email: HashMap<(String, String), Identity>,
}

#[derive(Debug)]
pub(super) struct MailmapLoad {
    pub mailmap: Mailmap,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct Identity {
    name: Option<String>,
    email: Option<String>,
}

struct MailmapEntry {
    proper_name: Option<String>,
    proper_email: Option<String>,
    commit_name: Option<String>,
    commit_email: String,
}

impl Mailmap {
    pub(super) fn resolve(&self, name: &str, email: &str) -> (String, String) {
        let email_key = normalize_email_key(email);
        let exact_key = (name.trim().to_string(), email_key.clone());
        let identity = self
            .by_name_email
            .get(&exact_key)
            .or_else(|| self.by_email.get(&email_key));

        match identity {
            Some(identity) => (
                identity
                    .name
                    .clone()
                    .unwrap_or_else(|| name.trim().to_string()),
                identity
                    .email
                    .clone()
                    .unwrap_or_else(|| email.trim().to_string()),
            ),
            None => (name.trim().to_string(), email.trim().to_string()),
        }
    }

    fn insert(&mut self, entry: MailmapEntry) {
        let identity = Identity {
            name: entry.proper_name,
            email: entry.proper_email,
        };
        let email_key = normalize_email_key(&entry.commit_email);
        self.by_email.insert(email_key.clone(), identity.clone());

        if let Some(commit_name) = entry.commit_name {
            self.by_name_email
                .insert((commit_name.trim().to_string(), email_key), identity);
        }
    }
}

pub(super) fn load_mailmap(workdir: &Path) -> MailmapLoad {
    let path = workdir.join(MAILMAP_FILE);
    let mut warnings = Vec::new();
    let mut mailmap = Mailmap::default();

    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return MailmapLoad { mailmap, warnings };
        }
        Err(error) => {
            warnings.push(format!(
                "failed to inspect '{}': {error}; ignoring mailmap",
                display_path(&path)
            ));
            return MailmapLoad { mailmap, warnings };
        }
    };

    if metadata.file_type().is_symlink() {
        warnings.push(format!(
            "ignoring symbolic .mailmap '{}'",
            display_path(&path)
        ));
        return MailmapLoad { mailmap, warnings };
    }

    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            warnings.push(format!(
                "failed to read '{}': {error}; ignoring mailmap",
                display_path(&path)
            ));
            return MailmapLoad { mailmap, warnings };
        }
    };

    for (index, line) in text.lines().enumerate() {
        let line_no = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.len() > MAX_MAILMAP_LINE_BYTES {
            warnings.push(format!(
                ".mailmap line {line_no} exceeds {MAX_MAILMAP_LINE_BYTES} bytes and was skipped"
            ));
            continue;
        }

        match parse_mailmap_line(trimmed) {
            Some(entry) => mailmap.insert(entry),
            None => warnings.push(format!(
                ".mailmap line {line_no} is malformed and was skipped"
            )),
        }
    }

    MailmapLoad { mailmap, warnings }
}

fn parse_mailmap_line(line: &str) -> Option<MailmapEntry> {
    let spans = email_spans(line)?;
    match spans.as_slice() {
        [first, second] => parse_two_email_line(line, first, second),
        [only] => parse_one_email_line(line, only),
        _ => None,
    }
}

#[derive(Debug)]
struct EmailSpan {
    start: usize,
    end: usize,
    email: String,
}

fn email_spans(line: &str) -> Option<Vec<EmailSpan>> {
    let mut spans = Vec::new();
    let mut offset = 0;
    while let Some(relative_start) = line[offset..].find('<') {
        let start = offset + relative_start;
        let after_start = start + 1;
        let relative_end = line[after_start..].find('>')?;
        let end = after_start + relative_end;
        let email = line[after_start..end].trim();
        if email.is_empty() {
            return None;
        }
        spans.push(EmailSpan {
            start,
            end,
            email: email.to_string(),
        });
        offset = end + 1;
    }

    if spans.is_empty() || line[offset..].contains('>') {
        return None;
    }
    Some(spans)
}

fn parse_two_email_line(line: &str, first: &EmailSpan, second: &EmailSpan) -> Option<MailmapEntry> {
    if !line[second.end + 1..].trim().is_empty() {
        return None;
    }

    let proper_name = non_empty(line[..first.start].trim());
    let commit_name = non_empty(line[first.end + 1..second.start].trim());
    Some(MailmapEntry {
        proper_name,
        proper_email: Some(first.email.clone()),
        commit_name,
        commit_email: second.email.clone(),
    })
}

fn parse_one_email_line(line: &str, span: &EmailSpan) -> Option<MailmapEntry> {
    if !line[span.end + 1..].trim().is_empty() {
        return None;
    }

    let proper_name = non_empty(line[..span.start].trim())?;
    Some(MailmapEntry {
        proper_name: Some(proper_name),
        proper_email: None,
        commit_name: None,
        commit_email: span.email.clone(),
    })
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn normalize_email_key(email: &str) -> String {
    email.trim().to_ascii_lowercase()
}

fn display_path(path: &Path) -> String {
    path.display().to_string()
}
