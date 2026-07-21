#!/usr/bin/env python3
import os
import sys
import json
import tty
import termios
import select
import shutil
from datetime import datetime

# Formatting helper for context size
def format_context(sz):
    if sz is None:
        return "-"
    try:
        return f"{int(sz):,}"
    except Exception:
        return str(sz)

# Fast single-read parser for JSONL files
def get_session_info(filepath):
    try:
        with open(filepath, 'rb') as f:
            content = f.read()
            if not content.strip():
                return None, None, 0
                
            # Count turns efficiently at the byte level
            turn_count = content.count(b'"t":"msg"')
            
            # Extract first line (header)
            idx = content.find(b'\n')
            first_line = content[:idx] if idx != -1 else content
            header = json.loads(first_line.decode('utf-8', errors='ignore'))
            
            # Find last meta line efficiently
            meta = None
            pos = content.rfind(b'"t":"meta"')
            if pos != -1:
                start_line = content.rfind(b'\n', 0, pos)
                start_pos = start_line + 1 if start_line != -1 else 0
                end_line = content.find(b'\n', pos)
                end_pos = end_line if end_line != -1 else len(content)
                meta_line = content[start_pos:end_pos]
                try:
                    meta = json.loads(meta_line.decode('utf-8', errors='ignore'))
                except Exception:
                    pass
                    
            return header, meta, turn_count
    except Exception:
        return None, None, 0

# Resolve active sessions directory
def resolve_sessions_dir():
    home = os.path.expanduser("~")
    maki_fallback = os.path.join(home, ".maki", "sessions")
    maki_state = os.path.join(home, ".local", "state", "maki", "sessions")
    
    if os.path.isdir(maki_fallback):
        return maki_fallback
    elif os.path.isdir(maki_state):
        return maki_state
    return None

# Load all sessions from the active state directory (using fast os.scandir)
def load_sessions(sessions_dir):
    sessions = []
    try:
        for entry in os.scandir(sessions_dir):
            if entry.is_file() and entry.name.endswith(".jsonl"):
                header, meta, turns = get_session_info(entry.path)
                if not header:
                    continue
                    
                session_id = header.get("id", entry.name.replace(".jsonl", ""))
                cwd = header.get("cwd", "")
                created_at = header.get("created_at", 0)
                
                if meta:
                    title = meta.get("title", "Untitled").replace("\n", " ")
                    updated_epoch = meta.get("updated_at", created_at)
                    context_size = meta.get("context_size")
                else:
                    title = "Untitled"
                    updated_epoch = created_at
                    context_size = None
                    
                sessions.append({
                    "id": session_id,
                    "title": title,
                    "cwd": cwd,
                    "updated_at": updated_epoch,
                    "context_size": context_size,
                    "turns": turns
                })
    except Exception:
        pass
        
    sessions.sort(key=lambda s: s["updated_at"], reverse=True)
    return sessions

# Read interactive keypresses from raw terminal mode without buffering bugs
def get_key():
    fd = sys.stdin.fileno()
    old_settings = termios.tcgetattr(fd)
    try:
        tty.setraw(fd)
        select.select([fd], [], [])
        ch = os.read(fd, 1).decode('utf-8', errors='ignore')
        if ch == '\x1b':
            r, _, _ = select.select([fd], [], [], 0.02)
            if r:
                ch += os.read(fd, 2).decode('utf-8', errors='ignore')
    finally:
        termios.tcsetattr(fd, termios.TCSADRAIN, old_settings)
    return ch

# Query matching logic: split query into words, all words must be found in target
def matches_query(query, s):
    words = query.lower().split()
    target = f"{s['id']} {s['title']} {s['cwd']}".lower()
    return all(word in target for word in words)

def main():
    sessions_dir = resolve_sessions_dir()
    if not sessions_dir:
        print("Error: No sessions directory found.", file=sys.stderr)
        sys.exit(1)
        
    sessions = load_sessions(sessions_dir)
    if not sessions:
        print("Error: No sessions found.", file=sys.stderr)
        sys.exit(1)
        
    selected_idx = 0
    scroll_offset = 0
    first_draw = True
    last_total_lines = 0
    query = ""
    
    # Hide cursor
    sys.stdout.write("\x1b[?25l")
    sys.stdout.flush()
    
    try:
        while True:
            # Apply search filter dynamically
            if query:
                filtered_sessions = [s for s in sessions if matches_query(query, s)]
            else:
                filtered_sessions = sessions
                
            # Clamp selection index to filtered list
            selected_idx = max(0, min(selected_idx, len(filtered_sessions) - 1))
            
            # Query terminal size dynamically
            term_width, term_height = shutil.get_terminal_size()
            
            # num_visible bounds: viewport height minus Search (1) + Header (1) + separator (1) + footer spacing (3)
            num_visible = min(len(filtered_sessions), max(0, term_height - 6))
            
            # Scroll adjustments
            if selected_idx < scroll_offset:
                scroll_offset = selected_idx
            elif selected_idx >= scroll_offset + num_visible:
                scroll_offset = max(0, selected_idx - num_visible + 1)
                
            # Search (1) + Header (1) + Separator (1) + rows (num_visible) + Footer spacer (1) + instruction (1) = num_visible + 5 newlines
            total_lines = num_visible + 5
            
            if not first_draw:
                sys.stdout.write(f"\x1b[{last_total_lines}A\r")
            else:
                first_draw = False
                
            last_total_lines = total_lines
            
            # Clear from cursor down
            sys.stdout.write("\x1b[J")
            
            # Draw Search Prompt, clipped to viewport
            search_str = f"Search: {query}█"
            search_clipped = search_str[:term_width - 1]
            sys.stdout.write(f"\x1b[1m{search_clipped}\x1b[0m\n")
            
            # Draw Header, clipped to viewport
            header_str = f"{'Session ID':<36}   {'Title':<30}   {'Turns':>5}   {'Context':>10}   {'Last Updated':<19}   {'Working Directory':<40}"
            header_clipped = header_str[:term_width - 1]
            sys.stdout.write(f"\x1b[1m{header_clipped}\x1b[0m\n")
            sys.stdout.write("─" * len(header_clipped) + "\n")
            
            # Draw visible rows
            end_idx = min(len(filtered_sessions), scroll_offset + num_visible)
            for i in range(scroll_offset, end_idx):
                s = filtered_sessions[i]
                
                try:
                    dt = datetime.fromtimestamp(s["updated_at"]).strftime("%Y-%m-%d %H:%M:%S")
                except Exception:
                    dt = "Unknown"
                    
                ctx_str = format_context(s["context_size"])
                
                id_str = s["id"]
                title_str = s["title"][:30].ljust(30)
                turns_str = f"{s['turns']:>5}"
                ctx_str = ctx_str[:10].rjust(10)
                dt_str = dt[:19].ljust(19)
                cwd_str = s["cwd"][:40].ljust(40)
                
                line = f"{id_str}   \x1b[33m{title_str}\x1b[39m   {turns_str}   {ctx_str}   {dt_str}   {cwd_str}"
                line_clipped = line[:term_width - 1]
                
                if i == selected_idx:
                    sys.stdout.write(f"\x1b[48;5;237m{line_clipped}\x1b[0m\n")
                else:
                    sys.stdout.write(f"{line_clipped}\n")
                    
            # Clip footer instruction
            footer_str = "Use Ctrl-P/Ctrl-N (or Arrow keys) to navigate, Enter to load, Esc/Ctrl-C to quit"
            footer_clipped = footer_str[:term_width - 1]
            sys.stdout.write(f"\n\x1b[2m{footer_clipped}\x1b[0m")
            
            # Print session data directory path in footer
            dir_str = f"Session directory: {sessions_dir}"
            dir_clipped = dir_str[:term_width - 1]
            sys.stdout.write(f"\n\x1b[2m{dir_clipped}\x1b[0m")
            sys.stdout.flush()
            
            key = get_key()
            if key in ('\x1b[A', '\x10'):  # Up / Ctrl-P
                selected_idx = max(0, selected_idx - 1)
            elif key in ('\x1b[B', '\x0e'):  # Down / Ctrl-N
                selected_idx = min(len(filtered_sessions) - 1, selected_idx + 1)
            elif key in ('\r', '\n'):  # Enter
                if filtered_sessions:
                    # Clear TUI
                    sys.stdout.write(f"\x1b[{total_lines}A\r\x1b[J")
                    sys.stdout.write("\x1b[?25h")
                    sys.stdout.flush()
                    
                    session_id = filtered_sessions[selected_idx]["id"]
                    os.execvp("maki", ["maki", "-s", session_id])
                else:
                    pass
            elif key in ('\x1b', '\x03'):  # Esc / Ctrl-C
                # Clean exit
                sys.stdout.write(f"\x1b[{total_lines}A\r\x1b[J")
                sys.stdout.write("\x1b[?25h")
                sys.stdout.flush()
                break
            elif key in ('\x7f', '\x08'):  # Backspace / Ctrl-H
                if len(query) > 0:
                    query = query[:-1]
                    selected_idx = 0
            elif len(key) == 1 and 32 <= ord(key) <= 126:  # Printable characters
                query += key
                selected_idx = 0
    except KeyboardInterrupt:
        # Clean cleanup on SIGINT
        sys.stdout.write(f"\x1b[{total_lines}A\r\x1b[J")
        sys.stdout.write("\x1b[?25h")
        sys.stdout.flush()
        sys.exit(0)

if __name__ == "__main__":
    main()
