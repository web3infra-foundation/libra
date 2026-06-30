//! `libra fast-import` — import a `git fast-import` stream, a focused subset of
//! `git fast-import`.
//!
//! Supported directives: `blob`, `commit <ref>` (`mark`, `author`, `committer`,
//! `data`, `from`, `merge`, `M`, `D`, `deleteall`), `reset <ref>` (`from`),
//! `checkpoint`, `done`, and the lenient preamble `feature` / `option` /
//! `progress` (ignored). `tag`, `cat-blob`, `ls`, `get-mark`, `notemodify` (`N`),
//! and copy/rename (`C` / `R`) are not yet supported.
//!
//! Safety / resource bounds:
//! - Total input is capped (default 1 GiB, `fastimport.maxInputSize`).
//! - The number of blobs and commits created is capped (default 1_000_000,
//!   raise with `--max-count`); trees are derived and written through the
//!   shared `write-tree` path and are not separately counted.
//! - Refs must be `refs/…`, valid, and never point outside the repository.
//! - Object ids must match the repository hash length; duplicate marks are
//!   rejected.
//!
//! Transaction model: objects are written immediately, but ref updates are
//! buffered and only applied at a `checkpoint`, at `done`, or at a clean EOF.
//! A stream truncated mid-object fails before that flush, so refs are never
//! left half-updated; the orphaned objects are reclaimed by a later
//! `libra gc` (recover with `libra fsck` + `libra gc`).

use std::{
    collections::HashMap,
    fs,
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    str::FromStr,
};

use clap::Parser;
use git_internal::{
    hash::{ObjectHash, get_hash_kind},
    internal::object::{
        commit::Commit,
        signature::{Signature, SignatureType},
        tree::{TreeItem, TreeItemMode},
    },
};

use crate::{
    command::{load_object, save_object},
    internal::{branch::Branch, config::ConfigKv, tree_plumbing},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::OutputConfig,
        util,
    },
};

const DEFAULT_MAX_INPUT_BYTES: u64 = 1 << 30; // 1 GiB
const DEFAULT_MAX_OBJECTS: u64 = 1_000_000;

pub const FAST_IMPORT_EXAMPLES: &str = "\
EXAMPLES:
    libra fast-export main | libra fast-import   Round-trip history through a stream
    libra fast-import < repo.fastimport          Import a saved stream
    libra fast-import --input repo.fastimport    Import from a file";

/// Import a `git fast-import` stream into the repository.
#[derive(Parser, Debug)]
#[command(after_help = FAST_IMPORT_EXAMPLES)]
pub struct FastImportArgs {
    /// Read the stream from a file instead of stdin.
    #[clap(long, value_name = "FILE")]
    pub input: Option<PathBuf>,

    /// Raise the maximum number of blobs and commits a single import may create.
    #[clap(long, value_name = "N")]
    pub max_count: Option<u64>,

    /// Suppress the final summary.
    #[clap(long)]
    pub quiet: bool,
}

pub async fn execute(args: FastImportArgs) {
    if let Err(err) = execute_safe(args, &OutputConfig::default()).await {
        err.print_stderr();
        std::process::exit(err.exit_code());
    }
}

pub async fn execute_safe(args: FastImportArgs, _output: &OutputConfig) -> CliResult<()> {
    util::require_repo().map_err(|_| CliError::repo_not_found())?;

    let max_input = configured_max_input().await;
    let max_objects = args.max_count.unwrap_or(DEFAULT_MAX_OBJECTS);

    let reader: Box<dyn Read> = match &args.input {
        Some(path) => Box::new(fs::File::open(path).map_err(|error| {
            CliError::fatal(format!("cannot open '{}': {error}", path.display()))
                .with_exit_code(128)
                .with_stable_code(StableErrorCode::IoReadFailed)
        })?),
        None => Box::new(std::io::stdin()),
    };

    let mut importer = Importer::new(BufReader::new(reader), max_input, max_objects);
    let stats = importer.run().await?;

    if !args.quiet {
        println!(
            "fast-import: {} objects, {} ref(s) updated",
            stats.objects, stats.refs
        );
    }
    Ok(())
}

/// Read `fastimport.maxInputSize` (bytes), falling back to 1 GiB.
async fn configured_max_input() -> u64 {
    match ConfigKv::get("fastimport.maxInputSize")
        .await
        .ok()
        .flatten()
    {
        Some(entry) => entry
            .value
            .trim()
            .parse()
            .unwrap_or(DEFAULT_MAX_INPUT_BYTES),
        None => DEFAULT_MAX_INPUT_BYTES,
    }
}

struct ImportStats {
    objects: u64,
    refs: u64,
}

/// Streaming importer over a single fast-import stream.
struct Importer<R: BufRead> {
    reader: R,
    bytes_read: u64,
    max_input: u64,
    objects: u64,
    max_objects: u64,
    hash_hex_len: usize,
    /// `:N` mark → object id.
    marks: HashMap<u32, ObjectHash>,
    /// Ref updates staged since the last flush (committed at checkpoint/done/EOF).
    pending_refs: HashMap<String, ObjectHash>,
    /// Total refs actually written.
    refs_written: u64,
    /// One-line push-back buffer for lookahead during a `commit` body.
    pending_line: Option<Vec<u8>>,
}

impl<R: BufRead> Importer<R> {
    fn new(reader: R, max_input: u64, max_objects: u64) -> Self {
        Importer {
            reader,
            bytes_read: 0,
            max_input,
            objects: 0,
            max_objects,
            hash_hex_len: get_hash_kind().hex_len(),
            marks: HashMap::new(),
            pending_refs: HashMap::new(),
            refs_written: 0,
            pending_line: None,
        }
    }

    async fn run(&mut self) -> CliResult<ImportStats> {
        while let Some(line) = self.next_line()? {
            let line = trim_newline(&line);
            if line.is_empty() {
                continue;
            }
            let text = std::str::from_utf8(line)
                .map_err(|_| self.fatal("non-UTF-8 command line"))?
                .to_string();
            let (cmd, rest) = split_first(&text);
            match cmd {
                "blob" => self.cmd_blob()?,
                "commit" => self.cmd_commit(rest).await?,
                "reset" => self.cmd_reset(rest)?,
                "checkpoint" => self.flush_refs().await?,
                "done" => {
                    self.flush_refs().await?;
                    return Ok(self.stats());
                }
                // Lenient preamble / no-ops.
                "feature" | "option" => {}
                "progress" => eprintln!("{text}"),
                other => {
                    return Err(self.fatal(&format!("unsupported fast-import command '{other}'")));
                }
            }
        }
        // Clean EOF (the stream ended on a command boundary) commits the rest.
        self.flush_refs().await?;
        Ok(self.stats())
    }

    fn stats(&self) -> ImportStats {
        ImportStats {
            objects: self.objects,
            refs: self.refs_written,
        }
    }

    // ------------------------------------------------------------------
    // blob
    // ------------------------------------------------------------------

    fn cmd_blob(&mut self) -> CliResult<()> {
        let mark = self.read_optional_mark()?;
        let data = self.read_data_line()?;
        let blob = git_internal::internal::object::blob::Blob::from_content_bytes(data);
        self.save(&blob, blob.id)?;
        if let Some(mark) = mark {
            self.set_mark(mark, blob.id)?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // commit
    // ------------------------------------------------------------------

    async fn cmd_commit(&mut self, refspec: &str) -> CliResult<()> {
        let refname = self.validate_ref(refspec)?;

        let mark = self.read_optional_mark()?;
        let author_line = self.read_optional_prefixed("author")?;
        let committer_line = self
            .read_optional_prefixed("committer")?
            .ok_or_else(|| self.fatal("commit is missing a committer"))?;
        let message = match self.read_required_data()? {
            Some(data) => data,
            None => return Err(self.fatal("commit is missing its message data")),
        };

        let committer = parse_signature("committer", &committer_line)
            .map_err(|message| self.fatal(&message))?;
        let author = match &author_line {
            Some(line) => {
                parse_signature("author", line).map_err(|message| self.fatal(&message))?
            }
            None => {
                // Default the author to the committer, but as an `author`
                // signature — otherwise it would serialize as a second
                // `committer` line and the commit would not parse back.
                let mut author = committer.clone();
                author.signature_type = SignatureType::Author;
                author
            }
        };

        // Build the commit's tree state from its first parent, then apply ops.
        let mut parents: Vec<ObjectHash> = Vec::new();
        let mut state: HashMap<String, (TreeItemMode, ObjectHash)> = HashMap::new();

        // `from` (first parent) and `merge` (extra parents).
        while let Some(raw) = self.next_line()? {
            let line = trim_newline(&raw);
            if line.is_empty() {
                continue;
            }
            let text = std::str::from_utf8(line)
                .map_err(|_| self.fatal("non-UTF-8 commit body line"))?
                .to_string();
            let (kw, rest) = split_first(&text);
            match kw {
                "from" => {
                    let parent = self.resolve_commitish(rest)?;
                    self.load_tree_state(&parent, &mut state)?;
                    parents.clear();
                    parents.insert(0, parent);
                }
                "merge" => {
                    let parent = self.resolve_commitish(rest)?;
                    parents.push(parent);
                }
                _ => {
                    self.pending_line = Some(raw);
                    break;
                }
            }
        }

        // File changes.
        while let Some(raw) = self.next_line()? {
            let line = trim_newline(&raw);
            if line.is_empty() {
                break; // a blank line terminates the commit body
            }
            let text = std::str::from_utf8(line)
                .map_err(|_| self.fatal("non-UTF-8 commit body line"))?
                .to_string();
            let (kw, rest) = split_first(&text);
            match kw {
                "M" => self.apply_modify(rest, &mut state)?,
                "D" => apply_delete(rest, &mut state),
                "deleteall" => state.clear(),
                "blob" | "commit" | "reset" | "tag" | "checkpoint" | "done" | "feature"
                | "option" | "progress" => {
                    // Next top-level command — hand it back to the main loop.
                    self.pending_line = Some(raw);
                    break;
                }
                other => {
                    return Err(self.fatal(&format!(
                        "unsupported file-change command '{other}' in commit"
                    )));
                }
            }
        }

        let tree_id = tree_plumbing::write_tree_from_leaves(
            state
                .into_iter()
                .map(|(path, (mode, id))| (PathBuf::from(path), mode, id)),
        )
        .map_err(|error| self.fatal(&format!("failed to write tree: {error}")))?;

        let commit = Commit::new(
            author,
            committer,
            tree_id,
            parents,
            &message_string(&message),
        );
        self.save(&commit, commit.id)?;
        if let Some(mark) = mark {
            self.set_mark(mark, commit.id)?;
        }
        self.pending_refs.insert(refname, commit.id);
        Ok(())
    }

    fn apply_modify(
        &mut self,
        rest: &str,
        state: &mut HashMap<String, (TreeItemMode, ObjectHash)>,
    ) -> CliResult<()> {
        // `M <mode> <dataref> <path>`
        let mut parts = rest.splitn(3, ' ');
        let mode = parts.next().unwrap_or_default();
        let dataref = parts
            .next()
            .ok_or_else(|| self.fatal("malformed `M` (expected `M <mode> <dataref> <path>`)"))?;
        let path = parts
            .next()
            .ok_or_else(|| self.fatal("malformed `M` (missing path)"))?;
        let mode = TreeItemMode::tree_item_type_from_bytes(mode.as_bytes())
            .map_err(|_| self.fatal(&format!("invalid file mode '{mode}'")))?;
        let id = self.resolve_dataref(dataref)?;
        state.insert(unquote_path(path), (mode, id));
        Ok(())
    }

    // ------------------------------------------------------------------
    // reset
    // ------------------------------------------------------------------

    fn cmd_reset(&mut self, refspec: &str) -> CliResult<()> {
        let refname = self.validate_ref(refspec)?;
        // An optional `from` line sets the target; otherwise the ref is staged
        // for the next commit (a no-op for this MVP).
        if let Some(raw) = self.next_line()? {
            let line = trim_newline(&raw);
            let text = std::str::from_utf8(line)
                .map_err(|_| self.fatal("non-UTF-8 reset body"))?
                .to_string();
            let (kw, rest) = split_first(&text);
            if kw == "from" {
                let target = self.resolve_commitish(rest)?;
                self.pending_refs.insert(refname, target);
            } else if !line.is_empty() {
                self.pending_line = Some(raw);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // transaction
    // ------------------------------------------------------------------

    async fn flush_refs(&mut self) -> CliResult<()> {
        let pending = std::mem::take(&mut self.pending_refs);
        for (refname, oid) in pending {
            let Some(branch) = refname.strip_prefix("refs/heads/") else {
                // Only branch refs are persisted; other namespaces are accepted
                // in the stream but not yet written (documented).
                continue;
            };
            Branch::update_branch(branch, &oid.to_string(), None)
                .await
                .map_err(|error| self.fatal(&format!("failed to update {refname}: {error}")))?;
            self.refs_written += 1;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // primitives
    // ------------------------------------------------------------------

    fn save<T: git_internal::internal::object::ObjectTrait>(
        &mut self,
        object: &T,
        id: ObjectHash,
    ) -> CliResult<()> {
        self.objects += 1;
        if self.objects > self.max_objects {
            return Err(self.fatal(&format!(
                "object limit exceeded ({}); raise with --max-count",
                self.max_objects
            )));
        }
        save_object(object, &id)
            .map_err(|error| self.fatal(&format!("failed to write object: {error}")))
    }

    fn set_mark(&mut self, mark: u32, id: ObjectHash) -> CliResult<()> {
        if self.marks.insert(mark, id).is_some() {
            return Err(self.fatal(&format!("duplicate mark :{mark}")));
        }
        Ok(())
    }

    /// Resolve a `from`/`merge` target: `:mark`, a literal commit oid, or a ref.
    fn resolve_commitish(&mut self, spec: &str) -> CliResult<ObjectHash> {
        self.resolve_dataref(spec)
    }

    /// Resolve a `:mark` or literal object id.
    fn resolve_dataref(&mut self, spec: &str) -> CliResult<ObjectHash> {
        let spec = spec.trim();
        if let Some(mark) = spec.strip_prefix(':') {
            let mark: u32 = mark
                .parse()
                .map_err(|_| self.fatal(&format!("invalid mark reference '{spec}'")))?;
            return self
                .marks
                .get(&mark)
                .copied()
                .ok_or_else(|| self.fatal(&format!("undefined mark :{mark}")));
        }
        // Literal object id — must match the repository hash length.
        if spec.len() != self.hash_hex_len || !spec.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(self.fatal(&format!(
                "object id '{spec}' does not match the repository hash format"
            )));
        }
        ObjectHash::from_str(spec).map_err(|_| self.fatal(&format!("invalid object id '{spec}'")))
    }

    /// Load a commit's tree into the flat path → (mode, id) state map.
    fn load_tree_state(
        &mut self,
        commit_id: &ObjectHash,
        state: &mut HashMap<String, (TreeItemMode, ObjectHash)>,
    ) -> CliResult<()> {
        state.clear();
        let commit: Commit = load_object(commit_id)
            .map_err(|error| self.fatal(&format!("cannot read commit {commit_id}: {error}")))?;
        flatten_tree(&commit.tree_id, "", state).map_err(|error| self.fatal(&error))
    }

    /// Validate a ref target: must be `refs/…`, well-formed, and never escape.
    fn validate_ref(&self, refspec: &str) -> CliResult<String> {
        let refname = refspec.trim();
        if refname.is_empty() {
            return Err(self.fatal("missing ref name"));
        }
        if !refname.starts_with("refs/") {
            return Err(self.fatal(&format!(
                "ref '{refname}' is outside the repository (must be refs/…)"
            )));
        }
        if !util::is_valid_refname(refname) {
            return Err(self.fatal(&format!("invalid ref name '{refname}'")));
        }
        Ok(refname.to_string())
    }

    // ------------------------------------------------------------------
    // line / data reading (resource-bounded)
    // ------------------------------------------------------------------

    fn next_line(&mut self) -> CliResult<Option<Vec<u8>>> {
        if let Some(line) = self.pending_line.take() {
            return Ok(Some(line));
        }
        let mut buf = Vec::new();
        let n = self
            .reader
            .read_until(b'\n', &mut buf)
            .map_err(|error| self.fatal(&format!("read error: {error}")))?;
        if n == 0 {
            return Ok(None);
        }
        self.account(n as u64)?;
        Ok(Some(buf))
    }

    /// Read a command line that must begin with `data`, returning the payload.
    fn read_data_line(&mut self) -> CliResult<Vec<u8>> {
        match self.read_required_data()? {
            Some(data) => Ok(data),
            None => Err(self.fatal("expected a `data` directive")),
        }
    }

    /// Read the next line, requiring it to be a `data` directive.
    fn read_required_data(&mut self) -> CliResult<Option<Vec<u8>>> {
        let Some(raw) = self.next_line()? else {
            return Ok(None);
        };
        let line = trim_newline(&raw);
        let text = std::str::from_utf8(line)
            .map_err(|_| self.fatal("non-UTF-8 data header"))?
            .to_string();
        let (kw, rest) = split_first(&text);
        if kw != "data" {
            return Err(self.fatal(&format!("expected `data`, found `{kw}`")));
        }
        Ok(Some(self.read_data_payload(rest)?))
    }

    fn read_data_payload(&mut self, spec: &str) -> CliResult<Vec<u8>> {
        let spec = spec.trim();
        if let Some(delim) = spec.strip_prefix("<<") {
            // Delimited (here-doc) form: lines until one equals the delimiter.
            let delim = delim.to_string();
            let mut payload = Vec::new();
            loop {
                let Some(raw) = self.next_line()? else {
                    return Err(self.fatal("unterminated delimited `data`"));
                };
                let line = trim_newline(&raw);
                if line == delim.as_bytes() {
                    break;
                }
                payload.extend_from_slice(line);
                payload.push(b'\n');
            }
            Ok(payload)
        } else {
            let n: usize = spec
                .parse()
                .map_err(|_| self.fatal(&format!("invalid data length '{spec}'")))?;
            self.account(n as u64)?;
            let mut payload = vec![0u8; n];
            self.reader
                .read_exact(&mut payload)
                .map_err(|error| self.fatal(&format!("truncated data payload: {error}")))?;
            // Consume the optional trailing LF so it does not leak as a blank line.
            let next = match self.reader.fill_buf() {
                Ok(buf) => buf.first().copied(),
                Err(error) => return Err(self.fatal(&format!("read error: {error}"))),
            };
            if next == Some(b'\n') {
                self.reader.consume(1);
                self.account(1)?; // count the optional LF against the input cap
            }
            Ok(payload)
        }
    }

    fn read_optional_mark(&mut self) -> CliResult<Option<u32>> {
        match self.read_optional_prefixed("mark")? {
            Some(rest) => {
                let mark = rest.trim();
                let mark = mark.strip_prefix(':').unwrap_or(mark);
                let mark: u32 = mark
                    .parse()
                    .map_err(|_| self.fatal(&format!("invalid mark ':{mark}'")))?;
                Ok(Some(mark))
            }
            None => Ok(None),
        }
    }

    /// If the next line starts with `<keyword> `, consume it and return the
    /// remainder; otherwise push it back and return `None`.
    fn read_optional_prefixed(&mut self, keyword: &str) -> CliResult<Option<String>> {
        let Some(raw) = self.next_line()? else {
            return Ok(None);
        };
        let line = trim_newline(&raw);
        let text = std::str::from_utf8(line)
            .map_err(|_| self.fatal("non-UTF-8 header line"))?
            .to_string();
        let (kw, rest) = split_first(&text);
        if kw == keyword {
            Ok(Some(rest.to_string()))
        } else {
            self.pending_line = Some(raw);
            Ok(None)
        }
    }

    fn account(&mut self, n: u64) -> CliResult<()> {
        self.bytes_read = self.bytes_read.saturating_add(n);
        if self.bytes_read > self.max_input {
            return Err(self.fatal(&format!(
                "input exceeds the {}-byte limit (set fastimport.maxInputSize)",
                self.max_input
            )));
        }
        Ok(())
    }

    fn fatal(&self, message: &str) -> CliError {
        CliError::fatal(format!("fast-import: {message}"))
            .with_exit_code(128)
            .with_stable_code(StableErrorCode::CliInvalidArguments)
    }
}

// ----------------------------------------------------------------------------
// free helpers
// ----------------------------------------------------------------------------

fn trim_newline(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    if end > 0 && line[end - 1] == b'\n' {
        end -= 1;
    }
    if end > 0 && line[end - 1] == b'\r' {
        end -= 1;
    }
    &line[..end]
}

fn split_first(text: &str) -> (&str, &str) {
    match text.split_once(' ') {
        Some((head, rest)) => (head, rest),
        None => (text, ""),
    }
}

fn message_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// `D <path>` removes the path and, if it names a directory, everything beneath it.
fn apply_delete(rest: &str, state: &mut HashMap<String, (TreeItemMode, ObjectHash)>) {
    let path = unquote_path(rest.trim());
    let prefix = format!("{path}/");
    state.retain(|key, _| key != &path && !key.starts_with(&prefix));
}

/// fast-import quotes paths containing unusual characters with surrounding
/// double quotes; for this subset we just strip them.
fn unquote_path(path: &str) -> String {
    let trimmed = path.trim();
    if trimmed.len() >= 2 && trimmed.starts_with('"') && trimmed.ends_with('"') {
        trimmed[1..trimmed.len() - 1].to_string()
    } else {
        trimmed.to_string()
    }
}

/// Recursively flatten a tree into `path → (mode, id)` leaves (gitlinks kept as
/// `TreeItemMode::Commit`).
fn flatten_tree(
    tree_id: &ObjectHash,
    prefix: &str,
    out: &mut HashMap<String, (TreeItemMode, ObjectHash)>,
) -> Result<(), String> {
    let tree: git_internal::internal::object::tree::Tree =
        load_object(tree_id).map_err(|error| format!("cannot read tree {tree_id}: {error}"))?;
    for TreeItem { mode, id, name } in tree.tree_items {
        let path = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if mode == TreeItemMode::Tree {
            flatten_tree(&id, &path, out)?;
        } else {
            out.insert(path, (mode, id));
        }
    }
    Ok(())
}

/// Parse a fast-import identity line (`<name> <email> <when>`) into a signature
/// via the object layer, which already understands the `name <email> ts tz` form.
fn parse_signature(kind: &str, line: &str) -> Result<Signature, String> {
    Signature::from_data(format!("{kind} {line}").into_bytes())
        .map_err(|error| format!("invalid {kind} '{line}': {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_crlf_and_lf() {
        assert_eq!(trim_newline(b"abc\r\n"), b"abc");
        assert_eq!(trim_newline(b"abc\n"), b"abc");
        assert_eq!(trim_newline(b"abc"), b"abc");
    }

    #[test]
    fn split_first_separates_command_and_rest() {
        assert_eq!(
            split_first("commit refs/heads/main"),
            ("commit", "refs/heads/main")
        );
        assert_eq!(split_first("done"), ("done", ""));
    }

    #[test]
    fn unquote_strips_surrounding_quotes() {
        assert_eq!(unquote_path("\"a b\""), "a b");
        assert_eq!(unquote_path("plain"), "plain");
    }

    #[test]
    fn delete_removes_path_and_subtree() {
        let mut state: HashMap<String, (TreeItemMode, ObjectHash)> = HashMap::new();
        let id = ObjectHash::default();
        state.insert("dir/a".into(), (TreeItemMode::Blob, id));
        state.insert("dir/sub/b".into(), (TreeItemMode::Blob, id));
        state.insert("keep".into(), (TreeItemMode::Blob, id));
        apply_delete("dir", &mut state);
        assert_eq!(state.len(), 1);
        assert!(state.contains_key("keep"));
    }
}
