local h = require("custom_tools_helpers")
local th = require("maki.test_helpers")

local case = th.case
local eq = th.eq

local NO_MATCH_MSG = h.NO_MATCH_MSG

local function write_tool(root, meta_dir, name, tags, description, body)
  local path = maki.fs.joinpath(root, name)
  local _, mkdir_err = maki.fs.mkdir(maki.fs.dirname(path), { parents = true })
  assert(not mkdir_err, tostring(mkdir_err))
  local _, write_err = maki.fs.write(path, body or "#!/bin/sh\necho hi\n")
  assert(not write_err, tostring(write_err))
  local ok, meta_err = h.write_meta(meta_dir, name, tags or {}, description)
  assert(ok, tostring(meta_err))
  return path
end

local function with_read_stub(stub, fn)
  local orig = maki.fs.read
  maki.fs.read = stub
  local ok, err = pcall(fn)
  maki.fs.read = orig
  if not ok then
    error(err, 0)
  end
end

local function with_job_stub(result, fn)
  local orig_start, orig_wait, orig_stop = maki.fn.jobstart, maki.fn.jobwait, maki.fn.jobstop
  maki.fn.jobstart = function()
    return 1
  end
  maki.fn.jobwait = function()
    return result
  end
  maki.fn.jobstop = function() end
  local ok, err = pcall(fn)
  maki.fn.jobstart, maki.fn.jobwait, maki.fn.jobstop = orig_start, orig_wait, orig_stop
  if not ok then
    error(err, 0)
  end
end

local function case_tmp(name, fn)
  case(name, function()
    local dir = th.mktmpdir("custom_tools")
    local root = maki.fs.joinpath(dir, "root")
    local meta_dir = maki.fs.joinpath(dir, "registry")
    maki.fs.mkdir(root)
    maki.fs.mkdir(meta_dir)
    local ok, err = pcall(fn, root, meta_dir)
    th.rmtree(dir)
    if not ok then
      error(err, 0)
    end
  end)
end

case("safe_resolve_rejects_bad_paths", function()
  local bad = {
    { nil, "required" },
    { "", "required" },
    { 42, "must be a string" },
    { "/etc/passwd", "must be relative" },
    { "bad\0path", "must be relative" },
    { "..", "traversal" },
    { "../escape", "traversal" },
    { "a/../../escape", "traversal" },
    { "inside/../../../etc/shadow", "traversal" },
    { "C:\\foo", "must be relative" },
    { "c:foo", "must be relative" },
    { "\\escape", "must be relative" },
  }
  for _, v in ipairs(bad) do
    local _, err = h.safe_resolve("/tmp/tools", v[1])
    assert(
      err and err:find(v[2]),
      "input " .. tostring(v[1]) .. " should match '" .. v[2] .. "', got: " .. tostring(err)
    )
  end
end)

case("safe_resolve_accepts_nested_paths", function()
  local s = "[/\\\\]"
  local good = {
    { "run.sh", "run%.sh" },
    { "git/commit.sh", "git" .. s .. "commit%.sh" },
    { "team/git/deep/commit.sh", "team" .. s .. "git" .. s .. "deep" .. s .. "commit%.sh" },
    { "./run.sh", "run%.sh" },
  }
  for _, v in ipairs(good) do
    local p, err = h.safe_resolve("/tmp/tools", v[1])
    assert(p, "input " .. v[1] .. " should be accepted, got error: " .. tostring(err))
    assert(p:find(v[2]), "result should match pattern '" .. v[2] .. "', got: " .. p)
  end
end)

case("normalize_tag", function()
  eq(h.normalize_tag("Git Flow"), "git_flow")
  eq(h.normalize_tag("git-flow"), "git_flow")
  eq(h.normalize_tag("  GIT_FLOW  "), "git_flow")
  eq(h.normalize_tag("git--flow"), "git_flow", "repeated separators collapse")
  eq(h.normalize_tag("git!flow"), "git_flow", "punctuation coerced")
  eq(h.normalize_tag(nil), nil)
  eq(h.normalize_tag(""), nil)
  eq(h.normalize_tag(string.rep("a", 65)), string.rep("a", 64), "over 64 chars truncated")
end)

case("derive_tags_from_path", function()
  eq(table.concat(h.derive_tags("git/commit.sh"), ","), "git,commit")
  eq(table.concat(h.derive_tags("scripts/push.sh"), ","), "scripts,push")
  eq(table.concat(h.derive_tags("a/b/c/d/tool.py"), ","), "b,c,d,tool", "keeps the most specific components")
  eq(table.concat(h.derive_tags("push/push.sh"), ","), "push", "dedupes components")
  eq(#h.derive_tags("!!!/!!.sh"), 0, "unusable components are dropped")
end)

case("derive_description_from_leading_comment", function()
  eq(h.derive_description("#!/bin/sh\n# Push to both remotes\nset -e"), "Push to both remotes")
  eq(h.derive_description("\n\n// Node tool\ncode"), "Node tool")
  eq(h.derive_description("-- Lua tool\nreturn 1"), "Lua tool")
  eq(h.derive_description("#!/bin/sh\n# -----\n# Real\n"), "Real", "decorative rules are skipped")
  eq(h.derive_description("#!/bin/sh\n# shellcheck shell=bash\n# Real\n"), "Real", "directives are skipped")
  eq(h.derive_description("#!/bin/sh\nset -e\n# too late"), nil, "stops at the first code line")
  eq(h.derive_description("#!/bin/sh\n"), nil)
end)

case("capture_help_success_failure_timeout", function()
  with_job_stub({ stdout = "usage: tool", stderr = "", exit_code = 0 }, function()
    local help, err = h.capture_help("/bin/tool")
    eq(err, nil)
    eq(help, "usage: tool")
  end)

  with_job_stub({ stdout = "", stderr = "boom", exit_code = 2 }, function()
    local help, err = h.capture_help("/bin/tool")
    eq(help, "boom", "stderr is shown when stdout is empty")
    eq(err, "exited 2")
  end)

  with_job_stub({ stdout = "", stderr = "", exit_code = 3 }, function()
    local help, err = h.capture_help("/bin/tool")
    eq(help, nil)
    eq(err, "no output (exit 3)")
  end)

  with_job_stub(nil, function()
    local help, err = h.capture_help("/bin/tool")
    eq(help, nil)
    assert(err:find("timed out"), "timeout reported")
  end)
end)

case("normalize_tags_dedupes_and_reports", function()
  local out, rejected = h.normalize_tags({ "Git Flow", "git_flow", "Sessions", "", "git-flow" })
  eq(#out, 2, "variants dedupe to one normalized form")
  eq(out[1], "git_flow")
  eq(out[2], "sessions")
  eq(#rejected, 1, "empty string rejected")

  local out2, rejected2, coerced = h.normalize_tags({ "valid", "bad!tag", "!", "" })
  eq(#out2, 2)
  eq(out2[2], "bad_tag")
  eq(#rejected2, 2)
  eq(#coerced, 1)
  eq(coerced[1], "bad!tag -> bad_tag")
end)

case("validate_write_tags", function()
  local tags, err, note = h.validate_write_tags({ "git_flow", "sessions" })
  eq(err, nil)
  eq(tags[1], "git_flow")
  eq(tags[2], "sessions")
  eq(note, nil, "no note when nothing coerced")

  local ctags, cerr, cnote = h.validate_write_tags({ "git/commit" })
  eq(cerr, nil)
  eq(ctags[1], "git_commit")
  eq(cnote, "normalized: git/commit -> git_commit")

  local etags, eerr = h.validate_write_tags({})
  eq(eerr, nil, "empty tag list is valid, stem fallback")
  eq(#etags, 0)

  local rtags, rerr = h.validate_write_tags({ "good", "!", "??" })
  eq(rtags, nil, "any unsalvageable tag rejects the write")
  assert(rerr:find("!, ??", 1, true) and not rerr:find("good", 1, true), "error lists only the bad tags")
end)

case("validate_description", function()
  local s, err = h.validate_description("  Amend   the last commit  ")
  eq(err, nil)
  eq(s, "Amend the last commit", "whitespace collapsed")

  local _, missing = h.validate_description(nil)
  eq(missing, "'description' is required for write")

  local _, empty = h.validate_description("   ")
  eq(empty, "'description' must not be empty")

  local _, long = h.validate_description(string.rep("x", h.MAX_DESCRIPTION_LEN + 1))
  assert(long:find("exceeds " .. h.MAX_DESCRIPTION_LEN), "over-long description rejected")

  eq(h.validate_description(string.rep("x", h.MAX_DESCRIPTION_LEN)), string.rep("x", h.MAX_DESCRIPTION_LEN))
end)

case("merge_meta_keeps_omitted_fields", function()
  local existing = { tags = { "git" }, description = "Old" }
  local merged, err = h.merge_meta(existing, nil, "New")
  eq(err, nil)
  eq(merged.tags[1], "git")
  eq(merged.description, "New")

  local retagged = h.merge_meta(existing, { "commit" }, nil)
  eq(retagged.tags[1], "commit")
  eq(retagged.description, "Old")

  local with_body = h.merge_meta(existing, nil, nil, "echo hi")
  eq(with_body.body, "echo hi")
  eq(h.merge_meta(with_body, nil, nil).body, "echo hi", "omitted body keeps the existing one")
  eq(h.merge_meta(with_body, nil, nil, "").body, nil, "empty body clears it")

  local fresh = h.merge_meta(nil, { "Tools" }, "d")
  eq(fresh.tags[1], "tools")
  eq(fresh.description, "d")

  local _, missing = h.merge_meta(nil, {}, nil)
  eq(missing, "'description' is required for write")

  local _, bad = h.merge_meta(nil, { "!" }, "d")
  assert(bad:find("invalid tag"), "invalid tags rejected")
end)

case("merge_meta_rejects_non_string_body", function()
  local _, err = h.merge_meta(nil, {}, "d", 42)
  eq(err, "'body' must be a string")
end)

case("merge_meta_keeps_omitted_fields", function()
  local ok_inputs = {
    { command = "list" },
    { command = "read", tags = { "git" } },
    { command = "read", path = "git/commit.sh" },
    { command = "list", query = "push", group_by = "tag" },
    { command = "info", tags = { "git" } },
    { command = "info", query = "push" },
    { command = "info", tags = { "git" }, query = "push" },
    { command = "info", path = "git/commit.sh" },
    { command = "write", path = "git/commit.sh", description = "d" },
    { command = "write", path = "pub", description = "d", body = "echo hi" },
    { command = "write", path = "git/commit.sh", description = "d", tags = {} },
    { command = "delete", path = "git/commit.sh" },
  }
  for i, input in ipairs(ok_inputs) do
    eq(h.validate_input(input), nil, "ok input #" .. i .. " (" .. input.command .. ")")
  end

  local bad = {
    { { command = "find" }, "unknown command" },
    { { command = "read" }, "path.*or.*tags" },
    { { command = "read", path = "git/commit.sh", tags = { "git" } }, "not both" },
    { { command = "read", tags = "git" }, "must be an array" },
    { { command = "read", path = 42 }, "path.*string" },
    { { command = "info" }, "path.*tags.*query" },
    { { command = "info", path = "git/commit.sh", tags = { "git" } }, "not both" },
    { { command = "info", path = "git/commit.sh", query = "push" }, "not both" },
    { { command = "list", query = 42 }, "query.*string" },
    { { command = "list", group_by = "file" }, "group_by" },
    { { command = "write", path = "pub", body = 42 }, "body.*string" },
    { { command = "write", path = "git/commit.sh", description = 42 }, "description.*string" },
    { { command = "write" }, "path" },
    { { command = "delete" }, "path" },
  }
  for _, v in ipairs(bad) do
    local err = h.validate_input(v[1])
    assert(err and err:find(v[2]), "expected error matching '" .. v[2] .. "', got: " .. tostring(err))
  end
end)

case_tmp("format_list_empty_or_missing_says_no_tools", function(root, meta_dir)
  eq(h.format_list(meta_dir, root), h.NO_TOOLS_MSG)
  eq(h.format_list(meta_dir .. "_missing", root), h.NO_TOOLS_MSG)
end)

case_tmp("format_list_is_tool_centric", function(root, meta_dir)
  write_tool(
    root,
    meta_dir,
    "git/commit.sh",
    { "git", "commit" },
    "Amend the last commit message",
    "#!/bin/sh\ncommit\n"
  )
  write_tool(root, meta_dir, "git/pr.sh", { "git" }, "Open a pull request", "#!/bin/sh\npr\n")
  write_tool(root, meta_dir, "py/json.py", { "python" }, "Pretty-print JSON", "#!/usr/bin/env python3\n")

  local result, err = h.format_list(meta_dir, root)
  eq(err, nil)
  local _, commit_count = result:gsub("git/commit%.sh", "")
  eq(commit_count, 1, "a multi-tag tool is listed once")
  assert(
    result:find("%- git/commit%.sh %(%d+ bytes%) %[git, commit%]: Amend the last commit message"),
    "flat row carries tags and description"
  )
  assert(result:find("%- py/json%.py %(%d+ bytes%) %[python%]: Pretty%-print JSON"), "nested path shown")
  assert(not result:find("^git %(2%)"), "no tag sections in the default view")
  assert(not result:find("#!/bin/sh"), "index has no bodies")
end)

case_tmp("format_list_group_by_tag_keeps_sections", function(root, meta_dir)
  write_tool(root, meta_dir, "git/commit.sh", { "git", "commit" }, "Amend", "c")
  write_tool(root, meta_dir, "git/pr.sh", { "git" }, "Open a pull request", "p")
  write_tool(root, meta_dir, "py/json.py", { "python" }, "Pretty-print JSON", "j")

  local result, err = h.format_list(meta_dir, root, nil, nil, "tag")
  eq(err, nil)
  local git_pos = result:find("git %(2%)")
  local commit_pos = result:find("commit %(1%)")
  local python_pos = result:find("python %(1%)")
  assert(
    git_pos and commit_pos and python_pos and git_pos < commit_pos and commit_pos < python_pos,
    "groups sorted by frequency, then tag"
  )
  assert(result:find("  %- git/commit%.sh %(%d+ bytes%): Amend"), "grouped row keeps its description")
  assert(not result:find("%[git, commit%]"), "the section header already names the tag")
end)

case_tmp("format_list_query_matches_path_description_and_tags", function(root, meta_dir)
  write_tool(root, meta_dir, "git/commit.sh", { "git", "commit" }, "Amend the last commit", "c")
  write_tool(root, meta_dir, "git/pr.sh", { "git" }, "Open a pull request", "p")
  write_tool(root, meta_dir, "py/json.py", { "python" }, "Pretty-print JSON", "j")

  local by_desc = h.format_list(meta_dir, root, nil, "PRETTY")
  assert(by_desc:find("py/json%.py") and not by_desc:find("git/commit%.sh"), "description match is case-insensitive")

  local by_path = h.format_list(meta_dir, root, nil, "pr.sh")
  assert(by_path:find("git/pr%.sh") and not by_path:find("py/json%.py"), "path match")

  local by_tag = h.format_list(meta_dir, root, nil, "python")
  assert(by_tag:find("py/json%.py") and not by_tag:find("git/"), "tag match")

  local combined = h.format_list(meta_dir, root, { "git" }, "request")
  assert(combined:find("git/pr%.sh") and not combined:find("git/commit%.sh"), "tags and query combine")

  eq(h.format_list(meta_dir, root, nil, "nothing"), NO_MATCH_MSG)
end)

case_tmp("filter_files_combines_tags_and_query", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "auth" }, "Sign in", "a")
  write_tool(root, meta_dir, "b.sh", { "sessions" }, "List sessions", "b")
  local files = h.files_with_tags(meta_dir, root)

  local by_query = h.filter_files(files, nil, "sign")
  eq(#by_query, 1, "query alone filters")
  eq(by_query[1].name, "a.sh")

  local by_tag = h.filter_files(files, { sessions = true }, nil)
  eq(#by_tag, 1, "tag set alone filters")
  eq(by_tag[1].name, "b.sh")

  eq(#h.filter_files(files, { sessions = true }, "sign"), 0, "tags and query must both match")
end)

case_tmp("format_list_stem_fallback_for_unannotated_tools", function(root, meta_dir)
  write_tool(root, meta_dir, "notes.sh", {}, nil, "#!/bin/sh\n")
  assert(h.format_list(meta_dir, root):find("%- notes%.sh %(%d+ bytes%) %[notes%]"), "stem tag is shown")
  local filtered, err = h.format_list(meta_dir, root, { "notes" })
  eq(err, nil)
  assert(filtered:find("notes%.sh %(%d+ bytes%)"), "stem works as a filter tag too")

  write_tool(root, meta_dir, "git/commit.sh", {}, "Empty tags fall back")
  assert(
    h.format_list(meta_dir, root):find("%- git/commit%.sh %(%d+ bytes%) %[commit%]"),
    "empty metadata tags fall back to the stem"
  )
end)

case_tmp("format_list_excludes_registrations_whose_file_is_gone", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "auth" }, "A", "a")
  write_tool(root, meta_dir, "gone.sh", { "auth" }, "Gone", "g")
  maki.fs.rm(maki.fs.joinpath(root, "gone.sh"))

  local result = h.format_list(meta_dir, root, { "auth" })
  assert(result:find("a%.sh"), "present tool still listed")
  assert(not result:find("%- gone%.sh %(%d+ bytes%)"), "missing tool dropped from the index")
  assert(result:find("missing from the project"), "the missing registration is reported")
end)

case_tmp("format_list_filters_by_tag_union", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "auth" }, "A", "a")
  write_tool(root, meta_dir, "b.sh", { "sessions" }, "B", "b")
  write_tool(root, meta_dir, "c.sh", { "storage" }, "C", "c")

  local result, err = h.format_list(meta_dir, root, { "auth", "sessions" })
  eq(err, nil)
  assert(result:find("a%.sh") and result:find("b%.sh"), "union of requested tags")
  assert(not result:find("c%.sh"), "other tags filtered out")

  eq(h.format_list(meta_dir, root, { "nope" }), NO_MATCH_MSG)
end)

case_tmp("format_list_invalid_tags", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "auth" }, "A", "a")

  local result, err = h.format_list(meta_dir, root, { "!!!", "", "  " })
  eq(result, nil)
  assert(err:find("no valid tags") and err:find("!!!", 1, true), "all-invalid errors and names the raw values")

  local partial, perr = h.format_list(meta_dir, root, { "auth", "!!!" })
  eq(perr, nil)
  local wpos = partial:find("warning: ignored invalid tag")
  local bpos = partial:find("a%.sh")
  assert(wpos and bpos and wpos < bpos, "partial invalid warns first, then lists matches")
end)

case_tmp("format_list_prune_advisory_only_unfiltered_over_cap", function(root, meta_dir)
  for i = 1, h.MAX_TAGS + 1 do
    write_tool(root, meta_dir, "f" .. i .. ".sh", { "tag" .. i }, "tool " .. i, "x")
  end
  assert(
    h.format_list(meta_dir, root):find("to stay under " .. h.MAX_TAGS .. " tags"),
    "advisory when over the tag cap"
  )
  assert(not h.format_list(meta_dir, root, { "tag1" }):find("stay under"), "no advisory on a filtered list")
end)

case_tmp("format_list_caps_oversized_output", function(root, meta_dir)
  local stem = string.rep("a", 180)
  for i = 1, 130 do
    write_tool(root, meta_dir, stem .. i .. ".sh", { "tag" .. i }, "tool", "x")
  end
  local result = h.format_list(meta_dir, root)
  assert(#result <= h.MAX_FILE_BYTES + 200, "list output stays near the cap")
  assert(result:find("truncated at", 1, true), "oversized list is capped")
  assert(result:find(h.CAP_HINT_FILTER, 1, true), "cap hint suggests tag filtering")
end)

case("format_rejected", function()
  eq(h.format_rejected({}), nil)
  eq(h.format_rejected(nil), nil)
  assert(h.format_rejected({ "bad!tag", "" }):find("bad!tag"), "lists rejected tag names")
  local w = h.format_rejected({ string.rep("x", 200) })
  eq(#w, 64 + 3, "long values truncated to 64 chars plus ellipsis")
  eq(w:sub(-3), "...")
end)

case("metadata_round_trip", function()
  local yaml = h.encode_meta({ "git", "commit" }, "Amend the last commit", "git commit --amend\n")
  local meta, err = h.parse_meta(yaml)
  eq(err, nil)
  eq(meta.tags[1], "git")
  eq(meta.tags[2], "commit")
  eq(meta.description, "Amend the last commit")
  eq(meta.body, "git commit --amend\n")
end)

case("metadata_parses_block_body", function()
  local meta, err = h.parse_meta("description: d\ntags:\n- x\nbody: |\n  echo one\n  echo two\n")
  eq(err, nil)
  eq(meta.body, "echo one\necho two\n", "block scalar body survives decoding")
end)

case("metadata_without_description_keeps_tags", function()
  local yaml = h.encode_meta({ "git" }, nil)
  local meta, err = h.parse_meta(yaml)
  eq(err, nil)
  eq(meta.tags[1], "git")
  eq(meta.description, nil)
end)

case_tmp("metadata_read_falls_back_and_warns_on_invalid_yaml", function(root, meta_dir)
  local tags, description, _, warn = h.metadata_for(meta_dir, "a.sh")
  eq(tags[1], "a")
  eq(description, nil)
  eq(warn, nil, "a missing registration is not a warning")

  maki.fs.write(maki.fs.joinpath(root, "b.sh"), "b")
  local meta_path = h.meta_path(meta_dir, "b.sh")
  maki.fs.write(meta_path, ":\n  - :\n  bad")
  local btags, bdesc, _, bwarn = h.metadata_for(meta_dir, "b.sh")
  eq(btags[1], "b", "invalid metadata falls back to the stem")
  eq(bdesc, nil)
  assert(bwarn and bwarn:find("decode"), "invalid metadata warns, got: " .. tostring(bwarn))
end)

case_tmp("metadata_cache_serves_unchanged_tools", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "git" }, "Amend", "a")
  local files = h.files_with_tags(meta_dir, root)
  eq(files[1].tags[1], "git")

  with_read_stub(function()
    error("cache miss: unchanged metadata was re-read")
  end, function()
    local cached = h.files_with_tags(meta_dir, root)
    eq(cached[1].tags[1], "git")
    eq(cached[1].description, "Amend")
  end)
end)

case_tmp("metadata_cache_invalidated_when_registration_changes", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "git" }, "Amend", "a")
  eq(h.files_with_tags(meta_dir, root)[1].tags[1], "git")

  h.write_meta(meta_dir, "a.sh", { "storage" }, "A longer description so size differs too")
  local files = h.files_with_tags(meta_dir, root)
  eq(files[1].tags[1], "storage", "rewrite must evict the cached metadata")
  eq(files[1].description, "A longer description so size differs too")
end)

case_tmp("metadata_cache_never_caches_invalid_metadata", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "git" }, "Amend", "a")
  local meta_path = h.meta_path(meta_dir, "a.sh")
  maki.fs.write(meta_path, ":\n  - :\n  bad")

  local files, warnings = h.files_with_tags(meta_dir, root)
  eq(#warnings, 1, "invalid metadata is surfaced")
  eq(files[1].tags[1], "a", "stem fallback still lists the tool")

  h.write_meta(meta_dir, "a.sh", { "fixed" }, "now valid")
  local fixed, fwarn = h.files_with_tags(meta_dir, root)
  eq(fwarn and #fwarn, 0, "a fixed sidecar clears the warning")
  eq(fixed[1].tags[1], "fixed")
end)

case_tmp("format_read_union_headers_and_descriptions", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "auth" }, "A description", "echo A")
  write_tool(root, meta_dir, "b.sh", { "sessions" }, "B description", "echo B")
  write_tool(root, meta_dir, "git/c.sh", { "auth", "sessions" }, "C description", "echo C")

  local result, err = h.format_read(meta_dir, root, { "auth" })
  eq(err, nil)
  assert(result:find("a%.sh %(%d+ bytes%) %[auth%]"), "header shows size and tags")
  assert(result:find("git/c%.sh %(%d+ bytes%) %[auth, sessions%]"), "match on any tag shows all tags")
  assert(result:find("A description") and result:find("C description"), "descriptions included")
  assert(result:find("echo A") and result:find("echo C"), "sources included")
  assert(not result:find("B description"), "non-matching tool excluded")

  local union = h.format_read(meta_dir, root, { "auth", "sessions" })
  assert(union:find("a%.sh") and union:find("b%.sh") and union:find("git/c%.sh"), "tags select the union")

  eq(h.format_read(meta_dir, root, { "nope" }), NO_MATCH_MSG)
end)

case_tmp("format_read_normalizes_request_and_stem", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "auth_flow" }, "Auth", "echo auth")
  write_tool(root, meta_dir, "notes.sh", {}, nil, "no metadata")

  assert(
    h.format_read(meta_dir, root, { "Auth-Flow", "AUTH FLOW" }):find("a%.sh"),
    "request tags normalized before matching"
  )

  local result = h.format_read(meta_dir, root, { "notes" })
  assert(result:find("notes%.sh") and result:find("no metadata"), "stem pseudo-tag readable")
end)

case_tmp("format_read_invalid_tags", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "auth" }, "Auth", "a")

  local result, err = h.format_read(meta_dir, root, { "!!!", "" })
  eq(result, nil)
  assert(err:find("no valid tags") and err:find("!!!", 1, true), "all-invalid errors and names the raw values")

  local partial, perr = h.format_read(meta_dir, root, { "auth", "!!!" })
  eq(perr, nil)
  local wpos = partial:find("warning: ignored invalid tag")
  assert(wpos and wpos < partial:find("a%.sh"), "partial invalid warns before the matches")
end)

case("format_read_entry", function()
  local formatted = h.format_read_entry("git/commit.sh", 42, "#!/bin/sh\necho hi\n", { "git", "commit" }, "Amend")
  assert(formatted:find("^git/commit%.sh %(42 bytes%) %[git, commit%]"), "header carries size and tags")
  assert(formatted:find("\nAmend\n\n#!/bin/sh"), "description follows the header, source follows it")

  eq(
    h.format_read_entry("notes.sh", 12, "just a body", nil, nil),
    "notes.sh (12 bytes)\n\njust a body",
    "no brackets or description when unannotated"
  )
end)

case("format_info_entry", function()
  local formatted = h.format_info_entry("git/commit.sh", 42, { "git", "commit" }, "Amend", "Usage: commit.sh")
  assert(formatted:find("^git/commit%.sh %(42 bytes%) %[git, commit%]"), "header carries size and tags")
  assert(formatted:find("\nAmend\n\nUsage: commit%.sh"), "description then help")
  eq(
    h.format_info_entry("notes.sh", 12, nil, nil, "usage: notes"),
    "notes.sh (12 bytes)\n\nusage: notes",
    "no brackets or description when unannotated"
  )
end)

case("cap_help_truncates_with_marker", function()
  local out = h.cap_help(string.rep("x", h.HELP_MAX_BYTES + 10))
  assert(out:find("help truncated at " .. h.HELP_MAX_BYTES, 1, true), "truncation marker present")
  assert(#out < h.HELP_MAX_BYTES + 100, "help output stays near the cap")
end)

case_tmp("legacy_meta_migrates_when_project_file_exists", function(root, meta_dir)
  local project = maki.fs.joinpath(root, "git/commit.sh")
  maki.fs.mkdir(maki.fs.dirname(project), { parents = true })
  maki.fs.write(project, "#!/bin/sh\n")

  local legacy = h.legacy_meta_path(meta_dir, "git/commit.sh")
  maki.fs.mkdir(maki.fs.dirname(legacy), { parents = true })
  local yaml, enc_err = h.encode_meta({ "git" }, "Old tool")
  eq(enc_err, nil)
  maki.fs.write(legacy, yaml)

  local files, warnings = h.files_with_tags(meta_dir, root)
  eq(#warnings, 0)
  eq(#files, 1)
  eq(files[1].name, "git/commit.sh")
  eq(files[1].tags[1], "git")
  assert(maki.fs.metadata(h.meta_path(meta_dir, "git/commit.sh")), "metadata moved to the mirrored path")
  eq(maki.fs.metadata(legacy), nil, "legacy sidecar removed")
end)

case_tmp("legacy_meta_without_project_file_warns", function(root, meta_dir)
  local legacy = h.legacy_meta_path(meta_dir, "gone.sh")
  maki.fs.mkdir(maki.fs.dirname(legacy), { parents = true })
  local yaml = h.encode_meta({ "old" }, "Gone")
  maki.fs.write(legacy, yaml)

  local files, warnings = h.files_with_tags(meta_dir, root)
  eq(#files, 0)
  eq(#warnings, 1)
  assert(warnings[1]:find("legacy .meta registration", 1, true), "legacy layout named")
  assert(warnings[1]:find("re-register", 1, true), "warning is actionable")
end)

case_tmp("registry_prefers_mirrored_metadata_over_legacy", function(root, meta_dir)
  write_tool(root, meta_dir, "a.sh", { "new" }, "New", "a")
  local legacy = h.legacy_meta_path(meta_dir, "a.sh")
  maki.fs.mkdir(maki.fs.dirname(legacy), { parents = true })
  maki.fs.write(legacy, h.encode_meta({ "old" }, "Old"))

  local files, warnings = h.files_with_tags(meta_dir, root)
  eq(#warnings, 0)
  eq(#files, 1)
  eq(files[1].tags[1], "new", "current metadata wins")
end)

case_tmp("remove_tool_falls_back_to_legacy_metadata", function(root, meta_dir)
  local project = maki.fs.joinpath(root, "a.sh")
  maki.fs.write(project, "a")
  local legacy = h.legacy_meta_path(meta_dir, "a.sh")
  maki.fs.mkdir(maki.fs.dirname(legacy), { parents = true })
  maki.fs.write(legacy, h.encode_meta({ "old" }, "Old"))

  eq(h.remove_tool(meta_dir, "a.sh"), true)
  eq(maki.fs.metadata(legacy), nil, "legacy sidecar removed")
  assert(maki.fs.metadata(project) ~= nil, "project file stays")
end)

case_tmp("files_with_tags_lists_every_nested_tool_once", function(root, meta_dir)
  eq(#h.files_with_tags(meta_dir, root), 0, "empty registry lists nothing")

  write_tool(root, meta_dir, "z/b.sh", { "gotchas", "architecture" }, "B", "body")
  write_tool(root, meta_dir, "a.sh", { "architecture" }, "A", "longer body here")

  local files = h.files_with_tags(meta_dir, root)
  eq(#files, 2, "one entry per tool, not per tag")
  eq(files[1].name, "a.sh", "name sorted")
  eq(files[2].name, "z/b.sh")
  eq(files[1].size, #"longer body here")
  eq(table.concat(files[2].tags, ","), "gotchas,architecture", "registration tag order is kept")
  eq(files[2].description, "B")
end)

case_tmp("command_tool_needs_no_project_file", function(root, meta_dir)
  local yaml, enc_err = h.encode_meta({ "build" }, "Build and install", "cargo build -r\n")
  eq(enc_err, nil)
  maki.fs.write(h.meta_path(meta_dir, "pub"), yaml)

  local files, warnings = h.files_with_tags(meta_dir, root)
  eq(#warnings, 0)
  eq(#files, 1)
  eq(files[1].body, "cargo build -r\n")

  local listed = h.format_list(meta_dir, root)
  assert(listed:find("%- pub %(command%) %[build%]: Build and install"), "list marks command tools")

  local read = h.format_read(meta_dir, root, { "build" })
  assert(read:find("pub %(command%) %[build%]"), "read header is command-aware")
  assert(read:find("cargo build %-r"), "read returns the body")

  local info = h.format_info_entry("pub", "command", { "build" }, "Build and install", "command:\ncargo build -r")
  assert(info:find("pub %(command%) %[build%]"), "info header is command-aware")
end)

case("group_by_tag_sorts_by_count_then_name", function()
  local files = {
    { name = "a.sh", size = 1, tags = { "zeta", "shared" }, description = "A" },
    { name = "b.sh", size = 2, tags = { "alpha", "shared" }, description = "B" },
  }
  local groups = h.group_by_tag(files)
  eq(#groups, 3)
  eq(groups[1].tag, "shared", "most used tag first")
  eq(#groups[1].files, 2)
  eq(groups[1].files[1].size, 1, "a grouped row still knows its size")
  eq(groups[1].files[1].description, "A", "a grouped row still knows its description")
  eq(groups[2].tag, "alpha", "ties broken by tag name")
  eq(groups[3].tag, "zeta")
end)

case("format_size_switches_unit_at_one_kib", function()
  eq(h.format_size(1023), "1023B")
  eq(h.format_size(1024), "1.0K")
  eq(h.format_size(1536), "1.5K")
end)

case("detail_parts_put_the_description_first_and_the_size_last", function()
  local parts = h.detail_parts(1234, "Amend the last commit", { "gotchas", "architecture" })
  eq(parts[1][1], "Amend the last commit")
  eq(parts[1][2], "dim")
  eq(parts[1].elastic, true, "description is what a narrow row truncates")
  eq(parts[2][1], " · gotchas, architecture")
  eq(parts[2][2], "keybind_section", "tags wear the color the grouped view puts on its tag headers")
  eq(parts[3][1], " · 1.2K", "the size ends the row, so the picker lands it against the border")
  eq(parts[3][2], "dim")
  eq(parts[3].elastic, nil, "only the description gives up cells")

  local tagged = h.detail_parts(12, nil, { "git" })
  eq(tagged[1][1], "git")
  eq(tagged[1].elastic, true, "without a description the tags truncate")
  eq(tagged[2][1], " · 12B")

  local bare = h.detail_parts(12, nil, {})
  eq(bare[1][1], "12B", "no separator without tags or description")
  eq(bare[2], nil)
end)

case_tmp("cap_read_output_never_splits_utf8_codepoint", function(dir)
  local max = h.MAX_FILE_BYTES
  local big = "x" .. string.rep("\195\169", max)
  local out = h.cap_read_output(big, h.CAP_HINT_REWRITE)
  local prefix = out:match("^(.-)\n%.%.%. %(output truncated")
  assert(prefix, "truncation marker present")
  eq(#prefix, max - 1, "backs off one byte to the last codepoint boundary")
  eq(prefix:byte(#prefix), 0xA9, "prefix ends on a complete codepoint")
end)

case_tmp("remove_tool_unregisters_but_keeps_the_file", function(root, meta_dir)
  write_tool(root, meta_dir, "git/commit.sh", { "git" }, "Amend", "body")
  eq(h.remove_tool(meta_dir, "git/commit.sh"), true)
  eq(maki.fs.metadata(maki.fs.joinpath(root, "git/commit.sh")) ~= nil, true, "the project file stays")
  eq(maki.fs.metadata(h.meta_path(meta_dir, "git/commit.sh")), nil, "the registration is gone")

  local _, err = h.remove_tool(meta_dir, "git/commit.sh")
  eq(err, "'git/commit.sh' is not registered")
end)

th.report()
