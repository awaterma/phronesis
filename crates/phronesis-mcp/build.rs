fn main() {
    let source = "vendor/tree-sitter-java/src";
    let mut compiler = cc::Build::new();
    compiler.std("c11").include(source);
    if cfg!(target_env = "msvc") {
        compiler.flag("-utf-8");
    }
    compiler
        .file(format!("{source}/parser.c"))
        .compile("phronesis-java-parser");
    println!("cargo:rerun-if-changed={source}");
}
