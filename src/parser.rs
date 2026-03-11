use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol {
    pub module: String,
    pub qualname: String,
    pub file: String,
    pub line: usize,
    pub end_line: usize,
}

impl Symbol {
    pub fn full_name(&self) -> String {
        format!("{}.{}", self.module, self.qualname)
    }
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct FileSymbols {
    pub defined: Vec<Symbol>,
    pub calls: Vec<(String, String)>, // (caller_qualname, callee_name_str)
}

fn module_from_path(path: &Path, repo_root: &Path) -> String {
    let rel = path.strip_prefix(repo_root).unwrap_or(path);
    let s = rel.to_string_lossy();
    let s = s.strip_suffix(".py").unwrap_or(&s);
    let s = s.replace('/', ".").replace('\\', ".");
    // Strip trailing .__init__
    s.strip_suffix(".__init__").unwrap_or(&s).to_string()
}

struct DefInfo {
    qualname: String,
    start_line: usize,
    end_line: usize,
}

pub fn parse_file(path: &Path, repo_root: &Path) -> Result<FileSymbols> {
    let source =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_python::LANGUAGE;
    parser
        .set_language(&language.into())
        .context("setting Python language")?;

    let tree = parser.parse(&source, None).context("parsing failed")?;
    let root = tree.root_node();
    let module = module_from_path(path, repo_root);
    let file_str = path.to_string_lossy().to_string();

    let mut result = FileSymbols::default();
    let mut defs: Vec<DefInfo> = Vec::new();

    // Walk the tree to extract definitions and calls
    collect_definitions(root, &source, &module, &file_str, &mut result, &mut defs, &mut Vec::new());
    collect_calls(root, &source, &defs, &mut result);

    Ok(result)
}

fn collect_definitions(
    node: tree_sitter::Node,
    source: &str,
    module: &str,
    file_str: &str,
    result: &mut FileSymbols,
    defs: &mut Vec<DefInfo>,
    name_stack: &mut Vec<String>,
) {
    let kind = node.kind();

    match kind {
        "class_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let class_name = &source[name_node.byte_range()];
                name_stack.push(class_name.to_string());
                // Recurse into class body
                for i in 0..node.child_count() {
                    let child = node.child(i).unwrap();
                    collect_definitions(child, source, module, file_str, result, defs, name_stack);
                }
                name_stack.pop();
                return; // Already recursed
            }
        }
        "function_definition" => {
            if let Some(name_node) = node.child_by_field_name("name") {
                let fn_name = &source[name_node.byte_range()];
                let qualname = if name_stack.is_empty() {
                    fn_name.to_string()
                } else {
                    format!("{}.{}", name_stack.join("."), fn_name)
                };
                let start_line = node.start_position().row + 1;
                let end_line = node.end_position().row + 1;

                result.defined.push(Symbol {
                    module: module.to_string(),
                    qualname: qualname.clone(),
                    file: file_str.to_string(),
                    line: start_line,
                    end_line,
                });

                defs.push(DefInfo {
                    qualname: qualname.clone(),
                    start_line,
                    end_line,
                });

                // Recurse into nested functions
                name_stack.push(fn_name.to_string());
                for i in 0..node.child_count() {
                    let child = node.child(i).unwrap();
                    collect_definitions(child, source, module, file_str, result, defs, name_stack);
                }
                name_stack.pop();
                return;
            }
        }
        _ => {}
    }

    // Default: recurse into children
    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        collect_definitions(child, source, module, file_str, result, defs, name_stack);
    }
}

fn collect_calls(
    node: tree_sitter::Node,
    source: &str,
    defs: &[DefInfo],
    result: &mut FileSymbols,
) {
    if node.kind() == "call" {
        if let Some(func_node) = node.child_by_field_name("function") {
            let callee_name = extract_callee_name(func_node, source);
            if let Some(callee_name) = callee_name {
                let call_line = node.start_position().row + 1;
                // Find enclosing function
                let caller_qualname = find_enclosing_def(call_line, defs);
                if let Some(caller) = caller_qualname {
                    result.calls.push((caller, callee_name));
                }
            }
        }
    }

    for i in 0..node.child_count() {
        let child = node.child(i).unwrap();
        collect_calls(child, source, defs, result);
    }
}

fn extract_callee_name(node: tree_sitter::Node, source: &str) -> Option<String> {
    match node.kind() {
        "identifier" => Some(source[node.byte_range()].to_string()),
        "attribute" => {
            let attr_node = node.child_by_field_name("attribute")?;
            let attr_name = &source[attr_node.byte_range()];

            if let Some(obj_node) = node.child_by_field_name("object") {
                if obj_node.kind() == "identifier" {
                    let obj_name = &source[obj_node.byte_range()];
                    return Some(format!("{}.{}", obj_name, attr_name));
                }
            }
            Some(attr_name.to_string())
        }
        _ => None,
    }
}

fn find_enclosing_def(line: usize, defs: &[DefInfo]) -> Option<String> {
    // Find the innermost function definition that contains this line
    let mut best: Option<&DefInfo> = None;
    for d in defs {
        if line >= d.start_line && line <= d.end_line {
            match best {
                None => best = Some(d),
                Some(prev) => {
                    // Prefer the more nested (smaller range) def
                    if (d.end_line - d.start_line) < (prev.end_line - prev.start_line) {
                        best = Some(d);
                    }
                }
            }
        }
    }
    best.map(|d| d.qualname.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn parse_snippet(code: &str) -> FileSymbols {
        let mut f = NamedTempFile::with_suffix(".py").unwrap();
        f.write_all(code.as_bytes()).unwrap();
        let path = f.path().to_path_buf();
        let root = path.parent().unwrap();
        parse_file(&path, root).unwrap()
    }

    #[test]
    fn test_function_defs() {
        let fs = parse_snippet(
            r#"
def foo():
    pass

def bar():
    foo()
"#,
        );
        assert_eq!(fs.defined.len(), 2);
        assert_eq!(fs.defined[0].qualname, "foo");
        assert_eq!(fs.defined[1].qualname, "bar");
    }

    #[test]
    fn test_class_methods() {
        let fs = parse_snippet(
            r#"
class MyClass:
    def method_a(self):
        pass

    def method_b(self):
        self.method_a()
"#,
        );
        assert_eq!(fs.defined.len(), 2);
        assert_eq!(fs.defined[0].qualname, "MyClass.method_a");
        assert_eq!(fs.defined[1].qualname, "MyClass.method_b");
    }

    #[test]
    fn test_calls() {
        let fs = parse_snippet(
            r#"
def foo():
    pass

def bar():
    foo()
    x.baz()
"#,
        );
        assert_eq!(fs.calls.len(), 2);
        assert_eq!(fs.calls[0], ("bar".to_string(), "foo".to_string()));
        assert_eq!(fs.calls[1], ("bar".to_string(), "x.baz".to_string()));
    }

    #[test]
    fn test_nested_functions() {
        let fs = parse_snippet(
            r#"
def outer():
    def inner():
        pass
    inner()
"#,
        );
        assert_eq!(fs.defined.len(), 2);
        assert_eq!(fs.defined[0].qualname, "outer");
        assert_eq!(fs.defined[1].qualname, "outer.inner");
    }

    #[test]
    fn test_attribute_calls_preserve_qualifier() {
        let fs = parse_snippet(
            r#"
def my_task():
    pass

def caller():
    my_task.delay(1, 2)
    my_task.apply_async(args=[1])
    my_task.s(1)
"#,
        );
        // Parser preserves the full qualified name; resolution happens in callgraph
        assert_eq!(fs.calls.len(), 3);
        assert_eq!(fs.calls[0], ("caller".to_string(), "my_task.delay".to_string()));
        assert_eq!(fs.calls[1], ("caller".to_string(), "my_task.apply_async".to_string()));
        assert_eq!(fs.calls[2], ("caller".to_string(), "my_task.s".to_string()));
    }
}
