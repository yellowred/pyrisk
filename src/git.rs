use anyhow::{bail, Context, Result};
use git2::{DiffOptions, Repository};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::parser::{self, Symbol};

pub fn changed_symbols(repo_root: &Path, branch: &str) -> Result<Vec<Symbol>> {
    let repo = Repository::open(repo_root).context("opening git repository")?;

    // Resolve the branch/ref to compare against
    let branch_obj = repo
        .revparse_single(branch)
        .with_context(|| format!("resolving ref '{}'", branch))?;
    let branch_commit = branch_obj
        .peel_to_commit()
        .with_context(|| format!("peeling '{}' to commit", branch))?;
    let branch_tree = branch_commit.tree()?;

    // Get the current HEAD
    let head = repo.head().context("getting HEAD")?;
    let head_commit = head.peel_to_commit().context("peeling HEAD to commit")?;
    let head_tree = head_commit.tree()?;

    // Diff branch_tree -> head_tree (shows what HEAD changed relative to branch)
    let mut diff_opts = DiffOptions::new();
    diff_opts.context_lines(0);
    let diff = repo
        .diff_tree_to_tree(Some(&branch_tree), Some(&head_tree), Some(&mut diff_opts))
        .context("computing diff")?;

    // Collect changed line ranges per .py file
    let mut changed_files: HashMap<PathBuf, Vec<(usize, usize)>> = HashMap::new();

    use git2::Delta;

    let mut added_files: Vec<PathBuf> = Vec::new();

    diff.foreach(
        &mut |delta, _progress| {
            // Track newly added .py files
            if delta.status() == Delta::Added {
                if let Some(new_file) = delta.new_file().path() {
                    if new_file.to_string_lossy().ends_with(".py") {
                        added_files.push(repo_root.join(new_file));
                    }
                }
            }
            true
        },
        None,
        Some(&mut |delta, hunk| {
            if let Some(new_file) = delta.new_file().path() {
                let path_str = new_file.to_string_lossy();
                if path_str.ends_with(".py") {
                    let start = hunk.new_start() as usize;
                    let lines = hunk.new_lines() as usize;
                    let end = if lines == 0 { start } else { start + lines - 1 };
                    let full_path = repo_root.join(new_file);
                    changed_files
                        .entry(full_path)
                        .or_default()
                        .push((start, end));
                }
            }
            true
        }),
        None,
    )
    .context("iterating diff")?;

    // Mark entirely new files with a full-file range
    for path in added_files {
        changed_files.entry(path).or_insert_with(|| vec![(1, 999999)]);
    }

    // Parse each changed file and find symbols whose line ranges overlap changed hunks
    let mut symbols = Vec::new();
    for (path, changed_ranges) in &changed_files {
        if !path.exists() {
            continue; // Deleted file
        }
        match parser::parse_file(path, repo_root) {
            Ok(file_syms) => {
                for sym in &file_syms.defined {
                    if ranges_overlap(&changed_ranges, sym.line, sym.end_line) {
                        symbols.push(sym.clone());
                    }
                }
            }
            Err(e) => {
                eprintln!("Warning: failed to parse {}: {}", path.display(), e);
            }
        }
    }

    Ok(symbols)
}

fn ranges_overlap(changed: &[(usize, usize)], sym_start: usize, sym_end: usize) -> bool {
    changed
        .iter()
        .any(|&(cs, ce)| sym_start <= ce && sym_end >= cs)
}

pub fn find_default_branch(repo_root: &Path) -> Result<String> {
    let repo = Repository::open(repo_root).context("opening git repository")?;
    // Try common default branch names
    for name in &["main", "master"] {
        if repo.find_branch(name, git2::BranchType::Local).is_ok() {
            return Ok(name.to_string());
        }
    }
    bail!("Could not find default branch (tried 'main' and 'master'). Please specify a branch.")
}
