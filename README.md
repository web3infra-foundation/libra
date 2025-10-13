## Libra

`Libra` is a partial implementation of a **Git** client, developed using **Rust**. Our goal is not to create a 100% replica of Git (for those interested in such a project, please refer to the [gitoxide](https://github.com/Byron/gitoxide)). Instead, `libra` focus on implementing the basic functionalities of Git for learning **Git** and **Rust**. A key feature of `libra` is the replacement of the original **Git** internal storage architecture with **SQLite**.

## Example
```
$ libra --help
Simulates git commands

Usage: libra <COMMAND>

Commands:
  init     Initialize a new repository
  clone    Clone a repository into a new directory
  add      Add file contents to the index
  rm       Remove files from the working tree and from the index
  restore  Restore working tree files
  status   Show the working tree status
  log      Show commit logs
  diff    Show changes between commits, commit and working tree, etc
  branch   List, create, or delete branches
  commit   Record changes to the repository
  switch   Switch branches
  merge    Merge changes
  push     Update remote refs along with associated objects
  fetch    Download objects and refs from another repository
  pull     Fetch from and integrate with another repository or a local branch
  remote   Manage set of tracked repositories
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```
## Features

### Clean Code
Our code is designed to be clean and easy to read, 
ensuring that it is both maintainable and understandable for developers of all skill levels.

### Cross-Platform
- [x] Windows
- [x] Linux
- [x] MacOS

### Compatibility with Git
Our implementation is essentially fully compatible with `Git` 
(developed with reference to the `Git` documentation), 
including formats such as `objects`, `index`, `pack`, and `pack-index`. 
Therefore, it can interact seamlessly with `Git` servers (like `push` and `pull`).

### Differences from Git:
While maintaining compatibility with `Git`, we have made some innovations and changes:
we use an `SQLite` database to manage loosely structured files such as `config`, `HEAD`, and `refs`, 
achieving unified management.

### Commands

- init
  - [x] --bare
  - [x] --template <path>
  - [x] -b, --initial-branch <name>
  - [x] -q, --quiet
  - [x] --shared <perm>
  - [x] <repo_directory>
  - [ ] --separate-git-dir <dir>
  - [ ] --object-format <alg>

- clone
  - [x] <remote_repo>
  - [x] [local_path]
  - [x] -b, --branch <name>
  - [ ] --depth <n>
  - [ ] --single-branch
  - [ ] --recurse-submodules
  - [ ] --bare
  - [ ] --mirror

- add
  - [x] <pathspec...>
  - [x] -A, --all
  - [x] -u, --update
  - [x] --refresh
  - [x] -v, --verbose
  - [x] -n, --dry-run
  - [x] --ignore-errors
  - [ ] -f, --force
  - [ ] -p, --patch
  - [ ] -i, --interactive
  - [ ] -N, --intent-to-add
  - [ ] --chmod=(+x|-x)
  - [ ] --renormalize

- rm
  - [x] <pathspec...>
  - [x] --cached
  - [x] -r, --recursive
  - [x] -f, --force
  - [x] --dry-run
  - [ ] --ignore-unmatch
  - [ ] --pathspec-from-file <file>
  - [ ] --pathspec-file-nul

- status
  - [x] --porcelain
  - [x] -s, --short
  - [ ] --branch
  - [ ] --ignored
  - [ ] --untracked-files[=no|normal|all]
  - [ ] --show-stash

- commit
  - [x] -m, --message <msg>
  - [x] -F, --file <path>
  - [x] --allow-empty
  - [x] --conventional
  - [x] --amend
  - [x] -s, --signoff
  - [x] --disable-pre
  - [ ] -a, --all
  - [ ] -p, --patch
  - [ ] --no-verify
  - [ ] --no-edit
  - [ ] --author <name>
  - [ ] --date <when>
  - [ ] -S, --gpg-sign
  - [ ] --no-gpg-sign

- log
  - [x] -n, --number <n>
  - [x] --oneline
  - [x] -p, --patch
  - [x] --decorate
  - [x] --no-decorate
  - [x] [pathspec]
  - [ ] --graph
  - [ ] --pretty=<format>
  - [ ] --abbrev-commit
  - [ ] --name-only
  - [ ] --name-status
  - [ ] --stat
  - [ ] --since <date> / --until <date>
  - [ ] --author <pattern>

- tag
  - [x] <name>
  - [x] -l, --list [pattern]
  - [x] -d, --delete <name>
  - [x] -m, --message <msg>
  - [x] -f, --force
  - [ ] -a
  - [ ] -s, --sign
  - [ ] -u <keyid>
  - [ ] -n <n>
  - [ ] -v, --verify

- branch
  - [x] <new_branch> [commit_hash]
  - [x] -D, --delete <branch>
  - [x] -u, --set-upstream-to <upstream>
  - [x] --show-current
  - [x] -r, --remotes
  - [x] --list
  - [ ] -d, --delete (safe)
  - [ ] -m, --move <old> <new>
  - [ ] -M, --move --force <old> <new>
  - [ ] -a, --all
  - [ ] --unset-upstream
  - [ ] --format <fmt>

- switch
  - [x] <branch>
  - [x] -c, --create <new_branch>
  - [x] -d, --detach
  - [ ] -C, --force-create <branch>
  - [ ] --guess / --no-guess
  - [ ] --track
  - [ ] --merge
  - [ ] --conflict=<style>

- restore
  - [x] <pathspec...>
  - [x] -s, --source <commit>
  - [x] -W, --worktree
  - [x] -S, --staged
  - [ ] -p, --patch
  - [ ] --ignore-unmerged
  - [ ] --merge
  - [ ] --conflict=<style>

- reset
  - [x] [<target> (default HEAD)]
  - [x] --soft
  - [x] --mixed
  - [x] --hard
  - [x] [<pathspec...>]
  - [ ] --merge
  - [ ] --keep
  - [ ] --pathspec-from-file <file>

- diff
  - [x] --old <rev>
  - [x] --new <rev>
  - [x] --staged
  - [x] [pathspec]
  - [x] --algorithm <name>
  - [x] --output <file>
  - [ ] --cached
  - [ ] --name-only
  - [ ] --stat
  - [ ] --color
  - [ ] --word-diff
  - [ ] --ignore-space-at-eol / --ignore-space-change / --ignore-all-space
  - [ ] --submodule

- merge
  - [x] <branch>
  - [ ] --no-ff / --ff-only
  - [ ] --squash
  - [ ] --commit / --no-commit
  - [ ] -m, --message <msg>
  - [ ] --strategy <name>
  - [ ] --strategy-option <opt>

- rebase
  - [x] <upstream>
  - [ ] -i, --interactive
  - [ ] --onto <newbase>
  - [ ] --autostash
  - [ ] --continue / --abort / --skip

- cherry-pick
  - [x] <commits...>
  - [x] -n, --no-commit
  - [ ] -x
  - [ ] -e, --edit
  - [ ] -m, --mainline <parent>
  - [ ] --continue / --abort / --quit

- revert
  - [x] <commit>
  - [x] -n, --no-commit
  - [ ] --edit / --no-edit
  - [ ] -m, --mainline <parent>
  - [ ] --continue / --abort / --quit

- remote
  - [ ] rename <old> <new>
  - [ ] set-url <name> <newurl> [--add] [--delete] [--push] [--all]
  - [ ] get-url <name> [--push] [--all]
  - [ ] prune <name>
  - [ ] update [<group> | <remotes>...]
  - [ ] add -f
  - [ ] add --tags / --no-tags
  - [ ] add -t <branch>
  - [ ] add -m <master>
  - [ ] add --mirror=<push|fetch>
  - [ ] --verbose

- lfs
  - [x] track
  - [x] untrack
  - [x] locks
  - [x] lock
  - [x] unlock
  - [ ] install / uninstall
  - [ ] fetch / pull / push
  - [ ] ls-files
  - [ ] env / version

- push
  - [x] <repository> <refspec>
  - [x] -u, --set-upstream
  - [ ] --force / --force-with-lease
  - [ ] --tags / --all
  - [ ] --delete
  - [ ] --dry-run

- fetch
  - [x] [<repository>] [<refspec>]
  - [x] -a, --all
  - [ ] --tags
  - [ ] --prune
  - [ ] --force
  - [ ] --depth <n> / --shallow-exclude <ref>
  - [ ] --multiple

- pull
  - [x] <repository> <refspec>
  - [ ] --rebase
  - [ ] --ff-only / --no-ff
  - [ ] --squash
  - [ ] --strategy <name>

- reflog
  - [x] show [--pretty=<fmt>]
  - [x] delete <selectors...>
  - [x] exists <ref>
  - [ ] expire [--expire=<time>]

- checkout
  - [x] -b <new_branch> [start-point]
  - [x] <branch>
  - [ ] -B <new_branch> [start-point]
  - [ ] --detach
  - [ ] -f, --force

- index-pack
  - [x] <pack_file>
  - [x] -o <index_file>
  - [x] --index-version <n>
  - [ ] --stdin
  - [ ] --fix-thin
  - [ ] --verify

- config
  - [x] --add <name> <value>
  - [x] --get <name>
  - [x] --get-all <name>
  - [x] --unset <name>
  - [x] --unset-all <name>
  - [x] -l, --list
  - [x] --name-only
  - [x] -d, --default <value>
  - [ ] --global / --system / --local
  - [ ] --file <path>
  - [ ] --replace-all
  - [ ] --type=<bool|int|path>
#### Remote
- [x] `push`
- [x] `pull`
- [x] `clone`
- [x] `fetch`

### Others
- [x] `.gitignore`
- [x] `.gitattributes` (only for `lfs` now)
- [x] `LFS` (embedded, with p2p feature)
- [ ] `ssh`

## Development
Refs to [Development](../docs/libra/development.md)