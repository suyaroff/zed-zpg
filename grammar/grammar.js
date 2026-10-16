// One grammar for all zpg files: table pages (.pgt), the catalog (.pgdb) and
// row views (.pgr). Lines are explicit so that indented block lines and cells
// can't be confused; there are no extras, spaces belong to the tokens.
module.exports = grammar({
  name: "zpg",

  extras: (_) => [],

  rules: {
    document: ($) => seq(repeat(choice("\n", seq($._line, "\n"))), optional($._line)),

    _line: ($) =>
      choice(
        $.header,
        $.comment,
        $.separator,
        $.row,
        $.section,
        $.error_line,
        $.relation,
        $.field,
        $.block_line,
      ),

    header: (_) => token(prec(3, /# zpg v1[^\n]*/)),
    comment: (_) => token(prec(2, /#[^\n]*/)),

    // Table pages.
    separator: (_) => token(prec(2, /\|[-:| ]*-[-:| ]*\|[ \t]*/)),
    row: ($) => seq("|", repeat(seq(optional($.cell), "|")), optional($.cell)),
    cell: (_) => token(prec(1, /([^|\\\n]|\\[^\n])+/)),

    // Catalog.
    section: (_) => token(prec(2, /\[[^\]\n]*\][ \t]*/)),
    error_line: (_) => token(prec(2, /![^\n]*/)),
    relation: ($) => seq($.relname, optional($.kind), optional($.estimate)),
    relname: (_) => /[A-Za-z_"][^\s:]*/,
    kind: (_) => token(prec(3, /[ \t]+(table|view|matview|foreign|partitioned)/)),
    estimate: (_) => token(prec(3, /[ \t]+~\d+ rows[ \t]*/)),

    // Row views.
    field: ($) => seq($.field_name, optional($.value)),
    field_name: (_) => token(prec(2, /[^\s|#!\[:][^:\n]*:/)),
    value: (_) => /[^\n]+/,
    block_line: (_) => token(prec(2, /[ \t]+[^\n]*/)),
  },
});
