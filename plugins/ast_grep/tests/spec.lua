local core = require("astgrep")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq
local has = th.has

local FILE = maki.fs.abspath("/tmp/maki_astgrep_spec/a.rs")
local OTHER_FILE = maki.fs.abspath("/tmp/maki_astgrep_spec/b.rs")
local LONG_LINE = string.rep("α", 10)
local META_A = "$A=2 $Z=1"

local function match(file, line, text, extra)
  local m = {
    file = file,
    text = text,
    lines = text,
    range = { start = { line = line, column = 0 }, ["end"] = { line = line, column = #text } },
  }
  for key, value in pairs(extra or {}) do
    m[key] = value
  end
  return m
end

local function rows_of(entries, index)
  return entries[index or 1].rows
end

case("search_argv_puts_flags_before_the_path", function()
  local argv = core.json_argv({ pattern = "fn $F() {}", lang = "rust", include = "*.rs", path = "src/" })
  eq(table.concat(argv, " "), "ast-grep run -p fn $F() {} -l rust --globs *.rs --json=compact src/")
end)

case("search_argv_covers_kind_and_strictness", function()
  local argv = core.json_argv({ kind = "function_item", strictness = "smart" })
  eq(table.concat(argv, " "), "ast-grep run -k function_item --strictness smart --json=compact")
end)

case("apply_argv_rewrites_without_json", function()
  local argv = core.apply_argv({ pattern = "a($X)", rewrite = "b($X)", path = "src" })
  eq(table.concat(argv, " "), "ast-grep run -p a($X) -r b($X) --update-all src")
end)

case("validate_requires_a_selector", function()
  eq(core.validate({}, false), core.SEARCH_REQUIRED)
  eq(core.validate({ kind = "function_item" }, false), nil)
  eq(core.validate({ pattern = "fn $F() {}" }, false), nil)
end)

case("validate_requires_rewrite_and_path", function()
  eq(core.validate({ pattern = "a" }, true), core.REWRITE_REQUIRED)
  eq(core.validate({ pattern = "a", rewrite = "b" }, true), core.PATH_REQUIRED)
  eq(core.validate({ pattern = "a", rewrite = "b", path = "/tmp" }, true), nil)
end)

case("parse_matches_groups_rows_per_file", function()
  local stdout = maki.json.encode({
    match(FILE, 4, "fn main() {}", { metaVariables = { single = { F = { text = "main" } } } }),
    match(FILE, 9, "fn test() {\n  go();\n}"),
    match(OTHER_FILE, 0, "fn other() {}"),
  })

  local entries, total = core.parse_matches(stdout, 10, 500)

  eq(total, 3)
  eq(#entries, 2)
  eq(entries[1].path, FILE)

  local rows = rows_of(entries)
  eq(#rows, 5)
  eq(rows[1].kind, "line")
  eq(rows[1].nr, 5)
  eq(rows[2].kind, "meta")
  eq(rows[2].text, "$F=main")
  eq(rows[3].kind, "line")
  eq(rows[3].nr, 10)
  eq(rows[4].nr, 11)
  eq(rows[5].nr, 12)
  eq(rows_of(entries, 2)[1].nr, 1)
end)

case("parse_matches_keeps_replacements", function()
  local stdout = maki.json.encode({ match(FILE, 0, "foo()", { replacement = "bar()" }) })

  local entries = core.parse_matches(stdout, 10, 500)
  local rows = rows_of(entries)

  eq(#rows, 2)
  eq(rows[2].kind, "new")
  eq(rows[2].text, "bar()")
end)

case("parse_matches_honours_limit_but_reports_the_total", function()
  local stdout = maki.json.encode({
    match(FILE, 0, "first()"),
    match(FILE, 3, "second()"),
  })

  local entries, total = core.parse_matches(stdout, 1, 500)

  eq(total, 2)
  eq(#rows_of(entries), 1)
end)

case("parse_matches_sorts_meta_bindings", function()
  local stdout = maki.json.encode({
    match(FILE, 0, "call()", {
      metaVariables = { single = { Z = { text = "1" }, A = { text = "2" } } },
    }),
  })

  local rows = rows_of(core.parse_matches(stdout, 10, 500))
  eq(rows[2].text, META_A)
end)

case("parse_matches_cuts_long_lines_on_a_utf8_boundary", function()
  local stdout = maki.json.encode({ match(FILE, 0, LONG_LINE) })

  local rows = rows_of(core.parse_matches(stdout, 10, 9))
  eq(rows[1].text, string.rep("α", 4) .. "…")
end)

case("format_round_trips_through_parse_output", function()
  local stdout = maki.json.encode({
    match(FILE, 4, "fn main() {}", { metaVariables = { single = { F = { text = "main" } } } }),
    match(FILE, 12, "fn x() { go(); }", { replacement = "fn x() { stop(); }" }),
    match(OTHER_FILE, 0, "fn other() {}"),
  })
  local entries = core.parse_matches(stdout, 10, 500)

  local text = core.format(entries)
  has(text, FILE .. ":")
  has(text, "  5: fn main() {}")
  has(text, "  $F=main")
  has(text, "   -> fn x() { stop(); }")

  local restored = core.parse_output(text)
  eq(core.format(restored), text)
  eq(restored[1].rows[1].kind, "line")
  eq(restored[1].rows[2].kind, "meta")
  eq(restored[1].rows[4].kind, "new")
end)

case("parse_output_drops_notes_between_entries", function()
  local note = string.format(core.LIMIT_FMT, 1, 5)
  local entries = core.parse_output("src/a.rs:\n  1: x\n\n" .. note .. "\nsecond.rs:\n  9: y")

  eq(#entries, 2)
  eq(#entries[1].rows, 1)
  eq(#entries[2].rows, 1)
  eq(entries[2].rows[1].nr, 9)
end)

case("applied_count_reads_ast_grep_and_falls_back", function()
  eq(core.applied_count("Applied 7 changes\n", 0), 7)
  eq(core.applied_count("", 3), 3)
  eq(core.applied_count(nil, 4), 4)
end)

case("plural_files_agrees_on_the_file_count", function()
  eq(core.plural_files(core.MATCH_FMT, 2, 1), "2 matches in 1 file")
  eq(core.plural_files(core.CHANGED_FMT, 3, 2), "3 changes in 2 files")
  eq(string.format(core.APPLIED_FMT, core.plural_files(core.CHANGED_FMT, 1, 1)), "Applied 1 changes in 1 file.")
end)

case("limit_note_names_both_counts", function()
  has(core.limit_note(1, 5), "1 of 5")
end)

th.report()
