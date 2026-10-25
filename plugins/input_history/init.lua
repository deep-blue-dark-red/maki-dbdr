-- Ctrl+P opens a fuzzy-searchable picker over the prompts this project's
-- input box recalls, newest first. Enter puts the selected prompt back in the
-- box, replacing whatever draft is there.

local ListPicker = require("maki.list_picker")
local helpers = require("input_history_helpers")

local TITLE = " Input history "
local EMPTY = "No input history yet"
local READ_FAILED = "Cannot read input history: "
local RECALL_FAILED = "Cannot recall input: "
local RECALL_ATTEMPTS = 20
local RECALL_RETRY_MS = 5
local FOOTER = {
  { "↑↓", "select" },
  { "Enter", "recall" },
  { "Esc", "cancel" },
}

-- The picker closes asynchronously, and an input still covered by it refuses
-- the edit. Retry until that frame lands, but stop once the version moves:
-- then the user has typed, and their text wins over the recall.
local function recall(text)
  local version, session_id
  for attempt = 1, RECALL_ATTEMPTS do
    local st, read_err = maki.ui.input()
    if not st then
      maki.ui.flash(RECALL_FAILED .. tostring(read_err))
      return
    end
    if session_id and (st.version ~= version or st.session_id ~= session_id) then
      return
    end
    version, session_id = st.version, st.session_id
    local ok, edit_err = maki.ui.input_edit({
      start = 0,
      stop = #st.text,
      text = text,
      version = st.version,
      session_id = st.session_id,
    })
    if ok then
      return
    end
    if attempt == RECALL_ATTEMPTS then
      maki.ui.flash(RECALL_FAILED .. tostring(edit_err))
      return
    end
    maki.async.sleep(RECALL_RETRY_MS)
  end
end

local function open()
  local entries, err = maki.ui.input_history()
  if not entries then
    maki.ui.flash(READ_FAILED .. tostring(err))
    return
  end
  if #entries == 0 then
    maki.ui.flash(EMPTY)
    return
  end
  local event = ListPicker.open(helpers.rows(entries), {
    title = TITLE,
    footer = FOOTER,
    filter = helpers.filter,
    key = function(row)
      return row.text
    end,
  })
  if event.type == "choice" then
    recall(event.item.text)
  end
end

maki.keymap.set("n", "<C-p>", open, { desc = "Input history" })
