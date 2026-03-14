use std::collections::HashSet;
use std::path::Path;

use crate::callgraph::{CallGraph, RepoBaseline};
use crate::parser::Symbol;

#[derive(Debug)]
pub struct RiskScore {
    pub symbol: Symbol,
    pub direct_callers: usize,
    pub transitive_callers: usize,
    pub modules_affected: usize,
    pub uncovered_callers: usize,
    pub uncovered_ratio: f64,
    pub percentile: f64,
    pub score: f64,
}

impl RiskScore {
    pub fn risk_label(&self) -> &'static str {
        if self.percentile >= 0.90 && self.uncovered_ratio > 0.5 {
            "HIGH"
        } else if self.percentile >= 0.75 || self.uncovered_ratio > 0.5 {
            "MED"
        } else {
            "LOW"
        }
    }

    pub fn risk_bar(&self) -> &'static str {
        match self.risk_label() {
            "HIGH" => "████",
            "MED" => "███",
            _ => "█",
        }
    }
}

pub fn score_all(
    changed: &[Symbol],
    graph: &CallGraph,
    test_modules: &HashSet<String>,
    exclude: &[String],
    baseline: &RepoBaseline,
) -> Vec<RiskScore> {
    let mut scores: Vec<RiskScore> = changed
        .iter()
        .map(|sym| {
            let full = sym.full_name();
            let direct = graph.direct_callers_filtered(&full, exclude);
            let radius = graph.blast_radius(&full);

            // Filter radius by exclude patterns once, use for all calculations
            let filtered_radius: Vec<&(String, usize)> = radius.iter()
                .filter(|(caller_name, _)| {
                    graph.symbols.get(caller_name)
                        .map(|s| !exclude.iter().any(|ex| s.file.contains(ex.as_str())))
                        .unwrap_or(true)
                })
                .collect();
            let transitive = filtered_radius.len();

            // Collect unique modules affected
            let mut modules: HashSet<String> = HashSet::new();
            modules.insert(sym.module.clone());
            for (caller_name, _depth) in &filtered_radius {
                if let Some(caller_sym) = graph.symbols.get(caller_name) {
                    modules.insert(caller_sym.module.clone());
                }
            }
            let modules_affected = modules.len();

            // Count uncovered callers (callers whose module has no test file)
            let mut uncovered = 0;
            for (caller_name, _depth) in &filtered_radius {
                if let Some(caller_sym) = graph.symbols.get(caller_name) {
                    if !test_modules.contains(&caller_sym.module) {
                        uncovered += 1;
                    }
                }
            }

            let uncovered_ratio = uncovered as f64 / transitive.max(1) as f64;
            let percentile = baseline.percentile_of(direct);
            let score = percentile * 60.0 + uncovered_ratio * 40.0;

            RiskScore {
                symbol: sym.clone(),
                direct_callers: direct,
                transitive_callers: transitive,
                modules_affected,
                uncovered_callers: uncovered,
                uncovered_ratio,
                percentile,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::callgraph::CallGraph;
    use crate::parser::{FileSymbols, Symbol};

    fn make_sym(module: &str, qualname: &str, file: &str) -> Symbol {
        Symbol {
            module: module.to_string(),
            qualname: qualname.to_string(),
            file: file.to_string(),
            line: 1,
            end_line: 5,
        }
    }

    #[test]
    fn test_exclude_filters_modules_and_uncovered() {
        // target is called by a caller in __tests__ dir and a caller in src dir
        let fs1 = FileSymbols {
            defined: vec![make_sym("mod_a", "target", "src/mod_a.py")],
            calls: vec![],
        };
        let fs2 = FileSymbols {
            defined: vec![make_sym("mod_b", "prod_caller", "src/mod_b.py")],
            calls: vec![("prod_caller".to_string(), "target".to_string())],
        };
        let fs3 = FileSymbols {
            defined: vec![make_sym("mod_c", "test_caller", "__tests__/mod_c.py")],
            calls: vec![("test_caller".to_string(), "target".to_string())],
        };

        let graph = CallGraph::build(&[fs1, fs2, fs3]);
        let changed = vec![make_sym("mod_a", "target", "src/mod_a.py")];
        let test_modules: HashSet<String> = HashSet::new();
        let exclude = vec!["__tests__".to_string()];

        let baseline = graph.compute_baseline(&exclude);
        let scores = score_all(&changed, &graph, &test_modules, &exclude, &baseline);
        assert_eq!(scores.len(), 1);
        let s = &scores[0];

        // Only prod_caller should be counted (test_caller is excluded)
        assert_eq!(s.direct_callers, 1);
        assert_eq!(s.transitive_callers, 1);
        // modules: mod_a (self) + mod_b (prod_caller) = 2, NOT 3
        assert_eq!(s.modules_affected, 2);
        // uncovered: only prod_caller (1), NOT test_caller
        assert_eq!(s.uncovered_callers, 1);
        // uncovered_ratio: 1/1 = 1.0
        assert_eq!(s.uncovered_ratio, 1.0);
    }

    #[test]
    fn test_exclude_all_callers_zeros_modules_and_uncovered() {
        // All callers are in excluded dirs — modules and uncovered should be minimal
        let fs1 = FileSymbols {
            defined: vec![make_sym("mod_a", "target", "src/mod_a.py")],
            calls: vec![],
        };
        let fs2 = FileSymbols {
            defined: vec![make_sym("mod_t1", "caller1", "__tests__/mod_t1.py")],
            calls: vec![("caller1".to_string(), "target".to_string())],
        };
        let fs3 = FileSymbols {
            defined: vec![make_sym("mod_t2", "caller2", "__tests__/mod_t2.py")],
            calls: vec![("caller2".to_string(), "target".to_string())],
        };

        let graph = CallGraph::build(&[fs1, fs2, fs3]);
        let changed = vec![make_sym("mod_a", "target", "src/mod_a.py")];
        let test_modules: HashSet<String> = HashSet::new();
        let exclude = vec!["__tests__".to_string()];

        let baseline = graph.compute_baseline(&exclude);
        let scores = score_all(&changed, &graph, &test_modules, &exclude, &baseline);
        let s = &scores[0];

        assert_eq!(s.direct_callers, 0);
        assert_eq!(s.transitive_callers, 0);
        // Only the symbol's own module
        assert_eq!(s.modules_affected, 1);
        assert_eq!(s.uncovered_callers, 0);
        // uncovered_ratio: 0/max(0,1) = 0.0
        assert_eq!(s.uncovered_ratio, 0.0);
    }
}
