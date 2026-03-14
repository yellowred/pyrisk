use std::collections::{HashMap, HashSet, VecDeque};

use crate::parser::{FileSymbols, Symbol};

/// Check if a qualifier (e.g., "my_task" from "my_task.delay()") is itself
/// a known top-level function. Returns matching targets or empty vec.
fn resolve_qualifier_as_function(
    qual: &str,
    short_name_index: &HashMap<String, Vec<String>>,
    graph: &CallGraph,
) -> Vec<String> {
    if let Some(qual_targets) = short_name_index.get(qual) {
        qual_targets
            .iter()
            .filter(|t| {
                graph.symbols.get(*t)
                    .map(|s| !s.qualname.contains('.'))
                    .unwrap_or(false)
            })
            .cloned()
            .collect()
    } else {
        vec![]
    }
}

#[derive(Debug, Default)]
pub struct CallGraph {
    /// symbol full_name -> set of symbols that call it (their full_names)
    pub callers: HashMap<String, HashSet<String>>,
    /// symbol full_name -> set of symbols it calls (their full_names)
    pub callees: HashMap<String, HashSet<String>>,
    /// full_name -> Symbol metadata
    pub symbols: HashMap<String, Symbol>,
}

impl CallGraph {
    pub fn build(all_files: &[FileSymbols]) -> Self {
        let mut graph = CallGraph::default();

        // Index: short_name -> Vec<full_name>
        let mut short_name_index: HashMap<String, Vec<String>> = HashMap::new();

        // Register all defined symbols
        for fs in all_files {
            for sym in &fs.defined {
                let full = sym.full_name();
                graph.symbols.insert(full.clone(), sym.clone());

                // Index by the last component of qualname (the actual function/method name)
                let short = sym.qualname.rsplit('.').next().unwrap_or(&sym.qualname);
                short_name_index
                    .entry(short.to_string())
                    .or_default()
                    .push(full);
            }
        }

        // Resolve calls
        for fs in all_files {
            let module = fs.defined.first().map(|s| &s.module);
            for (caller_qualname, callee_name) in &fs.calls {
                // Build caller full_name
                let caller_full = if let Some(m) = module {
                    format!("{}.{}", m, caller_qualname)
                } else {
                    continue;
                };

                if !graph.symbols.contains_key(&caller_full) {
                    continue;
                }

                // Parse qualifier from callee_name (e.g., "self.create" or "ClassName.create")
                let (qualifier, method) = if let Some(dot_pos) = callee_name.rfind('.') {
                    (Some(&callee_name[..dot_pos]), &callee_name[dot_pos + 1..])
                } else {
                    (None, callee_name.as_str())
                };

                // Try to resolve callee by short name
                let resolved: Vec<String> = if let Some(targets) = short_name_index.get(method) {
                    let is_self_or_cls = qualifier == Some("self") || qualifier == Some("cls");

                    if let Some(qual) = qualifier {
                        // Resolve self/cls to the caller's class name
                        let class_name = if is_self_or_cls {
                            // "ClassName.method_name" -> "ClassName"
                            caller_qualname.rsplit('.').nth(1)
                        } else {
                            Some(qual)
                        };

                        if let Some(cls) = class_name {
                            let suffix = format!("{}.{}", cls, method);
                            let filtered: Vec<&String> = targets
                                .iter()
                                .filter(|t| {
                                    graph
                                        .symbols
                                        .get(*t)
                                        .map(|s| s.qualname.ends_with(&suffix))
                                        .unwrap_or(false)
                                })
                                .collect();

                            if filtered.is_empty() && is_self_or_cls {
                                // Fallback only for self/cls (handles inheritance)
                                targets.clone()
                            } else if filtered.is_empty() && !is_self_or_cls {
                                // For non-self qualifiers that don't match a Class.method pattern,
                                // check if the qualifier itself is a known top-level function
                                // (e.g., Celery task: my_task.delay() → resolve to my_task)
                                resolve_qualifier_as_function(qual, &short_name_index, &graph)
                            } else {
                                filtered.into_iter().cloned().collect()
                            }
                        } else {
                            // self/cls used in top-level function — treat as unqualified
                            targets.clone()
                        }
                    } else {
                        // Unqualified call: prefer same-module targets
                        if targets.len() > 1 {
                            if let Some(m) = module {
                                let same_module: Vec<String> = targets
                                    .iter()
                                    .filter(|t| {
                                        graph
                                            .symbols
                                            .get(*t)
                                            .map(|s| s.module == **m)
                                            .unwrap_or(false)
                                    })
                                    .cloned()
                                    .collect();
                                if !same_module.is_empty() {
                                    same_module
                                } else {
                                    targets.clone()
                                }
                            } else {
                                targets.clone()
                            }
                        } else {
                            targets.clone()
                        }
                    }
                } else if let Some(qual) = qualifier {
                    // Method name not found in index at all.
                    // Check if qualifier is a known top-level function
                    // (e.g., my_task.delay() where "delay" isn't defined anywhere)
                    let is_self_or_cls = qualifier == Some("self") || qualifier == Some("cls");
                    if !is_self_or_cls {
                        resolve_qualifier_as_function(qual, &short_name_index, &graph)
                    } else {
                        vec![]
                    }
                } else {
                    vec![]
                };

                for target in &resolved {
                    graph
                        .callers
                        .entry(target.clone())
                        .or_default()
                        .insert(caller_full.clone());
                    graph
                        .callees
                        .entry(caller_full.clone())
                        .or_default()
                        .insert(target.clone());
                }
            }
        }

        graph
    }

    /// BFS: return all transitive callers of a symbol, with depth
    pub fn blast_radius(&self, sym: &str) -> Vec<(String, usize)> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut queue: VecDeque<(String, usize)> = VecDeque::new();
        let mut result: Vec<(String, usize)> = Vec::new();

        visited.insert(sym.to_string());
        queue.push_back((sym.to_string(), 0));

        while let Some((current, depth)) = queue.pop_front() {
            if depth > 0 {
                result.push((current.clone(), depth));
            }
            if depth >= 10 {
                continue; // Cap depth
            }
            if let Some(callers) = self.callers.get(&current) {
                for caller in callers {
                    if visited.insert(caller.clone()) {
                        queue.push_back((caller.clone(), depth + 1));
                    }
                }
            }
        }

        result
    }

    /// BFS: find the caller chain from `sym` up to `target`.
    /// Returns the path as [sym, intermediate…, target] or empty if unreachable.
    pub fn call_chain(&self, sym: &str, target: &str) -> Vec<String> {
        let mut visited: HashSet<String> = HashSet::new();
        let mut parent: HashMap<String, String> = HashMap::new();
        let mut queue: VecDeque<String> = VecDeque::new();

        visited.insert(sym.to_string());
        queue.push_back(sym.to_string());

        while let Some(current) = queue.pop_front() {
            if current == target {
                // Reconstruct path
                let mut path = vec![current.clone()];
                let mut node = &current;
                while let Some(p) = parent.get(node) {
                    path.push(p.clone());
                    node = p;
                }
                path.reverse();
                return path;
            }
            if let Some(callers) = self.callers.get(&current) {
                for caller in callers {
                    if visited.insert(caller.clone()) {
                        parent.insert(caller.clone(), current.clone());
                        queue.push_back(caller.clone());
                    }
                }
            }
        }

        Vec::new()
    }

    /// Compute baseline: sorted direct caller counts for every symbol in the graph.
    /// Uses direct callers (O(1) per symbol) as a fast proxy for connectivity.
    pub fn compute_baseline(&self, exclude: &[String]) -> RepoBaseline {
        let mut counts: Vec<usize> = self
            .symbols
            .keys()
            .map(|full_name| self.direct_callers_filtered(full_name, exclude))
            .collect();
        counts.sort_unstable();
        RepoBaseline { sorted_counts: counts }
    }

    /// Direct callers count
    pub fn direct_callers(&self, sym: &str) -> usize {
        self.callers.get(sym).map(|s| s.len()).unwrap_or(0)
    }

    /// Direct callers count, excluding callers whose file path matches any exclude pattern
    pub fn direct_callers_filtered(&self, sym: &str, exclude: &[String]) -> usize {
        if exclude.is_empty() {
            return self.direct_callers(sym);
        }
        self.callers
            .get(sym)
            .map(|callers| {
                callers
                    .iter()
                    .filter(|c| {
                        self.symbols
                            .get(*c)
                            .map(|s| !exclude.iter().any(|ex| s.file.contains(ex.as_str())))
                            .unwrap_or(true)
                    })
                    .count()
            })
            .unwrap_or(0)
    }
}

#[derive(Debug)]
pub struct RepoBaseline {
    pub sorted_counts: Vec<usize>,
}

impl RepoBaseline {
    /// Returns the fraction of symbols with transitive caller count <= `count`.
    /// Result is in [0.0, 1.0].
    pub fn percentile_of(&self, count: usize) -> f64 {
        if self.sorted_counts.is_empty() {
            return 0.0;
        }
        let pos = self.sorted_counts.partition_point(|&c| c <= count);
        pos as f64 / self.sorted_counts.len() as f64
    }

    /// Return the value at the given percentile (0-100).
    pub fn p(&self, pct: usize) -> usize {
        if self.sorted_counts.is_empty() {
            return 0;
        }
        let idx = (pct as f64 / 100.0 * self.sorted_counts.len() as f64) as usize;
        self.sorted_counts[idx.min(self.sorted_counts.len() - 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{FileSymbols, Symbol};

    fn make_sym(module: &str, qualname: &str) -> Symbol {
        Symbol {
            module: module.to_string(),
            qualname: qualname.to_string(),
            file: "test.py".to_string(),
            line: 1,
            end_line: 5,
        }
    }

    #[test]
    fn test_basic_call_graph() {
        let fs = FileSymbols {
            defined: vec![
                make_sym("mod", "foo"),
                make_sym("mod", "bar"),
                make_sym("mod", "baz"),
            ],
            calls: vec![
                ("bar".to_string(), "foo".to_string()),
                ("baz".to_string(), "bar".to_string()),
            ],
        };

        let graph = CallGraph::build(&[fs]);
        assert_eq!(graph.direct_callers("mod.foo"), 1);

        let radius = graph.blast_radius("mod.foo");
        assert_eq!(radius.len(), 2); // bar and baz
    }

    #[test]
    fn test_qualified_call_disambiguates() {
        // Two classes with a "create" method
        let fs1 = FileSymbols {
            defined: vec![
                make_sym("mod_a", "Alpha.create"),
                make_sym("mod_b", "Beta.create"),
            ],
            calls: vec![],
        };

        // A caller that does Alpha.create() — should only link to Alpha.create
        let fs2 = FileSymbols {
            defined: vec![make_sym("mod_c", "do_stuff")],
            calls: vec![("do_stuff".to_string(), "Alpha.create".to_string())],
        };

        let graph = CallGraph::build(&[fs1, fs2]);
        assert_eq!(graph.direct_callers("mod_a.Alpha.create"), 1);
        assert_eq!(graph.direct_callers("mod_b.Beta.create"), 0);
    }

    #[test]
    fn test_self_call_disambiguates() {
        // Two classes with a "save" method
        let fs = FileSymbols {
            defined: vec![
                make_sym("mod", "Foo.save"),
                make_sym("mod", "Foo.process"),
                make_sym("mod", "Bar.save"),
            ],
            // Foo.process calls self.save() — should only link to Foo.save
            calls: vec![("Foo.process".to_string(), "self.save".to_string())],
        };

        let graph = CallGraph::build(&[fs]);
        assert_eq!(graph.direct_callers("mod.Foo.save"), 1);
        assert_eq!(graph.direct_callers("mod.Bar.save"), 0);
    }

    #[test]
    fn test_unqualified_call_matches_all_when_no_same_module() {
        // Bare call with no same-module candidate — matches all
        let fs1 = FileSymbols {
            defined: vec![make_sym("mod_a", "helper")],
            calls: vec![],
        };
        let fs2 = FileSymbols {
            defined: vec![make_sym("mod_b", "helper")],
            calls: vec![],
        };
        let fs3 = FileSymbols {
            defined: vec![make_sym("mod_c", "caller")],
            calls: vec![("caller".to_string(), "helper".to_string())],
        };

        let graph = CallGraph::build(&[fs1, fs2, fs3]);
        assert_eq!(graph.direct_callers("mod_a.helper"), 1);
        assert_eq!(graph.direct_callers("mod_b.helper"), 1);
    }

    #[test]
    fn test_unqualified_call_prefers_same_module() {
        // Bare call "run()" with same-module candidate — prefers same module
        let fs1 = FileSymbols {
            defined: vec![
                make_sym("mod_a", "caller"),
                make_sym("mod_a", "run"),
            ],
            calls: vec![("caller".to_string(), "run".to_string())],
        };
        let fs2 = FileSymbols {
            defined: vec![make_sym("mod_b", "run")],
            calls: vec![],
        };

        let graph = CallGraph::build(&[fs1, fs2]);
        assert_eq!(graph.direct_callers("mod_a.run"), 1);
        assert_eq!(graph.direct_callers("mod_b.run"), 0);
    }

    #[test]
    fn test_self_fallback_for_inheritance() {
        // self.save() where save is inherited from Parent — falls back to all
        let fs = FileSymbols {
            defined: vec![
                make_sym("mod", "Parent.save"),
                make_sym("mod", "Child.process"),
            ],
            calls: vec![("Child.process".to_string(), "self.save".to_string())],
        };

        let graph = CallGraph::build(&[fs]);
        assert_eq!(graph.direct_callers("mod.Parent.save"), 1);
    }

    #[test]
    fn test_method_call_on_known_function_resolves_to_function() {
        // Simulates: my_task.delay() where my_task is a top-level function (Celery task)
        let fs = FileSymbols {
            defined: vec![
                make_sym("mod", "my_task"),
                make_sym("mod", "caller"),
            ],
            calls: vec![
                ("caller".to_string(), "my_task.delay".to_string()),
                ("caller".to_string(), "my_task.gen_delay".to_string()),
                ("caller".to_string(), "my_task.run".to_string()),
            ],
        };

        let graph = CallGraph::build(&[fs]);
        assert_eq!(graph.direct_callers("mod.my_task"), 1); // caller (deduplicated)
    }

    #[test]
    fn test_method_call_on_class_method_no_false_positive() {
        // obj.save() should NOT match a top-level function "obj" if "obj" isn't defined
        // but SHOULD NOT match MyClass.save either (qualifier "obj" != "MyClass")
        let fs = FileSymbols {
            defined: vec![
                make_sym("mod", "MyClass.save"),
                make_sym("mod", "do_stuff"),
            ],
            calls: vec![("do_stuff".to_string(), "obj.save".to_string())],
        };

        let graph = CallGraph::build(&[fs]);
        assert_eq!(graph.direct_callers("mod.MyClass.save"), 0);
    }

    #[test]
    fn test_variable_qualifier_no_fallback() {
        // variable.create() where variable type is unknown — should NOT
        // fall back to matching unrelated functions named "create"
        let fs1 = FileSymbols {
            defined: vec![make_sym("mod_a", "Unrelated.create")],
            calls: vec![],
        };
        let fs2 = FileSymbols {
            defined: vec![make_sym("mod_b", "do_stuff")],
            // factory.create() — "factory" isn't a known class
            calls: vec![("do_stuff".to_string(), "factory.create".to_string())],
        };

        let graph = CallGraph::build(&[fs1, fs2]);
        assert_eq!(graph.direct_callers("mod_a.Unrelated.create"), 0);
    }
}
