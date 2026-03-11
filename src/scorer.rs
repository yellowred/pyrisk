use std::collections::HashSet;
use std::path::Path;

use crate::callgraph::CallGraph;
use crate::parser::Symbol;

#[derive(Debug)]
pub struct RiskScore {
    pub symbol: Symbol,
    pub direct_callers: usize,
    pub transitive_callers: usize,
    pub modules_affected: usize,
    pub uncovered_callers: usize,
    pub score: f64,
}

impl RiskScore {
    pub fn risk_label(&self) -> &'static str {
        if self.score >= 30.0 {
            "HIGH"
        } else if self.score >= 10.0 {
            "MED"
        } else {
            "LOW"
        }
    }

    pub fn risk_bar(&self) -> &'static str {
        if self.score >= 30.0 {
            "████"
        } else if self.score >= 10.0 {
            "███"
        } else {
            "█"
        }
    }
}

pub fn score_all(
    changed: &[Symbol],
    graph: &CallGraph,
    test_modules: &HashSet<String>,
) -> Vec<RiskScore> {
    let mut scores: Vec<RiskScore> = changed
        .iter()
        .map(|sym| {
            let full = sym.full_name();
            let direct = graph.direct_callers(&full);
            let radius = graph.blast_radius(&full);
            let transitive = radius.len();

            // Collect unique modules affected
            let mut modules: HashSet<String> = HashSet::new();
            modules.insert(sym.module.clone());
            for (caller_name, _depth) in &radius {
                if let Some(caller_sym) = graph.symbols.get(caller_name) {
                    modules.insert(caller_sym.module.clone());
                }
            }
            let modules_affected = modules.len();

            // Count uncovered callers (callers whose module has no test file)
            let mut uncovered = 0;
            for (caller_name, _depth) in &radius {
                if let Some(caller_sym) = graph.symbols.get(caller_name) {
                    if !test_modules.contains(&caller_sym.module) {
                        uncovered += 1;
                    }
                }
            }

            let score = (direct as f64) * 3.0
                + (transitive as f64) * 1.0
                + (modules_affected as f64) * 2.0
                + (uncovered as f64) * 4.0;

            RiskScore {
                symbol: sym.clone(),
                direct_callers: direct,
                transitive_callers: transitive,
                modules_affected,
                uncovered_callers: uncovered,
                score,
            }
        })
        .collect();

    scores.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
    scores
}

/// Find modules that have a corresponding test file
pub fn find_test_modules(repo_root: &Path) -> HashSet<String> {
    let mut test_modules = HashSet::new();

    for entry in walkdir::WalkDir::new(repo_root)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if name.ends_with(".py") && (name.starts_with("test_") || name.ends_with("_test.py")) {
                // Derive the module this test covers
                if let Ok(rel) = path.parent().unwrap_or(path).strip_prefix(repo_root) {
                    let module = rel
                        .to_string_lossy()
                        .replace('/', ".")
                        .replace('\\', ".");
                    let module = module.strip_suffix(".__init__").unwrap_or(&module);
                    if !module.is_empty() {
                        test_modules.insert(module.to_string());
                    }
                }
            }
        }
    }

    test_modules
}
