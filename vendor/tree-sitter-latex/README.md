`scanner.c` comes from https://github.com/latex-lsp/tree-sitter-latex at commit
fa8df44, unchanged. The `tree-sitter-latex` 0.1.0 crate publishes the parser
without it, so the crate doesn't link on its own. `tree_sitter/parser.h` comes
from that crate. Kyte's `build.rs` compiles the scanner.
