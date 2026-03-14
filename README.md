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
SYMBOL        CALLERS    MODULES    UNCOVERED    PERCENTILE    RISK
my_func       3 (12)     5          4 (80%)      p92           ████ HIGH
helper        1 (1)      2          0 (0%)       p65           █ LOW
```

| Column | Description |
|--------|-------------|
| SYMBOL | Changed function or method (`module.qualname`) |
| CALLERS | Direct callers (transitive callers in parentheses) |
| MODULES | Number of distinct modules affected |
| UNCOVERED | Callers in modules with no test file, with uncovered ratio |
| PERCENTILE | How connected this symbol is relative to all symbols in the repo |
| RISK | Risk label based on percentile and uncovered ratio (see below) |

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
4. **Baseline** — computes the direct caller distribution across all symbols in the repo
5. **Score** — BFS from each changed symbol through callers, then ranks each symbol by its percentile in the repo baseline and its uncovered caller ratio
6. **Cache** — parsed file data is cached in a `sled` database keyed by file path + mtime for fast incremental runs

## Risk scoring

Risk labels are anchored against the full repository baseline rather than arbitrary thresholds. When pyrisk runs, it computes the direct caller count for every symbol in the repo to build a distribution. Each changed symbol is then scored on two axes:

**Percentile** — where this symbol's direct caller count falls in the repo-wide distribution. A symbol at p90 has more direct callers than 90% of all symbols in the codebase.

**Uncovered ratio** — the fraction of transitive callers whose modules have no corresponding test file. A ratio of 100% means none of the code paths that depend on this symbol have test coverage.

The composite score combines both: `score = percentile * 60 + uncovered_ratio * 40`.

Labels are assigned based on both axes:

| Label | Condition |
|-------|-----------|
| HIGH | Percentile >= p90 **and** uncovered ratio > 50% |
| MED | Percentile >= p75 **or** uncovered ratio > 50% |
| LOW | Below both thresholds |

This means HIGH requires a symbol to be both highly connected in the repo (top 10%) and mostly untested — a concrete, defensible signal rather than an arbitrary score cutoff.

## License

MIT
