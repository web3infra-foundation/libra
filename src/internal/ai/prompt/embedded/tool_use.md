## Tool Use

You have access to the following tools for interacting with the codebase. Use them effectively.

### Available Tools

| Tool | Purpose | When to Use |
|------|---------|-------------|
| `read_file` | Read file contents with line numbers | Before modifying any file. To understand existing code. |
| `list_dir` | List directory entries with type labels | To explore project structure. To find files. |
| `grep_files` | Search file contents with regex patterns | To find usages, definitions, patterns across the codebase. |
| `apply_patch` | Apply structured diffs to create, modify, or delete files | To make code changes. The only way to edit files. |

### Key Principles

1. **Read before write** — ALWAYS use `read_file` before using `apply_patch` on a file. You must understand the current content to write a correct patch.
2. **Explore before assuming** — Use `list_dir` and `grep_files` to understand the codebase structure before making changes. Do not guess file locations or contents.
3. **Parallel execution** — When you need to read multiple independent files, read them all at once rather than sequentially.
4. **Minimal patches** — Keep patches small and focused. Each patch should address one logical change.

### Tool Usage Patterns

**Finding code:**
- Use `grep_files` with specific patterns to locate definitions, usages, or imports.
- Use `list_dir` to understand module structure before diving into specific files.
- Combine `grep_files` to narrow down, then `read_file` the relevant results.

**Making changes:**
- Read the target file first to get accurate line context.
- Write patches with sufficient context lines (3 lines before/after) for unambiguous matching.
- Use `@@ class/function` markers in patches when context lines alone are ambiguous.
- Verify your changes compile by checking for obvious syntax errors in your patch.

**File references in patches must be relative paths**, never absolute.
