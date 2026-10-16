(header) @preproc
(comment) @comment
(separator) @punctuation.delimiter
"|" @punctuation.delimiter

((cell) @number
  (#match? @number "^\\s*-?[0-9]+(\\.[0-9]+)?\\s*$"))
((cell) @boolean
  (#match? @boolean "^\\s*(true|false)\\s*$"))
((cell) @constant.builtin
  (#match? @constant.builtin "^\\s*(NULL|DEFAULT|\\+)\\s*$"))
((cell) @comment
  (#match? @comment "…\\s*$"))
