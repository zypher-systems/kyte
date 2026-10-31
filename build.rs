//! Compiles the two grammar files that can't come from a crate. See the
//! READMEs under `vendor/`.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(feature = "highlight")]
    {
        for (dir, file, lib) in [
            (
                "vendor/tree-sitter-latex",
                "scanner.c",
                "kyte-latex-scanner",
            ),
            ("vendor/tree-sitter-bibtex", "parser.c", "kyte-bibtex"),
        ] {
            println!("cargo:rerun-if-changed={dir}/{file}");
            cc::Build::new()
                .std("c11")
                .include(dir)
                .file(format!("{dir}/{file}"))
                .warnings(false)
                .compile(lib);
        }
    }
}
