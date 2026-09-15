use std::path::{Path, PathBuf};

fn compile_grammar(name: &str, grammar_dir: &Path, extra_include: Option<&Path>) {
    if !grammar_dir.join("src").exists() {
        eprintln!(
            "Warning: grammar src not found at {}, skipping {}",
            grammar_dir.display(),
            name
        );
        return;
    }

    let src_dir = grammar_dir.join("src");
    let mut build = cc::Build::new();
    build
        .include(&src_dir)
        .flag_if_supported("-Wno-unused-value")
        .flag_if_supported("-Wno-implicit-function-declaration")
        .flag_if_supported("-Wno-implicit-int-conversion")
        .flag_if_supported("-Wno-unused-variable")
        .flag_if_supported("-std=c11");

    if let Some(extra) = extra_include {
        if extra.exists() {
            build.include(extra);
        }
    }

    for entry in std::fs::read_dir(&src_dir).expect("read src dir") {
        let entry = entry.expect("read entry");
        let path = entry.path();
        if let Some(ext) = path.extension() {
            if ext == "c" {
                build.file(&path);
            }
        }
    }

    build.compile(name);
}

fn main() {
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir.parent().unwrap().parent().unwrap();

    let c_grammar_dir = workspace_root.join("grammars").join("tree-sitter-c");
    let cpp_grammar_dir = workspace_root.join("grammars").join("tree-sitter-cpp");

    // C grammar first
    compile_grammar("tree-sitter-c", &c_grammar_dir, None);

    // C++ grammar (depends on C grammar headers)
    compile_grammar(
        "tree-sitter-cpp",
        &cpp_grammar_dir,
        Some(c_grammar_dir.join("src").as_path()),
    );

    // Copy grammar JSON to OUT_DIR
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let grammars_out = out_dir.join("grammars");
    std::fs::create_dir_all(&grammars_out).ok();

    if c_grammar_dir.join("grammar.json").exists() {
        std::fs::copy(
            c_grammar_dir.join("grammar.json"),
            grammars_out.join("tree-sitter-c.json"),
        )
        .ok();
    }
    if cpp_grammar_dir.join("grammar.json").exists() {
        std::fs::copy(
            cpp_grammar_dir.join("grammar.json"),
            grammars_out.join("tree-sitter-cpp.json"),
        )
        .ok();
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}",
        c_grammar_dir.join("src").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        cpp_grammar_dir.join("src").display()
    );
}
