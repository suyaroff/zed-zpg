(header) @preproc
(comment) @comment
(field_name) @property
(block_line) @string

((value) @number
  (#match? @number "^\\s*-?[0-9]+(\\.[0-9]+)?\\s*$"))
((value) @boolean
  (#match? @boolean "^\\s*(true|false)\\s*$"))
((value) @constant.builtin
  (#match? @constant.builtin "^\\s*(NULL|DEFAULT)\\s*$"))
((value) @punctuation.special
  (#match? @punctuation.special "^\\s*\\|\\s*$"))
