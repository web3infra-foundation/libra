# `libra archive`

Historical design for creating an archive from a committed tree snapshot.

> Status: unpublished. `libra archive` is not registered in the public CLI in
> the current release. Running it returns the standard unknown-command error
> (`LBR-CLI-001`). The implementation notes below describe the preserved design
> in `src/command/archive.rs`, not a user-visible command contract.

## Synopsis

```bash
libra archive [OPTIONS] [TREEISH]
```

## Description

The unpublished design is analogous to `git archive`: it resolves a commit, branch,
tag, or abbreviated commit hash, walks that commit tree, and writes the tracked
files as an archive. The command does not modify the working tree or index.

When `TREEISH` is omitted, the command archives `HEAD`. The default format is
an uncompressed tar stream written to stdout. Use `--output <FILE>` when running
from an interactive shell so binary archive bytes are written to a file instead
of the terminal.

## Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `[TREEISH]` | | Commit, branch, tag, or abbreviated commit hash to archive | `HEAD` |
| `--format <FMT>` | `-f` | Archive format: `tar`, `tar.gz`, `tgz`, `tar.bz2`, `tbz2`, `tbz`, or `zip` | `tar` |
| `--output <FILE>` | `-o` | Write archive bytes to a file instead of stdout | stdout |
| `--prefix <PREFIX>` | | Prepend a relative directory prefix to each archived path | none |

`--prefix <PREFIX>` must be relative. Absolute prefixes and prefixes containing
`..` path components are rejected to prevent archive path traversal.

## Examples

```bash
# Write HEAD as an uncompressed tar archive.
libra archive -o project.tar

# Write a gzip-compressed release archive.
libra archive --format=tar.gz --prefix=project-v1.0/ -o project-v1.0.tar.gz v1.0

# Write a bzip2-compressed archive using the short format flag.
libra archive -f tbz2 -o project.tar.bz2 HEAD

# Write a zip archive for a branch.
libra archive --format=zip -o feature.zip feature-branch
```

## Output

If this command is published in a future release, success will write archive bytes to stdout or to the path passed
with `--output <FILE>`. It does not print a separate success message.

Tar archives preserve regular files, executable file modes, symlinks, nested
paths, empty files, and Unicode filenames. Zip archives are built in memory
first because the zip writer requires seekable output, then flushed to the
requested destination.

## Error Handling

| Scenario | StableErrorCode |
|----------|-----------------|
| Unknown `TREEISH` or empty repository | `LBR-CLI-003` |
| Unknown `--format <FMT>` value | `LBR-CLI-002` |
| Unsafe `--prefix <PREFIX>` | `LBR-CLI-002` |
| Referenced repository object cannot be read | `LBR-REPO-002` |
| Blob content cannot be read | `LBR-IO-001` |
| Output file cannot be created or written | `LBR-IO-002` |

Failure output uses Libra's standard structured CLI error report.
