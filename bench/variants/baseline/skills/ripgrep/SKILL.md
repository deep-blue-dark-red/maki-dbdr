---
name: ripgrep
description: Advanced search with rg via bash — counts, file lists, multiline, type filters, globs, replace preview. Use when the grep tool can't express the query.
---
# ripgrep (rg)

Run via bash. Prefer the grep tool for plain content search; use rg for the cases below.
Always quote the pattern in single quotes. rg respects .gitignore by default.

## Recipes

List files containing a match (no lines):
    rg -l 'pattern'

Count matches per file / total:
    rg -c 'pattern'          # per file
    rg --count-matches 'pattern' | awk -F: '{s+=$2} END{print s}'

Filter by language / file type:
    rg -t rust 'pattern'     # rg --type-list shows all types
    rg -g '*.tsx' -g '!*test*' 'pattern'   # include/exclude globs

Multiline match (pattern spans lines):
    rg -U 'fn foo\([^)]*\)\s*\{'

Fixed string (no regex escaping headaches):
    rg -F 'a.b(c)'

Word boundary / case:
    rg -w 'id'               # whole word only
    rg -i 'pattern'          # case-insensitive; -S smart case

Context around matches:
    rg -C 3 'pattern'        # -A after, -B before

Limit noise on huge trees:
    rg -m 5 'pattern'        # max 5 matches per file
    rg --max-columns 200 'pattern'

Search ignored/hidden files (build output, .env — only when explicitly needed):
    rg --no-ignore 'pattern'     # ignore .gitignore
    rg -uu 'pattern'             # + hidden files

Preview a replacement without touching files (rg never writes):
    rg 'old_name' -r 'new_name'

Structured output for scripting inside code_execution:
    rg --json 'pattern'

## Rules
- Never use `rg` output line numbers for edit old_string content — re-read the range first.
- `-e` for patterns starting with `-`: rg -e '-flag'.
- Exit code 1 just means no matches, not an error.
