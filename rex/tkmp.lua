-- tkmp for Rex: preview the markdown file nearest the cursor.
--
-- A key reads the focused terminal, finds the paths to .md files on it (in
-- ls output, an agent's transcript, a compiler message, a link's target),
-- and opens tkmp for the one nearest the cursor in a split beside it. To
-- pick one, right-click its link, choose Copy Link, then press the key: a
-- freshly copied .md path that is also on screen wins over the nearest. A
-- second press replaces that split instead of stacking another. Inside tkmp,
-- q closes it.
--
-- In init.lua:
--
--   require("tkmp").setup{ key = "cmd+shift+m" }
--
-- setup takes key (false for no key; the action "tkmp_open" is always in the
-- command palette), bin (the tkmp binary) and width (the split's share of
-- the space, 0.5).

local M = {
  bin = os.getenv("HOME") .. "/.cargo/bin/tkmp",
  width = 0.5,
}

local T = "com.superlogical.terminal"

-- The split tkmp opened last, so a second press replaces it.
local last

-- The directory that relative paths on screen are relative to: the
-- foreground program's, or else the shell's.
local function cwd_of(block)
  local p = rex.block.call(T, "process", { block_id = block })
  if p then
    local fg = p.foreground or p.child or {}
    if fg.cwd then return fg.cwd end
    if p.child and p.child.cwd then return p.child.cwd end
  end
  local d = rex.block.call(T, "pwd", { block_id = block })
  if d and d.pwd then
    return (d.pwd:gsub("^file://[^/]*", ""))
  end
end

local function exists(path)
  local f = io.open(path, "r")
  if f then
    f:close()
    return true
  end
  return false
end

local function unescape_url(s)
  return (s:gsub("%%(%x%x)", function(h) return string.char(tonumber(h, 16)) end))
end

-- Turns what was on screen into a path on disk, or nil.
local function resolve(raw, cwd)
  local p = raw
  if p:match("^file://") then
    p = unescape_url(p:gsub("^file://[^/]*", ""))
  end
  if p:sub(1, 2) == "~/" then
    p = os.getenv("HOME") .. p:sub(2)
  elseif p:sub(1, 1) ~= "/" then
    if not cwd then return nil end
    p = cwd .. "/" .. p:gsub("^%./", "")
  end
  if exists(p) then return p end
end

local function is_markdown(s)
  local l = s:lower()
  return l:match("%.md$") or l:match("%.markdown$") or l:match("%.mdx$")
end

-- Strips a :line or :line:col suffix and trailing punctuation.
local function clean(word)
  return (word:gsub("[%.,:;]+$", ""):gsub(":%d+:%d+$", ""):gsub(":%d+$", ""))
end

-- macOS links /tmp, /var and /etc into /private, and Copy Link can name a
-- file by either path, so paths are compared without the prefix.
local function canon(p)
  for _, d in ipairs{ "tmp", "var", "etc" } do
    if p:sub(1, #d + 10) == "/private/" .. d .. "/" then return p:sub(9) end
  end
  return p
end

-- Every .md path on a line, with the column it starts at.
local function paths_in(line)
  local found = {}
  -- Path characters; quotes, brackets, backticks and spaces end a path.
  for start, word in line:gmatch("()([%w%._%-%+~/:@%%]+)") do
    local w = clean(word)
    if is_markdown(w) then
      found[#found + 1] = { path = w, col = start }
    end
  end
  return found
end

-- Picks an existing .md file on screen: copied (a resolved path, or nil)
-- if it's there, else the one nearest the cursor. Output above the cursor
-- (what just printed) wins over output below it at the same distance.
function M.find(text, cursor_row, cwd, links, copied)
  copied = copied and canon(copied)
  -- Links whose visible text isn't the path. The newest one is the
  -- fallback when no path is written out.
  local newest_link
  for i = #links, 1, -1 do
    if is_markdown(links[i]) then
      local path = resolve(links[i], cwd)
      if path then
        if canon(path) == copied then return path end
        newest_link = newest_link or path
      end
    end
  end
  local lines = {}
  for l in (text .. "\n"):gmatch("(.-)\n") do
    lines[#lines + 1] = l
  end
  local best, best_score
  for i = math.max(1, #lines - 2000), #lines do
    for _, hit in ipairs(paths_in(lines[i])) do
      local path = resolve(hit.path, cwd)
      if path then
        if canon(path) == copied then return path end
        local d = cursor_row - i
        local score = d >= 0 and d or (-d * 3)
        if not best_score or score <= best_score then
          best, best_score = path, score
        end
      end
    end
  end
  return best or newest_link
end

-- The pasteboard's change count, then its text, in one call.
local PASTEBOARD = [[
ObjC.import("AppKit");
var p = $.NSPasteboard.generalPasteboard;
var s = p.stringForType($.NSPasteboardTypeString);
p.changeCount + "\n" + (s.isNil() ? "" : s.js)
]]

-- The change count of the copy that last won, so each copy is used once:
-- pressing the key again without copying goes back to the nearest path.
local used_count

-- The clipboard as a .md file on disk and its change count, or nil. Copy
-- Link puts a link's target there: an absolute path or a file:// URL.
local function copied_path(cwd)
  local ok, res = pcall(rex.exec, { "/usr/bin/osascript", "-l", "JavaScript", "-e", PASTEBOARD }, { timeout = 2 })
  if not ok or not res or not res.ok then return nil end
  local count, s = res.stdout:match("^(%d+)\n(.-)%s*$")
  if not count or count == used_count then return nil end
  s = s:match("^%s*(.-)$")
  if s == "" or s:find("\n") or #s > 4096 then return nil end
  s = clean(s)
  if is_markdown(s) then return resolve(s, cwd), count end
end

function M.open(ctx)
  local block = ctx.block_id
  if not block then
    return nil, "no terminal is focused"
  end
  local text, err = rex.block.call(T, "format", { block_id = block, format = "text" })
  if not text then
    return nil, err
  end
  local vt = rex.block.call(T, "format", {
    block_id = block, format = "vt", extra_cursor = true, extra_hyperlink = true,
  })
  local content = (vt and vt.content) or ""
  local links = {}
  for url in content:gmatch("\27%]8;[^;]*;([^\27\7]+)") do
    links[#links + 1] = url
  end
  -- The cursor's row on screen, as a line number in the text, which also
  -- holds the scrollback above the screen. Rows are counted in the VT
  -- output, because the text drops the blank rows at the bottom of a screen
  -- that isn't full yet.
  local size = rex.block.call(T, "size", { block_id = block }) or {}
  local screen_row = tonumber(content:match(".*\27%[(%d+);%d+H")) or (size.rows or 0)
  local total = select(2, (vt and content or text.content):gsub("\n", "")) + 1
  local cursor = total - (size.rows or 0) + screen_row

  local cwd = cwd_of(block)
  local copied, count = copied_path(cwd)
  local path = M.find(text.content, cursor, cwd, links, copied)
  if not path then
    return nil, "no .md file path on screen"
  end
  if copied and canon(path) == canon(copied) then
    used_count = count
  end

  local layout = rex.layout.block{
    flavor = "com.superlogical.terminal.shell",
    options = { command = { M.bin, path }, shell = "none", cwd = cwd },
  }
  if last and last ~= block then
    rex.call("block.close", { session_id = ctx.session_id, block_id = last })
  end
  local res, oerr = rex.session.new_split{
    anchor_id = block,
    direction = "horizontal",
    side = "after",
    ratio = 1 - M.width,
    focus = true,
    layout = layout,
  }
  if res and res.block_ids then
    last = res.block_ids[1]
  end
  return res, oerr
end

function M.setup(opts)
  opts = opts or {}
  M.bin = opts.bin or M.bin
  M.width = opts.width or M.width
  rex.action{
    name = "tkmp_open",
    title = "tkmp: preview the markdown file on screen",
    description = "Open the copied .md link, or the .md file nearest the cursor, in tkmp beside this block",
    category = "tkmp",
    keywords = "markdown preview mermaid view",
    run = function(ctx) return M.open(ctx) end,
  }
  if opts.key ~= false then
    rex.bind(opts.key or "cmd+shift+m", function(ctx)
      local ok, err = M.open(ctx)
      if not ok then rex.log("warn", "tkmp: " .. tostring(err)) end
      return true
    end)
  end
  return M
end

return M
