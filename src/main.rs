mod callgraph;
mod git;
mod index;
mod output;
mod parser;
mod scorer;

use anyhow::{Context, Result};
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Instant;
use ignore::WalkBuilder;

#[derive(Parser)]
#[command(
    name = "pyrisk",
    about = "Blast radius analyzer for Python codebases",
    after_help = "\
Output columns:
  SYMBOL      Changed function or method (module.qualname)
  CALLERS     Direct callers (transitive callers in parentheses)
  MODULES     Number of distinct modules affected by the change
  UNCOVERED   Callers in modules with no corresponding test file
  RISK        Risk score bar and label: LOW (<10), MED (10-30), HIGH (>30)"
)]
struct Cli {
    /// Git branch to compare against (default: main or master)
    branch: Option<String>,

    /// Path to repo root (default: cwd)
    #[arg(short, long)]
    repo: Option<PathBuf>,

    /// Show detailed caller breakdown per symbol
    #[arg(short, long)]
    verbose: bool,

    /// Output JSON instead of table
    #[arg(long)]
    json: bool,

    /// List uncovered callers for a symbol (substring match)
    #[arg(long)]
    uncovered: Option<String>,

    /// Exclude callers whose file path contains this folder name (repeatable)
    #[arg(long)]
    exclude: Vec<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Resolve repo root
    let repo_root = if let Some(ref r) = cli.repo {
        std::fs::canonicalize(r).context("resolving repo path")?
    } else {
        let repo = git2::Repository::discover(".").context("not in a git repository")?;
        repo.workdir()
            .context("bare repository not supported")?
            .to_path_buf()
    };

    // Resolve branch
    let branch = match cli.branch {
        Some(b) => b,
        None => git::find_default_branch(&repo_root)?,
    };

    if cli.verbose {
        let repo = git2::Repository::open(&repo_root).context("opening git repository")?;
        let head = repo.head().context("getting HEAD")?;
        let head_commit = head.peel_to_commit().context("peeling HEAD to commit")?;
        let head_name = head.shorthand().unwrap_or("detached");
        let head_sha = &head_commit.id().to_string()[..10];
        eprintln!("repo:   {}", repo_root.display());
        eprintln!("HEAD:   {} ({})", head_name, head_sha);
        eprintln!("base:   {}", branch);
        eprintln!();
    }

    // Step 1: Get changed symbols (filtered by --exclude)
    let diff_start = Instant::now();
    let changed: Vec<_> = git::changed_symbols(&repo_root, &branch)
        .with_context(|| format!("analyzing changes against '{}'", branch))?
        .into_iter()
        .filter(|s| !cli.exclude.iter().any(|ex| s.file.contains(ex.as_str())))
        .collect();
    if cli.verbose {
        eprintln!(
            "diff:   found {} changed symbols in {:.1}ms",
            changed.len(),
            diff_start.elapsed().as_secs_f64() * 1000.0
        );
        eprintln!();
    }

    if changed.is_empty() {
        println!("No changed Python symbols found relative to '{}'.", branch);
        return Ok(());
    }

    // Step 2: Walk all .py files (respects .gitignore)
    let py_files: Vec<PathBuf> = WalkBuilder::new(&repo_root)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "py"))
        .map(|e| e.into_path())
        .collect();

    let file_count = py_files.len();
    let start = Instant::now();

    // Step 3: Parse all files (with index caching)
    let idx = index::Index::open(&repo_root).ok();
    let pb = ProgressBar::new(file_count as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} Analyzing… {pos}/{len} files checked")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    let all_symbols: Mutex<Vec<parser::FileSymbols>> = Mutex::new(Vec::new());

    py_files.par_iter().for_each(|path| {
        let mtime = index::file_mtime(path);
        let syms = if let Some(ref idx) = idx {
            if let Some(cached) = idx.get(path, mtime) {
                cached
            } else {
                match parser::parse_file(path, &repo_root) {
                    Ok(s) => {
                        idx.insert(path, mtime, s.clone());
                        s
                    }
                    Err(e) => {
                        eprintln!("Warning: {}: {}", path.display(), e);
                        return;
                    }
                }
            }
        } else {
            match parser::parse_file(path, &repo_root) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Warning: {}: {}", path.display(), e);
                    return;
                }
            }
        };

        all_symbols.lock().unwrap().push(syms);
        pb.inc(1);
    });

    pb.finish_and_clear();
    let elapsed = start.elapsed();

    let all_syms = all_symbols.into_inner().unwrap();

    println!(
        "Analyzed {} Python files in {:.1}s ({} changed symbols)\n",
        file_count,
        elapsed.as_secs_f64(),
        changed.len()
    );

    // Step 4: Build call graph
    let pb2 = if cli.verbose {
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::with_template("{spinner:.cyan} Building call graph…")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        Some(pb)
    } else {
        None
    };
    let graph = callgraph::CallGraph::build(&all_syms);

    // Step 5: Score
    if let Some(ref pb) = pb2 {
        pb.set_message("Scoring risk…");
    }
    let test_modules = scorer::find_test_modules(&repo_root);
    let scores = scorer::score_all(&changed, &graph, &test_modules, &cli.exclude);
    if let Some(pb) = pb2 {
        pb.finish_and_clear();
    }

    // Step 6: Output
    if let Some(ref pattern) = cli.uncovered {
        output::render_uncovered(pattern, &scores, &graph, &test_modules, &cli.exclude);
    } else if cli.json {
        output::render_json(&scores);
    } else {
        output::render_table(&scores, cli.verbose);
    }

    Ok(())
}
