# pyrisk

Blast-radius analyzer for Python codebases. Compares your branch against a base branch, finds every changed function/method, builds a call graph of the entire repo, and scores each change by how many callers it affects and whether those callers have tests.

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Analyze current repo against main/master
pyrisk

# Analyze against a specific branch
pyrisk develop

# Point at a different repo
pyrisk -r ~/projects/myapp main

# Verbose output with caller breakdown
pyrisk -v

# JSON output
pyrisk --json

# Show uncovered callers for a specific symbol
pyrisk --uncovered my_function

# Exclude test directories from analysis
pyrisk --exclude __tests__ --exclude __itests__
```

## Output

```
SYMBOL        CALLERS    MODULES    UNCOVERED    RISK
my_func       3 (12)     5          4            ████ HIGH
helper        1 (1)      2          0            █ LOW
```

| Column | Description |
|--------|-------------|
| SYMBOL | Changed function or method (`module.qualname`) |
| CALLERS | Direct callers (transitive callers in parentheses) |
| MODULES | Number of distinct modules affected |
| UNCOVERED | Callers in modules with no corresponding test file |
| RISK | Score bar and label: LOW (<10), MED (10-30), HIGH (>30) |

## Options

| Flag | Description |
|------|-------------|
| `[BRANCH]` | Git branch to compare against (default: `main` or `master`) |
| `-r, --repo <PATH>` | Path to repo root (default: current directory) |
| `-v, --verbose` | Show detailed caller breakdown per symbol |
| `--json` | Output JSON instead of table |
| `--uncovered <SYMBOL>` | List uncovered callers for a symbol (substring match) |
| `--exclude <FOLDER>` | Exclude paths containing this folder name (repeatable) |

## How it works

1. **Diff** — uses `git2` to find functions/methods changed between HEAD and the base branch
2. **Parse** — uses `tree-sitter` to parse every `.py` file and extract definitions + call sites
3. **Call graph** — builds a whole-repo call graph with qualified name resolution (`self.method()` resolves to the correct class, `ClassName.method()` disambiguates across classes)
4. **Score** — BFS from each changed symbol through callers, counting direct/transitive callers, affected modules, and uncovered callers
5. **Cache** — parsed file data is cached in a `sled` database keyed by file path + mtime for fast incremental runs

## Risk scoring

```
score = (direct_callers * 3) + (transitive_callers * 1) + (modules_affected * 2) + (uncovered_callers * 4)
```

| Label | Score |
|-------|-------|
| LOW | < 10 |
| MED | 10 - 30 |
| HIGH | > 30 |

## License

MIT
