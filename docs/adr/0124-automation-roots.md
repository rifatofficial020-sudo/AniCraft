# ADR 0124: Automation roots for the agent entry points

* Status: accepted (2026-11-20)
* Relates to: ADR 0004 (destructive actions), ADR 0036 (the file layer, `vfs`), ADR 0104 (the
  live control port), ADR 0097 (saved commands), ADR 0116 (video through the user's ffmpeg),
  AGENTS.md rule 18 (`standards/app-agents.md` in storytold/craft-contrib), PR #497 (the
  `ffmpeg` parameter is refused over `mcp` and `serve`), roadmap M8

## Context

`anicraft-cli mcp` hands an AI assistant every command the app has, and many take a path: open,
save, import, export, video, plug-ins, saved commands, presets, swatches, data tables, templates,
recovery. Until now an assistant could read or write any file the person could. The Craft
standard (rule 18) says an agent entry point MUST refuse to start without
`--automation-read-root` and `--automation-write-root` and MUST refuse file access outside
them.

Three things make "check the path parameter" not enough:

* **Paths a command works out itself.** Save without a path writes the document's own file;
  Publish writes beside it; a document can link a video anywhere on the disk; a saved command's
  steps are commands with paths of their own.
* **Spellings.** `..`, symbolic links, Windows junctions, letter case, 8.3 names, `\\?\` and
  device names (`NUL`) all name a file other than the one the text suggests.
* **The window.** With `--bridge` the commands run in the desktop app, on the thread that also
  runs the person's own commands, which must not be limited.

storytold/craft-libs has the agent *settings* (`craft-control`: `AgentSettings::read_root`,
`write_root`, `HeadlessFolders`) but no helper that resolves a path and decides whether it is
inside a root; each Craft app has its own. PhotoCraft's flags are the model for ours.

## Decision

1. **Both flags are required for `mcp`**, with or without `--bridge`. Without them the program
   prints why and exits with code 2 before it opens a file or a connection.
   `--automation-read-root` may be repeated; `--automation-write-root` is given once. Each must
   be a folder that exists; it is resolved once at start. The two may be the same folder.
   `serve` takes the same flags, optionally: with them it is confined as `mcp` is, without
   them it is the plain CLI it was. `exec`, `run` and the other subcommands, which a person
   types, and the desktop app are not limited.

2. **Read and write are separate.** Reading a file's contents or listing a folder needs a read
   root; creating, replacing, renaming or deleting needs the write root. A write root grants
   no reading, except the question "is it there, how large is it?", which either kind of root
   answers: a command asks that before it replaces a file, and ADR 0004's confirmation must
   keep working in the write root.

3. **Resolution** (`engine::access::resolve`): the path is made absolute; the longest part
   that exists is resolved by the operating system (`canonicalize`: `..`, symbolic links,
   junctions, letter case and short names all become the one real spelling); what doesn't
   exist yet (a file about to be written) is appended, with `..` applied to it by name, which
   is exact for folders that aren't there. A link that points nowhere is refused, since writing
   through it would create its target. The result must start with a resolved root, component
   by component. So case-insensitivity on Windows comes from the file system's own answer,
   not from our folding of names. Before any of that, on Windows, a path on another computer
   (`\\server\share\…`) or a device path (`\\.\…`) is refused by its spelling alone, unless a
   root is on that same share: resolving it would contact the server and offer it the
   person's sign-in. (A mapped drive letter can't be told from a local one.)

4. **Where the check sits: in the engine, in three places**, under one scoped, per-thread
   policy (`access::confine(roots)`, in force until its guard drops):
   * `Session::execute` checks every path a command is *given* before anything else
     (`access::PATH_PARAMS`: each command's path parameters and whether it reads or writes
     there), so the refusal comes before the confirmation check or the command looks at the
     file, and names the parameter. A test fails when a command with a path parameter is in
     neither that table nor the list of commands whose "path" is not a file.
   * `vfs`, which commands read and write through, asks the policy in every function. This
     covers the paths a command works out itself. A refused question answers "no"; a refused
     read or write is an error naming the roots.
   * The few places that pass a path to something that opens it itself ask explicitly: ffmpeg
     (Export Video's output, Import Video's and a reference video's input), the XFL folder
     reader, the plug-in registry (install, remove, reload).

   Putting it in the engine rather than in the MCP layer is the point: a saved command's steps,
   a plug-in's calls and Publish all end in the same `vfs` calls. The policy is per thread
   because `vfs` is a set of free functions with no session at hand (its in-memory store is
   per thread for the same reason), and scoped because the window runs the person's commands
   on the same thread between an assistant's calls.

5. **AniCraft's own temporary files stay allowed**: the soundtrack WAV Export Video hands to
   ffmpeg is registered by its exact path (`access::own`) for as long as it lives. Nothing an
   assistant can name is exempt, the temp folder included.

6. **What the person chose at the start is theirs.** The file and `--plugins` folder on `mcp`'s
   command line are opened before the policy applies. From then on the assistant's commands
   are confined: a plain `file.save` of a document opened from outside the write root is
   refused, and so is `plugin.reload` of a plug-ins folder outside the read root.

7. **The bridge carries the roots.** `mcp --bridge` sends its (resolved) roots with every
   `tools.call`; the window applies them to that call only, then runs its own follow-up (the
   status line, Open Recent) outside the policy. A `tools.call` with no roots may touch no
   file at all, so a program that speaks the control protocol itself gets no more than the
   bridge. The protocol version is now 2, and the bridge will not send a command to a window
   that answers `hello` with 1: an older AniCraft would ignore the roots.

8. **The error an assistant reads names the root.** When a command fails after an access was
   refused, the tool result is the refusal (`… is outside the folder automation may write to:
   the automation write root is …`), not whatever the command made of a file that "wasn't
   there".

9. **The `ffmpeg` refusal (#497) stays**, and is checked first.

## Consequences

* An assistant that should read a project and write renders elsewhere gets exactly that; one
  that should work in one folder gets both flags with the same folder.
* Existing MCP client configurations stop working until the two flags are added; the error
  says what to add. This is the standard's intent.
* `Session::execute` does one table lookup more per command when a policy is in force, and
  nothing when not. Each confined file operation costs one or two `canonicalize` calls.
* A document inside the read root that links a video outside it opens, but the video shows as
  unavailable to the assistant.

## Limits, on purpose

* **Check, then use.** The path is checked and then opened by name, so a program running as
  the same user that swaps a folder for a link between the two could redirect one operation.
  Such a program can already read the person's files itself; closing the gap means opening
  every file relative to a held folder handle, which is a rewrite of `vfs`.
* **Hard links** inside a root to files outside it can't be told apart from files.
* **The XFL folder reader** refuses links for the package's files but follows a linked
  *folder* inside a package; an assistant cannot create one through AniCraft.
* **What ffmpeg opens on its own.** ffmpeg is handed the checked file's resolved path, but a
  media file can name others (an HLS playlist's segments), and ffmpeg follows those under its
  own rules, not ours. Import Video inside the roots should be given footage, not playlists
  from an untrusted source.
* The roots limit AniCraft's commands. They are not a sandbox for the assistant's other tools.

## Not in this slice

* Roots saved in the app's preferences, and a switch in the window (craft-libs
  `AgentSettings`), so the desktop app can cap what any bridge asks for.
* A shared `craft-libs` helper for resolving and checking roots, so the Craft apps stop
  carrying one each (proposed in storytold/craft-libs#2527 and #45; `engine::access::resolve` and `Roots` are written to
  move as they are).
* `ANICRAFT_AUTOMATION_*_ROOT` environment variables.

## Alternatives considered

* **Checking only the parameters in the MCP layer.** Misses every path a command works out,
  and every nested command.
* **A lexical check** (no file system). Cannot see links or junctions, which the task names.
* **Roots on the desktop app only** (`anicraft --control … --automation-read-root …`), as
  PhotoCraft does. The standard puts the requirement on `<app>-cli mcp`, and the shortcut that
  starts the window can't know the assistant's project folder. An app-side cap can be added on
  top.
* **A field on `Session`** instead of a per-thread policy. `vfs` has no session, and the
  window can change which session is current in the middle of a call.
