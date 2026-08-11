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

## Automatic provisioning

**winmux auto-provisions this on first run per distro (`~/.winmux/.setup-v5`); this
document remains the contract and the manual path.**

On launch the app streams a setup script into `wsl.exe [-d <distro>] -- bash -s` for every
distro it knows about (each workspace's, plus the WSL default) — stdin, so no Windows-side
guess at the WSL home path and no dependency on a distro's `interop`/`automount` settings.
Once per distro it:

- installs the script below at `~/.winmux/bin/winmux-notify.sh` (executable),
- installs the **`winmux` CLI** at `~/.winmux/bin/winmux` (executable) — the command-line
  half of the send and query channels below (`winmux ls` / `winmux send` / `winmux id`), so
  nothing has to assemble an escape sequence or repeat the tty resolution by hand. Every
  winmux terminal has `~/.winmux/bin` prepended to `PATH`
  (`apps/winmux/src-tauri/src/host.rs::bash_argv`), so inside a tab it is just `winmux`,
- replaces the old `~/.winmux/bin/winmux-send.sh` (setup v3) with a two-line wrapper that
  execs `winmux send "$@"`, so anything still pointing at that path keeps working,
- installs the `winmux-send` skill at `~/.claude/skills/winmux-send/SKILL.md` (the agent
  send and query channels below; source: `scripts/wsl/skills/winmux-send/SKILL.md`),
- merges the three hooks into `~/.claude/settings.json`, keeping every existing value (see
  the migration rules below),
- adds a `notify` key to `~/.codex/config.toml` **only if** that file exists and has no
  `notify` of its own (Codex runs it once per completed turn, which maps to `winmux:idle`);
  a missing file means Codex is not installed there and nothing is created,
- records what it did in `~/.winmux/setup.log`, then writes the marker.

The hook merge treats each of the three events on its own, and never touches a hook that is
not one of ours:

| Existing hook for the event | Result | Logged as |
|---|---|---|
| Runs `~/.winmux/bin/winmux-notify.sh` (however it is spelled — `$HOME`, `~`, absolute) | Left exactly as it is, custom arguments included | `already wired` |
| Runs a `winmux-notify.sh` from **another** path (a hand-wired `~/.claude/hooks/…`, an older install) | The **path** is rewritten to `"$HOME/.winmux/bin/winmux-notify.sh"`; the arguments after it stay byte-for-byte | `migrated` |
| Mentions `winmux-notify.sh` somewhere other than the leading word (`bash ~/…/winmux-notify.sh …`) | Left alone — rewriting that shape would be guesswork, and it already covers the event | `left untouched` |
| None | A new entry is appended | `added` |

Migration exists because a hand-wired hook points at an older copy of *this* contract, which
goes stale as the script below changes (the tty fallback, the stdin JSON body). Nothing is
ever duplicated: an event that has any `winmux-notify.sh` hook never gets a second one.

Any failure leaves the marker unwritten, so the next launch retries. A distro without
`python3` gets the notify script but no hook merge (the merge has to preserve an existing
`settings.json`, which rules out text munging) — install `python3` or wire it by hand.

The installer lives in `apps/winmux/src-tauri/src/provision.rs`, and the copies it embeds
are **byte-identical** to their sources — the "Example hook script" below and
`scripts/wsl/skills/winmux-send/SKILL.md`: change both halves together.

## OSC meaning contract

winmux interprets four kinds of sequences.

| Sequence | Meaning | Status (`agentStatus`) | Unread dot |
|---|---|---|---|
| `OSC 777;notify;winmux:running;<body>` | Agent work started | `running` | no |
| `OSC 777;notify;winmux:needsInput;<body>` | Waiting for user input | `needsInput` | yes |
| `OSC 777;notify;winmux:idle;<body>` | Work finished | `idle` | yes |
| `OSC 777;winmux-send;<target>;<base64>` | Text delivered to another pane's stdin (next section) | unchanged | no |
| `OSC 777;winmux-query;<kind>;<base64>` | Metadata answered into a file the sender names (section after that) | unchanged | no |
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
- `winmux-send` and `winmux-query` are the two `OSC 777`s that are **not** notifications:
  they change no state at all, raise no dot, and are not coalesced into the 100ms flush
  window. They are actions, and each has its own section below.

## Agent send channel — `OSC 777;winmux-send`

The agent-facing way to put text into **another pane's terminal**. This is the designed
successor to the retired manual send mode ([ADR-0005](../../docs/adr/0005-inter-pane-text-passing.md)):
it is addressed by tab id or title, it works while the target is off screen (a background tab,
or a whole background workspace), and it never goes through the frontend — the Rust side writes
the bytes to the target session's stdin directly.

**The channel is confined to the sender's workspace.** A workspace is the project isolation
unit, so a channel that crossed it would give a mis-aimed line a blast radius reaching shells
that have nothing to do with the work in hand (user decision 2026-08-11). The confinement
applies to **both** addressing modes and to the query channel below: a tab in another
workspace matches neither its title nor its globally unique `#id`, and does not appear in
`winmux ls`. Ids stay globally unique — uniqueness is a property of the address, not a key
past the boundary. If the sender's session cannot be mapped back to a tab at all (it always
can — it is a live session that just emitted the OSC), there is no boundary to draw and
nothing is sent.

The skill that teaches an agent to use it is `scripts/wsl/skills/winmux-send/SKILL.md`
(auto-provisioned to `~/.claude/skills/winmux-send/SKILL.md`), and the command it tells the
agent to call is `winmux send`:

```bash
winmux send '#181' 'cargo test'     # runs the line in tab 181
winmux send -l '#181' 'cargo test'  # literal: only pre-fills the prompt
winmux send build 'cargo test'      # by title substring instead of id
```

The CLI encodes the text, appends the trailing newline (unless `-l`), and resolves the
terminal device with the same two-step discipline as `winmux-notify.sh` — so it works from a
hook too. Delivery stays silent (exit 0 either way); only a usage error is reported.

The sequence it emits:

```
ESC ] 777 ; winmux-send ; <target> ; <base64> BEL
```

| Field | Contract |
|---|---|
| `winmux-send` | Literal kind marker. Everything else after `777;` keeps its old meaning. |
| `<target>` | `#<decimal>` addresses a **tab id** exactly. Anything else is matched **case-insensitively as a substring** of a tab's title (the title the target set with `OSC 0`). Either way the candidates are the running terminal tabs of the **sender's own workspace**. |
| `<base64>` | Standard base64 (`A-Za-z0-9+/`, optional `=` padding) of the raw bytes. No URL-safe alphabet, no embedded whitespace or newline. |

| Situation | Result |
|---|---|
| Exactly one running terminal tab **in the sender's workspace** matches | Its stdin receives the decoded bytes verbatim |
| No match | Nothing is sent |
| Two or more matches | Nothing is sent — the first match is **never** picked |
| The sender's own tab matches | It is excluded before counting |
| The only match is in another workspace | Nothing is sent — it was never a candidate |
| Decoded size > 32 KiB | Rejected |
| Malformed base64 / missing text field | Rejected |

**Id addressing.** `#<target>` is an id only when everything after `#` is decimal digits that
parse as a `u64`; `#build`, `#`, `#1.2` and an out-of-range number all fall back to title
matching, so a tab whose title starts with `#` stays reachable by title. An id resolves to
one tab or to none — `Ambiguous` cannot happen, because ids are unique across the app; an id
belonging to another workspace resolves to none, exactly like an id that does not exist. The
tab's own id is in its `WINMUX_TAB` (below), and `winmux ls` lists the rest of its workspace;
a title is the weaker address because a prompt hook may rewrite it on every prompt.

- **Nothing is written back to the sender**, on success or failure — a diagnostic in someone
  else's terminal (and in its replay buffer) is worse pollution than a missed send. Failures
  go to the app's stderr only, so sending is silent fire-and-forget.
- The bytes reach the target exactly as sent: no bracketed paste, no quoting, no trailing
  newline added. **Include the newline** if the target shell should run the line.
- The effective size limit is far below 32 KiB: the OSC scanner discards any payload over
  64 KiB before parsing — sized so the full 32 KiB text contract survives base64
  expansion. Pass a path, not
  a file.
- No state changes, so no snapshot is published and nothing is persisted.

**Environment.** Every winmux terminal exports `WINMUX=1` and prepends `~/.winmux/bin` to
`PATH`; a tab with per-tab history also exports `WINMUX_TAB=<tab id>` (its own stable tab id).
That is how an agent knows it is inside winmux at all — the skill's description keys off
`WINMUX` — and the id is the tab's self-reference for a reply address. The wrapper that sets
them is `apps/winmux/src-tauri/src/host.rs::bash_argv`, so they reach the login shell and
every child of it. The `PATH` entry survives the login shell because the Debian/Ubuntu rc
convention *prepends* to `PATH` (`PATH="$HOME/bin:$PATH"`) rather than reassigning it.

**Security.** Any terminal program on the machine that can write to a pane's PTY can inject
input into another pane this way. That is intended — winmux assumes your own machine and
cooperating agents — and this channel is a convenience, **not** a privilege boundary. The
size cap, the unique-match requirement, the self-exclusion and the workspace confinement are
misfire guards, not security controls: they bound the blast radius of a *mistake*, and none
of them stops a program that is already free to write to the target's PTY itself.

## Agent query channel — `OSC 777;winmux-query`

The read half of the agent channel: it answers "what tabs are open?" so an agent can pick a
target id instead of guessing at a title. It shares the send channel's **workspace
confinement** — it enumerates the requester's own workspace and nothing else, because a list
that reached further would offer targets the send half refuses. Unlike the notify and send
channels this one has a **reply**, and because the OSC stream is one-way (into the app) the
reply is a **file the sender names in the request**.

```
ESC ] 777 ; winmux-query ; <kind> ; <base64 reply path> BEL
```

| Field | Contract |
|---|---|
| `winmux-query` | Literal kind marker. |
| `<kind>` | The question. `list-tabs` is the only one the app answers; any other value is ignored (so a newer CLI against an older app simply gets no reply, and vice versa). |
| `<base64 reply path>` | Standard base64 of an absolute Linux path that **must start with `/tmp/`**. Both fields are required — `777;winmux-query;list-tabs` with no path is not a query at all, since there is nowhere to answer. |

**`/tmp/` is enforced at the string level — a misfire guard, not a privilege boundary.** The
reply is a file *write performed by the winmux app*. The content is only metadata the app
already owns, but leaving the path free would make this channel a way for anything that can
write to a PTY to overwrite `~/.bashrc` or `~/.claude/settings.json`. Path validation
(`crate::send::decode_reply_path`) rejects `..`, backslashes and NUL *before* the prefix
check, so `/tmp/../home/u/.bashrc` does not get through. What it does **not** block is a
pre-planted symlink (`/tmp/x → $HOME`): the write follows it server-side. Like the send
channel, this channel assumes cooperating processes on your own machine; a
canonicalize-at-write recheck is a recorded backlog item pending real-hardware 9P semantics.

The reply for `list-tabs`:

```json
{"tabs": [{"tab": 181, "title": "build", "workspaceId": 1, "workspaceName": "winmux",
           "pane": 3, "active": true, "kind": "terminal", "status": "running"}],
 "self_tab": 176}
```

| Field | Meaning |
|---|---|
| `tabs` | Every open tab **of the requester's workspace**, in pane → tab order. Viewer tabs are included: this answers "what is open", not "what can I send to". |
| `workspaceId` / `workspaceName` | The requester's own workspace — the same value on every row, kept because it names the context the list is scoped to (the reply schema did not change with the confinement). |
| `kind` | `terminal` \| `folderBrowser` \| `textViewer` \| `markdownViewer` |
| `status` | `running` \| `exited` (terminals) \| `viewer` (a tab with no process). Only `running` terminals are send targets. |
| `self_tab` | The requester's own tab id, or `null` when the app cannot map the session back to a tab. That case also empties `tabs`: with no workspace to scope to, there is nothing to enumerate. |

- **The file appears only when it is complete.** The app writes `<path>.partial` and renames
  it into place, so a reader that waits for the path to exist never sees half-written JSON —
  which matters because a write that crosses the 9P boundary from Windows into WSL does not
  land in one piece. The rename stays inside the same directory, so it never crosses a
  filesystem boundary.
- **Failure is silent, exactly like send.** A bad path, an unknown kind, a serialization or
  write failure — all of it goes to the app's stderr and **nothing** is written back to the
  requester's terminal. The reply file never appearing is the only signal the requester gets,
  which is why `winmux ls` times out (2s) rather than waiting forever.
- No state changes: no snapshot is published, nothing is persisted, and the query is not
  coalesced into the 100ms notification flush window (two queries in one window must both be
  answered).
- Queries share one in-flight cap with sends (8 concurrent) — they contend for the same
  blocking thread pool, so one counter guards both.

`winmux ls` is the CLI half. It `mktemp`s a name under `/tmp`, removes the placeholder, emits
the query, polls for the path to appear (0.05s, giving up at 2s), renders the JSON as a table,
and deletes the file. **The `COMMAND` column is not part of the reply** — the app has no idea
what runs inside a tab. The CLI fills it from `/proc` on its own side by finding the process
whose environment has `WINMUX_TAB=<id>` and reading its terminal's foreground process group,
which is why a tab whose shell lives in another WSL distro or in a Windows shell shows `?`.

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

`~/.claude/hooks/winmux-notify.sh` (must be made executable: `chmod +x`) — auto-provisioning
installs this same text at `~/.winmux/bin/winmux-notify.sh` instead, and migrates a hook
still pointing at the manual path onto that copy (see the table above), so the block below is
the canonical source for the copy embedded in `apps/winmux/src-tauri/src/provision.rs`
(**keep the two byte-identical**):

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
