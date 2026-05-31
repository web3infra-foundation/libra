//! CLI handler for `libra notes` — add, show, list, or remove notes attached to commits.

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    internal::notes::{self, DEFAULT_NOTES_REF},
    utils::{
        error::{CliError, CliResult, StableErrorCode},
        output::{OutputConfig, emit_json_data},
        text::short_display_hash,
    },
};

const NOTES_EXAMPLES: &str = "\
EXAMPLES:
    libra notes add -m \"Reviewed-by: Alice\"         Add a note to HEAD
    libra notes show                                  Show the note on HEAD
    libra notes list                                  List all notes
    libra notes remove abc1234                        Remove a note
    libra notes add -f -m \"Updated\" HEAD            Force-overwrite a note";

#[derive(Parser, Debug)]
#[command(about = "Add, show, list, or remove notes attached to commits")]
#[command(after_help = NOTES_EXAMPLES)]
pub struct NotesArgs {
    #[command(subcommand)]
    pub subcommand: Option<NotesSubcommand>,

    /// Operate on a specific notes ref (default: refs/notes/commits)
    #[clap(long, default_value = DEFAULT_NOTES_REF)]
    pub ref_: String,
}

#[derive(Subcommand, Debug)]
pub enum NotesSubcommand {
    /// Add a note to an object (defaults to HEAD)
    Add {
        /// Object to annotate (defaults to HEAD)
        #[clap(required = false)]
        object: Option<String>,

        /// Note message text (repeatable; blank lines separate messages)
        #[clap(short, long)]
        message: Vec<String>,

        /// Read note message from file (- for stdin)
        #[clap(short = 'F', long)]
        file: Vec<String>,

        /// Overwrite an existing note
        #[clap(short, long)]
        force: bool,
    },
    /// List note objects and the commits they annotate
    List {
        /// Object to list notes for (omit to list all)
        #[clap(required = false)]
        object: Option<String>,
    },
    /// Show the note text for an object
    Show {
        /// Object to show the note for (defaults to HEAD)
        #[clap(required = false)]
        object: Option<String>,
    },
    /// Remove notes for one or more objects
    Remove {
        /// Objects to remove notes from (defaults to HEAD)
        #[clap(required = false)]
        objects: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "action")]
pub enum NotesOutput {
    #[serde(rename = "add")]
    Add {
        #[serde(rename = "ref")]
        notes_ref: String,
        object: String,
        note_hash: String,
    },
    #[serde(rename = "list")]
    List {
        #[serde(rename = "ref")]
        notes_ref: String,
        notes: Vec<NotesListEntry>,
    },
    #[serde(rename = "show")]
    Show {
        #[serde(rename = "ref")]
        notes_ref: String,
        object: String,
        note_hash: String,
        text: String,
    },
    #[serde(rename = "remove")]
    Remove {
        #[serde(rename = "ref")]
        notes_ref: String,
        removed: Vec<NotesRemovedEntry>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct NotesListEntry {
    pub note_hash: Option<String>,
    pub annotated_object: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NotesRemovedEntry {
    pub object: String,
    pub note_hash: String,
}

pub async fn execute(args: NotesArgs) {
    let argv: Vec<String> = std::env::args().collect();
    if let Err(err) = execute_safe(args, &OutputConfig::default(), &argv).await {
        err.print_stderr();
    }
}

/// Safe entry point that returns structured [`CliResult`].
///
/// `argv` is the raw process argument vector used to reconstruct
/// the original `-m`/`-F` occurrence order. Callers that do not spawn
/// from `main()` should provide the same slice they passed to clap.
pub async fn execute_safe(
    args: NotesArgs,
    output: &OutputConfig,
    argv: &[String],
) -> CliResult<()> {
    let notes_ref = &args.ref_;
    notes::validate_notes_ref(notes_ref).map_err(|e| CliError::from(NotesCliError::from(e)))?;

    let subcommand = args
        .subcommand
        .unwrap_or(NotesSubcommand::List { object: None });

    match subcommand {
        NotesSubcommand::Add {
            object,
            message: _,
            file: _,
            force,
        } => {
            let content = build_note_content(argv)?;
            let result = notes::add(
                notes_ref,
                object.as_deref().unwrap_or("HEAD"),
                &content,
                force,
            )
            .await
            .map_err(NotesCliError::from)?;
            let out = NotesOutput::Add {
                notes_ref: result.notes_ref,
                object: result.object,
                note_hash: result.note_hash,
            };
            render_output(&out, output)?;
        }
        NotesSubcommand::List { object } => {
            let entries = notes::list(notes_ref, object.as_deref())
                .await
                .map_err(NotesCliError::from)?;
            let out = NotesOutput::List {
                notes_ref: notes_ref.to_string(),
                notes: entries
                    .into_iter()
                    .map(|e| NotesListEntry {
                        note_hash: e.note_hash,
                        annotated_object: e.annotated_object,
                    })
                    .collect(),
            };
            render_output(&out, output)?;
        }
        NotesSubcommand::Show { object } => {
            let (obj_hash, note_hash, text) = notes::show(notes_ref, object.as_deref())
                .await
                .map_err(NotesCliError::from)?;
            let out = NotesOutput::Show {
                notes_ref: notes_ref.to_string(),
                object: obj_hash,
                note_hash,
                text,
            };
            render_output(&out, output)?;
        }
        NotesSubcommand::Remove { objects } => {
            let to_remove = if objects.is_empty() {
                vec!["HEAD".to_string()]
            } else {
                objects
            };
            let removed = notes::remove(notes_ref, &to_remove)
                .await
                .map_err(NotesCliError::from)?;
            let out = NotesOutput::Remove {
                notes_ref: notes_ref.to_string(),
                removed: removed
                    .into_iter()
                    .map(|(obj, hash)| NotesRemovedEntry {
                        object: obj,
                        note_hash: hash,
                    })
                    .collect(),
            };
            render_output(&out, output)?;
        }
    }

    Ok(())
}

/// A content source with the type and value as it appeared on the command line.
#[derive(Debug)]
enum ContentPart {
    Message(String),
    File(String),
}

/// Walk the raw process arguments to rebuild the original `-m`/`-F` occurrence
/// order.  Clap splits them into separate `Vec`s, but `git notes` semantics
/// require that the paragraph order matches the command-line order (e.g.
/// `-F header -m trailer` must produce `header\n\ntrailer`, not the reverse).
fn ordered_content_parts(argv: &[String]) -> Vec<ContentPart> {
    let mut parts = Vec::new();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        if arg == "-m" || arg == "--message" {
            i += 1;
            if i < argv.len() {
                parts.push(ContentPart::Message(argv[i].clone()));
            }
        } else if arg == "-F" || arg == "--file" {
            i += 1;
            if i < argv.len() {
                parts.push(ContentPart::File(argv[i].clone()));
            }
        } else if let Some(val) = arg.strip_prefix("-m") {
            if !val.is_empty() {
                parts.push(ContentPart::Message(val.to_string()));
            }
        } else if let Some(val) = arg.strip_prefix("-F") {
            if !val.is_empty() {
                parts.push(ContentPart::File(val.to_string()));
            }
        } else if let Some(val) = arg.strip_prefix("--message=") {
            parts.push(ContentPart::Message(val.to_string()));
        } else if let Some(val) = arg.strip_prefix("--file=") {
            parts.push(ContentPart::File(val.to_string()));
        }
        i += 1;
    }
    parts
}

fn build_note_content(argv: &[String]) -> CliResult<String> {
    let ordered = ordered_content_parts(argv);

    let mut parts: Vec<String> = Vec::new();

    for part in &ordered {
        match part {
            ContentPart::Message(msg) => parts.push(msg.clone()),
            ContentPart::File(file_path) => {
                let data = if file_path == "-" {
                    std::io::read_to_string(std::io::stdin())
                        .map_err(|e| CliError::io(format!("failed to read stdin: {e}")))?
                } else {
                    std::fs::read_to_string(file_path)
                        .map_err(|e| CliError::io(format!("failed to read '{file_path}': {e}")))?
                };
                parts.push(data);
            }
        }
    }

    if ordered.is_empty() {
        return Err(
            CliError::command_usage("provide a message with '-m <msg>' or '-F <file>'.")
                .with_stable_code(StableErrorCode::CliInvalidArguments),
        );
    }

    let content = parts.join("\n\n");
    if content.trim().is_empty() {
        return Err(CliError::command_usage(
            "empty note content is not allowed. Provide non-empty text with '-m' or a non-empty file with '-F'.",
        )
        .with_stable_code(StableErrorCode::CliInvalidArguments));
    }

    Ok(content)
}

fn render_output(result: &NotesOutput, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("notes", result, output);
    }

    if output.quiet {
        return Ok(());
    }

    match result {
        NotesOutput::Add {
            notes_ref,
            object,
            note_hash: _,
        } => {
            println!(
                "Added note to {} in {}",
                short_display_hash(object),
                notes_ref
            );
        }
        NotesOutput::List {
            notes_ref: _,
            notes,
        } => {
            for entry in notes {
                match &entry.note_hash {
                    Some(hash) => println!(
                        "{} {}",
                        short_display_hash(hash),
                        short_display_hash(&entry.annotated_object)
                    ),
                    None => println!("(none) {}", short_display_hash(&entry.annotated_object)),
                }
            }
        }
        NotesOutput::Show { text, .. } => {
            print!("{text}");
        }
        NotesOutput::Remove { notes_ref, removed } => {
            for entry in removed {
                println!(
                    "Removed note from {} in {}",
                    short_display_hash(&entry.object),
                    notes_ref
                );
            }
        }
    }

    Ok(())
}

// ── Error mapping ──────────────────────────────────────────────────

#[derive(Debug, thiserror::Error)]
enum NotesCliError {
    #[error("{0}")]
    Notes(#[from] notes::NotesError),
}

impl From<NotesCliError> for CliError {
    fn from(error: NotesCliError) -> Self {
        let message = error.to_string();
        match &error {
            NotesCliError::Notes(inner) => match inner {
                notes::NotesError::InvalidNotesRef(_) => CliError::fatal(message)
                    .with_stable_code(StableErrorCode::CliInvalidArguments)
                    .with_hint(
                        "notes refs must start with 'refs/notes/'; e.g. 'refs/notes/commits'.",
                    ),
                notes::NotesError::AlreadyExists { .. } => CliError::fatal(message)
                    .with_stable_code(StableErrorCode::ConflictOperationBlocked)
                    .with_hint("use '-f' to overwrite the existing note."),
                notes::NotesError::NotFound { .. } => CliError::fatal(message)
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra notes list' to see which objects have notes."),
                notes::NotesError::InvalidObject(_, _) => CliError::fatal(message)
                    .with_stable_code(StableErrorCode::CliInvalidTarget)
                    .with_hint("use 'libra log' to find valid commit references."),
                notes::NotesError::HeadUnborn => CliError::fatal(message)
                    .with_stable_code(StableErrorCode::RepoStateInvalid)
                    .with_hint("create a commit first before adding notes."),
                notes::NotesError::QueryFailed(_) => {
                    CliError::fatal(message).with_stable_code(StableErrorCode::IoReadFailed)
                }
                notes::NotesError::ResolveFailed(_) => {
                    CliError::fatal(message).with_stable_code(StableErrorCode::RepoCorrupt)
                }
                notes::NotesError::StoreBlobFailed(_) => {
                    CliError::fatal(message).with_stable_code(StableErrorCode::IoWriteFailed)
                }
            },
        }
    }
}
