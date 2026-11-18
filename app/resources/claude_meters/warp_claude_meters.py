#!/usr/bin/env python3
"""Warp meters bridge for Claude Code.

Forwards Claude Code usage meters and lifecycle events to Warp over the
`warp://cli-agent` OSC 777 channel, so Warp can show per-tab context usage,
tokens, cost, plan limits, compaction and status. Standard library only.

Modes (first argument):
  statusline  Claude Code `statusLine` command. Reads the status JSON on stdin,
              sends a `status_line` event to Warp and prints a status line. If
              `<claude dir>/warp/statusline_chain` exists, its content is run
              as a shell command with the same stdin and its output is printed
              instead, so an existing status line keeps working.
  hook        Claude Code hook command for SessionStart, UserPromptSubmit,
              Notification, PostToolUse, Stop, PreCompact and PostCompact.
              Prints nothing (stdout of some hooks is added to the context).

Environment:
  WARP_CLAUDE_METERS_FORCE=1        emit even when TERM_PROGRAM is not Warp
  WARP_CLAUDE_METERS_LIFECYCLE=0|1  force lifecycle events off/on (default: on
                                    unless the official Warp Claude Code plugin
                                    is installed, which already sends them)
  WARP_CLAUDE_TRANSCRIPT_BACKUP=1   copy the transcript before each compaction
                                    (also enabled by `<claude dir>/warp/backup_enabled`)
  WARP_CLAUDE_BACKUP_DIR=<dir>      backup directory
                                    (default `<claude dir>/warp/transcript-backups`)

The script never fails loudly: any error is swallowed so Claude Code is never
blocked or disturbed.
"""

import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import time

PROTOCOL_VERSION = 1
SENTINEL = "warp://cli-agent"
MAX_TEXT = 300
MAX_SEEN_IDS = 500
CHAIN_TIMEOUT_SECS = 5
# Bounds the work of one invocation on a long transcript; the rest is read on later ones.
MAX_TRANSCRIPT_READ_BYTES = 8 * 1024 * 1024
# An unchanged status line is re-sent this often, so a pane that missed it still catches up.
UNCHANGED_RESEND_SECS = 30
# "hook" or "statusline"; set by main().
MODE = "statusline"
# Sequences a hook returns to Claude Code as `terminalSequence`.
PENDING_SEQUENCES = []


def claude_dir():
    configured = os.environ.get("CLAUDE_CONFIG_DIR")
    if configured:
        return configured
    return os.path.join(os.path.expanduser("~"), ".claude")


def warp_dir():
    return os.path.join(claude_dir(), "warp")


def in_warp():
    if os.environ.get("WARP_CLAUDE_METERS_FORCE") == "1":
        return True
    return os.environ.get("TERM_PROGRAM") == "WarpTerminal"


def official_plugin_installed():
    path = os.path.join(claude_dir(), "plugins", "installed_plugins.json")
    try:
        with open(path, encoding="utf-8") as handle:
            plugins = json.load(handle).get("plugins", {})
        return bool(plugins.get("warp@claude-code-warp"))
    except Exception:
        return False


def lifecycle_enabled():
    forced = os.environ.get("WARP_CLAUDE_METERS_LIFECYCLE")
    if forced in ("0", "1"):
        return forced == "1"
    return not official_plugin_installed()


def clip(text, limit=MAX_TEXT):
    if not isinstance(text, str):
        return None
    text = text.strip()
    return text[:limit] if text else None


def send(payload, dedupe_key=None):
    """Writes one OSC 777 notification to the controlling terminal.

    With `dedupe_key`, a payload identical to the last one sent under that key is skipped unless
    UNCHANGED_RESEND_SECS have passed.
    """
    if not in_warp():
        return
    payload = {key: value for key, value in payload.items() if value is not None}
    payload.setdefault("v", PROTOCOL_VERSION)
    payload.setdefault("agent", "claude")
    # ensure_ascii keeps ESC/BEL/C1 bytes out of the sequence; `;` is escaped because the
    # terminal splits OSC parameters on it.
    body = json.dumps(payload, ensure_ascii=True, separators=(",", ":")).replace(";", "\\u003b")
    if dedupe_key is not None and is_unchanged(dedupe_key, body):
        return
    sequence = "\x1b]777;notify;%s;%s\x07" % (SENTINEL, body)
    # Claude Code 2.1.141+ gives hooks and the status line no controlling terminal. Hooks hand
    # the sequence back as `terminalSequence`; the status line parks it for the next hook.
    if MODE == "hook" and supports_terminal_sequence():
        PENDING_SEQUENCES.append(sequence)
        return
    if write_tty(sequence):
        return
    if MODE == "statusline":
        park_sequence(payload.get("session_id"), sequence)
        if dedupe_key is not None:
            forget_sent(dedupe_key)


def write_tty(sequence):
    try:
        fd = os.open("/dev/tty", os.O_WRONLY | getattr(os, "O_NOCTTY", 0))
    except OSError:
        return False
    try:
        os.write(fd, sequence.encode("ascii"))
        return True
    except OSError:
        return False
    finally:
        os.close(fd)


TERMINAL_SEQUENCE_MIN_VERSION = (2, 1, 141)


def supports_terminal_sequence():
    match = re.search(r"(\d+)\.(\d+)\.(\d+)", os.environ.get("CLAUDE_CODE_VERSION") or "")
    if not match:
        return False
    return tuple(int(part) for part in match.groups()) >= TERMINAL_SEQUENCE_MIN_VERSION


def park_sequence(session_id, sequence):
    path = state_path(session_id, ".pending")
    if path:
        write_state_file(path, sequence)


def take_parked_sequence(session_id):
    path = state_path(session_id, ".pending")
    if not path:
        return None
    try:
        with open(path, encoding="ascii") as handle:
            sequence = handle.read()
        os.remove(path)
    except (OSError, UnicodeDecodeError):
        return None
    return sequence if sequence.startswith("\x1b]777;notify;") else None


def common_fields(data):
    return {
        "session_id": clip(data.get("session_id"), 128),
        "cwd": clip(data.get("cwd") or (data.get("workspace") or {}).get("current_dir"), 1024),
        "transcript_path": clip(data.get("transcript_path"), 1024),
    }


# --- Per-session state -----------------------------------------------------------------------


def state_dir():
    """`<claude dir>/warp/state`, created private. Returns None unless it is a real directory
    owned by this user, so a planted symlink or foreign directory is never written through."""
    base = os.path.join(warp_dir(), "state")
    try:
        os.makedirs(base, mode=0o700, exist_ok=True)
        info = os.lstat(base)
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
            return None
        if stat.S_IMODE(info.st_mode) != 0o700:
            os.chmod(base, 0o700)
    except OSError:
        return None
    return base


def state_path(session_id, suffix=".json"):
    safe = re.sub(r"[^A-Za-z0-9_-]", "", session_id or "")[:128]
    if not safe:
        return None
    base = state_dir()
    if base is None:
        return None
    return os.path.join(base, safe + suffix)


def write_state_file(path, text):
    """Replaces `path` atomically; the temp name is per process so concurrent runs never mix."""
    tmp = "%s.%d.tmp" % (path, os.getpid())
    try:
        with open(tmp, "w", encoding="utf-8") as handle:
            handle.write(text)
        os.replace(tmp, path)
    except OSError:
        try:
            os.unlink(tmp)
        except OSError:
            pass


def is_unchanged(key, body):
    """Whether `body` equals the last payload sent under `key` recently. Records it otherwise."""
    path = state_path(key, ".sent")
    if path is None:
        return False
    digest = hashlib.sha256(body.encode("ascii")).hexdigest()
    try:
        with open(path, encoding="utf-8") as handle:
            previous = handle.read()
        if previous == digest and time.time() - os.path.getmtime(path) < UNCHANGED_RESEND_SECS:
            return True
    except OSError:
        pass
    write_state_file(path, digest)
    return False


def forget_sent(key):
    """Makes the next payload under `key` go out even if unchanged (e.g. a new pane needs it)."""
    path = state_path(key, ".sent")
    if path is None:
        return
    try:
        os.unlink(path)
    except OSError:
        pass


# --- Session token totals -------------------------------------------------------------------


def session_tokens(session_id, transcript_path):
    """Sums usage over every assistant message of the transcript, incrementally.

    Claude Code writes one transcript line per content block, each repeating the
    message's usage, so usage is counted once per message id. Returns None until the
    whole transcript has been read, so a partial total is never shown.
    """
    if not transcript_path or not os.path.isfile(transcript_path):
        return None
    path = state_path(session_id)
    if path is None:
        return None
    state = {"file": transcript_path, "offset": 0, "input": 0, "output": 0, "seen": []}
    try:
        with open(path, encoding="utf-8") as handle:
            saved = json.load(handle)
        if saved.get("file") == transcript_path:
            state.update(saved)
    except Exception:
        pass

    size = os.path.getsize(transcript_path)
    if size < state["offset"]:
        state.update(offset=0, input=0, output=0, seen=[], last_id=None)
    seen = list(state["seen"])
    seen_set = set(seen)
    with open(transcript_path, "rb") as handle:
        handle.seek(state["offset"])
        chunk = handle.read(MAX_TRANSCRIPT_READ_BYTES)
    end = chunk.rfind(b"\n")
    if end < 0 and len(chunk) == MAX_TRANSCRIPT_READ_BYTES:
        # A single line longer than the cap: skip it; its tail fails to parse and is ignored.
        state["offset"] += len(chunk)
    if end >= 0:
        for raw in chunk[: end + 1].splitlines():
            try:
                entry = json.loads(raw)
            except Exception:
                continue
            if not isinstance(entry, dict) or entry.get("type") != "assistant":
                continue
            message = entry.get("message")
            if not isinstance(message, dict):
                continue
            usage = message.get("usage")
            if not isinstance(usage, dict):
                continue
            message_id = message.get("id") or entry.get("uuid")

            def tokens(key):
                value = usage.get(key)
                return value if isinstance(value, int) and value > 0 else 0

            input_tokens = (
                tokens("input_tokens")
                + tokens("cache_creation_input_tokens")
                + tokens("cache_read_input_tokens")
            )
            output_tokens = tokens("output_tokens")
            if message_id and message_id == state.get("last_id"):
                # Later blocks of a streamed message may carry larger running totals.
                state["input"] += max(0, input_tokens - state.get("last_input", 0))
                state["output"] += max(0, output_tokens - state.get("last_output", 0))
                state["last_input"] = max(input_tokens, state.get("last_input", 0))
                state["last_output"] = max(output_tokens, state.get("last_output", 0))
                continue
            if message_id in seen_set:
                continue
            if message_id:
                seen.append(message_id)
                seen_set.add(message_id)
            state["input"] += input_tokens
            state["output"] += output_tokens
            state.update(last_id=message_id, last_input=input_tokens, last_output=output_tokens)
        state["offset"] += end + 1
    state["seen"] = seen[-MAX_SEEN_IDS:]

    write_state_file(path, json.dumps(state))
    if size - state["offset"] > MAX_TRANSCRIPT_READ_BYTES:
        return None
    return {"input": state["input"], "output": state["output"]}


# --- statusLine ------------------------------------------------------------------------------


def pick(source, *keys):
    return {key: source[key] for key in keys if isinstance(source, dict) and key in source}


def build_meters(data):
    context = pick(
        data.get("context_window") or {},
        "used_percentage",
        "remaining_percentage",
        "context_window_size",
        "total_input_tokens",
        "total_output_tokens",
    )
    limits = data.get("rate_limits") or {}
    meters = {
        "model": pick(data.get("model") or {}, "id", "display_name"),
        "context_window": context,
        "cost": pick(
            data.get("cost") or {},
            "total_cost_usd",
            "total_lines_added",
            "total_lines_removed",
        ),
        "rate_limits": {
            window: pick(limits.get(window) or {}, "used_percentage", "resets_at")
            for window in ("five_hour", "seven_day")
            if isinstance(limits.get(window), dict)
        },
    }
    try:
        totals = session_tokens(data.get("session_id"), data.get("transcript_path"))
    except Exception:
        totals = None
    if totals:
        meters["session_tokens"] = totals
    return meters


def default_status_line(data):
    parts = []
    model = (data.get("model") or {}).get("display_name")
    if model:
        parts.append(str(model))
    used = (data.get("context_window") or {}).get("used_percentage")
    if isinstance(used, (int, float)):
        parts.append("ctx %d%%" % round(used))
    limits = data.get("rate_limits") or {}
    for label, window in (("sesión", "five_hour"), ("semana", "seven_day")):
        pct = (limits.get(window) or {}).get("used_percentage")
        if isinstance(pct, (int, float)):
            parts.append("%s %d%%" % (label, round(pct)))
    return re.sub(r"[\x00-\x1f\x7f-\x9f]", "", " · ".join(parts))


def chained_status_line(raw):
    chain_file = os.path.join(warp_dir(), "statusline_chain")
    try:
        with open(chain_file, encoding="utf-8") as handle:
            command = handle.read().strip()
    except OSError:
        return None
    if not command:
        return None
    try:
        result = subprocess.run(
            ["/bin/sh", "-c", command],
            input=raw,
            capture_output=True,
            timeout=CHAIN_TIMEOUT_SECS,
        )
        return result.stdout
    except Exception:
        return None


def run_statusline():
    raw = sys.stdin.buffer.read()
    try:
        data = json.loads(raw.decode("utf-8", "replace"))
    except Exception:
        data = {}
    if isinstance(data, dict) and data:
        try:
            payload = {"event": "status_line", "meters": build_meters(data)}
            payload.update(common_fields(data))
            send(payload, dedupe_key=data.get("session_id"))
        except Exception:
            pass
    chained = chained_status_line(raw)
    if chained is not None:
        # Bytes as-is: the chained command's encoding and escape sequences are its own.
        sys.stdout.buffer.write(chained)
        sys.stdout.buffer.flush()
    elif isinstance(data, dict):
        sys.stdout.write(default_status_line(data))


# --- Hooks -----------------------------------------------------------------------------------


def backup_enabled():
    if os.environ.get("WARP_CLAUDE_TRANSCRIPT_BACKUP") in ("1", "true", "yes"):
        return True
    return os.path.exists(os.path.join(warp_dir(), "backup_enabled"))


def backup_transcript(data, trigger):
    source = data.get("transcript_path")
    if not backup_enabled() or not source or not os.path.isfile(source):
        return None
    target_dir = os.environ.get("WARP_CLAUDE_BACKUP_DIR") or os.path.join(
        warp_dir(), "transcript-backups"
    )
    os.makedirs(target_dir, mode=0o700, exist_ok=True)
    session = re.sub(r"[^A-Za-z0-9_-]", "", data.get("session_id") or "session")[:64]
    stamp = time.strftime("%Y%m%d-%H%M%S")
    target = os.path.join(target_dir, "%s-%s-%s.jsonl" % (session, stamp, trigger or "compact"))
    shutil.copy2(source, target)
    return target


def prompt_path(session_id):
    return state_path(session_id, ".prompt")


def remember_prompt(session_id, query):
    path = prompt_path(session_id)
    if not path or not query:
        return
    write_state_file(path, query)


def recall_prompt(session_id):
    path = prompt_path(session_id)
    if not path:
        return None
    try:
        with open(path, encoding="utf-8") as handle:
            return clip(handle.read())
    except OSError:
        return None


def run_hook():
    try:
        data = json.loads(sys.stdin.buffer.read().decode("utf-8", "replace"))
    except Exception:
        return
    if not isinstance(data, dict):
        return
    name = data.get("hook_event_name")
    base = common_fields(data)
    # Meters the status line could not deliver ride along with this hook's output.
    if in_warp() and supports_terminal_sequence():
        parked = take_parked_sequence(data.get("session_id"))
        if parked:
            PENDING_SEQUENCES.append(parked)
    if name == "SessionStart":
        forget_sent(data.get("session_id"))
    lifecycle = lifecycle_enabled()
    payload = None

    if name in ("PreCompact", "PostCompact"):
        trigger = data.get("trigger") if data.get("trigger") in ("manual", "auto") else None
        payload = {"event": "pre_compact" if name == "PreCompact" else "post_compact"}
        payload["trigger"] = trigger
        if name == "PreCompact":
            try:
                payload["backup_path"] = backup_transcript(data, trigger)
            except Exception:
                pass
    elif name == "SessionStart" and data.get("source") == "compact" and not lifecycle:
        # The compaction is over. Without lifecycle events Warp would otherwise only learn that
        # from PostCompact, which older Claude Code versions lack.
        payload = {"event": "post_compact"}
    elif not lifecycle:
        return
    elif name == "SessionStart":
        payload = {"event": "session_start"}
    elif name == "UserPromptSubmit":
        query = clip(data.get("prompt"))
        remember_prompt(data.get("session_id"), query)
        payload = {"event": "prompt_submit", "query": query}
    elif name == "Notification":
        kind = data.get("notification_type")
        message = clip(data.get("message"))
        if kind == "permission_prompt":
            payload = {"event": "permission_request", "summary": message}
        elif kind == "idle_prompt":
            payload = {"event": "idle_prompt", "summary": message}
        elif kind == "elicitation_dialog":
            payload = {"event": "question_asked", "summary": message}
    elif name == "PostToolUse":
        payload = {"event": "tool_complete", "tool_name": clip(data.get("tool_name"), 64)}
    elif name == "Stop":
        # Warp replaces the session's prompt with the one on `stop`; resend it so the tab title
        # keeps showing what the agent worked on.
        payload = {"event": "stop", "query": recall_prompt(data.get("session_id"))}

    if payload is None:
        return
    for key, value in base.items():
        payload.setdefault(key, value)
    send(payload)


def main():
    global MODE
    mode = sys.argv[1] if len(sys.argv) > 1 else "statusline"
    MODE = "hook" if mode == "hook" else "statusline"
    try:
        if mode == "hook":
            run_hook()
        else:
            run_statusline()
    except Exception:
        pass
    if MODE == "hook" and PENDING_SEQUENCES:
        sys.stdout.write(json.dumps({"terminalSequence": "".join(PENDING_SEQUENCES)}))
        sys.stdout.flush()
    return 0


if __name__ == "__main__":
    sys.exit(main())
