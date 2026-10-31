; Colors for tree-sitter-latex, which ships no query. Later patterns win.

(command_name) @function

(begin
  command: _ @keyword
  name: (curly_group_text
    (text) @type))

(end
  command: _ @keyword
  name: (curly_group_text
    (text) @type))

(part
  command: _ @keyword
  text: (curly_group) @text.title)

(chapter
  command: _ @keyword
  text: (curly_group) @text.title)

(section
  command: _ @keyword
  text: (curly_group) @text.title)

(subsection
  command: _ @keyword
  text: (curly_group) @text.title)

(subsubsection
  command: _ @keyword
  text: (curly_group) @text.title)

(paragraph
  command: _ @keyword
  text: (curly_group) @text.title)

(subparagraph
  command: _ @keyword
  text: (curly_group) @text.title)

(package_include
  command: _ @keyword
  paths: (curly_group_path_list) @string)

(class_include
  command: _ @keyword
  path: (curly_group_path) @string)

(citation
  command: _ @function
  keys: (curly_group_text_list) @constant)

(label_definition
  command: _ @function
  name: (curly_group_text) @constant)

(label_reference
  command: _ @function
  names: (curly_group_text_list) @constant)

(caption
  command: _ @keyword)

(graphics_include
  command: _ @keyword
  path: (curly_group_path) @string)

[
  (inline_formula)
  (displayed_equation)
  (math_environment)
] @string

(verbatim_environment
  verbatim: (comment) @text.literal)

(listing_environment
  code: (source_code) @text.literal)

(minted_environment
  code: (source_code) @text.literal)

[
  (line_comment)
  (block_comment)
  (comment_environment)
] @comment
