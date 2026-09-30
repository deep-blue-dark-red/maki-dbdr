local M = {}

-- Largest cut at or before `i` where `s:sub(1, cut)` ends on a UTF-8
-- codepoint boundary: walk back over continuation bytes (0x80..0xBF) so a
-- byte offset never leaves half a character behind. Hand-rolled rather than
-- built on utf8.offset, which errors when handed a continuation position.
function M.backoff(s, i)
  i = math.min(i, #s)
  while i > 0 do
    local b = s:byte(i + 1)
    if not b or b < 0x80 or b >= 0xC0 then
      break
    end
    i = i - 1
  end
  return i
end

return M
