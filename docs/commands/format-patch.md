# `libra format-patch`

Generate mbox-formatted email patch files from commits.

## Synopsis

```bash
libra format-patch [OPTIONS] [revision-range]
```

## Description

`libra format-patch` walks a revision range (`A..B` or a single commit treated
as `<commit>..HEAD`), produces one patch file per non-merge commit (named with
the `--suffix`, default `.patch`), and
formats each as an mbox message with RFC 2822 headers, a plain-text diffstat,
and a unified diff. The output is compatible with `git am`.

Merge commits are skipped by default. When the revision range resolves to zero
commits, the command exits with an error.

## Options

| Flag | Short | Description | Default |
|------|-------|-------------|---------|
| `[revision-range]` | | `A..B` range or single commit; single commit means `<commit>..HEAD` | `HEAD` |
| `--output-directory <DIR>` | `-o` | Write patch files into `DIR` | current directory |
| `--stdout` | | Print all patches to stdout | false |
| `--numbered` | `-n` | Name files with a leading sequence number (`0001-subject.patch`) | false |
| `--start-number <N>` | | Start numbering at `N` | 1 |
| `--subject-prefix <PREFIX>` | | Use `PREFIX` instead of `PATCH` in the Subject: line | `PATCH` |
| `--cover-letter` | | Generate a `0000-cover-letter.patch` template | false |
| `--thread` | | Add `In-Reply-To` and `References` headers (default on) | true |
| `--no-thread` | | Disable threading headers | false |
| `--in-reply-to <MESSAGE_ID>` | | Make the first mail a reply to the given Message-ID | none |
| `--reroll-count <N>` | `-v` | Mark as version `N` (changes `[PATCH]` to `[PATCH vN]`) | none |
| `--signoff` | `-s` | Append a `Signed-off-by` trailer to each commit message | false |
| `--full-index` | | Show full object IDs in diff index header lines | false |
| `--no-stat` | | Suppress the diffstat summary | false |
| `--keep-subject` | | Keep the original `[PATCH]` prefix in the commit subject | false |
| `--suffix <SFX>` | | Filename suffix for generated patches (e.g. `.txt`) | `.patch` |
| `--zero-commit` | | Use an all-zero hash in each patch's `From <hash>` envelope line | false |

## Examples

### Basic range
```bash
# Generate patches for the last three commits
libra format-patch HEAD~3..HEAD

# Numbered patches in a directory
libra format-patch -n -o patches/ main..feature

# With cover letter and threading
libra format-patch --cover-letter --thread origin/main..

# Version 2, replying to a previous thread
libra format-patch -v 2 --in-reply-to '<msgid@example>' origin/main..

# Pipe to an external tool
libra format-patch --stdout origin/main.. | git am
```

## Output Format

Each patch file is an mbox message:

```
From <commit-oid> <unix-mbox-date>
From: Author Name <email>
Date: <RFC 2822 date>
Subject: [PATCH n/m] commit subject
MIME-Version: 1.0
Content-Type: text/plain; charset=UTF-8
Content-Transfer-Encoding: 8bit

commit message body
---
diffstat summary
unified diff
--
<libra-version>
```

With `--json` or `--machine`, `data.patches` lists every generated output.
When `--cover-letter` is set, the list includes `0000-cover-letter` (with the
configured suffix, default `.patch`) as record number `0` before the commit
patch records.

## Error Handling

| Scenario | StableErrorCode |
|----------|-----------------|
| Not in a Libra repository | `LBR-REPO-001` |
| Unknown revision or empty range | `LBR-CLI-003` |
| Output file write failure | `LBR-IO-002` |
| Output directory creation failure | `LBR-IO-002` |
