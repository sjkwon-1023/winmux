# winmux OSC contract — Claude Code hook / shell prompt

This is the **contract document** that implements the path described in 계획 v2 section 9
("에이전트 상태 및 알림"). It defines the meaning of the OSC sequences winmux interprets
(fixed in stage 18) and shows, as examples, the Claude Code hook and shell prompt halves.

```
Claude Code hook (UserPromptSubmit / Notification / Stop)
  → writes OSC 777 to the resolved TTY (/dev/tty, or an ancestor process's pts)
  → the Rust PTY reader (winmux-core::osc::OscScanner) detects it
  → batched over a 100ms flush window (glue OscRouter)
  → updates the tab unread dot / pane badge / workspace sidebar status
```

v1 uses the PTY output itself (OSC sequences) as the delivery path, with no IPC server,
named pipe, or Windows helper CLI.

## OSC meaning contract

winmux interprets four kinds of sequences.

| Sequence | Meaning | Status (`agentStatus`) | Unread dot |
|---|---|---|---|
| `OSC 777;notify;winmux:running;<body>` | Agent work started | `running` | no |
| `OSC 777;notify;winmux:needsInput;<body>` | Waiting for user input | `needsInput` | yes |
| `OSC 777;notify;winmux:idle;<body>` | Work finished | `idle` | yes |
| Any other `OSC 777` / every `OSC 9` | Status-neutral notification | **unchanged** | yes |
| `OSC 0` (and the alias `OSC 2`) | Tab title | unchanged | no |
| `OSC 7` `file://host/path` | Tab cwd (respawn location on restart) | unchanged | no |

Detailed rules:

- **A status token must match the entire title field exactly** (`winmux:running` /
  `winmux:needsInput` / `winmux:idle`). Any deviation falls through to a status-neutral
  notification — this is the boundary that keeps 777s emitted by other tools, or an
  OSC 9 such as ConEmu's progress report, from claiming agent status.
- If `body` is non-empty it is kept as the sidebar preview (`lastAgentMessage`). **An empty
  body does not clear a previously received message** — firing `running` with no body
  leaves the preceding needsInput text in place. Messages are truncated at 500 characters.
- `running` is a progress signal, so it does not raise a dot. Only `needsInput`, `idle`,
  and status-neutral notifications set unread.
- **A tab that is on screen does not set unread** (active workspace + that pane's active
  tab), because its content is already in front of the user.
- Waiting for input wins: while some tab is at `needsInput`, `running`/`idle` from a
  **different** tab cannot override the workspace status. Only the tab that raised it can
  demote it (once the user responds, that tab's `UserPromptSubmit` → `running` demotes it
  naturally).
- The semicolon (`;`) is the field separator. If one appears inside title/body the parser
  mis-splits the fields, so the emitting side substitutes it.
- A restart resets all notifications and statuses (a dead session's needsInput does not
  survive a restart — 계획 v2 section 11).

## tty resolution discipline — direct `/dev/tty` → ancestor pts fallback

**Claude Code itself consumes a hook's stdout** (it is processed as the hook's result and
used for UI/logs or discarded; the bytes do not flow through to the terminal screen). If a
hook script simply writes the OSC sequence to standard output with `echo` or `printf`,
those bytes never reach the real PTY stream and the Rust PTY reader detects nothing.

A hook script must therefore write the OSC sequence **directly to the terminal device, not
to standard output**. The problem is finding that device: a single `> /dev/tty` is not
enough.

**Measured (Claude Code 2.1.226, checkpoint 2):** the hook process **has no controlling
TTY**, so `> /dev/tty` fails with `No such device or address` (ENXIO). Meanwhile **the main
Claude Code process is still attached to the `/dev/pts/N` that winmux opened** — the device
is alive, only the hook side lacks a handle to it. That is why `winmux_emit` in the example
below resolves the tty in two steps.

1. **Direct `/dev/tty`** — if a controlling TTY exists (running it by hand, launching the
   hook through another path, a future version where this premise changes), this is the
   right answer and it is done in one shot.
2. **Ancestor pts fallback** — if step 1 fails, walk up from the process itself through the
   PPID in `/proc/<pid>/stat`, up to 8 hops, and if the `readlink` of fd 0/1/2 of any of
   those processes points at `/dev/pts/*`, write there. **The nearest ancestor wins** — even
   with nested terminals it picks its own pane's pts rather than the outer terminal's.

If both fail (no pts in the ancestor chain — e.g. the hook runs after being reparented to
init), it **gives up silently and exits 0.** A failed notification delivery breaking the
Claude session is worse than missing one notification. The depth limit of 8 and the
`/dev/pts/*` whitelist are the boundaries that keep this search from dragging on or leaking
OSC bytes into the wrong target (a log file, a pipe).

(The shell prompt snippet below is unaffected by this problem — a shell has its own tty and
its stdout *is* the PTY, so neither a redirect nor a fallback is needed.)

## Example hook script

`~/.claude/hooks/winmux-notify.sh` (must be made executable: `chmod +x`):

```bash
#!/usr/bin/env bash
# Called from a Claude Code hook to emit a winmux status token as OSC 777 to the real
# terminal device.
# Arguments: $1 = status token (winmux:running | winmux:needsInput | winmux:idle)
#            $2 = body (optional). The Notification event prefers .message from the stdin JSON.
set -euo pipefail

STATUS="${1:?usage: winmux-notify.sh <winmux:running|winmux:needsInput|winmux:idle> [body]}"
BODY="${2:-}"

# Write the OSC bytes to the real terminal device. This implements the two steps of the
# "tty resolution discipline" above.
#   1) /dev/tty — if a controlling TTY exists, this is the right answer.
#   2) /proc ancestor chain — the hook process of Claude Code 2.1.226 has no controlling
#      TTY, so 1) fails with ENXIO ("No such device or address"). In that case, walk up
#      from itself through its parents and write to the /dev/pts/* that fd 0/1/2 of each
#      process points at. The main Claude Code process is attached to winmux's pts, so it
#      is found a few hops up.
# If neither works, give up silently — a failed notification must not break the Claude session.
winmux_emit() {
  local payload="$1"

  if { printf '%s' "$payload" > /dev/tty; } 2>/dev/null; then
    return 0
  fi

  local pid=$$ depth=0 fd target stat ppid
  while [[ "$pid" -gt 1 && "$depth" -lt 8 ]]; do
    for fd in 0 1 2; do
      target="$(readlink "/proc/$pid/fd/$fd" 2>/dev/null || true)"
      [[ "$target" == /dev/pts/* ]] || continue
      if { printf '%s' "$payload" > "$target"; } 2>/dev/null; then
        return 0
      fi
    done
    # /proc/<pid>/stat has the form "<pid> (<comm>) <state> <ppid> ...". comm can contain
    # spaces and parentheses, so cut from after the last ')' and read the ppid that
    # follows state.
    stat="$(cat "/proc/$pid/stat" 2>/dev/null || true)"
    [[ -n "$stat" ]] || break
    stat="${stat##*) }"
    ppid="${stat#* }"
    ppid="${ppid%% *}"
    [[ "$ppid" =~ ^[0-9]+$ ]] || break
    pid="$ppid"
    depth=$((depth + 1))
  done

  return 1
}

# Claude Code passes the event information as JSON on stdin when it runs a hook.
# For the Notification event, the .message field holds the human-readable notification text.
# Without jq, fall back to the default body received as an argument (the hook keeps working).
if [[ ! -t 0 ]]; then
  INPUT_JSON="$(cat)"
  if command -v jq > /dev/null 2>&1; then
    FROM_JSON="$(printf '%s' "$INPUT_JSON" | jq -r '.message // empty' 2>/dev/null || true)"
    if [[ -n "$FROM_JSON" ]]; then
      BODY="$FROM_JSON"
    fi
  fi
fi

# A semicolon (;) left inside the body makes the parser mis-split the fields, so substitute it.
BODY="${BODY//;/,}"

# OSC 777 format: ESC ] 777 ; notify ; title ; body BEL
winmux_emit "$(printf '\033]777;notify;%s;%s\007' "$STATUS" "$BODY")" || true

# Even if the emission fails, the hook exits successfully (miss a notification rather than
# break the session).
exit 0
```

## Example settings.json

Maps the three events to the three status tokens:

```json
{
  "hooks": {
    "UserPromptSubmit": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/winmux-notify.sh winmux:running"
          }
        ]
      }
    ],
    "Notification": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/winmux-notify.sh winmux:needsInput 'needs input'"
          }
        ]
      }
    ],
    "Stop": [
      {
        "matcher": "",
        "hooks": [
          {
            "type": "command",
            "command": "~/.claude/hooks/winmux-notify.sh winmux:idle done"
          }
        ]
      }
    ]
  }
}
```

- `UserPromptSubmit`: right after the user submits a prompt — "work started" (`running`).
  It passes no body, so the preceding preview text stays as it is.
- `Notification`: when user input is needed, e.g. a permission confirmation — the stdin
  JSON's `.message` carries the actual text (e.g. "Claude needs your permission to use Bash").
- `Stop`: when Claude Code finishes its response and returns to waiting — "work done"
  (`idle`).

### (Optional) Carrying the last response text on Stop

`Stop`'s stdin JSON contains `.transcript_path` (JSONL), so the last assistant message can
be extracted and used as the preview. It is not in the base example because it deepens the
jq dependency — if you want it, append it to the `BODY` resolution part of the script above:

```bash
TRANSCRIPT="$(printf '%s' "$INPUT_JSON" | jq -r '.transcript_path // empty')"
if [[ -n "$TRANSCRIPT" && -r "$TRANSCRIPT" ]]; then
  LAST="$(jq -rs '[.[] | select(.type == "assistant")] | last
                  | .message.content[]? | select(.type == "text") | .text' \
          "$TRANSCRIPT" 2>/dev/null | tail -n 1 || true)"
  if [[ -n "$LAST" ]]; then
    BODY="$LAST"
  fi
fi
```

## Emitting title and cwd from the shell prompt (OSC 0 / OSC 7)

The tab title and cwd are emitted by the shell on every prompt, not by a hook. In the WSL
side's `~/.bashrc`:

```bash
# On every prompt, emit the current directory (OSC 7) and the tab title (OSC 0).
# The shell's stdout is the PTY itself, so no /dev/tty redirect is needed.
__winmux_osc() {
  # OSC 7: file://<host>/<path> — winmux ignores host and uses only the path (ST terminator).
  printf '\033]7;file://%s%s\033\\' "${HOSTNAME:-wsl}" "$PWD"
  # OSC 0: tab title — here, the directory name (BEL terminator).
  printf '\033]0;%s\007' "${PWD##*/}"
}
PROMPT_COMMAND="__winmux_osc${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
```

- winmux percent-decodes the OSC 7 path. If the path contains `%`, it must be encoded as
  `%25` per the convention to be accurate (if what follows `%` is not two hex digits, it is
  left as a literal).
- This cwd is used as the **respawn location after a restart** — the shell reopens in the
  directory it was last in.
- To change the title to something else, such as an agent name, just swap the OSC 0 string.
  Even if ConPTY re-encodes OSC 0 as OSC 2 in transit, winmux receives it with the same
  meaning.

## Verification

1. Use `scripts/wsl/osc-test.sh` first to confirm "does an OSC written to /dev/tty reach the
   winmux app" (OSC 777 is cases 7, 8, and 9). This script runs directly from the shell and
   therefore has a tty, so it only takes step 1 — its purpose is to see whether the delivery
   path itself is alive.
2. Then run Claude Code inside a winmux terminal and confirm that the three hooks actually
   update the tab dot, pane badge, and sidebar status/preview
   (`docs/WINDOWS-BUILD.md` section 10, checkpoint 2).
3. If the hook is silent, start by looking at where the fallback broke. Inside a winmux
   terminal, reproduce a tty-less context with
   `setsid -w bash -c 'printf "" > /dev/tty' ; echo $?`, and check whether the main Claude
   process is attached to `/dev/pts/*` with
   `for p in $(pgrep -f 'claude'); do readlink /proc/$p/fd/1; done`. Check as well that the
   ancestor chain is within 8 hops.

## Notes

- BEL (`\007`) is not relied on as the only completion signal (계획 v2 section 9) — winmux's
  `OscScanner` recognizes both BEL and ST (`ESC \`) as terminators.
- "The hook has no controlling TTY" is a **fact measured on Claude Code 2.1.226**, not a
  guaranteed contract. That is exactly why the example script keeps step 1 — if a later
  version hands the hook a tty, it works as is without taking the fallback. Conversely, if
  Claude Code comes to run somewhere that is not a pts (a pipe-only daemon, say), step 2
  will not find a target either, and at that point we have to move to the file/socket
  watching alternative.
- If ConPTY turns out to swallow OSC 777 (spike-plan.md section 6, checklist item 1), this
  hook path has to be replaced with the file/socket watching alternative (see 계획 v2
  section 2, "단일 실패점"). The examples in this document rest on the premise that OSC
  passthrough is alive. Real-world passthrough of OSC 0/7 is still unverified, and its
  failure would be independent of the notification path (777/9).
