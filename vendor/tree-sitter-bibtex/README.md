`parser.c` and `tree_sitter/parser.h` come from the `tree-sitter-bibtex` 0.1.0
crate, unchanged. That crate needs an older tree-sitter than Kyte's, so it can't
be a dependency, but its parser format is one tree-sitter 0.27 still reads.
Upstream is https://github.com/latex-lsp/tree-sitter-bibtex.
