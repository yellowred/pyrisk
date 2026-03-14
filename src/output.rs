use std::collections::HashSet;

use comfy_table::{modifiers::UTF8_ROUND_CORNERS, presets::UTF8_FULL, ContentArrangement, Table, Cell, Color};
use serde::Serialize;

use crate::callgraph::CallGraph;
use crate::scorer::RiskScore;

pub fn render_table(scores: &[RiskScore], verbose: bool) {
    if scores.is_empty() {
        println!("No changed symbols found.");
        return;
    }

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL)
        .apply_modifier(UTF8_ROUND_CORNERS)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(vec!["SYMBOL", "CALLERS", "MODULES", "UNCOVERED", "PERCENTILE", "RISK"]);

    for s in scores {
        let sym_name = s.symbol.full_name();
        let callers_str = format!("{} ({})", s.direct_callers, s.transitive_callers);
        let uncovered_str = format!("{} ({:.0}%)", s.uncovered_callers, s.uncovered_ratio * 100.0);
        let percentile_str = format!("p{:.0}", s.percentile * 100.0);
        let risk_str = format!("{} {}", s.risk_bar(), s.risk_label());

        let color = match s.risk_label() {
            "HIGH" => Color::Red,
            "MED" => Color::Yellow,
            _ => Color::DarkGrey,
        };

        table.add_row(vec![
            Cell::new(&sym_name),
            Cell::new(&callers_str),
            Cell::new(s.modules_affected),
            Cell::new(&uncovered_str),
            Cell::new(&percentile_str),
            Cell::new(&risk_str).fg(color),
        ]);
    }

    println!("{table}");

    if verbose {
        println!("\n--- Detailed caller breakdown ---\n");
        for s in scores {
            println!("{}:", s.symbol.full_name());
            println!(
                "  Direct: {}, Transitive: {}, Percentile: p{:.0}, Uncovered: {}/{} ({:.0}%), Score: {:.1}",
                s.direct_callers, s.transitive_callers,
                s.percentile * 100.0,
                s.uncovered_callers, s.transitive_callers,
                s.uncovered_ratio * 100.0,
                s.score
            );
            println!();
        }
    }
}

#[derive(Serialize)]
struct JsonEntry {
    symbol: String,
    module: String,
    file: String,
    line: usize,
    direct_callers: usize,
    transitive_callers: usize,
    modules_affected: usize,
    uncovered_callers: usize,
    uncovered_ratio: f64,
    percentile: f64,
    score: f64,
    risk: String,
}

pub fn render_json(scores: &[RiskScore]) {
    let entries: Vec<JsonEntry> = scores
        .iter()
        .map(|s| JsonEntry {
            symbol: s.symbol.full_name(),
            module: s.symbol.module.clone(),
            file: s.symbol.file.clone(),
            line: s.symbol.line,
            direct_callers: s.direct_callers,
            transitive_callers: s.transitive_callers,
            modules_affected: s.modules_affected,
            uncovered_callers: s.uncovered_callers,
            uncovered_ratio: s.uncovered_ratio,
            percentile: s.percentile,
            score: s.score,
            risk: s.risk_label().to_string(),
        })
        .collect();

    println!("{}", serde_json::to_string_pretty(&entries).unwrap());
}

pub fn render_uncovered(
    pattern: &str,
    scores: &[RiskScore],
    graph: &CallGraph,
    test_modules: &HashSet<String>,
    exclude: &[String],
) {
    // Find matching symbols by substring match on full_name
    let matches: Vec<&RiskScore> = scores
        .iter()
        .filter(|s| s.symbol.full_name().contains(pattern))
        .collect();

    if matches.is_empty() {
        println!("No symbols matching '{}' found in changed set.", pattern);
        return;
    }

    for s in &matches {
        let full = s.symbol.full_name();
        let radius = graph.blast_radius(&full);

        // Filter to callers whose module is not in test_modules
        // and whose file path does not contain any excluded folder
        let uncovered: Vec<_> = radius
            .iter()
            .filter_map(|(caller_name, _depth)| {
                let caller_sym = graph.symbols.get(caller_name)?;
                if !test_modules.contains(&caller_sym.module)
                    && !exclude.iter().any(|ex| caller_sym.file.contains(ex))
                {
                    Some(caller_sym)
                } else {
                    None
                }
            })
            .collect();

        println!("{}  ({} uncovered callers)", full, uncovered.len());
        for caller in &uncovered {
            let caller_full = caller.full_name();
            let chain = graph.call_chain(&full, &caller_full);
            if chain.len() <= 2 {
                // Direct caller — no intermediate chain to show
                println!(
                    "  {}:{}  [{}]",
                    caller.file, caller.line, caller.module
                );
            } else {
                // Show the intermediate call chain
                println!(
                    "  {}:{}  [{}]",
                    caller.file, caller.line, caller.module
                );
                let steps: Vec<&str> = chain.iter().rev().map(|s| s.as_str()).collect();
                println!("    chain: {}", steps.join(" calls "));
            }
        }
        if uncovered.is_empty() {
            println!("  (all callers are covered by tests)");
        }
        println!();
    }
}

